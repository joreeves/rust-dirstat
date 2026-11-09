//! Squarified treemap layout and WinDirStat-style cushion rendering.

pub mod layout;
pub mod render;

pub use layout::{Layout, LayoutItem, Rect, layout};
pub use render::render;
