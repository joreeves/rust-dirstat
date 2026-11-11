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
    tx.send(update::Release { version: update::Version(9, 0, 0), tag: "v9.0.0".into(), url: url.into(), installer: None }).unwrap();
    h.run_steps(2);
    // Tests run from target\, not an installed copy: the notice links to the release page.
    h.get_by_label("⬆ Update available: v9.0.0");
    assert!(h.state().update_rx.is_none());
    assert_eq!(h.state().update.as_ref().map(|r| r.url.as_str()), Some(url));
}

#[test]
fn an_installed_copy_offers_to_update_itself() {
    let mut h = Harness::builder()
        .with_size(egui::vec2(1200.0, 800.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .build_eframe(|cc| App::new(cc, Vec::new(), Method::Walk));
    h.state_mut().installed = true;
    // Nothing listens on port 9 here, so the download fails straight away (tests never download).
    let installer = update::Asset {
        name: "rust-dirstat-v9.0.0-windows-x64-setup.exe".into(),
        url: "http://127.0.0.1:9/setup.exe".into(),
        size: 1000,
    };
    let url = "https://github.com/joreeves/rust-dirstat/releases/tag/v9.0.0";
    h.state_mut().update =
        Some(update::Release { version: update::Version(9, 0, 0), tag: "v9.0.0".into(), url: url.into(), installer: Some(installer) });
    h.run_steps(2);
    h.get_by_label("What's new");
    h.get_by_label("⬆ Update to v9.0.0").click();
    h.step();
    assert!(h.state().updating.is_some(), "the download started");
    wait(&mut h, "the download to fail", |app| app.update_error.is_some());
    h.run_steps(2);
    assert!(h.state().updating.is_none());
    h.get_by_label_contains("Update failed: could not reach GitHub");
    h.get_by_label("Try again to v9.0.0");
}

#[test]
fn window_icon_is_the_256px_app_icon() {
    let icon = window_icon();
    assert_eq!((icon.width, icon.height), (256, 256));
    assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    // Transparent corners (rounded tile), opaque centre.
    assert_eq!(icon.rgba[3], 0, "top-left corner is transparent");
    assert_eq!(icon.rgba[(128 * 256 + 128) * 4 + 3], 255, "centre is opaque");
}

fn wheel_event(unit: egui::MouseWheelUnit, y: f32, modifiers: Modifiers) -> Event {
    Event::MouseWheel {
        unit,
        delta: egui::vec2(0.0, y),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
        modifiers,
    }
}

#[test]
fn wheel_notches_are_passed_on_as_steps_egui_applies_at_once() {
    let moved = Event::PointerMoved(egui::pos2(5.0, 5.0));
    let touchpad = wheel_event(egui::MouseWheelUnit::Point, 30.0, Modifiers::NONE);
    let mut events = vec![moved.clone(), wheel_event(egui::MouseWheelUnit::Line, -3.0, Modifiers::SHIFT), touchpad.clone()];
    scroll_wheel_immediately(&mut events, 40.0);

    assert_eq!(events.first(), Some(&moved));
    assert_eq!(events.last(), Some(&touchpad), "point deltas are left as they are");
    let steps = &events[1..events.len() - 1];
    assert!(steps.len() > 1);
    let mut total = egui::Vec2::ZERO;
    for step in steps {
        let Event::MouseWheel { unit, delta, phase, source, modifiers } = step else { panic!("{step:?}") };
        assert_eq!(*unit, egui::MouseWheelUnit::Point);
        assert_eq!((*phase, *source, *modifiers), (egui::TouchPhase::Move, egui::MouseWheelSource::Wheel, Modifiers::SHIFT));
        assert!(delta.length() < 8.0, "{delta:?} is under egui's smoothing threshold");
        total += *delta;
    }
    assert!((total - egui::vec2(0.0, -120.0)).length() < 1e-3, "3 lines at 40 points each, got {total:?}");
}

/// Shows a tree built in memory, without scanning.
fn start_with_tree(t: Tree) -> Harness<'static, App> {
    let mut h = Harness::builder()
        .with_size(egui::vec2(1200.0, 800.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .build_eframe(|cc| App::new(cc, Vec::new(), Method::Walk));
    h.state_mut().set_tree(t, vec![PathBuf::from("D:\\")]);
    wait(&mut h, "treemap", |app| app.loaded.is_some() && app.treemap.is_current());
    h
}

#[test]
fn clicking_among_many_small_files_selects_a_file_not_their_folder() {
    // Like a browser cache: a few large files and thousands of small ones in one folder.
    use crate::model::flags;
    let mut t = Tree::new("D:\\");
    let root = t.root();
    let cache = t.add_child(root, "cache", 0, flags::DIR);
    for i in 0..4u64 {
        t.add_child(cache, &format!("large{i}_0"), 25_000_000, 0);
    }
    // Each under 2 px² of the treemap: together about an eighth of it.
    for i in 0..40_000u64 {
        t.add_child(cache, &format!("small{i:05}_0"), 400 + (i * 37) % 600, 0);
    }
    t.add_child(root, "video.mp4", 100_000_000, 0);
    t.finalize();
    let mut h = start_with_tree(t);

    // Each small file has a tile, and clicking it selects that file.
    for name in ["small00000_0", "small20000_0", "small39999_0"] {
        let id = node_named(h.state(), name);
        let pos = tile_center(&h, id);
        click_at(&mut h, pos, PointerButton::Primary);
        assert_eq!(view(h.state()).selected, Some(id), "{name}");
    }
    // Nowhere in the folder's area does a click select the folder itself.
    let cache = node_named(h.state(), "cache");
    let area = h.state().treemap.tile_rect(tree(h.state()), cache).expect("folder tile");
    for fy in [0.05, 0.35, 0.65, 0.95] {
        for fx in [0.05, 0.35, 0.65, 0.95] {
            let pos = area.lerp_inside(egui::vec2(fx, fy));
            click_at(&mut h, pos, PointerButton::Primary);
            let selected = view(h.state()).selected.expect("a selection");
            let n = tree(h.state()).node(selected);
            assert!(n.is_file(), "clicking {pos:?} selected {}", n.name);
        }
    }
}

#[test]
fn a_wheel_notch_scrolls_the_folder_list_by_a_full_step_right_away() {
    let mut big = Tree::new("D:\\");
    let root = big.root();
    for f in 0..300u64 {
        big.add_child(root, &format!("file{f:03}.bin"), 1_000_000 - f, 0);
    }
    big.finalize();
    let mut h = start_with_tree(big);
    let row_top = |h: &Harness<'_, App>| h.get_by_label("📄 file012.bin").rect().top();
    h.hover_at(h.get_by_label("📄 file008.bin").rect().center());
    h.run_steps(2);
    let before = row_top(&h);

    // The app does this in raw_input_hook, which the test harness does not call.
    let speed = h.ctx.options(|o| o.input_options.line_scroll_speed);
    let mut events = vec![wheel_event(egui::MouseWheelUnit::Line, -1.0, Modifiers::NONE)];
    scroll_wheel_immediately(&mut events, speed);
    for event in events {
        h.event(event);
    }
    // egui takes the wheel input after laying out the list, so it shows in the next frame.
    h.run_steps(2);
    let moved = before - row_top(&h);
    assert!((moved - speed).abs() < 0.5, "moved {moved} of {speed} points");
}

/// Prints frame times (UI pass plus tessellation) on a 400,000-file tree while hovering and
/// scrolling: `cargo test --release --lib frame_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn frame_cost() {
    use crate::model::flags;
    let mut big = Tree::new("D:\\");
    let root = big.root();
    for d in 0..200u64 {
        let dir = big.add_child(root, &format!("folder {d:03}"), 0, flags::DIR);
        for f in 0..2000u64 {
            let size = 1000 + (d * 7919 + f * 104_729) % 5_000_000;
            big.add_child(dir, &format!("file{f:04}.e{}", (d * 7 + f) % 40), size, 0);
        }
    }
    big.finalize();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1280.0, 840.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .build_eframe(|cc| App::new(cc, Vec::new(), Method::Walk));
    h.state_mut().set_tree(big, vec![PathBuf::from("D:\\")]);
    wait(&mut h, "treemap", |app| app.loaded.is_some() && app.treemap.is_current());
    // Expand the first folder so the table has 2,000+ rows.
    let first = tree(h.state()).node(tree(h.state()).root()).children[0];
    let view = &mut h.state_mut().loaded.as_mut().unwrap().view;
    view.expanded.insert(first);
    view.rows_dirty = true;
    h.run_steps(2);
    let tiles: Vec<Pos2> = {
        let t = tree(h.state());
        t.node(first).children.iter().take(40).filter_map(|&c| h.state().treemap.tile_center(t, c)).collect()
    };
    assert!(!tiles.is_empty());

    fn measure(h: &mut Harness<'_, App>, name: &str, mut input: impl FnMut(&mut Harness<'_, App>, usize)) {
        let mut times: Vec<f64> = (0..120)
            .map(|i| {
                input(h, i);
                let start = Instant::now();
                h.step();
                let shapes = h.output().shapes.clone();
                h.ctx.tessellate(shapes, 1.0);
                start.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        times.sort_by(f64::total_cmp);
        let avg = times.iter().sum::<f64>() / times.len() as f64;
        println!("{name:<28} avg {avg:6.2} ms   p95 {:6.2} ms   max {:6.2} ms", times[times.len() * 95 / 100], times[times.len() - 1]);
    }
    let wheel = |h: &mut Harness<'_, App>, dy: f32| {
        h.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, dy),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: Modifiers::NONE,
        });
    };
    measure(&mut h, "hover tree rows", |h, i| h.hover_at(egui::pos2(300.0, 90.0 + (i % 14) as f32 * 22.0)));
    measure(&mut h, "scroll tree (wheel)", |h, i| {
        h.hover_at(egui::pos2(300.0, 200.0));
        wheel(h, if i % 40 < 20 { -1.0 } else { 1.0 });
    });
    measure(&mut h, "hover treemap", |h, i| h.hover_at(tiles[i % tiles.len()]));
    h.state_mut().loaded.as_mut().unwrap().view.highlight_ext = Some(0);
    measure(&mut h, "hover treemap + type shown", |h, i| h.hover_at(tiles[i % tiles.len()]));
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
    wait(&mut h, "highlighted treemap", |app| app.treemap.shown_highlight() == Some(Some(bin)));
    h.get_by_label("Clear highlight").click();
    h.step();
    h.step();
    assert_eq!(view(h.state()).highlight_ext, None);
    wait(&mut h, "plain treemap", |app| app.treemap.shown_highlight() == Some(None));
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

