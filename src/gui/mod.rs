//! egui front end: drive picker, scan progress, directory tree, file types and treemap.

mod actions;
mod ext_view;
#[cfg(test)]
mod tests;
mod tree_view;
mod treemap_view;

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, atomic::Ordering};
use std::time::{Duration, Instant};

use eframe::egui::{self, Button, Color32, ProgressBar, RichText};

use self::actions::{Action, ExportFormat};
use crate::model::{NodeId, ScanNode, Tree};
use crate::platform::{self, DriveInfo};
use crate::scan::{self, Cancelled, Method, Progress};
use crate::update;
use crate::util::{format_count, format_size};

/// Opens the main window. If `initial` is not empty, scanning starts right away (several
/// locations are combined into one view).
pub fn run(initial: Vec<PathBuf>, method: Method) -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("{} {}", crate::APP_NAME, env!("CARGO_PKG_VERSION")))
            .with_icon(window_icon())
            .with_inner_size([1280.0, 840.0])
            .with_min_inner_size([720.0, 480.0]),
        // Show each frame as soon as it is drawn, so hover highlights and scrolling keep up with
        // the mouse pointer. With vsync, every frame waits for the display's next refresh (and
        // eframe's default lets two wait in line): up to 31 ms per frame over Remote Desktop,
        // whose display runs at about 32 Hz. The app only draws when something changes, so this
        // doesn't keep the GPU busy, and in a normal window Windows' compositor still shows
        // whole frames. wgpu falls back to vsync where it has to.
        wgpu_options: eframe::WgpuConfiguration {
            surface: eframe::SurfaceConfig {
                present_mode: eframe::wgpu::PresentMode::AutoNoVsync,
                desired_maximum_frame_latency: Some(1),
            },
            ..Default::default()
        },
        ..Default::default()
    };
    eframe::run_native(
        crate::APP_NAME,
        options,
        Box::new(move |cc| {
            let mut app = App::new(cc, initial, method);
            if !update::disabled() {
                app.check_for_updates(&cc.egui_ctx);
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("could not open the window: {e}"))
}

/// Largest wheel step egui applies at once; bigger ones it spreads over about 0.1 s.
const IMMEDIATE_SCROLL_STEP: f32 = 7.0;

/// Makes mouse-wheel scrolling move with the wheel. egui eases each wheel notch over several
/// frames, so lists keep gliding after the wheel stops and the hover highlight trails the
/// pointer meanwhile. It applies small point steps (as from a touchpad) immediately, so each
/// notch is passed on as a run of those, all within the same frame.
fn scroll_wheel_immediately(events: &mut Vec<egui::Event>, line_scroll_speed: f32) {
    if !events.iter().any(|e| matches!(e, egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, .. })) {
        return;
    }
    let mut out = Vec::with_capacity(events.len());
    for event in events.drain(..) {
        match event {
            egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta, phase, source, modifiers } => {
                let points = delta * line_scroll_speed;
                let pieces = (points.length() / IMMEDIATE_SCROLL_STEP).ceil().clamp(1.0, 1000.0) as usize;
                let piece = points / pieces as f32;
                out.extend((0..pieces).map(|_| egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: piece,
                    phase,
                    source,
                    modifiers,
                }));
            }
            other => out.push(other),
        }
    }
    *events = out;
}

/// The app icon for the title bar and taskbar (the .ico embedded by build.rs covers the exe).
fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../../assets/icon-256.png")).expect("the bundled icon is a valid PNG")
}

/// Narrowest the File types panel can be dragged.
const TYPES_MIN_WIDTH: f32 = 240.0;

/// "C:\ + D:\" style label for a list of scan targets.
fn paths_label(paths: &[PathBuf]) -> String {
    paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(" + ")
}

struct ScanJob {
    paths: Vec<PathBuf>,
    progress: Arc<Progress>,
    rx: Receiver<anyhow::Result<Tree>>,
    started: Instant,
}

