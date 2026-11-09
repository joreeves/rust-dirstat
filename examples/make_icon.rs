//! Regenerates the app icon files from `assets/icon.svg` (and `assets/icon-small.svg` for the
//! smallest sizes):
//!
//! ```text
//! cargo run --example make_icon [-- <preview dir>]
//! ```
//!
//! Writes `assets/rust-dirstat.ico` (embedded in the exe and used by the installer) and
//! `assets/icon-256.png` (the window icon and README image). With a preview directory, it also
//! writes each size there as a PNG.

use std::path::{Path, PathBuf};

use resvg::{tiny_skia, usvg};

/// Sizes Windows picks from: Explorer views, taskbar, title bar, Start menu, high-DPI variants.
const SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];
/// Up to this size the simplified drawing is used, so the blocks stay distinct.
const SMALL_MAX: u32 = 24;

fn render(svg: &str, size: u32) -> Vec<u8> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).expect("valid SVG");
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("non-zero size");
    let scale = size as f32 / tree.size().width();
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.encode_png().expect("PNG encoding")
}

/// An .ico file whose entries are PNG images (supported since Windows Vista).
fn ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let count = u16::try_from(images.len()).expect("few images");
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]); // reserved, type 1 = icon
    out.extend_from_slice(&count.to_le_bytes());
    let mut offset = 6 + 16 * u32::from(count);
    for (size, png) in images {
        let dim = if *size >= 256 { 0 } else { *size as u8 }; // 0 means 256
        let len = u32::try_from(png.len()).expect("small image");
        out.extend_from_slice(&[dim, dim, 0, 0]); // width, height, colors, reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // color planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += len;
    }
    for (_, png) in images {
        out.extend_from_slice(png);
    }
    out
}

fn main() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let read = |name: &str| std::fs::read_to_string(assets.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let (full, small) = (read("icon.svg"), read("icon-small.svg"));
    let images: Vec<(u32, Vec<u8>)> =
        SIZES.iter().map(|&s| (s, render(if s <= SMALL_MAX { &small } else { &full }, s))).collect();

    let ico_path = assets.join("rust-dirstat.ico");
    std::fs::write(&ico_path, ico(&images)).expect("write the .ico");
    let png_path = assets.join("icon-256.png");
    std::fs::write(&png_path, render(&full, 256)).expect("write the PNG");
    println!("wrote {} and {}", ico_path.display(), png_path.display());

    if let Some(dir) = std::env::args_os().nth(1).map(PathBuf::from) {
        std::fs::create_dir_all(&dir).expect("create the preview directory");
        for (size, png) in &images {
            std::fs::write(dir.join(format!("icon-{size}.png")), png).expect("write a preview");
        }
        println!("wrote previews to {}", dir.display());
    }
}
