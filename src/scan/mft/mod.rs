//! Fast scanner that reads the NTFS Master File Table directly from the raw volume.
//!
//! The MFT is read sequentially in large chunks and parsed in parallel. Each record yields its
//! sizes and `$FILE_NAME` links (parent + name); the tree is then built by joining children to
//! parents, without ever reading directory indexes. This needs an elevated process (the raw
//! volume `\\.\X:` is only readable by administrators); callers fall back to the walker otherwise.

pub mod boot;
pub mod chunk;
pub mod error;
pub mod fixup;
pub mod index;
pub mod io;
pub mod le;
pub mod record;
pub mod runs;
#[cfg(test)]
pub(crate) mod testutil;

use std::path::Path;
use std::sync::atomic::Ordering;

use rayon::prelude::*;

pub use error::MftError;

use self::chunk::{ChunkOut, parse_chunk};
use self::io::{AlignedBuf, MftLayout, ReadAt, bootstrap, read_mft_range};
use self::record::FileRef;
use super::Progress;
use crate::model::Tree;

/// Bytes read and parsed per work item. A multiple of every valid cluster and record size.
const CHUNK_BYTES: u64 = 4 * 1024 * 1024;

/// Scans `target` (a drive root or any folder on a local NTFS volume) via the MFT.
#[cfg(windows)]
pub fn scan(target: &Path, progress: &Progress) -> Result<Tree, MftError> {
    use std::path::{Component, Prefix};

    let canonical = std::fs::canonicalize(target)?;
    let mut letter = None;
    let mut components = Vec::new();
    for comp in canonical.components() {
        match comp {
            Component::Prefix(p) => match p.kind() {
                Prefix::VerbatimDisk(d) | Prefix::Disk(d) => letter = Some(char::from(d).to_ascii_uppercase()),
                _ => return Err(MftError::NotLocalNtfs("not on a drive letter".into())),
            },
            Component::Normal(name) => {
                use std::os::windows::ffi::OsStrExt;
                components.push(name.encode_wide().collect::<Vec<u16>>());
            }
            _ => {}
        }
    }
    let letter = letter.ok_or_else(|| MftError::NotLocalNtfs("not on a drive letter".into()))?;
    let drive_root = format!("{letter}:\\");
    match crate::platform::filesystem_name(Path::new(&drive_root)) {
        Some(fs) if fs.eq_ignore_ascii_case("NTFS") => {}
        Some(fs) => return Err(MftError::NotLocalNtfs(format!("{drive_root} is {fs}"))),
        None => return Err(MftError::NotLocalNtfs(format!("cannot query {drive_root}"))),
    }

    let volume = io::open_volume(letter)?;
    let root_path = crate::scan::display_path(&canonical);
    scan_volume(&volume, || io::open_volume(letter), &components, &root_path, progress, |frn, serial| {
        // Confirm the resolved record really is the folder we were asked for (it may be a
        // folder mounted from another volume, or newer than the on-disk MFT).
        match crate::platform::file_id(&canonical) {
            Some((vol_serial, file_index)) => vol_serial == serial as u32 && file_index == frn,
            None => true,
        }
    })
}

#[cfg(not(windows))]
pub fn scan(_target: &Path, _progress: &Progress) -> Result<Tree, MftError> {
    Err(MftError::Unsupported("MFT scanning is only available on Windows"))
}

