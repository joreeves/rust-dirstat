//! MFT scanner vs directory walker on real volumes. Needs an elevated terminal:
//!
//! ```text
//! cargo test --release --test mft_admin -- --ignored --nocapture
//! ```
//!
//! Set `DIRSTAT_TEST_DIR` to place the fixture on another NTFS volume (default: %TEMP%).
#![cfg(windows)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use dirstat::model::Tree;
use dirstat::platform;
use dirstat::scan::{self, Method, Progress};

/// relative path -> (is_dir, is_link, size)
fn listing(tree: &Tree) -> BTreeMap<String, (bool, bool, u64)> {
    let mut out = BTreeMap::new();
    let mut paths: Vec<String> = Vec::new();
    for (id, depth) in tree.preorder(tree.root(), None) {
        paths.truncate(depth);
        let n = tree.node(id);
        let path = match paths.last() {
            None => String::new(),
            Some(p) if p.is_empty() => n.name.to_string(),
            Some(p) => format!("{p}\\{}", n.name),
        };
        if depth > 0 {
            out.insert(path.clone(), (n.is_dir(), n.is_link(), n.size));
        }
        paths.push(path);
    }
    out
}

fn elevated_or_skip() -> bool {
    if platform::is_elevated() {
        true
    } else {
        eprintln!("skipped: not elevated (run from an administrator terminal)");
        false
    }
}

/// Forces NTFS to write pending MFT changes so the raw reader sees them.
fn flush_volume(path: &Path) {
    use std::os::windows::fs::OpenOptionsExt;
    let canonical = fs::canonicalize(path).unwrap();
    let letter = canonical.to_string_lossy().chars().nth(4).unwrap();
    let volume = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0x3)
        .open(format!(r"\\.\{letter}:"))
        .expect("open volume for flush");
    volume.sync_all().expect("flush volume");
}

fn write(path: &Path, len: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![0x5Au8; len]).unwrap();
}

fn fixture() -> (PathBuf, tempfile::TempDir) {
    let base = std::env::var_os("DIRSTAT_TEST_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let dir = tempfile::Builder::new().prefix("rust-dirstat-mft-").tempdir_in(base).unwrap();
    let root = dir.path().to_path_buf();
    write(&root.join("empty.bin"), 0);
    write(&root.join("resident.txt"), 100);
    write(&root.join("nonresident.bin"), 4097);
    write(&root.join("ten-mib.bin"), 10 * 1024 * 1024);
    for i in 0..1000 {
        write(&root.join("many").join(format!("file{i:04}.dat")), i);
    }
    write(&root.join("ünïcödé").join("名前.txt").join("ファイル.bin"), 333);
    let mut deep = root.join("deep");
    for i in 0..30 {
        deep.push(format!("level{i}"));
    }
    write(&deep.join("bottom.txt"), 42);
    write(&root.join("links").join("original.bin"), 5555);
    for i in 0..100 {
        fs::hard_link(root.join("links").join("original.bin"), root.join("links").join(format!("link{i:03}.bin"))).unwrap();
    }
    write(&root.join("streams.txt"), 10);
    for i in 0..50 {
        fs::write(format!("{}:stream{i}", root.join("streams.txt").display()), vec![1u8; 8192]).unwrap();
    }
    write(&root.join("compressed").join("zeros.bin"), 1024 * 1024);
    let _ = std::process::Command::new("compact").args(["/c", "/q"]).arg(root.join("compressed").join("zeros.bin")).output();
    let _ = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(root.join("junction"))
        .arg(root.join("many"))
        .output();
    flush_volume(&root);
    (root, dir)
}

#[test]
#[ignore]
fn mft_matches_walk_on_fixture() {
    if !elevated_or_skip() {
        return;
    }
    let (root, _guard) = fixture();
    let mft = scan::scan(&root, Method::Mft, &Progress::new()).expect("MFT scan");
    let walk = scan::scan(&root, Method::Walk, &Progress::new()).expect("walk scan");
    assert_eq!(mft.info.method, "MFT");
    assert_eq!(mft.info.bad_records, 0);
    let (a, b) = (listing(&mft), listing(&walk));
    let only_mft: Vec<_> = a.keys().filter(|k| !b.contains_key(*k)).collect();
    let only_walk: Vec<_> = b.keys().filter(|k| !a.contains_key(*k)).collect();
    let differ: Vec<_> = a.iter().filter(|(k, v)| b.get(*k).is_some_and(|w| w != *v)).map(|(k, v)| (k, v, b[k])).collect();
    assert!(only_mft.is_empty(), "only in MFT: {only_mft:?}");
    assert!(only_walk.is_empty(), "only in walk: {only_walk:?}");
    assert!(differ.is_empty(), "different: {differ:?}");
    assert_eq!(a.keys().filter(|k| k.starts_with("links\\")).count(), 101);
    // Every file size agrees with the (non-stale) metadata of an opened file.
    for (rel, (is_dir, is_link, size)) in &a {
        if !is_dir && !is_link {
            assert_eq!(*size, fs::metadata(root.join(rel)).unwrap().len(), "{rel}");
        }
    }
    println!("fixture: {} entries match; MFT {:?} vs walk {:?}", a.len(), mft.info.elapsed, walk.info.elapsed);
}

#[test]
#[ignore]
fn mft_matches_walk_on_system32() {
    if !elevated_or_skip() {
        return;
    }
    let target = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join("System32");
    let mft = scan::scan(&target, Method::Mft, &Progress::new()).expect("MFT scan");
    let walk = scan::scan(&target, Method::Walk, &Progress::new()).expect("walk scan");
    let (a, b) = (listing(&mft), listing(&walk));
    let common: Vec<_> = b.keys().filter(|k| a.contains_key(*k)).collect();
    let mut differ: Vec<_> = common.iter().filter(|k| a[**k] != b[**k]).map(|k| (k.as_str(), a[*k], b[*k])).collect();
    differ.sort_by_key(|(_, x, y)| std::cmp::Reverse(x.2.abs_diff(y.2)));
    println!(
        "System32: MFT {} entries in {:?}, walk {} entries in {:?} ({} unreadable), {} common, {} differ",
        a.len(),
        mft.info.elapsed,
        b.len(),
        walk.info.elapsed,
        walk.info.unreadable,
        common.len(),
        differ.len()
    );
    for d in differ.iter().take(20) {
        println!("  {d:?}");
    }
    assert!(common.len() as f64 >= b.len() as f64 * 0.99, "most walked entries should be found by the MFT scan");
    assert!(differ.len() as f64 <= common.len() as f64 * 0.001, "more than 0.1% of entries differ");
}
