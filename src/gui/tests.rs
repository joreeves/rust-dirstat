//! Headless GUI tests driven through egui_kittest (AccessKit queries + simulated input).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Key, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::*;

mod screenshots;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    for (rel, len) in [
        ("alpha_dir/big.bin", 600_000),
        ("alpha_dir/nested_dir/nested.dat", 250_000),
        ("small.txt", 120_000),
        ("tiny.log", 30_000),
    ] {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![0u8; len]).unwrap();
    }
    Fixture { _dir: dir, root }
}

fn start(root: &Path) -> Harness<'static, App> {
    let root = root.to_path_buf();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1200.0, 800.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        // Emoji icons come from system fonts at runtime; the headless harness only has egui's.
        .allow_missing_glyphs()
        .build_eframe(move |cc| App::new(cc, vec![root], Method::Walk));
    wait(&mut h, "scan and first treemap render", |app| app.loaded.is_some() && app.treemap.is_current());
    h
}

fn wait(h: &mut Harness<'_, App>, what: &str, cond: impl Fn(&App) -> bool) {
    let start = Instant::now();
    while !cond(h.state()) {
        assert!(start.elapsed() < Duration::from_secs(30), "timed out waiting for {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.step();
}

fn tree(app: &App) -> &Tree {
    &app.loaded.as_ref().unwrap().tree
}

fn view(app: &App) -> &View {
    &app.loaded.as_ref().unwrap().view
}

fn node_named(app: &App, name: &str) -> NodeId {
    let t = tree(app);
    t.preorder(t.root(), None).map(|(id, _)| id).find(|&id| &*t.node(id).name == name).unwrap()
}

fn click_at(h: &mut Harness<'_, App>, pos: Pos2, button: PointerButton) {
    h.hover_at(pos);
    h.step();
    h.event(Event::PointerButton { pos, button, pressed: true, modifiers: Modifiers::NONE });
    h.event(Event::PointerButton { pos, button, pressed: false, modifiers: Modifiers::NONE });
    h.step();
    h.step();
}

fn tile_center(h: &Harness<'_, App>, id: NodeId) -> Pos2 {
    let app = h.state();
    app.treemap.tile_center(tree(app), id).expect("tile on screen")
}

/// Renders the main layout to PNGs for visual inspection:
/// `DIRSTAT_SNAPSHOT_DIR=... cargo test --lib render_layout -- --ignored`.
#[test]
#[ignore]
fn render_layout() {
    let Some(out) = std::env::var_os("DIRSTAT_SNAPSHOT_DIR").map(PathBuf::from) else { return };
    let fx = fixture();
    let root = fx.root.clone();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1280.0, 840.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .wgpu()
        .build_eframe(move |cc| App::new(cc, vec![root], Method::Walk));
    wait(&mut h, "scan and first treemap render", |app| app.loaded.is_some() && app.treemap.is_current());
    h.render().unwrap().save(out.join("layout-default.png")).unwrap();
    // Drag the File types panel edge to the left, then far to the right.
    let edge = h.get_by_label("File types").rect().left() - 9.0;
    for (name, dx) in [("layout-wide-types.png", -350.0), ("layout-narrow-types.png", 500.0)] {
        let y = 300.0;
        h.hover_at(egui::pos2(edge, y));
        h.step();
        h.drag_at(egui::pos2(edge, y));
        h.step();
        for i in 1..=10 {
            h.hover_at(egui::pos2(edge + dx * i as f32 / 10.0, y));
            h.step();
        }
        h.drop_at(egui::pos2(edge + dx, y));
        h.run_steps(3);
        h.render().unwrap().save(out.join(name)).unwrap();
        // Return to the default width for the next case.
        let now = h.get_by_label("File types").rect().left() - 9.0;
        h.hover_at(egui::pos2(now, y));
        h.step();
        h.drag_at(egui::pos2(now, y));
        h.step();
        h.hover_at(egui::pos2(edge, y));
        h.step();
        h.drop_at(egui::pos2(edge, y));
        h.run_steps(3);
    }
}

/// Renders the welcome screen and the open Drives menu (see `render_layout`).
#[test]
#[ignore]
fn render_welcome() {
    let Some(out) = std::env::var_os("DIRSTAT_SNAPSHOT_DIR").map(PathBuf::from) else { return };
    let mut h = Harness::builder()
        .with_size(egui::vec2(1280.0, 840.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .wgpu()
        .build_eframe(|cc| App::new(cc, Vec::new(), Method::Auto));
    h.run_steps(3);
    let first = h.state().drives.first().map(|d| d.root.clone());
    if let Some(root) = first {
        h.state_mut().selected_drives.insert(root);
    }
    h.run_steps(2);
    h.render().unwrap().save(out.join("welcome.png")).unwrap();
    h.get_by_label("💽 Drives").click();
    h.run_steps(3);
    h.render().unwrap().save(out.join("drives-menu.png")).unwrap();
}

#[test]
fn welcome_screen_offers_drives_and_folder_picker() {
    let mut h = Harness::builder()
        .with_size(egui::vec2(1200.0, 800.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        // Walk, so an elevated run (CI) does not start a raw-volume MFT read of a real drive.
        .build_eframe(|cc| App::new(cc, Vec::new(), Method::Walk));
    h.run_steps(3);
    assert!(h.state().loaded.is_none() && h.state().scan.is_none());
    h.get_by_label(crate::APP_NAME);
    h.get_by_label_contains("Select one or more drives");
    assert_eq!(h.query_all_by_label("📂 Open folder…").count(), 2, "toolbar + welcome button");
    for d in &h.state().drives {
        h.get_by_label(&d.root.display().to_string());
    }
    // Clicking a drive only selects it; the Scan button starts the scan (cancelled right away).
    if let Some(d) = h.state().drives.first().cloned() {
        h.get_by_label(&d.root.display().to_string()).click();
        h.run_steps(2);
        assert!(h.state().scan.is_none(), "selecting a drive must not scan");
        assert!(h.state().selected_drives.contains(&d.root));
        h.get_by_label("Scan 1 selected drive").click();
        h.step();
        h.step();
        assert!(h.state().scan.is_some(), "scan started for {}", d.root.display());
        h.get_by_label("Cancel").click();
        h.step();
        wait(&mut h, "cancelled scan", |app| app.scan.is_none());
        assert!(h.state().message.as_ref().is_some_and(|m| m.text == "Scan cancelled"));
    }
}

#[test]
fn update_notice_appears_when_a_newer_release_is_found() {
    let mut h = Harness::builder()
        .with_size(egui::vec2(1200.0, 800.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .build_eframe(|cc| App::new(cc, Vec::new(), Method::Walk));
    h.run_steps(2);
    assert!(h.query_by_label_contains("Update available").is_none());
    // Stand in for the background check (tests never call GitHub).
    let (tx, rx) = mpsc::channel();
    h.state_mut().update_rx = Some(rx);
    h.run_steps(2);
    assert!(h.query_by_label_contains("Update available").is_none(), "nothing until the check answers");
    let url = "https://github.com/joreeves/rust-dirstat/releases/tag/v9.0.0";
    tx.send(update::Release { version: update::Version(9, 0, 0), tag: "v9.0.0".into(), url: url.into() }).unwrap();
    h.run_steps(2);
    h.get_by_label("⬆ Update available: v9.0.0");
    assert!(h.state().update_rx.is_none());
    assert_eq!(h.state().update.as_ref().map(|r| r.url.as_str()), Some(url));
}

/// Two temp folders stand in for drives so the test does not depend on the machine.
fn fake_drives(h: &mut Harness<'_, App>, roots: &[&Path]) {
    h.state_mut().drives = roots
        .iter()
        .map(|r| DriveInfo { root: r.to_path_buf(), filesystem: "NTFS".into(), total: 10_000_000, free: 1_000_000 })
        .collect();
    h.run_steps(2);
}

#[test]
fn multiple_drives_can_be_ticked_and_scanned_together() {
    let (a, b) = (fixture(), fixture());
    fs::write(b.root.join("extra.iso"), vec![0u8; 500_000]).unwrap();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1200.0, 800.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .build_eframe(|cc| App::new(cc, Vec::new(), Method::Walk));
    fake_drives(&mut h, &[&a.root, &b.root]);

    // Nothing ticked: the multi-drive button is disabled.
    h.get_by_label("Scan selected drives").click();
    h.run_steps(2);
    assert!(h.state().scan.is_none());

    // The drive button and the checkbox both select a drive, and neither starts a scan.
    h.get_by_label(&a.root.display().to_string()).click();
    h.run_steps(2);
    assert_eq!(h.state().selected_drives.len(), 1);
    h.get_by_label(&a.root.display().to_string()).click();
    h.run_steps(2);
    assert!(h.state().selected_drives.is_empty(), "clicking again deselects");
    for checkbox in h.get_all_by_role(egui::accesskit::Role::CheckBox) {
        checkbox.click();
    }
    h.run_steps(2);
    assert_eq!(h.state().selected_drives.len(), 2);
    assert!(h.state().scan.is_none());
    h.get_by_label("Scan 2 selected drives").click();
    h.step();
    wait(&mut h, "multi-drive scan", |app| app.loaded.is_some() && app.treemap.is_current());

    let app = h.state();
    let t = tree(app);
    let root = t.node(t.root());
    assert_eq!(root.size, 2_500_000);
    assert_eq!(root.children.len(), 2);
    // Ticked drives are scanned in drive order. The tree shows each location by its full path
    // (temp folders can be 8.3 short names, e.g. RUNNER~1 on CI).
    let mut expected = vec![a.root.clone(), b.root.clone()];
    expected.sort();
    let shown: Vec<PathBuf> = expected.iter().map(|p| scan::normalize_root(p)).collect();
    assert_eq!(&*root.name, format!("{} + {}", shown[0].display(), shown[1].display()));
    assert_eq!(app.loaded.as_ref().unwrap().targets, expected);
    // Each drive shows as its own top-level row, and paths below it stay absolute.
    let b_shown = scan::normalize_root(&b.root);
    h.get_by_label(&format!("📁 {}", b_shown.display()));
    let extra = node_named(app, "extra.iso");
    assert_eq!(t.path_of(extra).unwrap(), b_shown.join("extra.iso"));

    // Rescan repeats the same multi-drive scan.
    h.get_by_label("⟳ Rescan").click();
    h.step();
    assert!(h.state().scan.as_ref().is_some_and(|s| s.paths.len() == 2));
    wait(&mut h, "multi-drive rescan", |app| app.loaded.is_some());
}

#[test]
fn drives_menu_stays_open_while_ticking() {
    let (a, b) = (fixture(), fixture());
    let mut h = start(&a.root);
    fake_drives(&mut h, &[&a.root, &b.root]);
    h.get_by_label("💽 Drives").click();
    h.run_steps(2);
    let boxes = h.get_all_by_role(egui::accesskit::Role::CheckBox).count();
    assert_eq!(boxes, 2);
    let before = h.state().loaded.as_ref().unwrap().targets.clone();
    for i in 0..2 {
        h.get_all_by_role(egui::accesskit::Role::CheckBox).nth(i).unwrap().click();
        h.run_steps(2);
        assert_eq!(h.state().selected_drives.len(), i + 1);
        assert!(h.state().scan.is_none(), "ticking a drive must not scan");
    }
    // Still open: the checkboxes and scan buttons are still there.
    assert_eq!(h.get_all_by_role(egui::accesskit::Role::CheckBox).count(), 2);
    assert_eq!(h.state().loaded.as_ref().unwrap().targets, before);
    h.get_by_label("Scan 2 selected drives").click();
    h.step();
    assert!(h.state().scan.as_ref().is_some_and(|s| s.paths.len() == 2));
    wait(&mut h, "multi-drive scan from the menu", |app| app.loaded.is_some());
}

#[test]
fn scan_populates_tree_types_and_status() {
    let fx = fixture();
    let h = start(&fx.root);
    let app = h.state();
    assert_eq!(tree(app).node(tree(app).root()).size, 1_000_000);
    assert_eq!(tree(app).info.method, "walk");
    // Root is expanded: its children are listed; grandchildren are not.
    h.get_by_label("📁 alpha_dir");
    h.get_by_label("📄 small.txt");
    assert!(h.query_by_label("📄 big.bin").is_none());
    // File type list, largest first.
    h.get_by_label(".bin");
    h.get_by_label(".dat");
    // Status bar summary.
    h.get_by_label_contains(&format!("{}  •  4 files  •  2 folders", format_size(1_000_000)));
}

#[test]
fn clicking_rows_and_keyboard_navigation() {
    let fx = fixture();
    let mut h = start(&fx.root);
    h.get_by_label("📁 alpha_dir").click();
    h.step();
    let alpha = node_named(h.state(), "alpha_dir");
    assert_eq!(view(h.state()).selected, Some(alpha));

    h.key_press(Key::ArrowRight); // expand
    h.step();
    h.step();
    h.get_by_label("📄 big.bin");
    h.key_press(Key::ArrowDown); // first child: nested_dir (250 KB) < big.bin (600 KB) -> big.bin first
    h.step();
    h.step();
    assert_eq!(view(h.state()).selected, Some(node_named(h.state(), "big.bin")));
    h.key_press(Key::ArrowLeft); // file: go to parent
    h.step();
    h.key_press(Key::ArrowLeft); // collapse
    h.step();
    h.step();
    assert_eq!(view(h.state()).selected, Some(alpha));
    assert!(h.query_by_label("📄 big.bin").is_none());
}

#[test]
fn treemap_click_selects_and_reveals_in_tree() {
    let fx = fixture();
    let mut h = start(&fx.root);
    let nested = node_named(h.state(), "nested.dat");
    let pos = tile_center(&h, nested);
    click_at(&mut h, pos, PointerButton::Primary);
    assert_eq!(view(h.state()).selected, Some(nested));
    // Its folders were expanded so the row is visible.
    h.get_by_label("📄 nested.dat");
    let status = format!("nested.dat  ({})", format_size(250_000));
    h.get_by_label_contains(&status);
}

#[test]
fn treemap_double_click_zooms_and_toolbar_zooms_out() {
    let fx = fixture();
    let mut h = start(&fx.root);
    let nested = node_named(h.state(), "nested.dat");
    let nested_dir = node_named(h.state(), "nested_dir");
    let alpha = node_named(h.state(), "alpha_dir");
    let pos = tile_center(&h, nested);
    h.hover_at(pos);
    h.step();
    // Two clicks one frame (1/60 s) apart: inside egui's double-click window.
    for _ in 0..2 {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.step();
    }
    h.step();
    assert_eq!(view(h.state()).zoom, nested_dir, "double-clicking a file zooms to its folder");
    wait(&mut h, "zoomed render", |app| app.treemap.is_current());

    h.get_by_label("🔍 Zoom out").click();
    h.step();
    h.step();
    assert_eq!(view(h.state()).zoom, alpha);
    h.get_by_label("Reset").click();
    h.step();
    h.step();
    assert_eq!(view(h.state()).zoom, tree(h.state()).root());
}

#[test]
fn context_menu_copy_path() {
    let fx = fixture();
    let mut h = start(&fx.root);
    h.get_by_label("📄 small.txt").click_secondary();
    h.step();
    h.step();
    h.get_by_label("Copy path").click();
    h.step();
    h.step();
    let msg = h.state().message.as_ref().map(|m| m.text.clone()).unwrap_or_default();
    assert!(msg.starts_with("Copied") && msg.ends_with("small.txt"), "{msg}");
    // The menu also offers deletion and closes after a choice.
    assert!(h.query_by_label("Move to Recycle Bin…").is_none());
}

#[test]
fn extension_row_toggles_treemap_highlight() {
    let fx = fixture();
    let mut h = start(&fx.root);
    let bin = tree(h.state()).exts().iter().position(|e| &*e.name == "bin").unwrap() as u32;
    h.get_by_label(".bin").click();
    h.step();
    h.step();
    assert_eq!(view(h.state()).highlight_ext, Some(bin));
    h.get_by_label("Clear highlight").click();
    h.step();
    h.step();
    assert_eq!(view(h.state()).highlight_ext, None);
}

#[test]
fn delete_key_opens_confirmation_and_cancel_keeps_file() {
    let fx = fixture();
    let mut h = start(&fx.root);
    h.get_by_label("📄 small.txt").click();
    h.step();
    h.key_press(Key::Delete);
    h.step();
    h.step();
    assert!(h.state().confirm_delete.is_some());
    h.get_by_label("Move to Recycle Bin?");
    h.get_by_label("Cancel").click();
    h.step();
    h.step();
    assert!(h.state().confirm_delete.is_none());
    assert!(fx.root.join("small.txt").exists());
}

#[test]
fn completed_delete_and_rescan_update_tree_and_treemap() {
    let fx = fixture();
    let mut h = start(&fx.root);
    let big = node_named(h.state(), "big.bin");
    let alpha = node_named(h.state(), "alpha_dir");

    // A finished Recycle Bin job removes the node and re-renders.
    let path = tree(h.state()).path_of(big).unwrap();
    h.state_mut().finish_job(JobResult::Deleted { node: big, path, result: Ok(()) });
    assert_eq!(tree(h.state()).node(tree(h.state()).root()).size, 400_000);
    assert!(!h.state().treemap.is_current());
    wait(&mut h, "re-render after delete", |app| app.treemap.is_current());
    assert!(h.state().treemap.tile_center(tree(h.state()), big).is_none());

    // Rescan of a folder picks up changes on disk.
    fs::write(fx.root.join("alpha_dir").join("fresh.iso"), vec![1u8; 70_000]).unwrap();
    let ctx = h.ctx.clone();
    h.state_mut().apply(Action::Rescan(alpha), &ctx);
    wait(&mut h, "rescan", |app| app.jobs_running == 0);
    let t = tree(h.state());
    // big.bin still exists on disk (only the job result was simulated), so it reappears.
    assert_eq!(t.node(alpha).size, 600_000 + 250_000 + 70_000);
    assert_eq!(t.node(t.root()).size, 1_070_000);
    assert!(h.state().message.as_ref().is_some_and(|m| m.text.starts_with("Rescanned")));
}
