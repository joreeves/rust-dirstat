//! README screenshots, rendered from a made-up drive so they contain nothing from this machine
//! (no real paths, user names or drive sizes). Regenerate with:
//!
//! ```text
//! cargo test --lib readme_screenshots -- --ignored
//! ```
//!
//! The PNGs are written to `docs/screenshots/`.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use crate::model::flags;

use super::*;

const KB: u64 = 1 << 10;
const MB: u64 = 1 << 20;
const GB: u64 = 1 << 30;

/// Deterministic xorshift, so every run draws the same picture.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn size(&mut self, range: &Range<u64>) -> u64 {
        range.start + self.next() % (range.end - range.start).max(1)
    }

    fn count(&mut self, range: Range<usize>) -> usize {
        range.start + (self.next() as usize) % (range.end - range.start).max(1)
    }
}

struct Demo {
    tree: Tree,
    rng: Rng,
    /// Running number for generated file names.
    serial: usize,
}

impl Demo {
    fn dir(&mut self, parent: NodeId, name: &str) -> NodeId {
        self.tree.add_child(parent, name, 0, flags::DIR)
    }

    fn file(&mut self, parent: NodeId, name: &str, size: u64) {
        self.tree.add_child(parent, name, size, 0);
    }

    /// `count` files named `{prefix}{serial}.{ext}` with sizes drawn from `sizes`.
    fn files(&mut self, parent: NodeId, count: usize, prefix: &str, ext: &str, sizes: Range<u64>) {
        for _ in 0..count {
            self.serial += 1;
            let size = self.rng.size(&sizes);
            self.file(parent, &format!("{prefix}{:04}.{ext}", self.serial), size);
        }
    }
}

/// A plausible personal data drive: games, videos, photos, music, code, backups and VMs.
fn demo_tree() -> Tree {
    let mut d = Demo { tree: Tree::new("D:\\"), rng: Rng(0x9E37_79B9_7F4A_7C15), serial: 0 };
    let root = d.tree.root();

    let games = d.dir(root, "Games");
    for (name, paks, pak_size) in [("Starfall Odyssey", 14, GB..5 * GB), ("Pixel Racer", 6, 512 * MB..3 * GB), ("Dungeon Tactics", 5, GB..2 * GB)] {
        let game = d.dir(games, name);
        let content = d.dir(game, "Content");
        d.files(content, paks, "pakchunk", "pak", pak_size);
        let movies = d.dir(game, "Movies");
        d.files(movies, 18, "cinematic", "bik", 40 * MB..400 * MB);
        let bin = d.dir(game, "Binaries");
        d.file(bin, "Game.exe", 180 * MB);
        d.files(bin, 30, "module", "dll", MB..60 * MB);
    }

    let videos = d.dir(root, "Videos");
    for (name, count, ext, sizes) in [
        ("Holidays", 40, "mp4", 200 * MB..2560 * MB),
        ("Recordings", 22, "mkv", 300 * MB..3 * GB),
        ("Drone", 30, "mov", 100 * MB..1536 * MB),
    ] {
        let dir = d.dir(videos, name);
        d.files(dir, count, "clip", ext, sizes);
    }

    let photos = d.dir(root, "Photos");
    for year in 2021..=2025 {
        let y = d.dir(photos, &year.to_string());
        for month in 1..=12 {
            let m = d.dir(y, &format!("{month:02}"));
            let (jpgs, raws) = (d.rng.count(15..70), d.rng.count(0..25));
            d.files(m, jpgs, "IMG_", "jpg", 2 * MB..9 * MB);
            d.files(m, raws, "IMG_", "cr3", 22 * MB..32 * MB);
            let clips = d.rng.count(0..3);
            d.files(m, clips, "VID_", "mp4", 40 * MB..400 * MB);
        }
    }

    let music = d.dir(root, "Music");
    let library = d.dir(music, "Library");
    for artist in ["Aurora Lane", "The Night Shift", "Kite Theory", "Mono Coast", "Velvet Static", "Paper Lanterns", "Northbound"] {
        let a = d.dir(library, artist);
        for album in 1..=3 {
            let al = d.dir(a, &format!("Album {album}"));
            d.files(al, 12, "track", "flac", 20 * MB..48 * MB);
        }
    }
    let podcasts = d.dir(music, "Podcasts");
    d.files(podcasts, 90, "episode", "mp3", 30 * MB..95 * MB);

    let projects = d.dir(root, "Projects");
    let web = d.dir(projects, "website");
    let modules = d.dir(web, "node_modules");
    for i in 1..=70 {
        let pkg = d.dir(modules, &format!("package-{i:02}"));
        let n = d.rng.count(4..40);
        d.files(pkg, n, "index", "js", KB..600 * KB);
        d.file(pkg, "package.json", 2 * KB);
    }
    let src = d.dir(web, "src");
    d.files(src, 45, "component", "ts", 2 * KB..40 * KB);
    let rust = d.dir(projects, "rust-app");
    let target = d.dir(rust, "target");
    let release = d.dir(target, "release");
    d.file(release, "app.exe", 14 * MB);
    d.file(release, "app.pdb", 46 * MB);
    let deps = d.dir(release, "deps");
    d.files(deps, 160, "lib", "rlib", 100 * KB..40 * MB);
    d.files(deps, 160, "lib", "rmeta", 50 * KB..8 * MB);
    let rsrc = d.dir(rust, "src");
    d.files(rsrc, 30, "module", "rs", KB..60 * KB);
    let data = d.dir(projects, "data-analysis");
    d.files(data, 18, "export", "csv", 50 * MB..GB);
    d.files(data, 10, "table", "parquet", 100 * MB..800 * MB);
    d.files(data, 14, "notebook", "ipynb", 100 * KB..6 * MB);

    let backups = d.dir(root, "Backups");
    let laptop = d.dir(backups, "Laptop");
    d.files(laptop, 6, "backup", "zip", 5 * GB..20 * GB);
    let phone = d.dir(backups, "Phone");
    d.files(phone, 4, "backup", "zip", 3 * GB..12 * GB);

    let vms = d.dir(root, "Virtual Machines");
    let ubuntu = d.dir(vms, "Ubuntu");
    d.file(ubuntu, "Ubuntu.vhdx", 38 * GB);
    let dev = d.dir(vms, "Windows Dev");
    d.file(dev, "Windows Dev.vhdx", 64 * GB);
    let isos = d.dir(vms, "ISOs");
    d.files(isos, 3, "installer", "iso", 3 * GB..6 * GB);

    let docs = d.dir(root, "Documents");
    d.files(docs, 120, "document", "pdf", 100 * KB..20 * MB);
    d.files(docs, 80, "report", "docx", 30 * KB..3 * MB);
    d.files(docs, 40, "budget", "xlsx", 20 * KB..2 * MB);
    d.files(docs, 25, "slides", "pptx", MB..60 * MB);

    d.tree.finalize();
    d.tree.info.method = "MFT";
    d.tree.info.elapsed = Duration::from_millis(1900);
    d.tree
}

