//! Portable parallel directory walker built on `std::fs` and rayon.
//!
//! On Windows `DirEntry::metadata` reuses the data returned by `FindNextFileW`, so listing a
//! directory costs one enumeration and no per-file system calls.

use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;

use rayon::prelude::*;

use super::{Cancelled, Progress};
use crate::model::{ScanNode, Tree, flags};

/// Walks `root` and returns a finalized tree.
pub fn scan(root: &Path, progress: &Progress) -> Result<Tree, Cancelled> {
    let node = walk(root, progress)?;
    let mut tree = Tree::from_scan(root, node);
    tree.info.method = "walk";
    tree.info.unreadable = count_unreadable(&tree);
    Ok(tree)
}

/// Walks `root` into an owned [`ScanNode`] tree (used for full scans and for rescans of a
/// single folder).
pub fn walk(root: &Path, progress: &Progress) -> Result<ScanNode, Cancelled> {
    let name = root.display().to_string();
    let node = walk_dir(root, name, progress);
    if progress.is_cancelled() { Err(Cancelled) } else { Ok(node) }
}

fn walk_dir(path: &Path, name: String, progress: &Progress) -> ScanNode {
    let mut node = ScanNode { name, size: 0, flags: flags::DIR, children: Vec::new() };
    if progress.is_cancelled() {
        return node;
    }
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => {
            node.flags |= flags::UNREADABLE;
            return node;
        }
    };

    let mut subdirs = Vec::new();
    let (mut files, mut bytes) = (0u64, 0u64);
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let Ok(file_type) = entry.file_type() else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if file_type.is_symlink() {
            // On Windows this is true for every name-surrogate reparse point (symlinks,
            // junctions, mount points). Never follow them: they would double count or loop.
            node.children.push(ScanNode { name, size: 0, flags: flags::LINK, children: Vec::new() });
        } else if file_type.is_dir() {
            subdirs.push((entry.path(), name));
        } else {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            files += 1;
            bytes += size;
            node.children.push(ScanNode { name, size, flags: 0, children: Vec::new() });
        }
    }
    progress.files.fetch_add(files, Ordering::Relaxed);
    progress.bytes.fetch_add(bytes, Ordering::Relaxed);
    let dirs_seen = progress.dirs.fetch_add(1, Ordering::Relaxed);
    if dirs_seen.is_multiple_of(64) {
        progress.set_current(path.display().to_string());
    }

    let sub: Vec<ScanNode> =
        subdirs.into_par_iter().map(|(p, n)| walk_dir(&p, n, progress)).collect();
    node.children.extend(sub);
    node
}

fn count_unreadable(tree: &Tree) -> u64 {
    tree.preorder(tree.root(), None).filter(|&(id, _)| tree.node(id).is_unreadable()).count() as u64
}
