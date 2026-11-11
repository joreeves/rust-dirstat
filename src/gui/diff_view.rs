//! The comparison view: the table of differences, the summary panel, and the treemap tooltip
//! and right-click menu for a comparison.

use eframe::egui::{self, Align, Button, Color32, Layout, RichText, Sense};
use egui_extras::{Column, TableBuilder};

use super::View;
use super::actions::Action;
use super::tree_view::{expander, handle_keys, icon, right};
use crate::diff::{Diff, Side, Status, format_delta};
use crate::model::NodeId;
use crate::util::{format_count, format_size};

/// Narrowest width of the Name column.
const NAME_MIN_WIDTH: f32 = 140.0;
/// Room for column spacing, the scroll bar and panel margins.
const CHROME_WIDTH: f32 = 48.0;
/// Width the table needs with its default column sizes.
pub const MIN_WIDTH: f32 = NAME_MIN_WIDTH + 85.0 + 85.0 + 90.0 + 150.0 + CHROME_WIDTH;

/// The statuses listed in the summary (folders are never "Contents differ" leaves).
const SUMMARY: [Status; 5] = [Status::OnlyLeft, Status::OnlyRight, Status::LargerLeft, Status::LargerRight, Status::Same];

fn swatch(ui: &mut egui::Ui, status: Status) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), Sense::hover());
    let [cr, cg, cb] = status.rgb();
    ui.painter().rect_filled(r, 3.0, Color32::from_rgb(cr, cg, cb));
}

fn side_size(diff: &Diff, id: NodeId, side: Side) -> String {
    let e = diff.entry(id);
    if e.on(side) { format_size(e.size(side)) } else { "—".to_owned() }
}

/// What the Status column says: how the item differs, or for a folder on both sides, how many
/// differences it holds.
fn status_text(diff: &Diff, id: NodeId) -> String {
    let e = diff.entry(id);
    match e.status {
        Status::LargerLeft | Status::LargerRight | Status::Changed if diff.tree.node(id).is_dir() => match e.changes {
            1 => "1 difference".to_owned(),
            n => format!("{} differences", format_count(u64::from(n))),
        },
        status => status.label().to_owned(),
    }
}

/// The comparison as a tree table: left and right sizes, the difference and the status.
pub fn table(ui: &mut egui::Ui, diff: &Diff, view: &mut View, show_identical: bool, actions: &mut Vec<Action>) {
    let tree = &diff.tree;
    if view.rows_dirty {
        view.rebuild_rows_where(tree, |id| show_identical || diff.entry(id).changes > 0);
    }
    handle_keys(ui, tree, view, actions, false);
    let scroll_row = if std::mem::take(&mut view.scroll_to_selected) {
        view.selected.and_then(|s| view.rows.iter().position(|&(id, _)| id == s))
    } else {
        None
    };
    let row_height = ui.text_style_height(&egui::TextStyle::Body).max(16.0) + 4.0;
    let mut table = TableBuilder::new(ui)
        .id_salt("diff")
        .striped(true)
        .resizable(true)
        .animate_scrolling(false)
        .sense(Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::remainder().at_least(NAME_MIN_WIDTH).clip(true).resizable(false))
        .column(Column::initial(85.0).range(60.0..=200.0).clip(true))
        .column(Column::initial(85.0).range(60.0..=200.0).clip(true))
        .column(Column::initial(90.0).range(60.0..=200.0).clip(true))
        .column(Column::initial(150.0).range(80.0..=300.0).clip(true));
    if let Some(row) = scroll_row {
        table = table.scroll_to_row(row, None);
    }
    let mut needed_width = view.tree_min_width;
    table
        .header(22.0, |mut header| {
            header.col(|ui| {
                ui.strong("Name");
            });
            header.col(|ui| right(ui, RichText::new("Left").strong()));
            header.col(|ui| right(ui, RichText::new("Right").strong()));
            header.col(|ui| right(ui, RichText::new("Difference").strong()));
            header.col(|ui| {
                ui.strong("Status");
            });
        })
        .body(|body| {
            needed_width = NAME_MIN_WIDTH + body.widths().iter().skip(1).sum::<f32>() + CHROME_WIDTH;
            body.rows(row_height, view.rows.len(), |mut row| {
                let (id, depth) = view.rows[row.index()];
                let n = tree.node(id);
                let e = diff.entry(id);
                let same = e.status == Status::Same;
                row.set_selected(view.selected == Some(id));
                row.col(|ui| {
                    ui.add_space(f32::from(depth) * 16.0);
                    if n.is_dir() && !n.children.is_empty() && (show_identical || e.changes > 0) {
                        if expander(ui, view.expanded.contains(&id)).clicked() {
                            actions.push(Action::Toggle(id));
                        }
                    } else {
                        ui.add_space(18.0);
                    }
                    let mut text = RichText::new(format!("{} {}", icon(n), n.name));
                    if same {
                        text = text.weak();
                    }
                    if n.is_synthetic() {
                        text = text.italics();
                    }
                    if n.is_unreadable() {
                        text = text.color(Color32::from_rgb(230, 160, 40));
                    }
                    let label = ui.label(text);
                    if n.is_unreadable() {
                        label.on_hover_text("This folder could not be read on one side (access denied?)");
                    }
                });
                for side in [Side::Left, Side::Right] {
                    row.col(|ui| {
                        let text = RichText::new(side_size(diff, id, side));
                        right(ui, if e.on(side) { text } else { text.weak() });
                    });
                }
                row.col(|ui| {
                    if e.left != e.right {
                        right(ui, format_delta(e.left, e.right));
                    }
                });
                row.col(|ui| {
                    swatch(ui, e.status);
                    let text = RichText::new(status_text(diff, id));
                    let label = ui.label(if same { text.weak() } else { text });
                    if n.is_dir() && e.status != Status::Same {
                        label.on_hover_text(e.status.label());
                    }
                });
                let response = row.response();
                if response.double_clicked() && n.is_dir() {
                    actions.push(Action::Toggle(id));
                } else if response.clicked() || response.secondary_clicked() {
                    actions.push(Action::Select { id, reveal: false });
                }
                response.context_menu(|ui| context_menu(ui, diff, id, actions));
            });
        });
    view.tree_min_width = needed_width;
}