fn demo_drives() -> Vec<DriveInfo> {
    [("C:\\", "NTFS", 476 * GB, 121 * GB), ("D:\\", "NTFS", 1863 * GB, 902 * GB), ("E:\\", "exFAT", 931 * GB, 618 * GB)]
        .into_iter()
        .map(|(root, fs, total, free)| DriveInfo { root: PathBuf::from(root), filesystem: fs.into(), total, free })
        .collect()
}

/// A rendering harness that uses the system fonts for icons, like the real window does.
fn harness() -> Harness<'static, App> {
    let mut h = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .wgpu()
        .build_eframe(|cc| {
            cc.egui_ctx.add_font_provider(Arc::new(egui_system_font_provider::SystemFontProvider::new()));
            App::new(cc, Vec::new(), Method::Auto)
        });
    let app = h.state_mut();
    app.drives = demo_drives();
    app.elevated = true;
    h.run_steps(3);
    h
}

fn load_demo(h: &mut Harness<'_, App>) -> Arc<Tree> {
    h.state_mut().set_tree(demo_tree(), vec![PathBuf::from("D:\\")]);
    wait(h, "demo treemap", |app| app.loaded.is_some() && app.treemap.is_current());
    h.state().loaded.as_ref().unwrap().tree.clone()
}

fn find(tree: &Tree, path: &[&str]) -> NodeId {
    path.iter().fold(tree.root(), |at, name| {
        *tree.node(at).children.iter().find(|&&c| &*tree.node(c).name == *name).unwrap_or_else(|| panic!("no {name}"))
    })
}

fn save(h: &mut Harness<'_, App>, name: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs").join("screenshots");
    fs::create_dir_all(&dir).unwrap();
    h.run_steps(3);
    h.render().unwrap().save(dir.join(name)).unwrap();
}