enum JobResult {
    Rescanned { node: NodeId, result: Result<ScanNode, Cancelled> },
    Deleted { node: NodeId, path: PathBuf, result: Result<(), String> },
    Exported { path: PathBuf, result: Result<(), String> },
}

/// UI state that refers into the current tree.
pub(crate) struct View {
    pub expanded: HashSet<NodeId>,
    /// Visible rows of the tree table: (node, indent level).
    pub rows: Vec<(NodeId, u16)>,
    pub rows_dirty: bool,
    pub selected: Option<NodeId>,
    pub scroll_to_selected: bool,
    /// Root of the treemap.
    pub zoom: NodeId,
    pub highlight_ext: Option<u32>,
    /// Width the tree table currently needs (depends on its column widths).
    pub tree_min_width: f32,
}

impl View {
    fn new(tree: &Tree) -> Self {
        Self {
            expanded: HashSet::from([tree.root()]),
            rows: Vec::new(),
            rows_dirty: true,
            selected: None,
            scroll_to_selected: false,
            zoom: tree.root(),
            highlight_ext: None,
            tree_min_width: tree_view::MIN_WIDTH,
        }
    }

    pub fn rebuild_rows(&mut self, tree: &Tree) {
        self.rows.clear();
        let mut stack = vec![(tree.root(), 0u16)];
        while let Some((id, depth)) = stack.pop() {
            self.rows.push((id, depth));
            if self.expanded.contains(&id) {
                stack.extend(tree.node(id).children.iter().rev().map(|&c| (c, depth + 1)));
            }
        }
        self.rows_dirty = false;
    }

    fn reveal(&mut self, tree: &Tree, id: NodeId) {
        let chain = tree.ancestry(id);
        self.expanded.extend(chain[..chain.len() - 1].iter().copied());
        self.selected = Some(id);
        self.scroll_to_selected = true;
        self.rows_dirty = true;
    }

    /// Drops references to nodes that are no longer part of the tree.
    fn validate(&mut self, tree: &Tree) {
        if self.selected.is_some_and(|s| !tree.is_reachable(s)) {
            self.selected = None;
        }
        if !tree.is_reachable(self.zoom) {
            self.zoom = tree.root();
        }
        self.rows_dirty = true;
    }
}

struct Loaded {
    tree: Arc<Tree>,
    view: View,
    /// What was scanned (used by Rescan).
    targets: Vec<PathBuf>,
}

struct Message {
    text: String,
    error: bool,
    at: Instant,
}