/// The side panel: both locations, totals per kind of difference (click one to highlight it in
/// the treemap) and the identical-items switch.
pub fn summary(ui: &mut egui::Ui, diff: &Diff, view: &View, show_identical: bool, actions: &mut Vec<Action>) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.strong("Comparison");
        if view.highlight_ext.is_some() && ui.small_button("Clear highlight").clicked() {
            actions.push(Action::HighlightExt(None));
        }
    });
    ui.add_space(6.0);
    for (label, loc) in [("Left", &diff.left), ("Right", &diff.right)] {
        ui.horizontal(|ui| {
            ui.strong(label);
            ui.add(egui::Label::new(RichText::new(loc.root.display().to_string()).monospace()).truncate());
        });
        ui.label(format!("{} in {} files, {} folders", format_size(loc.size), format_count(loc.files), format_count(loc.dirs)));
        ui.add_space(4.0);
    }
    let (l, r) = (diff.left.size, diff.right.size);
    ui.label(match r.cmp(&l) {
        std::cmp::Ordering::Greater => format!("Right is {} larger", format_size(r - l)),
        std::cmp::Ordering::Less => format!("Right is {} smaller", format_size(l - r)),
        std::cmp::Ordering::Equal => "Both have the same total size".to_owned(),
    });
    ui.add_space(8.0);

    egui::Grid::new("diff-totals").num_columns(3).striped(true).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.label("");
        ui.strong("Items");
        ui.strong("Size");
        ui.end_row();
        for status in SUMMARY {
            let total = diff.totals[status.index()];
            ui.horizontal(|ui| {
                swatch(ui, status);
                let highlighted = view.highlight_ext == Some(status.index() as u32);
                // Identical items have no tiles to highlight.
                let clickable = status != Status::Same && total.items > 0;
                let button = ui.add_enabled(clickable, Button::selectable(highlighted, status.label()));
                if button.on_hover_text("Highlight these in the treemap").clicked() {
                    actions.push(Action::HighlightExt((!highlighted).then_some(status.index() as u32)));
                }
            });
            ui.label(format_count(total.items));
            ui.label(format_size(total.bytes));
            ui.end_row();
        }
    });
    ui.add_space(8.0);
    let mut show = show_identical;
    if ui.checkbox(&mut show, "Show identical items in the table").changed() {
        actions.push(Action::ShowIdentical(show));
    }
    ui.add_space(8.0);
    ui.label(
        RichText::new(
            "Items are matched by path. Files count as the same when their sizes match (contents \
             aren't compared). Folders are sorted by how many bytes differ inside them.",
        )
        .weak(),
    );
}

/// Items of the right-click menu for an item of a comparison.
pub fn context_menu(ui: &mut egui::Ui, diff: &Diff, id: NodeId, actions: &mut Vec<Action>) {
    let node = diff.tree.node(id);
    let has_path = |side| !node.is_synthetic() && diff.path(id, side).is_some();
    let mut item = |ui: &mut egui::Ui, enabled: bool, label: &str, action: Action| {
        if ui.add_enabled(enabled, Button::new(label)).clicked() {
            actions.push(action);
            ui.close();
        }
    };
    for side in [Side::Left, Side::Right] {
        item(ui, has_path(side), &format!("Reveal {} in Explorer", side.label()), Action::RevealIn(id, side));
    }
    for side in [Side::Left, Side::Right] {
        item(ui, has_path(side), &format!("Copy {} path", side.label()), Action::CopyPathIn(id, side));
    }
    ui.separator();
    item(ui, true, "Select in tree", Action::Select { id, reveal: true });
    if node.is_dir() {
        item(ui, node.size > 0, "Zoom treemap here", Action::Zoom(id));
    }
}

/// Treemap tooltip for an item of a comparison.
pub fn tooltip(ui: &mut egui::Ui, diff: &Diff, id: NodeId) {
    let n = diff.tree.node(id);
    let e = diff.entry(id);
    ui.strong(&*n.name);
    ui.label(RichText::new(diff.display_path(id)).monospace().weak());
    ui.label(format!("Left: {}    Right: {}", side_size(diff, id, Side::Left), side_size(diff, id, Side::Right)));
    ui.horizontal(|ui| {
        swatch(ui, e.status);
        ui.label(format!("{} ({})", e.status.label(), format_delta(e.left, e.right)));
    });
    if n.is_dir() && e.changes > 0 {
        ui.label(format!("{}: {} differ", status_text(diff, id), format_size(n.size)));
    }
}
