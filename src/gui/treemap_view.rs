//! The treemap panel: renders on a background thread into a texture, then handles hover,
//! selection, zoom and the context menu on top of it.

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, ColorImage, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle,
    TextureOptions, pos2,
};

use super::View;
use super::actions::{Action, context_menu};
use crate::model::{NodeId, Tree};
use crate::treemap::{self, Layout};
use crate::util::{format_count, format_size, percent};

/// Everything that determines the rendered image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    zoom: NodeId,
    size: [usize; 2],
    generation: u64,
    dark: bool,
}

struct Rendered {
    key: Key,
    layout: Layout,
    image: ColorImage,
}

#[derive(Default)]
pub struct TreemapView {
    texture: Option<TextureHandle>,
    /// Layout matching the current texture.
    layout: Option<Layout>,
    shown: Option<Key>,
    pending: Option<mpsc::Receiver<Rendered>>,
    wanted: Option<(Key, Instant)>,
    /// Bumped whenever the tree changes.
    generation: u64,
    context_node: Option<NodeId>,
    /// Screen area of the treemap in the last frame.
    last_rect: Option<Rect>,
}

impl TreemapView {
    /// Forget everything (new tree).
    pub fn reset(&mut self) {
        *self = Self { generation: self.generation + 1, ..Default::default() };
    }

    /// The tree changed in place (delete / rescan): re-render, keeping the old image meanwhile.
    pub fn invalidate(&mut self) {
        self.generation += 1;
    }

    /// Screen position of the center of the tile showing `id`, if it has its own tile.
    #[cfg(test)]
    pub(crate) fn tile_center(&self, tree: &Tree, id: NodeId) -> Option<Pos2> {
        let (layout, rect) = (self.layout.as_ref()?, self.last_rect?);
        let (idx, exact) = layout.find(tree, id)?;
        let r = layout.items[idx].rect;
        exact.then(|| {
            pos2(
                rect.min.x + (r.x0 + r.x1) / 2.0 * rect.width() / layout.width,
                rect.min.y + (r.y0 + r.y1) / 2.0 * rect.height() / layout.height,
            )
        })
    }

    /// True once a rendered image for the current tree is on screen.
    #[cfg(test)]
    pub(crate) fn is_current(&self) -> bool {
        self.shown.is_some_and(|k| k.generation == self.generation) && self.pending.is_none()
    }

    /// True once the image on screen is current and rooted at `zoom`.
    #[cfg(test)]
    pub(crate) fn is_showing(&self, zoom: NodeId) -> bool {
        self.is_current() && self.shown.is_some_and(|k| k.zoom == zoom)
    }
}

pub fn show(ui: &mut egui::Ui, tree: &Arc<Tree>, view: &mut View, tm: &mut TreemapView, actions: &mut Vec<Action>) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.strong("Treemap");
        let zoom_path = tree.path_of(view.zoom).map(|p| p.display().to_string()).unwrap_or_else(|| tree.node(view.zoom).name.to_string());
        ui.label(RichText::new(zoom_path).monospace());
        if view.zoom != tree.root() {
            if ui.small_button("⬆ Zoom out").clicked() {
                actions.push(Action::ZoomOut);
            }
            if ui.small_button("Reset").clicked() {
                actions.push(Action::ZoomReset);
            }
        }
        ui.label(RichText::new("double-click a tile to zoom in • right-click for actions").weak());
    });
    ui.add_space(2.0);

    let rect = ui.available_rect_before_wrap();
    if rect.width() < 2.0 || rect.height() < 2.0 {
        return;
    }
    let response = ui.allocate_rect(rect, Sense::click());
    tm.last_rect = Some(rect);
    let ctx = ui.ctx().clone();
    let ppp = ctx.pixels_per_point();
    let max_side = ctx.input(|i| i.max_texture_side).max(1);
    let size = [
        ((rect.width() * ppp).round() as usize).clamp(1, max_side),
        ((rect.height() * ppp).round() as usize).clamp(1, max_side),
    ];
    let background = ui.visuals().extreme_bg_color;
    let key = Key { zoom: view.zoom, size, generation: tm.generation, dark: ui.visuals().dark_mode };

    receive(tm, &ctx);
    request(tm, tree, key, background, &ctx);

    let painter = ui.painter_at(rect);
    match &tm.texture {
        Some(tex) => {
            painter.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => {
            painter.rect_filled(rect, 0.0, background);
            painter.text(rect.center(), Align2::CENTER_CENTER, "Rendering…", FontId::proportional(16.0), ui.visuals().weak_text_color());
        }
    }
    if tree.node(view.zoom).size == 0 {
        painter.text(rect.center(), Align2::CENTER_CENTER, "Nothing to show (empty)", FontId::proportional(16.0), ui.visuals().weak_text_color());
    }
    let Some(layout) = &tm.layout else { return };
    if layout.width <= 0.0 || layout.height <= 0.0 {
        return;
    }

    let (sx, sy) = (rect.width() / layout.width, rect.height() / layout.height);
    let to_screen = |r: treemap::Rect| {
        Rect::from_min_max(
            pos2(rect.min.x + r.x0 * sx, rect.min.y + r.y0 * sy),
            pos2(rect.min.x + r.x1 * sx, rect.min.y + r.y1 * sy),
        )
    };
    let hit = |p: Pos2| layout.hit_test((p.x - rect.min.x) / sx, (p.y - rect.min.y) / sy);

    if let Some(ext) = view.highlight_ext {
        let mut drawn = 0;
        for item in layout.items.iter().filter(|i| i.is_leaf() && !i.merged && i.color_ext == ext) {
            if !tree.node(item.node).is_file() {
                continue;
            }
            let r = to_screen(item.rect);
            painter.rect_filled(r, 0.0, Color32::from_white_alpha(110));
            if r.width() > 3.0 && r.height() > 3.0 {
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::WHITE), StrokeKind::Inside);
            }
            drawn += 1;
            if drawn > 50_000 {
                break;
            }
        }
    }

    if let Some(sel) = view.selected
        && tree.is_reachable(sel)
        && let Some((idx, _)) = layout.find(tree, sel)
    {
        let r = to_screen(layout.items[idx].rect);
        painter.rect_stroke(r, 0.0, Stroke::new(3.0, Color32::BLACK), StrokeKind::Inside);
        painter.rect_stroke(r.shrink(1.0), 0.0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Inside);
    }

    let hovered = response.hover_pos().and_then(hit);
    if let Some(i) = hovered {
        let r = to_screen(layout.items[i].rect);
        painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_white_alpha(200)), StrokeKind::Inside);
    }

    if response.double_clicked() {
        if let Some(i) = hovered {
            actions.push(Action::Zoom(layout.items[i].node));
        }
    } else if (response.clicked() || response.secondary_clicked())
        && let Some(i) = hovered {
            actions.push(Action::Select { id: layout.items[i].node, reveal: true });
        }
    if response.secondary_clicked() {
        tm.context_node = hovered.map(|i| layout.items[i].node);
    }

    let response = match hovered {
        Some(i) => response.on_hover_ui_at_pointer(|ui| tooltip(ui, tree, view.zoom, layout, i)),
        None => response,
    };
    let context_node = tm.context_node;
    response.context_menu(|ui| {
        if let Some(id) = context_node.filter(|&id| tree.is_reachable(id)) {
            context_menu(ui, tree, id, actions);
        }
    });
}

