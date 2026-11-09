//! Extension colors shared by the treemap and the extension list.

use crate::model::NO_EXT;

/// Distinct hues for the extensions with the most bytes (index 0 = largest).
pub const PALETTE: [[u8; 3]; 12] = [
    [66, 133, 244],  // blue
    [234, 67, 53],   // red
    [52, 168, 83],   // green
    [251, 188, 5],   // amber
    [171, 71, 188],  // purple
    [0, 172, 193],   // cyan
    [255, 112, 67],  // orange
    [124, 179, 66],  // lime
    [92, 107, 192],  // indigo
    [240, 98, 146],  // pink
    [38, 166, 154],  // teal
    [161, 136, 127], // brown
];

/// Color for every extension outside the top [`PALETTE`] entries.
pub const OTHER: [u8; 3] = [158, 158, 158];

/// Color for an extension index from `Tree::exts()` (which is sorted by bytes).
pub fn ext_color(ext: u32) -> [u8; 3] {
    if ext == NO_EXT {
        return OTHER;
    }
    PALETTE.get(ext as usize).copied().unwrap_or(OTHER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_lookup() {
        assert_eq!(ext_color(0), PALETTE[0]);
        assert_eq!(ext_color(11), PALETTE[11]);
        assert_eq!(ext_color(12), OTHER);
        assert_eq!(ext_color(NO_EXT), OTHER);
    }
}
