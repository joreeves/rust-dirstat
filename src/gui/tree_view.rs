//! The directory tree table (virtualized: only visible rows are laid out).

use eframe::egui::{self, Align, Align2, Color32, FontId, Key, Layout, RichText, Sense};
use egui_extras::{Column, TableBuilder};

use super::View;
use super::actions::{Action, context_menu};
use crate::model::{Node, NodeId, Tree};
use crate::util::{format_count, format_size};

/// Bar colors per nesting level, like WinDirStat's subtree percentage column.
const LEVEL_COLORS: [Color32; 6] = [
    Color32::from_rgb(70, 110, 200),
    Color32::from_rgb(200, 90, 70),
    Color32::from_rgb(70, 160, 90),
    Color32::from_rgb(190, 150, 50),
    Color32::from_rgb(140, 90, 180),
    Color32::from_rgb(60, 150, 160),
];

/// Narrowest width of the Name column.
const NAME_MIN_WIDTH: f32 = 140.0;
/// Room for column spacing, the scroll bar and panel margins.
const CHROME_WIDTH: f32 = 48.0;
/// Width the tree table needs with its default column sizes (keeps the File types panel from
/// covering it).
pub const MIN_WIDTH: f32 = NAME_MIN_WIDTH + 120.0 + 85.0 + 80.0 + 70.0 + CHROME_WIDTH;

pub fn show(ui: &mut egui::Ui, tree: &Tree, view: &mut View, actions: &mut Vec<Action>) {
    if view.rows_dirty {
        view.rebuild_rows(tree);
    }
    handle_keys(ui, tree, view, actions, true);
    let scroll_row = if std::mem::take(&mut view.scroll_to_selected) {
        view.selected.and_then(|s| view.rows.iter().position(|&(id, _)| id == s))
    } else {
        None
    };
    let row_height = ui.text_style_height(&egui::TextStyle::Body).max(16.0) + 4.0;

    // Name takes whatever width is left; the other columns have fixed (user-resizable) widths
    // and clip, so they never grow on their own and the table always fits its panel.
    let mut table = TableBuilder::new(ui)
        .id_salt("tree")
        .striped(true)
        .resizable(true)
        .animate_scrolling(false)
        .sense(Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::remainder().at_least(NAME_MIN_WIDTH).clip(true).resizable(false))
        .column(Column::initial(120.0).range(60.0..=300.0).clip(true))
        .column(Column::initial(85.0).range(60.0..=200.0).clip(true))
        .column(Column::initial(80.0).range(50.0..=200.0).clip(true))
        .column(Column::initial(70.0).range(50.0..=200.0).clip(true));
    if let Some(row) = scroll_row {
        // Instant, and only as far as needed (no movement if the row is already visible).
        table = table.scroll_to_row(row, None);
    }
    let mut needed_width = view.tree_min_width;
    table
        .header(22.0, |mut header| {
            header.col(|ui| {
                ui.strong("Name");
            });
            header.col(|ui| {
                ui.strong("% of parent");
            });
            header.col(|ui| right(ui, RichText::new("Size").strong()));
            header.col(|ui| right(ui, RichText::new("Files").strong()));
            header.col(|ui| right(ui, RichText::new("Folders").strong()));
        })
        .body(|body| {
            needed_width = NAME_MIN_WIDTH + body.widths().iter().skip(1).sum::<f32>() + CHROME_WIDTH;
            body.rows(row_height, view.rows.len(), |mut row| {
                let (id, depth) = view.rows[row.index()];
                let n = tree.node(id);
                row.set_selected(view.selected == Some(id));
                row.col(|ui| {
                    ui.add_space(f32::from(depth) * 16.0);
                    if n.is_dir() && !n.children.is_empty() {
                        if expander(ui, view.expanded.contains(&id)).clicked() {
                            actions.push(Action::Toggle(id));
                        }
                    } else {
                        ui.add_space(18.0);
                    }
                    let mut text = RichText::new(format!("{} {}", icon(n), n.name));
                    if n.is_synthetic() {
                        text = text.italics();
                    }
                    if n.is_unreadable() {
                        text = text.color(Color32::from_rgb(230, 160, 40));
                    }
                    let label = ui.label(text);
                    if n.is_unreadable() {
                        label.on_hover_text("This folder could not be read (access denied?)");
                    }
                });
                row.col(|ui| percent_bar(ui, tree, id, depth));
                row.col(|ui| right(ui, format_size(n.size)));
                row.col(|ui| {
                    if n.is_dir() {
                        right(ui, format_count(n.files));
                    }
                });
                row.col(|ui| {
                    if n.is_dir() {
                        right(ui, format_count(n.dirs));
                    }
                });
                let response = row.response();
                if response.double_clicked() {
                    actions.push(if n.is_dir() { Action::Toggle(id) } else { Action::Open(id) });
                } else if response.clicked() || response.secondary_clicked() {
                    actions.push(Action::Select { id, reveal: false });
                }
                response.context_menu(|ui| context_menu(ui, tree, id, actions));
            });
        });
    view.tree_min_width = needed_width;
}