fn compared(app: &App) -> &Compared {
    app.compared.as_ref().expect("a comparison on screen")
}

/// The node of the comparison at `rel` (a path relative to the compared roots).
fn diff_node(app: &App, rel: &str) -> NodeId {
    let d = &compared(app).diff;
    d.tree
        .preorder(d.tree.root(), None)
        .map(|(id, _)| id)
        .find(|&id| d.relative_path(id).is_some_and(|p| p == Path::new(rel)))
        .unwrap_or_else(|| panic!("{rel} not in the comparison"))
}

/// Two fixtures where small.txt is larger on the right, tiny.log is only on the left and
/// extra.iso only on the right.
fn differing_fixtures() -> (Fixture, Fixture) {
    let (a, b) = (fixture(), fixture());
    fs::write(b.root.join("small.txt"), vec![0u8; 150_000]).unwrap();
    fs::remove_file(b.root.join("tiny.log")).unwrap();
    fs::write(b.root.join("extra.iso"), vec![0u8; 500_000]).unwrap();
    (a, b)
}

fn idle_harness() -> Harness<'static, App> {
    Harness::builder()
        .with_size(egui::vec2(1200.0, 800.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .build_eframe(|cc| App::new(cc, Vec::new(), Method::Walk))
}

/// The summary panel's button for a kind of difference (the table shows the same words).
fn summary_button<'a>(h: &'a Harness<'_, App>, label: &'a str) -> egui_kittest::Node<'a> {
    h.get_by(move |n| n.role() == egui::accesskit::Role::Button && n.label().as_deref() == Some(label))
}

