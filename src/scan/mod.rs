//! Scanning entry point: chooses between the raw NTFS MFT reader and the directory walker.

pub mod mft;
pub mod walk;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use crate::model::Tree;

/// Which scanner to use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Method {
    /// MFT when possible (local NTFS volume, elevated), otherwise the directory walk.
    #[default]
    Auto,
    /// Raw NTFS MFT read only; fails instead of falling back.
    Mft,
    /// Directory walk only (works everywhere, no elevation needed).
    Walk,
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Method::Auto => "Auto",
            Method::Mft => "MFT",
            Method::Walk => "Walk",
        })
    }
}

/// Live counters shared between a running scan and the UI.
#[derive(Debug, Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    /// MFT records parsed so far / in total (0 total while walking).
    pub records_done: AtomicU64,
    pub records_total: AtomicU64,
    /// A human-readable description of what is happening right now.
    pub current: Mutex<String>,
    pub cancel: AtomicBool,
}

impl Progress {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn set_current(&self, text: impl Into<String>) {
        if let Ok(mut cur) = self.current.try_lock() {
            *cur = text.into();
        }
    }

    pub fn current(&self) -> String {
        self.current.lock().map(|c| c.clone()).unwrap_or_default()
    }

    /// The file, folder and byte counters.
    pub fn counters(&self) -> [u64; 3] {
        [&self.files, &self.dirs, &self.bytes].map(|c| c.load(Ordering::Relaxed))
    }

    /// Restores counters saved with [`Progress::counters`] (used when falling back from MFT to
    /// the walker, so a partial MFT pass is not counted twice) and clears the MFT counters.
    pub fn restore_counters(&self, saved: [u64; 3]) {
        for (c, v) in [&self.files, &self.dirs, &self.bytes].into_iter().zip(saved) {
            c.store(v, Ordering::Relaxed);
        }
        self.records_done.store(0, Ordering::Relaxed);
        self.records_total.store(0, Ordering::Relaxed);
    }
}

/// Returned when the user cancels a scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("scan cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// Scans `path` with the requested method. With [`Method::Auto`], MFT failures (other than
/// cancellation) fall back to the walker and the reason is recorded in `tree.info.note`.
pub fn scan(path: &Path, method: Method, progress: &Progress) -> anyhow::Result<Tree> {
    let start = Instant::now();
    let path = &normalize_root(path);
    if !path.is_dir() {
        anyhow::bail!("{} is not a directory", path.display());
    }
    let mut tree = match method {
        Method::Walk => walk::scan(path, progress)?,
        Method::Mft => mft::scan(path, progress).map_err(|e| match e {
            mft::MftError::Cancelled => anyhow::Error::new(Cancelled),
            other => anyhow::anyhow!("MFT scan failed: {other}"),
        })?,
        Method::Auto => {
            let saved = progress.counters();
            match mft::scan(path, progress) {
                Ok(tree) => tree,
                Err(err) if !err.should_fallback() => return Err(Cancelled.into()),
                Err(err) => {
                    progress.restore_counters(saved);
                    let mut tree = walk::scan(path, progress)?;
                    tree.info.note = Some(format!("MFT unavailable: {err}"));
                    tree
                }
            }
        }
    };
    tree.info.elapsed = start.elapsed();
    Ok(tree)
}

/// Scans several locations (e.g. drives) one after another and combines them under a virtual
/// root. A single path gives the same result as [`scan`]. Locations that fail are skipped and
/// noted, unless all of them fail; cancelling stops everything.
pub fn scan_many(paths: &[PathBuf], method: Method, progress: &Progress) -> anyhow::Result<Tree> {
    match paths {
        [] => anyhow::bail!("nothing to scan"),
        [one] => return scan(one, method, progress),
        _ => {}
    }
    let mut parts = Vec::new();
    let mut skipped = Vec::new();
    for path in paths {
        match scan(path, method, progress) {
            Ok(tree) => parts.push(tree),
            Err(e) if e.is::<Cancelled>() => return Err(e),
            Err(e) => skipped.push(format!("{} skipped: {e:#}", path.display())),
        }
    }
    if parts.is_empty() {
        anyhow::bail!("{}", skipped.join("; "));
    }
    let mut tree = if parts.len() == 1 { parts.remove(0) } else { Tree::combine(parts) };
    if !skipped.is_empty() {
        let mut notes: Vec<String> = tree.info.note.take().into_iter().collect();
        notes.extend(skipped);
        tree.info.note = Some(notes.join("; "));
    }
    Ok(tree)
}

/// Absolute, canonical form of a scan root as users expect to see it (no `\\?\` prefix).
pub fn normalize_root(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).map(|c| display_path(&c)).unwrap_or_else(|_| path.to_path_buf())
}

/// Strips the Windows verbatim prefix: `\\?\C:\x` -> `C:\x`, `\\?\UNC\srv\share` -> `\\srv\share`.
pub fn display_path(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}