pub struct App {
    method: Method,
    elevated: bool,
    drives: Vec<DriveInfo>,
    /// Drives ticked for a multi-drive scan.
    selected_drives: BTreeSet<PathBuf>,
    scan: Option<ScanJob>,
    loaded: Option<Loaded>,
    treemap: treemap_view::TreemapView,
    jobs_tx: Sender<JobResult>,
    jobs_rx: Receiver<JobResult>,
    jobs_running: usize,
    confirm_delete: Option<NodeId>,
    message: Option<Message>,
    /// A newer release, once the background update check has found one.
    update: Option<update::Release>,
    update_rx: Option<Receiver<update::Release>>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, initial: Vec<PathBuf>, method: Method) -> Self {
        cc.egui_ctx.global_style_mut(|style| style.interaction.selectable_labels = false);
        let (jobs_tx, jobs_rx) = mpsc::channel();
        let mut app = Self {
            method,
            elevated: platform::is_elevated(),
            drives: platform::drives(),
            selected_drives: BTreeSet::new(),
            scan: None,
            loaded: None,
            treemap: treemap_view::TreemapView::default(),
            jobs_tx,
            jobs_rx,
            jobs_running: 0,
            confirm_delete: None,
            message: None,
            update: None,
            update_rx: None,
        };
        if !initial.is_empty() {
            app.start_scan(initial, &cc.egui_ctx);
        }
        app
    }

    /// Asks GitHub for the latest release in the background. Only a newer release is reported;
    /// failures (offline, proxy, rate limit) are silent.
    fn check_for_updates(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            if let Ok(update::Outcome::Available(release)) = update::check() {
                let _ = tx.send(release);
                ctx.request_repaint();
            }
        });
        if spawned.is_ok() {
            self.update_rx = Some(rx);
        }
    }

    fn info(&mut self, text: impl Into<String>) {
        self.message = Some(Message { text: text.into(), error: false, at: Instant::now() });
    }

    fn error(&mut self, text: impl Into<String>) {
        self.message = Some(Message { text: text.into(), error: true, at: Instant::now() });
    }

    fn start_scan(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        if self.scan.is_some() || paths.is_empty() {
            return;
        }
        self.loaded = None; // free the old tree before building a new one
        self.treemap.reset();
        self.confirm_delete = None;
        let progress = Arc::new(Progress::new());
        let (tx, rx) = mpsc::channel();
        let (thread_progress, thread_paths, method, ctx2) = (progress.clone(), paths.clone(), self.method, ctx.clone());
        std::thread::Builder::new()
            .name("scan".into())
            .spawn(move || {
                let result = scan::scan_many(&thread_paths, method, &thread_progress);
                let _ = tx.send(result);
                ctx2.request_repaint();
            })
            .expect("failed to spawn scan thread");
        self.scan = Some(ScanJob { paths, progress, rx, started: Instant::now() });
    }

    fn set_tree(&mut self, tree: Tree, targets: Vec<PathBuf>) {
        let root = tree.node(tree.root());
        let mut text = format!(
            "Scanned {} files ({}) in {:.1} s using {}",
            format_count(root.files),
            format_size(root.size),
            tree.info.elapsed.as_secs_f64(),
            tree.info.method
        );
        if let Some(note) = &tree.info.note {
            text.push_str(&format!(" — {note}"));
        }
        self.loaded = Some(Loaded { view: View::new(&tree), tree: Arc::new(tree), targets });
        self.treemap.reset();
        self.info(text);
    }

    fn poll(&mut self) {
        if let Some(job) = &self.scan {
            match job.rx.try_recv() {
                Ok(result) => {
                    let job = self.scan.take().expect("scan job present");
                    match result {
                        Ok(tree) => self.set_tree(tree, job.paths),
                        Err(e) if e.is::<Cancelled>() => self.info("Scan cancelled"),
                        Err(e) => self.error(format!("Scan of {} failed: {e:#}", paths_label(&job.paths))),
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.scan = None;
                    self.error("The scan stopped unexpectedly");
                }
            }
        }
        while let Ok(result) = self.jobs_rx.try_recv() {
            self.jobs_running = self.jobs_running.saturating_sub(1);
            self.finish_job(result);
        }
        if let Some(rx) = &self.update_rx {
            match rx.try_recv() {
                Ok(release) => {
                    self.update = Some(release);
                    self.update_rx = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.update_rx = None,
            }
        }
    }

    fn finish_job(&mut self, result: JobResult) {
        match result {
            JobResult::Exported { path, result: Ok(()) } => self.info(format!("Exported to {}", path.display())),
            JobResult::Exported { path, result: Err(e) } => self.error(format!("Export to {} failed: {e}", path.display())),
            JobResult::Deleted { path, result: Err(e), .. } => self.error(format!("Could not delete {}: {e}", path.display())),
            JobResult::Deleted { node, path, result: Ok(()) } => {
                if let Some(l) = &mut self.loaded {
                    let tree = Arc::make_mut(&mut l.tree);
                    if tree.is_reachable(node) {
                        let parent = tree.node(node).parent;
                        tree.remove(node);
                        l.view.validate(tree);
                        if l.view.selected.is_none() {
                            l.view.selected = parent;
                        }
                        self.treemap.invalidate();
                    }
                }
                self.info(format!("Moved {} to the Recycle Bin", path.display()));
            }
            JobResult::Rescanned { result: Err(_), .. } => self.info("Rescan cancelled"),
            JobResult::Rescanned { node, result: Ok(scan) } => {
                if let Some(l) = &mut self.loaded {
                    let tree = Arc::make_mut(&mut l.tree);
                    if tree.is_reachable(node) {
                        tree.graft(node, scan);
                        l.view.validate(tree);
                        self.treemap.invalidate();
                        let path = tree.path_of(node).map(|p| p.display().to_string()).unwrap_or_default();
                        self.info(format!("Rescanned {path}"));
                    }
                }
            }
        }
    }

    fn node_path(&self, id: NodeId) -> Option<PathBuf> {
        self.loaded.as_ref().and_then(|l| l.tree.path_of(id))
    }

    fn apply(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::Scan(paths) => self.start_scan(paths, ctx),
            Action::ToggleDrive(root) => {
                if !self.selected_drives.remove(&root) {
                    self.selected_drives.insert(root);
                }
            }
            Action::RescanAll => {
                if let Some(targets) = self.loaded.as_ref().map(|l| l.targets.clone()) {
                    self.start_scan(targets, ctx);
                }
            }
            Action::ChooseFolder => {
                if let Some(path) = rfd::FileDialog::new().set_title("Choose a folder to analyze").pick_folder() {
                    self.start_scan(vec![path], ctx);
                }
            }
            Action::CancelScan => {
                if let Some(job) = &self.scan {
                    job.progress.cancel.store(true, Ordering::Relaxed);
                }
            }
            Action::SetMethod(m) => self.method = m,
            Action::Elevate => {
                let mut args = vec!["gui".to_string()];
                if let Some(l) = &self.loaded {
                    args.extend(l.targets.iter().map(|p| p.display().to_string()));
                }
                args.push("--method".into());
                args.push(self.method.to_string().to_lowercase());
                if platform::relaunch_elevated(&args) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    self.error("Could not restart as administrator");
                }
            }
            Action::Open(id) => {
                if let Some(p) = self.node_path(id)
                    && !platform::open_path(&p)
                {
                    self.error(format!("Could not open {}", p.display()));
                }
            }
            Action::Reveal(id) => {
                if let Some(p) = self.node_path(id)
                    && let Err(e) = platform::reveal_in_explorer(&p)
                {
                    self.error(format!("Could not open Explorer: {e}"));
                }
            }
            Action::CopyPath(id) => {
                if let Some(p) = self.node_path(id) {
                    ctx.copy_text(p.display().to_string());
                    self.info(format!("Copied {}", p.display()));
                }
            }
            Action::AskDelete(id) => self.confirm_delete = Some(id),
            Action::Delete(id) => {
                if let Some(path) = self.node_path(id) {
                    let tx = self.jobs_tx.clone();
                    let ctx2 = ctx.clone();
                    self.jobs_running += 1;
                    self.info(format!("Moving {} to the Recycle Bin…", path.display()));
                    std::thread::spawn(move || {
                        let result = trash::delete(&path).map_err(|e| e.to_string());
                        let _ = tx.send(JobResult::Deleted { node: id, path, result });
                        ctx2.request_repaint();
                    });
                }
            }
            Action::Rescan(id) => {
                if let Some(path) = self.node_path(id) {
                    let tx = self.jobs_tx.clone();
                    let ctx2 = ctx.clone();
                    self.jobs_running += 1;
                    self.info(format!("Rescanning {}…", path.display()));
                    std::thread::spawn(move || {
                        let result = scan::walk::walk(&path, &Progress::new());
                        let _ = tx.send(JobResult::Rescanned { node: id, result });
                        ctx2.request_repaint();
                    });
                }
            }
            Action::Export(format) => self.export(format, ctx),
            other => {
                if let Some(l) = &mut self.loaded {
                    apply_view_action(&l.tree, &mut l.view, other);
                }
            }
        }
    }

    fn export(&mut self, format: ExportFormat, ctx: &egui::Context) {
        let Some(l) = &self.loaded else { return };
        let (ext, label) = match format {
            ExportFormat::Csv => ("csv", "CSV"),
            ExportFormat::Json => ("json", "JSON"),
        };
        let Some(path) = rfd::FileDialog::new()
            .set_title(format!("Export {label}"))
            .add_filter(label, &[ext])
            .set_file_name(format!("{}.{ext}", crate::APP_NAME))
            .save_file()
        else {
            return;
        };
        let tree = l.tree.clone();
        let tx = self.jobs_tx.clone();
        let ctx2 = ctx.clone();
        self.jobs_running += 1;
        self.info(format!("Exporting to {}…", path.display()));
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<()> {
                let out = std::io::BufWriter::new(std::fs::File::create(&path)?);
                match format {
                    ExportFormat::Csv => crate::export::write_csv(&tree, tree.root(), None, out),
                    ExportFormat::Json => crate::export::write_json(&tree, tree.root(), None, out),
                }
            })()
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(JobResult::Exported { path, result });
            ctx2.request_repaint();
        });
    }

    fn toolbar(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.horizontal_wrapped(|ui| {
            let idle = self.scan.is_none();
            let ready = idle && self.loaded.is_some();
            if ui.add_enabled(idle, Button::new("📂 Open folder…")).clicked() {
                actions.push(Action::ChooseFolder);
            }
            ui.add_enabled_ui(idle, |ui| {
                // Stays open while drives are ticked; closes when a scan starts.
                let config = egui::containers::menu::MenuConfig::new()
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
                egui::containers::menu::MenuButton::new("💽 Drives").config(config).ui(ui, |ui| {
                    ui.label(RichText::new("Tick the drives to scan, then click Scan selected drives.").weak());
                    ui.add_space(4.0);
                    for d in &self.drives {
                        // Clicking the row only (un)ticks it; scans start from the buttons below.
                        let mut ticked = self.selected_drives.contains(&d.root);
                        let label = format!(
                            "{}   {} free of {}   {}",
                            d.root.display(),
                            format_size(d.free),
                            format_size(d.total),
                            d.filesystem
                        );
                        if ui.checkbox(&mut ticked, label).changed() {
                            actions.push(Action::ToggleDrive(d.root.clone()));
                        }
                    }
                    ui.separator();
                    if self.multi_drive_buttons(ui, actions) {
                        ui.close();
                    }
                });
            });
            ui.separator();
            if ui.add_enabled(ready, Button::new("⟳ Rescan")).on_hover_text("Scan the same location again").clicked() {
                actions.push(Action::RescanAll);
            }
            let can_go_up = self.loaded.as_ref().is_some_and(|l| l.view.selected.is_some_and(|s| l.tree.node(s).parent.is_some()));
            if ui.add_enabled(ready && can_go_up, Button::new("⬆ Up")).on_hover_text("Select the parent folder").clicked() {
                actions.push(Action::SelectParent);
            }
            let zoomed = self.loaded.as_ref().is_some_and(|l| l.view.zoom != l.tree.root());
            if ui.add_enabled(ready && zoomed, Button::new("🔍 Zoom out")).on_hover_text("Show the parent folder in the treemap").clicked() {
                actions.push(Action::ZoomOut);
            }
            ui.add_enabled_ui(ready, |ui| {
                ui.menu_button("💾 Export", |ui| {
                    if ui.button("CSV (one row per file and folder)…").clicked() {
                        actions.push(Action::Export(ExportFormat::Csv));
                        ui.close();
                    }
                    if ui.button("JSON (nested tree)…").clicked() {
                        actions.push(Action::Export(ExportFormat::Json));
                        ui.close();
                    }
                });
            });
            ui.separator();
            ui.label("Scan method:");
            ui.add_enabled_ui(idle, |ui| {
                egui::ComboBox::from_id_salt("method").selected_text(self.method.to_string()).show_ui(ui, |ui| {
                    for (m, hint) in [
                        (Method::Auto, "MFT when possible, otherwise folder walk"),
                        (Method::Mft, "Read the NTFS Master File Table (administrator)"),
                        (Method::Walk, "Walk folders (works everywhere)"),
                    ] {
                        if ui.selectable_label(self.method == m, m.to_string()).on_hover_text(hint).clicked() {
                            actions.push(Action::SetMethod(m));
                        }
                    }
                });
            });
            if !self.elevated
                && cfg!(windows)
                && ui.button("🛡 Restart as administrator").on_hover_text("Enables the fast NTFS MFT scan").clicked()
            {
                actions.push(Action::Elevate);
            }
        });
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(l) = &self.loaded {
                let t = &l.tree;
                let root = t.node(t.root());
                ui.label(format!(
                    "{}  •  {} files  •  {} folders  •  {} in {:.1} s",
                    format_size(root.size),
                    format_count(root.files),
                    format_count(root.dirs),
                    t.info.method,
                    t.info.elapsed.as_secs_f64()
                ));
                if t.info.unreadable > 0 {
                    ui.label(RichText::new(format!("⚠ {} unreadable folders", format_count(t.info.unreadable))).color(Color32::from_rgb(230, 160, 40)));
                }
                if let Some(sel) = l.view.selected.filter(|&s| t.is_reachable(s)) {
                    ui.separator();
                    let n = t.node(sel);
                    let path = t.path_of(sel).map(|p| p.display().to_string()).unwrap_or_else(|| n.name.to_string());
                    ui.label(format!("{}  ({})", path, format_size(n.size)));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(if self.elevated { "Administrator" } else { "Standard user" }).weak());
                if let Some(release) = &self.update {
                    ui.separator();
                    ui.hyperlink_to(RichText::new(format!("⬆ Update available: {}", release.tag)).strong(), &release.url)
                        .on_hover_text(format!(
                            "You have {}. Opens the release page, where you can download the installer.",
                            env!("CARGO_PKG_VERSION")
                        ));
                }
                if let Some(m) = &self.message
                    && m.at.elapsed() < Duration::from_secs(15)
                {
                    ui.separator();
                    let color = if m.error { Color32::from_rgb(235, 90, 70) } else { ui.visuals().text_color() };
                    ui.label(RichText::new(&m.text).color(color));
                }
            });
        });
    }

    /// "Scan N selected drives" and "Scan all local drives" buttons. Returns true if a scan was
    /// requested.
    fn multi_drive_buttons(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) -> bool {
        let mut started = false;
        ui.horizontal(|ui| {
            let selected: Vec<PathBuf> = self.selected_drives.iter().cloned().collect();
            let label = match selected.len() {
                0 => "Scan selected drives".to_string(),
                1 => "Scan 1 selected drive".to_string(),
                n => format!("Scan {n} selected drives"),
            };
            let hint = if selected.is_empty() { "Tick drives first".to_string() } else { paths_label(&selected) };
            if ui.add_enabled(!selected.is_empty(), Button::new(label)).on_hover_text(hint).clicked() {
                actions.push(Action::Scan(selected));
                started = true;
            }
            if ui.add_enabled(!self.drives.is_empty(), Button::new("Scan all local drives")).clicked() {
                actions.push(Action::Scan(self.drives.iter().map(|d| d.root.clone()).collect()));
                started = true;
            }
        });
        started
    }

    fn welcome(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.vertical_centered(|ui| {
            ui.add_space(32.0);
            ui.label(RichText::new(crate::APP_NAME).size(34.0).strong());
            ui.label("See where your disk space goes. Select one or more drives and click Scan, or open a folder.");
            ui.add_space(24.0);
            ui.allocate_ui(egui::vec2(600.0, 0.0), |ui| {
                for d in &self.drives {
                    let used = d.total.saturating_sub(d.free);
                    let frac = if d.total == 0 { 0.0 } else { used as f32 / d.total as f32 };
                    ui.horizontal(|ui| {
                        // The checkbox and the drive button both just (un)select the drive.
                        let mut ticked = self.selected_drives.contains(&d.root);
                        let tick = ui.checkbox(&mut ticked, "");
                        let label = RichText::new(d.root.display().to_string()).size(18.0).strong();
                        let button = ui.add_sized([84.0, 40.0], Button::new(label).selected(ticked));
                        if tick.changed() || button.clicked() {
                            actions.push(Action::ToggleDrive(d.root.clone()));
                        }
                        ui.vertical(|ui| {
                            ui.label(format!(
                                "{}  •  {} used of {}  •  {} free",
                                if d.filesystem.is_empty() { "?" } else { &d.filesystem },
                                format_size(used),
                                format_size(d.total),
                                format_size(d.free)
                            ));
                            ui.add(ProgressBar::new(frac).desired_width(440.0).desired_height(10.0));
                        });
                    });
                    ui.add_space(8.0);
                }
                ui.add_space(4.0);
                self.multi_drive_buttons(ui, actions);
                ui.add_space(8.0);
                if ui.add_sized([180.0, 32.0], Button::new("📂 Open folder…")).clicked() {
                    actions.push(Action::ChooseFolder);
                }
                if !self.elevated && cfg!(windows) {
                    ui.add_space(20.0);
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.label("Running without administrator rights: scans use the slower folder walk.");
                        if ui.button("🛡 Restart as administrator for the fast MFT scan").clicked() {
                            actions.push(Action::Elevate);
                        }
                    });
                }
            });
        });
    }

    fn delete_dialog(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(id) = self.confirm_delete else { return };
        let Some(l) = &self.loaded else {
            self.confirm_delete = None;
            return;
        };
        let tree = &l.tree;
        let node = tree.node(id);
        let path = tree.path_of(id).map(|p| p.display().to_string()).unwrap_or_default();
        let response = egui::Modal::new(egui::Id::unique("confirm-delete")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.heading("Move to Recycle Bin?");
            ui.add_space(6.0);
            ui.label(RichText::new(&path).monospace());
            ui.add_space(4.0);
            if node.is_dir() {
                ui.label(format!(
                    "Folder with {} files and {} folders, {} in total.",
                    format_count(node.files),
                    format_count(node.dirs),
                    format_size(node.size)
                ));
            } else {
                ui.label(format!("File, {}.", format_size(node.size)));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let delete = Button::new(RichText::new("Move to Recycle Bin").color(Color32::WHITE)).fill(Color32::from_rgb(190, 60, 45));
                if ui.add(delete).clicked() {
                    actions.push(Action::Delete(id));
                    ui.close();
                }
                if ui.button("Cancel").clicked() {
                    ui.close();
                }
            });
        });
        if response.should_close() {
            self.confirm_delete = None;
        }
    }
}