fn is_disabled(node: egui_kittest::Node<'_>) -> bool {
    use egui_kittest::kittest::NodeT;
    node.accesskit_node().is_disabled()
}

#[test]
fn compare_two_folders_from_the_dialog() {
    let (a, b) = differing_fixtures();
    let mut h = idle_harness();
    fake_drives(&mut h, &[&a.root, &b.root]);

    // The dialog suggests the first two drives.
    h.get_by_label("↔ Compare…").click();
    // The dialog's grid is laid out invisibly in its first frame.
    h.run_steps(4);
    let dialog = h.state().compare_dialog.as_ref().expect("dialog open");
    assert_eq!(dialog.left, a.root.display().to_string());
    assert_eq!(dialog.right, b.root.display().to_string());
    h.get_by_label("Compare").click();
    h.step();
    assert!(h.state().compare_job.is_some());
    assert!(h.state().compare_dialog.is_none());
    wait(&mut h, "comparison", |app| app.compared.is_some() && app.treemap.is_current());

    let app = h.state();
    let d = &compared(app).diff;
    assert_eq!(d.changes(), 3);
    assert_eq!((d.left.size, d.right.size), (1_000_000, 1_500_000));
    assert!(app.message.as_ref().is_some_and(|m| m.text.starts_with("Found 3 differences")));
    h.get_by_label_contains("  •  3 differences  •  ");
    h.get_by_label("Right is 488 KiB larger");

    // Differences are listed; identical items only on request.
    h.get_by_label("📄 extra.iso");
    h.get_by_label("📄 small.txt");
    h.get_by_label("📄 tiny.log");
    h.get_by_label("+29.3 KiB");
    assert!(h.query_by_label("📁 alpha_dir").is_none());
    h.get_by_label("Show identical items in the table").click();
    h.run_steps(2);
    h.get_by_label("📁 alpha_dir");
    assert!(compared(h.state()).show_identical);

    // A kind of difference in the summary highlights its tiles.
    summary_button(&h, "Only in right").click();
    h.run_steps(2);
    let only_right = crate::diff::Status::OnlyRight.index() as u32;
    assert_eq!(compared(h.state()).view.highlight_ext, Some(only_right));
    wait(&mut h, "highlighted treemap", |app| app.treemap.shown_highlight() == Some(Some(only_right)));
    assert!(is_disabled(summary_button(&h, "Same size")), "identical items have no tiles");

    // Clicking a tile selects the item and the status bar shows both sizes.
    let extra = diff_node(h.state(), "extra.iso");
    let pos = h.state().treemap.tile_center(&compared(h.state()).diff.tree, extra).expect("extra.iso tile");
    click_at(&mut h, pos, PointerButton::Primary);
    assert_eq!(compared(h.state()).view.selected, Some(extra));
    h.get_by_label_contains(&format!("extra.iso  (left —, right {})", format_size(500_000)));

    // The right-click menu works per side: extra.iso is only on the right.
    h.get_by_label("📄 extra.iso").click_secondary();
    h.run_steps(2);
    assert!(is_disabled(h.get_by_label("Reveal left in Explorer")));
    h.get_by_label("Copy right path").click();
    h.run_steps(2);
    let msg = h.state().message.as_ref().map(|m| m.text.clone()).unwrap_or_default();
    let expected = scan::normalize_root(&b.root).join("extra.iso");
    assert_eq!(msg, format!("Copied {}", expected.display()));

    // Rescan compares the same two locations again.
    fs::write(a.root.join("tiny.log"), vec![0u8; 10]).unwrap();
    h.get_by_label("⟳ Rescan").click();
    h.step();
    assert!(h.state().compare_job.as_ref().is_some_and(|j| j.paths == [a.root.clone(), b.root.clone()]));
    wait(&mut h, "second comparison", |app| app.compared.is_some());
    let tiny = diff_node(h.state(), "tiny.log");
    assert_eq!(compared(h.state()).diff.entry(tiny).left, 10);

    // Scanning a single location replaces the comparison.
    h.state_mut().selected_drives.insert(a.root.clone());
    h.get_by_label("💽 Drives").click();
    h.run_steps(2);
    h.get_by_label("Scan 1 selected drive").click();
    h.step();
    wait(&mut h, "single scan", |app| app.loaded.is_some());
    assert!(h.state().compared.is_none());
}