/// Volume-independent core of the scan, also used by tests with an in-memory volume image.
///
/// `components` are the UTF-16 path components of the target below the volume root (empty for
/// a whole-volume scan). `verify(frn, volume_serial)` is called for subdirectory targets.
pub fn scan_volume<R: ReadAt>(
    first: &R,
    open: impl Fn() -> Result<R, MftError> + Sync,
    components: &[Vec<u16>],
    root_path: &Path,
    progress: &Progress,
    verify: impl FnOnce(u64, u64) -> bool,
) -> Result<Tree, MftError> {
    progress.set_current("Reading $MFT layout");
    let layout = bootstrap(first)?;
    // Per-volume progress: a multi-drive scan restarts the bar for each drive.
    progress.records_done.store(0, Ordering::Relaxed);
    progress.records_total.store(layout.record_count, Ordering::Relaxed);
    progress.set_current(format!("Reading {} MFT records", crate::util::format_count(layout.record_count)));

    let chunks = read_chunks(first, &open, &layout, progress)?;
    progress.set_current("Linking records");
    let index = index::merge(chunks, layout.record_count);

    let whole_volume = components.is_empty();
    let root_record = if whole_volume {
        record::ROOT_RECORD as u32
    } else {
        let rec = index
            .resolve(components)
            .ok_or_else(|| MftError::NotFound("folder not found in the MFT (it may be newer than the last flush)".into()))?;
        if !verify(FileRef::new(u64::from(rec), index.seq(rec)).0, layout.boot.serial) {
            return Err(MftError::NotFound("folder does not match its MFT record".into()));
        }
        rec
    };

    progress.set_current("Building tree");
    let mut tree = Tree::new(root_path);
    index.build_tree(root_record, &mut tree, whole_volume, progress)?;
    tree.finalize();
    tree.info.method = "MFT";
    tree.info.bad_records = index.bad_records;
    Ok(tree)
}