/// An older backup of [`demo_tree`]: some folders are missing (not backed up yet), an old one
/// that was since deleted is still there, and some files have other sizes.
fn demo_backup(src: &Tree) -> Tree {
    let mut d = Demo { tree: Tree::new("E:\\Backup"), rng: Rng(0x2545_F491_4F6C_DD1D), serial: 0 };
    let not_backed_up = ["Games\\Pixel Racer", "Photos\\2025", "Projects\\rust-app\\target", "Videos\\Drone"];
    let mut stack = vec![(src.root(), d.tree.root())];
    while let Some((from, to)) = stack.pop() {
        for &c in &src.node(from).children {
            let n = src.node(c);
            let path = src.path_of(c).unwrap();
            let rel = path.strip_prefix("D:\\").unwrap();
            if not_backed_up.iter().any(|p| rel == std::path::Path::new(p)) {
                continue;
            }
            if n.is_dir() {
                let id = d.dir(to, &n.name);
                stack.push((c, id));
                continue;
            }
            let size = match crate::model::extension_of(&n.name).as_str() {
                // Disk images and laptop backups have grown since; phone backups have shrunk.
                "vhdx" => n.size - 9 * GB,
                "zip" if rel.starts_with("Backups\\Laptop") => n.size - d.rng.size(&(GB..3 * GB)),
                "zip" => n.size + d.rng.size(&(GB..2 * GB)),
                "docx" | "xlsx" if d.rng.next().is_multiple_of(4) => n.size / 2,
                _ => n.size,
            };
            d.file(to, &n.name, size);
        }
    }
    let root = d.tree.root();
    let old = d.dir(root, "Old Laptop");
    d.files(old, 9, "archive", "zip", 2 * GB..5 * GB);
    let docs = d.dir(old, "Documents");
    d.files(docs, 60, "document", "pdf", 100 * KB..20 * MB);
    d.tree.finalize();
    d.tree.info.method = "walk";
    d.tree.info.elapsed = Duration::from_millis(6400);
    d.tree
}

#[test]
#[ignore]
fn readme_screenshots() {
    // Main window: a folder selected in the tree and outlined in the treemap.
    let mut h = harness();
    let tree = load_demo(&mut h);
    let year = find(&tree, &["Photos", "2024"]);
    h.state_mut().loaded.as_mut().unwrap().view.reveal(&tree, year);
    save(&mut h, "main.png");

    // Zoomed into Photos, with .jpg highlighted and a tile's tooltip showing.
    let photos = find(&tree, &["Photos"]);
    h.state_mut().loaded.as_mut().unwrap().view.zoom = photos;
    h.get_by_label(".jpg").click();
    wait(&mut h, "zoomed treemap", |app| app.treemap.is_showing(photos));
    let photo = tree
        .preorder(find(&tree, &["Photos", "2023"]), None)
        .map(|(id, _)| id)
        .filter(|&id| tree.node(id).name.ends_with(".jpg"))
        .max_by_key(|&id| tree.node(id).size)
        .unwrap();
    let at = tile_center(&h, photo);
    for dx in [2.0, 1.0, 0.0] {
        h.hover_at(at + egui::vec2(dx, 0.0));
        h.step();
    }
    // The tooltip opens once the pointer has rested for egui's tooltip delay.
    for _ in 0..120 {
        if h.query_by_label_contains("Type: .jpg").is_some() {
            break;
        }
        h.step();
    }
    assert!(h.query_by_label_contains("Type: .jpg").is_some(), "tooltip shown");
    save(&mut h, "file-types.png");

    // Welcome screen with two drives ticked for a combined scan.
    let mut h = harness();
    h.state_mut().elevated = false;
    h.state_mut().selected_drives.extend([PathBuf::from("D:\\"), PathBuf::from("E:\\")]);
    save(&mut h, "welcome.png");

    // The drive compared with its backup, with a folder missing from the backup selected.
    let mut h = harness();
    let left = demo_tree();
    let mut diff = crate::diff::Diff::new(&left, &demo_backup(&left));
    diff.elapsed = Duration::from_millis(6900);
    h.state_mut().set_diff(diff, [PathBuf::from("D:\\"), PathBuf::from("E:\\Backup")]);
    wait(&mut h, "comparison treemap", |app| app.compared.is_some() && app.treemap.is_current());
    let c = h.state_mut().compared.as_mut().unwrap();
    let photos = find(&c.diff.tree, &["Photos", "2025"]);
    let tree = &c.diff.tree;
    c.view.reveal(tree, photos);
    save(&mut h, "compare.png");
}