/// The icon shown before a node's name.
pub(super) fn icon(n: &Node) -> &'static str {
    if n.is_synthetic() {
        "🗄"
    } else if n.is_link() {
        "🔗"
    } else if n.is_dir() {
        "📁"
    } else {
        "📄"
    }
}

/// A painted disclosure triangle (the default fonts lack the arrow glyphs).
pub(super) fn expander(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(18.0, 16.0), Sense::click());
    let color = if response.hovered() { ui.visuals().strong_text_color() } else { ui.visuals().weak_text_color() };
    let c = rect.center();
    let points = if open {
        vec![c + egui::vec2(-4.5, -2.5), c + egui::vec2(4.5, -2.5), c + egui::vec2(0.0, 3.5)]
    } else {
        vec![c + egui::vec2(-2.5, -4.5), c + egui::vec2(3.5, 0.0), c + egui::vec2(-2.5, 4.5)]
    };
    ui.painter().add(egui::Shape::convex_polygon(points, color, egui::Stroke::NONE));
    response
}

pub(super) fn right(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.label(text);
    });
}

fn percent_bar(ui: &mut egui::Ui, tree: &Tree, id: NodeId, depth: u16) {
    let n = tree.node(id);
    let parent_size = n.parent.map_or(n.size, |p| tree.node(p).size);
    let frac = if parent_size == 0 { 0.0 } else { n.size as f32 / parent_size as f32 };
    let height = (ui.available_height() - 6.0).max(8.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    let mut fill = rect;
    fill.set_width(rect.width() * frac.clamp(0.0, 1.0));
    painter.rect_filled(fill, 2.0, LEVEL_COLORS[usize::from(depth) % LEVEL_COLORS.len()]);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        format!("{:.1}%", frac * 100.0),
        FontId::proportional(11.0),
        ui.visuals().strong_text_color(),
    );
}

/// Arrow keys move the selection and Left/Right collapse/expand. When `editable`, Enter opens
/// a file and Delete deletes; otherwise Enter only expands or collapses folders.
pub(super) fn handle_keys(ui: &egui::Ui, tree: &Tree, view: &View, actions: &mut Vec<Action>, editable: bool) {
    if ui.ctx().egui_wants_keyboard_input() || view.rows.is_empty() {
        return;
    }
    let (down, up, right, left, enter, delete, home, end) = ui.input(|i| {
        (
            i.key_pressed(Key::ArrowDown),
            i.key_pressed(Key::ArrowUp),
            i.key_pressed(Key::ArrowRight),
            i.key_pressed(Key::ArrowLeft),
            i.key_pressed(Key::Enter),
            i.key_pressed(Key::Delete),
            i.key_pressed(Key::Home),
            i.key_pressed(Key::End),
        )
    });
    let select = |actions: &mut Vec<Action>, id: NodeId| actions.push(Action::Select { id, reveal: true });
    if home {
        select(actions, view.rows[0].0);
    }
    if end {
        select(actions, view.rows[view.rows.len() - 1].0);
    }
    let Some(sel) = view.selected else {
        if down {
            select(actions, view.rows[0].0);
        }
        return;
    };
    let Some(idx) = view.rows.iter().position(|&(id, _)| id == sel) else { return };
    let node = tree.node(sel);
    if down && idx + 1 < view.rows.len() {
        select(actions, view.rows[idx + 1].0);
    }
    if up && idx > 0 {
        select(actions, view.rows[idx - 1].0);
    }
    if right && node.is_dir() && !view.expanded.contains(&sel) {
        actions.push(Action::Toggle(sel));
    }
    if left {
        if node.is_dir() && view.expanded.contains(&sel) {
            actions.push(Action::Toggle(sel));
        } else if let Some(p) = node.parent {
            select(actions, p);
        }
    }
    if enter && node.is_dir() {
        actions.push(Action::Toggle(sel));
    } else if enter && editable {
        actions.push(Action::Open(sel));
    }
    if editable && delete && sel != tree.root() {
        actions.push(Action::AskDelete(sel));
    }
}
