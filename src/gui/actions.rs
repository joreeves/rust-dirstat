//! User intents collected while drawing a frame and applied afterwards (keeps borrows simple).

use std::path::PathBuf;

use eframe::egui::{self, Button};

use crate::model::{NodeId, Tree};
use crate::scan::Method;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    Json,
}

#[derive(Clone, Debug)]
pub enum Action {
    /// Scan one or more locations (several are combined under a virtual root).
    Scan(Vec<PathBuf>),
    /// Add or remove a drive from the multi-drive selection.
    ToggleDrive(PathBuf),
    RescanAll,
    ChooseFolder,
    CancelScan,
    SetMethod(Method),
    Elevate,
    /// Select a node; `reveal` expands its ancestors and scrolls the tree to it.
    Select { id: NodeId, reveal: bool },
    Toggle(NodeId),
    SelectParent,
    Zoom(NodeId),
    ZoomOut,
    ZoomReset,
    HighlightExt(Option<u32>),
    Open(NodeId),
    Reveal(NodeId),
    CopyPath(NodeId),
    AskDelete(NodeId),
    Delete(NodeId),
    Rescan(NodeId),
    Export(ExportFormat),
}

/// Items of the right-click menu for a node (used by the tree table and the treemap).
pub fn context_menu(ui: &mut egui::Ui, tree: &Tree, id: NodeId, actions: &mut Vec<Action>) {
    let node = tree.node(id);
    let path = tree.path_of(id).filter(|_| !node.is_synthetic());
    let has_path = path.is_some();
    // Never offer to delete a drive root (the children of a multi-drive view).
    let deletable = path.as_ref().is_some_and(|p| p.parent().is_some()) && id != tree.root();
    let mut item = |ui: &mut egui::Ui, enabled: bool, label: &str, action: Action| {
        if ui.add_enabled(enabled, Button::new(label)).clicked() {
            actions.push(action);
            ui.close();
        }
    };
    item(ui, has_path, "Open", Action::Open(id));
    item(ui, has_path, "Reveal in Explorer", Action::Reveal(id));
    item(ui, has_path, "Copy path", Action::CopyPath(id));
    ui.separator();
    item(ui, true, "Select in tree", Action::Select { id, reveal: true });
    if node.is_dir() {
        item(ui, node.size > 0, "Zoom treemap here", Action::Zoom(id));
        item(ui, has_path, "Rescan this folder", Action::Rescan(id));
    }
    ui.separator();
    item(ui, deletable, "Move to Recycle Bin…", Action::AskDelete(id));
}
