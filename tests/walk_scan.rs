//! Walker scans of a temporary directory tree with known sizes.

use std::fs;
use std::path::Path;

use dirstat::model::Tree;
use dirstat::platform;
use dirstat::scan::{self, Method, Progress};

fn write(path: &Path, len: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![b'x'; len]).unwrap();
}

fn find(tree: &Tree, name: &str) -> Option<dirstat::model::NodeId> {
    tree.preorder(tree.root(), None).map(|(id, _)| id).find(|&id| &*tree.node(id).name == name)
}

/// Creates a directory junction (no elevation needed). Returns false if `mklink` is unavailable.
#[cfg(windows)]
fn junction(link: &Path, target: &Path) -> bool {
    std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn junction(link: &Path, target: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[test]
fn walk_counts_sizes_and_skips_junctions() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(&root.join("a.txt"), 100);
    write(&root.join("sub").join("b.bin"), 5000);
    write(&root.join("sub").join("deep").join("c.dat"), 12345);
    write(&root.join("sub").join("deep").join("Ünïcödé 文件.txt"), 7);
    fs::create_dir(root.join("empty")).unwrap();
    let has_junction = junction(&root.join("link-to-sub"), &root.join("sub"));

    let progress = Progress::new();
    let tree = scan::scan(root, Method::Walk, &progress).unwrap();
    let r = tree.node(tree.root());
    assert_eq!(r.size, 100 + 5000 + 12345 + 7, "junction target must not be counted twice");
    assert_eq!(r.files, 4);
    assert_eq!(r.dirs, 3);
    assert_eq!(tree.info.method, "walk");
    assert_eq!(tree.info.unreadable, 0);
    assert!(find(&tree, "Ünïcödé 文件.txt").is_some());

    if has_junction {
        let link = find(&tree, "link-to-sub").expect("junction listed");
        let n = tree.node(link);
        assert!(n.is_link() && n.size == 0 && n.children.is_empty());
    }

    // Exts are sorted by bytes: .dat (12345) > .bin (5000) > .txt (107).
    let names: Vec<_> = tree.exts().iter().filter(|e| e.files > 0).map(|e| e.name.to_string()).collect();
    assert_eq!(names, ["dat", "bin", "txt"]);
    assert_eq!(progress.files.load(std::sync::atomic::Ordering::Relaxed), 4);
}

#[test]
fn auto_method_falls_back_when_not_elevated() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("f.bin"), 4096);
    let tree = scan::scan(dir.path(), Method::Auto, &Progress::new()).unwrap();
    assert_eq!(tree.node(tree.root()).size, 4096);
    if cfg!(windows) && !platform::is_elevated() {
        assert_eq!(tree.info.method, "walk");
        let note = tree.info.note.as_deref().unwrap_or_default();
        assert!(note.contains("MFT unavailable"), "note: {note}");
    }
}

#[test]
fn forced_mft_reports_an_error_instead_of_falling_back() {
    if !cfg!(windows) || platform::is_elevated() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let err = scan::scan(dir.path(), Method::Mft, &Progress::new()).unwrap_err();
    assert!(format!("{err:#}").contains("MFT scan failed"), "{err:#}");
}

#[test]
fn rescan_graft_reflects_changes() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("keep").join("x.bin"), 10);
    write(&dir.path().join("work").join("old.bin"), 1000);
    let mut tree = scan::scan(dir.path(), Method::Walk, &Progress::new()).unwrap();
    assert_eq!(tree.node(tree.root()).size, 1010);

    fs::remove_file(dir.path().join("work").join("old.bin")).unwrap();
    write(&dir.path().join("work").join("new.bin"), 300);
    let work = find(&tree, "work").unwrap();
    let path = tree.path_of(work).unwrap();
    let rescanned = scan::walk::walk(&path, &Progress::new()).unwrap();
    tree.graft(work, rescanned);
    assert_eq!(tree.node(tree.root()).size, 310);
    assert!(find(&tree, "new.bin").is_some());
}

#[test]
fn scan_many_combines_locations_and_skips_failures() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write(&a.path().join("one.bin"), 100);
    write(&b.path().join("sub").join("two.bin"), 50);
    let missing = a.path().join("does-not-exist");
    let paths = vec![a.path().to_path_buf(), missing.clone(), b.path().to_path_buf()];
    let progress = Progress::new();
    let tree = scan::scan_many(&paths, Method::Walk, &progress).unwrap();
    let root = tree.node(tree.root());
    assert_eq!((root.size, root.files, root.children.len()), (150, 2, 2));
    assert_eq!(tree.path_of(tree.root()), None);
    let two = find(&tree, "two.bin").unwrap();
    assert_eq!(tree.path_of(two).unwrap(), scan::normalize_root(b.path()).join("sub").join("two.bin"));
    let note = tree.info.note.unwrap_or_default();
    assert!(note.contains("skipped"), "{note}");
    assert_eq!(progress.files.load(std::sync::atomic::Ordering::Relaxed), 2, "counters accumulate across locations");

    // A single location is a plain tree; all-failing is an error.
    let single = scan::scan_many(&paths[..1], Method::Walk, &Progress::new()).unwrap();
    assert!(single.path_of(single.root()).is_some());
    assert!(scan::scan_many(&[missing], Method::Walk, &Progress::new()).is_err());
}

#[test]
fn missing_path_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    assert!(scan::scan(&dir.path().join("does-not-exist"), Method::Walk, &Progress::new()).is_err());
}