#[test]
fn compare_dialog_checks_the_locations() {
    let (a, _) = differing_fixtures();
    let mut h = idle_harness();
    fake_drives(&mut h, &[&a.root]);
    h.get_by_label("↔ Compare two drives or folders…").click();
    h.run_steps(4);
    // One drive: the right side is left for the user to choose.
    assert_eq!(h.state().compare_dialog.as_ref().unwrap().right, "");
    h.get_by_label("Choose a drive or folder for both sides.");
    assert!(is_disabled(h.get_by_label("Compare")));

    let same = a.root.join("alpha_dir").display().to_string();
    let dialog = h.state_mut().compare_dialog.as_mut().unwrap();
    dialog.left = same.clone();
    dialog.right = format!("{}\\", same.to_uppercase());
    h.run_steps(2);
    h.get_by_label("Choose two different locations.");
    h.get_by_label("Compare").click();
    h.run_steps(2);
    assert!(h.state().compare_job.is_none(), "nothing to compare");

    h.state_mut().compare_dialog.as_mut().unwrap().right = a.root.join("missing").display().to_string();
    h.run_steps(2);
    h.get_by_label("The right location isn't a drive or folder.");

    h.state_mut().compare_dialog.as_mut().unwrap().right = a.root.display().to_string();
    h.run_steps(2);
    h.get_by_label("⇄ Swap sides").click();
    h.run_steps(2);
    let dialog = h.state().compare_dialog.as_ref().unwrap();
    assert_eq!((dialog.left.as_str(), dialog.right.as_str()), (a.root.display().to_string().as_str(), same.as_str()));
    assert!(!is_disabled(h.get_by_label("Compare")));

    // Clicking next to the dialog keeps it; Escape closes it.
    click_at(&mut h, egui::pos2(5.0, 790.0), PointerButton::Primary);
    assert!(h.state().compare_dialog.is_some());
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(h.state().compare_dialog.is_none());
    h.get_by_label("↔ Compare…").click();
    h.run_steps(4);
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(h.state().compare_dialog.is_none());
}