fn apply_view_action(tree: &Tree, view: &mut View, action: Action) {
    match action {
        Action::Select { id, reveal } => {
            if !tree.is_reachable(id) {
                return;
            }
            if reveal {
                view.reveal(tree, id);
            } else {
                view.selected = Some(id);
            }
        }
        Action::Toggle(id) => {
            if !view.expanded.remove(&id) {
                view.expanded.insert(id);
            }
            view.rows_dirty = true;
        }
        Action::SelectParent => {
            if let Some(p) = view.selected.and_then(|s| tree.node(s).parent) {
                view.reveal(tree, p);
            }
        }
        Action::Zoom(id) => {
            let target = if tree.node(id).is_dir() { Some(id) } else { tree.node(id).parent };
            if let Some(t) = target.filter(|&t| tree.node(t).size > 0) {
                view.zoom = t;
            }
        }
        Action::ZoomOut => {
            if let Some(p) = tree.node(view.zoom).parent {
                view.zoom = p;
            }
        }
        Action::ZoomReset => view.zoom = tree.root(),
        Action::HighlightExt(ext) => view.highlight_ext = ext,
        _ => {}
    }
}

impl eframe::App for App {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let speed = ctx.options(|o| o.input_options.line_scroll_speed);
        scroll_wheel_immediately(&mut raw_input.events, speed);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll();
        let mut actions = Vec::new();

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(2.0);
            self.toolbar(ui, &mut actions);
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));

        if let Some(job) = &self.scan {
            egui::CentralPanel::default().show(ui, |ui| scanning_ui(ui, job, &mut actions));
            // About 60 fps for the spinner and counters (frames aren't paced by vsync).
            ctx.request_repaint_after(Duration::from_millis(16));
        } else if let Some(loaded) = &mut self.loaded {
            let tree = loaded.tree.clone();
            let height = ui.available_height();
            egui::Panel::bottom("treemap")
                .resizable(true)
                .default_size(height * 0.5)
                .min_size(120.0)
                .show(ui, |ui| treemap_view::show(ui, &tree, &mut loaded.view, &mut self.treemap, &mut actions));
            // Keep enough room for the tree table so the panel never covers its columns.
            let max_types = (ui.available_width() - loaded.view.tree_min_width).max(TYPES_MIN_WIDTH);
            egui::Panel::right("types")
                .resizable(true)
                .default_size(330.0)
                .size_range(TYPES_MIN_WIDTH..=max_types)
                .show(ui, |ui| ext_view::show(ui, &tree, &loaded.view, &mut actions));
            egui::CentralPanel::default().show(ui, |ui| tree_view::show(ui, &tree, &mut loaded.view, &mut actions));
        } else {
            egui::CentralPanel::default().show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.welcome(ui, &mut actions));
            });
        }

        self.delete_dialog(&ctx, &mut actions);
        for action in actions {
            self.apply(action, &ctx);
        }
        if self.jobs_running > 0 {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

/// egui's spinner, except that it leaves repainting to the caller. egui's asks for the next
/// frame right away, which without vsync means redrawing as fast as the GPU allows.
fn spinner(ui: &mut egui::Ui) {
    let size = ui.style().spacing.interact_size.y;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    response.widget_info(|| egui::WidgetInfo::new(egui::Role::ProgressIndicator));
    let radius = size * 0.5 - 2.0;
    let n = (radius.round() as usize).clamp(8, 128);
    let time = ui.input(|i| i.time);
    let start = time * std::f64::consts::TAU;
    let end = start + 240f64.to_radians() * time.sin();
    let points = (0..n)
        .map(|i| {
            let (sin, cos) = (start + (end - start) * i as f64 / n as f64).sin_cos();
            rect.center() + radius * egui::vec2(cos as f32, sin as f32)
        })
        .collect();
    ui.painter().add(egui::Shape::line(points, egui::Stroke::new(3.0, ui.visuals().strong_text_color())));
}

fn scanning_ui(ui: &mut egui::Ui, job: &ScanJob, actions: &mut Vec<Action>) {
    let p = &job.progress;
    ui.vertical_centered(|ui| {
        ui.add_space(48.0);
        spinner(ui);
        ui.add_space(8.0);
        ui.heading(format!("Scanning {}", paths_label(&job.paths)));
        ui.add_space(12.0);
        ui.label(
            RichText::new(format!(
                "{} files   •   {} folders   •   {}",
                format_count(p.files.load(Ordering::Relaxed)),
                format_count(p.dirs.load(Ordering::Relaxed)),
                format_size(p.bytes.load(Ordering::Relaxed))
            ))
            .size(16.0),
        );
        let total = p.records_total.load(Ordering::Relaxed);
        if total > 0 {
            let done = p.records_done.load(Ordering::Relaxed).min(total);
            ui.add_space(8.0);
            ui.add(
                ProgressBar::new(done as f32 / total as f32)
                    .desired_width(420.0)
                    .text(format!("MFT records {} / {}", format_count(done), format_count(total))),
            );
        }
        ui.add_space(8.0);
        ui.label(RichText::new(p.current()).weak());
        ui.label(format!("{:.1} s", job.started.elapsed().as_secs_f64()));
        ui.add_space(12.0);
        let cancelling = p.is_cancelled();
        if ui.add_enabled(!cancelling, Button::new(if cancelling { "Cancelling…" } else { "Cancel" })).clicked() {
            actions.push(Action::CancelScan);
        }
    });
}
