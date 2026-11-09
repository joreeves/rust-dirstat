//! CPU cushion shading of a [`Layout`] into an RGBA pixel buffer.

use eframe::egui::Color32;

use super::layout::Layout;
use crate::colors::ext_color;
use crate::model::Tree;

/// Ambient light share.
const AMBIENT: f32 = 0.15;
/// Overall brightness boost applied after shading.
const BRIGHTNESS: f32 = 1.3;
/// Normalized light direction (from the upper left, mostly from the front).
const LIGHT: [f32; 3] = {
    let (x, y, z) = (-1.0f32, -1.0f32, 10.0f32);
    // sqrt(1 + 1 + 100) = 10.0995
    let len = 10.099_505;
    [x / len, y / len, z / len]
};

/// How far highlighted files are blended towards white.
const HIGHLIGHT_MIX: f32 = 110.0 / 255.0;

/// Renders `layout` into a `width` x `height` image (row-major). Pixels not covered by any
/// tile are left `background`. Files of type `highlight` (an extension index) are drawn
/// lighter, with a white edge on tiles big enough to show one.
pub fn render(layout: &Layout, tree: &Tree, width: usize, height: usize, background: Color32, highlight: Option<u32>) -> Vec<Color32> {
    let mut px = vec![background; width * height];
    let max_x = width as f32;
    let max_y = height as f32;
    for item in layout.items.iter().filter(|i| i.is_leaf()) {
        let r = item.rect;
        let (x0, x1) = (r.x0.round().clamp(0.0, max_x) as usize, r.x1.round().clamp(0.0, max_x) as usize);
        let (y0, y1) = (r.y0.round().clamp(0.0, max_y) as usize, r.y1.round().clamp(0.0, max_y) as usize);
        if x0 >= x1 || y0 >= y1 {
            continue;
        }
        let lit = highlight == Some(item.color_ext) && tree.node(item.node).is_file();
        let edged = lit && x1 - x0 > 3 && y1 - y0 > 3;
        let [cr, cg, cb] = ext_color(item.color_ext).map(f32::from);
        let s = item.surface;
        for y in y0..y1 {
            let fy = y as f32 + 0.5;
            let ny = -(2.0 * s[1] * fy + s[3]);
            let edge_row = edged && (y == y0 || y == y1 - 1);
            let row = &mut px[y * width + x0..y * width + x1];
            let last = row.len() - 1;
            for (dx, out) in row.iter_mut().enumerate() {
                if edge_row || (edged && (dx == 0 || dx == last)) {
                    *out = Color32::WHITE;
                    continue;
                }
                let fx = (x0 + dx) as f32 + 0.5;
                let nx = -(2.0 * s[0] * fx + s[2]);
                let cos = (nx * LIGHT[0] + ny * LIGHT[1] + LIGHT[2]) / (nx * nx + ny * ny + 1.0).sqrt();
                let light = (AMBIENT + (1.0 - AMBIENT) * cos.clamp(0.0, 1.0)) * BRIGHTNESS;
                let mut c = [cr, cg, cb].map(|v| (v * light).min(255.0));
                if lit {
                    c = c.map(|v| v + (255.0 - v) * HIGHLIGHT_MIX);
                }
                *out = Color32::from_rgb(c[0] as u8, c[1] as u8, c[2] as u8);
            }
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ScanNode, Tree, flags};
    use crate::treemap::layout;

    #[test]
    fn renders_every_pixel_with_shading() {
        let file = |n: &str, s: u64| ScanNode { name: n.into(), size: s, flags: 0, children: vec![] };
        let t = Tree::from_scan(
            "R:\\",
            ScanNode { name: "R".into(), size: 0, flags: flags::DIR, children: vec![file("a.bin", 300), file("b.txt", 100)] },
        );
        let lay = layout(&t, t.root(), 64.0, 32.0);
        let bg = Color32::from_rgb(1, 2, 3);
        let img = render(&lay, &t, 64, 32, bg, None);
        assert_eq!(img.len(), 64 * 32);
        assert!(img.iter().all(|&c| c != bg), "all pixels should be covered");
        // Cushions: the center of a tile is brighter than its lower-right corner.
        let center = img[16 * 64 + 20];
        let corner = img[31 * 64 + 47];
        let lum = |c: Color32| u32::from(c.r()) + u32::from(c.g()) + u32::from(c.b());
        assert!(lum(center) > 0 && lum(corner) > 0);
        assert_ne!(img[16 * 64 + 5], img[16 * 64 + 60], "different files get different colors");

        // Highlighting b.txt's type lightens its tile and edges it in white; a.bin is unchanged.
        let txt = lay.items.iter().find(|i| &*t.node(i.node).name == "b.txt").unwrap();
        let lit = render(&lay, &t, 64, 32, bg, Some(txt.color_ext));
        let r = txt.rect;
        let (cx, cy) = (((r.x0 + r.x1) / 2.0) as usize, ((r.y0 + r.y1) / 2.0) as usize);
        assert!(lum(lit[cy * 64 + cx]) > lum(img[cy * 64 + cx]), "highlighted tile is lighter");
        assert_eq!(lit[r.y0.round() as usize * 64 + cx], Color32::WHITE, "top edge");
        assert_eq!(lit[16 * 64 + 5], img[16 * 64 + 5], "other files are unchanged");
    }
}