#[test]
fn a_failed_comparison_is_reported() {
    let (a, _) = differing_fixtures();
    let mut h = idle_harness();
    let ctx = h.ctx.clone();
    h.state_mut().apply(Action::Compare(a.root.clone(), a.root.join("missing")), &ctx);
    wait(&mut h, "failed comparison", |app| app.compare_job.is_none());
    let msg = h.state().message.as_ref().unwrap();
    assert!(msg.error && msg.text.starts_with("Comparison failed: could not scan"), "{}", msg.text);
    assert!(h.state().compared.is_none());
    h.get_by_label("↔ Compare two drives or folders…");
}

/// Renders the comparison view and the Compare dialog (see `render_layout`).
#[test]
#[ignore]
fn render_compare() {
    let Some(out) = std::env::var_os("DIRSTAT_SNAPSHOT_DIR").map(PathBuf::from) else { return };
    let (a, b) = differing_fixtures();
    for i in 0..40 {
        fs::write(b.root.join("alpha_dir").join(format!("added-{i}.dat")), vec![0u8; 5_000 + i * 3_000]).unwrap();
    }
    let mut h = Harness::builder()
        .with_size(egui::vec2(1280.0, 840.0))
        .with_step_dt(1.0 / 60.0)
        .with_accessibility_check(false)
        .wgpu()
        .build_eframe(|cc| {
            // System fonts for the icons, like the real window.
            cc.egui_ctx.add_font_provider(std::sync::Arc::new(egui_system_font_provider::SystemFontProvider::new()));
            App::new(cc, Vec::new(), Method::Walk)
        });
    fake_drives(&mut h, &[&a.root, &b.root]);
    h.get_by_label("↔ Compare…").click();
    h.run_steps(4);
    h.render().unwrap().save(out.join("compare-dialog.png")).unwrap();
    h.get_by_label("Compare").click();
    h.step();
    wait(&mut h, "comparison", |app| app.compared.is_some() && app.treemap.is_current());
    let alpha = diff_node(h.state(), "alpha_dir");
    let ctx = h.ctx.clone();
    h.state_mut().apply(Action::Toggle(alpha), &ctx);
    h.run_steps(3);
    h.render().unwrap().save(out.join("compare-view.png")).unwrap();
}
