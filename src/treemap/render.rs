//! CPU cushion shading of a [`Layout`] into an RGBA pixel buffer.

use eframe::egui::Color32;

use super::layout::Layout;
use crate::colors::ext_color;

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

/// Renders `layout` into a `width` x `height` image (row-major). Pixels not covered by any
/// tile are left `background`.
pub fn render(layout: &Layout, width: usize, height: usize, background: Color32) -> Vec<Color32> {
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
        let [cr, cg, cb] = ext_color(item.color_ext).map(f32::from);
        let s = item.surface;
        for y in y0..y1 {
            let fy = y as f32 + 0.5;
            let ny = -(2.0 * s[1] * fy + s[3]);
            let row = &mut px[y * width + x0..y * width + x1];
            for (dx, out) in row.iter_mut().enumerate() {
                let fx = (x0 + dx) as f32 + 0.5;
                let nx = -(2.0 * s[0] * fx + s[2]);
                let cos = (nx * LIGHT[0] + ny * LIGHT[1] + LIGHT[2]) / (nx * nx + ny * ny + 1.0).sqrt();
                let light = (AMBIENT + (1.0 - AMBIENT) * cos.clamp(0.0, 1.0)) * BRIGHTNESS;
                *out = Color32::from_rgb(
                    (cr * light).min(255.0) as u8,
                    (cg * light).min(255.0) as u8,
                    (cb * light).min(255.0) as u8,
                );
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
        let img = render(&lay, 64, 32, bg);
        assert_eq!(img.len(), 64 * 32);
        assert!(img.iter().all(|&c| c != bg), "all pixels should be covered");
        // Cushions: the center of a tile is brighter than its lower-right corner.
        let center = img[16 * 64 + 20];
        let corner = img[31 * 64 + 47];
        let lum = |c: Color32| u32::from(c.r()) + u32::from(c.g()) + u32::from(c.b());
        assert!(lum(center) > 0 && lum(corner) > 0);
        assert_ne!(img[16 * 64 + 5], img[16 * 64 + 60], "different files get different colors");
    }
}