fn read_chunks<R: ReadAt>(
    first: &R,
    open: &(impl Fn() -> Result<R, MftError> + Sync),
    layout: &MftLayout,
    progress: &Progress,
) -> Result<Vec<ChunkOut>, MftError> {
    let rs = u64::from(layout.boot.record_size);
    let cs = layout.boot.cluster_size;
    let per_chunk = CHUNK_BYTES / rs;
    let total = layout.record_count;
    let n_chunks = total.div_ceil(per_chunk);

    let parse_one = |reader: &R, buf: &mut AlignedBuf, ci: usize| -> Result<ChunkOut, MftError> {
        if progress.is_cancelled() {
            return Err(MftError::Cancelled);
        }
        let first_record = ci as u64 * per_chunk;
        let count = per_chunk.min(total - first_record);
        let bytes = (count * rs).div_ceil(cs) * cs;
        let slice = &mut buf.as_mut_slice()[..bytes as usize];
        read_mft_range(reader, layout, first_record, slice)?;
        let out = parse_chunk(&mut slice[..(count * rs) as usize], first_record, rs as usize);
        progress.records_done.fetch_add(count, Ordering::Relaxed);
        Ok(out)
    };

    // Each worker gets its own volume handle (I/O on one synchronous handle is serialized) and
    // its own aligned buffer. If a handle cannot be reopened, fall back to the first one.
    (0..n_chunks as usize)
        .into_par_iter()
        .with_min_len(2)
        .map_init(
            || (open().ok(), AlignedBuf::new(CHUNK_BYTES as usize)),
            |(own, buf), ci| match own {
                Some(reader) => parse_one(reader, buf, ci),
                None => parse_one(first, buf, ci),
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::testutil::{AlignedMemVolume, sample_volume};
    use super::*;

    fn listing(tree: &Tree) -> Vec<(String, u64, bool)> {
        let mut v: Vec<_> = tree
            .preorder(tree.root(), None)
            .skip(1)
            .map(|(id, _)| {
                let names: Vec<_> = tree.ancestry(id).iter().skip(1).map(|&a| tree.node(a).name.to_string()).collect();
                (names.join("/"), tree.node(id).size, tree.node(id).is_dir())
            })
            .collect();
        v.sort();
        v
    }

    #[test]
    fn end_to_end_whole_volume() {
        let vol = sample_volume();
        let progress = Progress::new();
        let tree = scan_volume(&vol, || Ok(vol.clone()), &[], Path::new("T:\\"), &progress, |_, _| true).unwrap();
        let l = listing(&tree);
        let has = |p: &str, size: u64| assert!(l.iter().any(|(n, s, _)| n == p && *s == size), "missing {p} ({size}) in {l:#?}");
        has("<NTFS metadata>/$MFT", 32 * 1024);
        has("Docs/a.txt", 100);
        has("Docs/Sub/deep.dat", 5000);
        has("big.iso", 1_000_000);
        has("Junction", 0);
        assert!(!l.iter().any(|(n, _, _)| n.contains("ghost")), "record past the initialized size was parsed");
        // The extension record of $MFT (record 16) must not show up as a file.
        assert_eq!(tree.node(tree.root()).size, 32 * 1024 + 100 + 5000 + 1_000_000);
        assert_eq!(tree.info.method, "MFT");
        assert_eq!(tree.info.bad_records, 0);
        assert_eq!(progress.records_total.load(Ordering::Relaxed), 28);
        assert_eq!(progress.records_done.load(Ordering::Relaxed), 28);
    }

    #[test]
    fn end_to_end_subdirectory() {
        let vol = sample_volume();
        let comps = vec!["docs".encode_utf16().collect::<Vec<u16>>()];
        let mut verified = None;
        let tree = scan_volume(&vol, || Ok(vol.clone()), &comps, Path::new("T:\\Docs"), &Progress::new(), |frn, _| {
            verified = Some(frn);
            true
        })
        .unwrap();
        assert_eq!(verified, Some(FileRef::new(20, 1).0));
        assert_eq!(tree.node(tree.root()).size, 5100);
        assert_eq!(tree.root_path(), PathBuf::from("T:\\Docs"));
        let err = scan_volume(&vol, || Ok(vol.clone()), &comps, Path::new("T:\\Docs"), &Progress::new(), |_, _| false);
        assert!(matches!(err, Err(MftError::NotFound(_))));
        let missing = vec!["nope".encode_utf16().collect::<Vec<u16>>()];
        assert!(matches!(
            scan_volume(&vol, || Ok(vol.clone()), &missing, Path::new("T:\\nope"), &Progress::new(), |_, _| true),
            Err(MftError::NotFound(_))
        ));
    }

    #[test]
    fn cancel_is_reported() {
        let vol = sample_volume();
        let progress = Progress::new();
        progress.cancel.store(true, Ordering::Relaxed);
        let r = scan_volume(&vol, || Ok(vol.clone()), &[], Path::new("T:\\"), &progress, |_, _| true);
        assert!(matches!(r, Err(MftError::Cancelled)));
    }

    #[test]
    fn non_ntfs_image_is_rejected() {
        let mut bytes = sample_volume().bytes().to_vec();
        bytes[3..11].copy_from_slice(b"EXFAT   ");
        let vol = AlignedMemVolume::new(bytes, 512);
        let r = scan_volume(&vol, || Ok(vol.clone()), &[], Path::new("T:\\"), &Progress::new(), |_, _| true);
        assert!(matches!(r, Err(MftError::Corrupt(_))));
    }

    /// Parse throughput on 64 MiB of synthetic records: `cargo test --release -- --ignored throughput --nocapture`.
    #[test]
    #[ignore]
    fn throughput() {
        use super::record::RECORD_IN_USE;
        let n = 64 * 1024;
        let mut buf = vec![0u8; n * 1024];
        for i in 0..n {
            let mut rb = testutil::RecordBuilder::new(1024, i as u32, 1);
            rb.flags(RECORD_IN_USE).file_name(5, 5, 1, &format!("file number {i}.txt"), 0, 0).resident_data("", &[0; 64]);
            buf[i * 1024..(i + 1) * 1024].copy_from_slice(&rb.build());
        }
        let start = std::time::Instant::now();
        let out: Vec<_> = buf.par_chunks_mut(CHUNK_BYTES as usize).enumerate().map(|(ci, c)| parse_chunk(c, (ci * 4096) as u64, 1024)).collect();
        let el = start.elapsed();
        let links: usize = out.iter().map(|o| o.links.len()).sum();
        assert_eq!(links, n);
        println!("parsed {n} records in {el:?} ({:.0} records/s)", n as f64 / el.as_secs_f64());
    }
}