fn tooltip(ui: &mut egui::Ui, tree: &Tree, zoom: NodeId, layout: &Layout, i: usize) {
    let item = &layout.items[i];
    let n = tree.node(item.node);
    let zoom_size = tree.node(zoom).size;
    if item.merged {
        ui.strong(format!("Many small items in {}", n.name));
        ui.label("Zoom in to see them individually.");
        return;
    }
    let path = tree.path_of(item.node).map(|p| p.display().to_string()).unwrap_or_else(|| n.name.to_string());
    ui.strong(&*n.name);
    ui.label(RichText::new(path).monospace().weak());
    ui.label(format!("{}  ({:.2}% of view)", format_size(n.size), percent(n.size, zoom_size)));
    if n.is_dir() {
        ui.label(format!("{} files, {} folders", format_count(n.files), format_count(n.dirs)));
    } else if let Some(e) = tree.exts().get(n.ext as usize) {
        ui.label(format!("Type: {}", e.display_name()));
    }
}

fn receive(tm: &mut TreemapView, ctx: &egui::Context) {
    let Some(rx) = &tm.pending else { return };
    match rx.try_recv() {
        Ok(r) => {
            match &mut tm.texture {
                Some(t) => t.set(r.image, TextureOptions::NEAREST),
                None => tm.texture = Some(ctx.load_texture("treemap", r.image, TextureOptions::NEAREST)),
            }
            tm.layout = Some(r.layout);
            tm.shown = Some(r.key);
            tm.pending = None;
        }
        Err(mpsc::TryRecvError::Empty) => {}
        Err(mpsc::TryRecvError::Disconnected) => tm.pending = None,
    }
}

/// Starts a render for `key` unless one is running; resizes are debounced.
fn request(tm: &mut TreemapView, tree: &Arc<Tree>, key: Key, background: Color32, ctx: &egui::Context) {
    if tm.shown == Some(key) {
        return;
    }
    let now = Instant::now();
    let since = match tm.wanted {
        Some((k, t)) if k == key => now.duration_since(t),
        _ => {
            tm.wanted = Some((key, now));
            Duration::ZERO
        }
    };
    let resize_only = tm.shown.is_some_and(|s| s.zoom == key.zoom && s.generation == key.generation && s.dark == key.dark);
    let delay = if resize_only { Duration::from_millis(150) } else { Duration::ZERO };
    if tm.pending.is_none() && since >= delay {
        tm.pending = Some(spawn_render(tree.clone(), key, background, ctx.clone()));
    } else {
        ctx.request_repaint_after(delay.saturating_sub(since).max(Duration::from_millis(16)));
    }
}

fn spawn_render(tree: Arc<Tree>, key: Key, background: Color32, ctx: egui::Context) -> mpsc::Receiver<Rendered> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("treemap".into())
        .spawn(move || {
            let [w, h] = key.size;
            let layout = treemap::layout(&tree, key.zoom, w as f32, h as f32);
            let pixels = treemap::render(&layout, w, h, background);
            let image = ColorImage::new([w, h], pixels);
            let _ = tx.send(Rendered { key, layout, image });
            ctx.request_repaint();
        })
        .expect("failed to spawn treemap thread");
    rx
}
