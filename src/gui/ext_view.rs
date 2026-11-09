//! The file-type (extension) list. Clicking a row highlights matching tiles in the treemap.

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense};
use egui_extras::{Column, TableBuilder};

use super::View;
use super::actions::Action;
use crate::colors::ext_color;
use crate::model::Tree;
use crate::util::{format_count, format_size, percent};

pub fn show(ui: &mut egui::Ui, tree: &Tree, view: &View, actions: &mut Vec<Action>) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.strong("File types");
        if view.highlight_ext.is_some() && ui.small_button("Clear highlight").clicked() {
            actions.push(Action::HighlightExt(None));
        }
    });
    let total = tree.node(tree.root()).size;
    let rows: Vec<usize> = tree.exts().iter().enumerate().filter(|(_, e)| e.files > 0).map(|(i, _)| i).collect();
    let row_height = ui.text_style_height(&egui::TextStyle::Body).max(16.0) + 4.0;
    // Extension takes the leftover width; the number columns are fixed and clip. Nothing here
    // grows with its content, so the panel can be dragged narrower again after widening it.
    TableBuilder::new(ui)
        .id_salt("exts")
        .striped(true)
        .sense(Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::remainder().at_least(70.0).clip(true))
        .column(Column::exact(72.0).clip(true))
        .column(Column::exact(44.0).clip(true))
        .column(Column::exact(58.0).clip(true))
        .header(20.0, |mut h| {
            h.col(|ui| {
                ui.strong("Extension");
            });
            h.col(|ui| right(ui, RichText::new("Size").strong()));
            h.col(|ui| right(ui, RichText::new("%").strong()));
            h.col(|ui| right(ui, RichText::new("Files").strong()));
        })
        .body(|body| {
            body.rows(row_height, rows.len(), |mut row| {
                let i = rows[row.index()];
                let e = &tree.exts()[i];
                let selected = view.highlight_ext == Some(i as u32);
                row.set_selected(selected);
                row.col(|ui| {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), Sense::hover());
                    let [cr, cg, cb] = ext_color(i as u32);
                    ui.painter().rect_filled(r, 3.0, Color32::from_rgb(cr, cg, cb));
                    ui.label(e.display_name());
                });
                row.col(|ui| right(ui, format_size(e.bytes)));
                row.col(|ui| right(ui, format!("{:.1}", percent(e.bytes, total))));
                row.col(|ui| right(ui, format_count(e.files)));
                if row.response().clicked() {
                    actions.push(Action::HighlightExt(if selected { None } else { Some(i as u32) }));
                }
            });
        });
}

fn right(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.label(text);
    });
}
