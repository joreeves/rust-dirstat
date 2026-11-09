//! Squarified treemap layout (Bruls, Huizing & van Wijk) with cushion surface coefficients
//! (van Wijk & van de Wetering) accumulated per level.

use crate::model::{NO_EXT, NodeId, Tree};

/// Initial cushion ridge height.
pub const CUSHION_HEIGHT: f32 = 0.38;
/// Ridge height multiplier per nesting level.
pub const CUSHION_SCALE: f32 = 0.91;
/// Directories whose tile is smaller than this many square pixels (about 2 x 2) are drawn as one
/// tile, in the color of their largest file. Larger ones are split into a tile per child, however
/// small: tiles under a pixel go to whichever wins the rounding when drawn, as in WinDirStat.
const MIN_SUBDIVIDE_AREA: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self { x0, y0, x1, y1 }
    }
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }
    pub fn area(&self) -> f32 {
        self.width().max(0.0) * self.height().max(0.0)
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LayoutItem {
    pub rect: Rect,
    /// The tree node shown by this tile.
    pub node: NodeId,
    /// Children occupy `items[first_child..first_child + child_count]`.
    pub first_child: u32,
    pub child_count: u32,
    /// Cushion coefficients: [x², y², x, y].
    pub surface: [f32; 4],
    /// Extension index used to color this tile when it is a leaf.
    pub color_ext: u32,
    pub depth: u16,
}

impl LayoutItem {
    pub fn is_leaf(&self) -> bool {
        self.child_count == 0
    }
}

/// A computed treemap. Item 0 is the root; children of an item are stored contiguously.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub items: Vec<LayoutItem>,
    pub width: f32,
    pub height: f32,
}

impl Layout {
    /// Index of the deepest tile containing `(x, y)`.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<usize> {
        let mut cur = 0usize;
        if !self.items.first()?.rect.contains(x, y) {
            return None;
        }
        loop {
            let it = &self.items[cur];
            let start = it.first_child as usize;
            let next = (start..start + it.child_count as usize).find(|&c| self.items[c].rect.contains(x, y));
            match next {
                Some(c) => cur = c,
                None => return Some(cur),
            }
        }
    }

    /// Index of the tile that shows `target`, or of its closest ancestor that has a tile.
    /// The bool is true when the tile is `target` itself.
    pub fn find(&self, tree: &Tree, target: NodeId) -> Option<(usize, bool)> {
        let root = self.items.first()?.node;
        let chain = tree.ancestry(target);
        let pos = chain.iter().position(|&n| n == root)?;
        let mut cur = 0usize;
        for &want in &chain[pos + 1..] {
            let it = &self.items[cur];
            let start = it.first_child as usize;
            let found = (start..start + it.child_count as usize)
                .find(|&c| self.items[c].node == want);
            match found {
                Some(c) => cur = c,
                None => return Some((cur, false)),
            }
        }
        Some((cur, true))
    }
}

/// Lays out the subtree of `root` in a `width` x `height` pixel area.
pub fn layout(tree: &Tree, root: NodeId, width: f32, height: f32) -> Layout {
    let mut items = Vec::new();
    if width < 1.0 || height < 1.0 || tree.node(root).removed {
        return Layout { items, width, height };
    }
    let rect = Rect::new(0.0, 0.0, width, height);
    let mut surface = [0.0; 4];
    add_ridge(&mut surface, rect, CUSHION_HEIGHT);
    items.push(LayoutItem {
        rect,
        node: root,
        first_child: 0,
        child_count: 0,
        surface,
        color_ext: leaf_color(tree, root),
        depth: 0,
    });

    let mut values = Vec::new();
    let mut rects = Vec::new();
    let mut i = 0;
    while i < items.len() {
        let it = items[i];
        i += 1;
        let node = tree.node(it.node);
        if !node.is_dir() || node.size == 0 || it.rect.area() < MIN_SUBDIVIDE_AREA {
            continue;
        }
        // Children are sorted largest first, so the empty ones come last.
        let kids: Vec<NodeId> = node.children.iter().copied().filter(|&c| tree.node(c).size > 0).collect();
        if kids.is_empty() {
            continue;
        }
        values.clear();
        values.extend(kids.iter().map(|&c| tree.node(c).size as f64));
        rects.clear();
        squarify(&values, it.rect, &mut rects);

        let h = CUSHION_HEIGHT * CUSHION_SCALE.powi(i32::from(it.depth) + 1);
        let first = items.len() as u32;
        for (&kid, r) in kids.iter().zip(&rects) {
            let mut s = it.surface;
            add_ridge(&mut s, *r, h);
            items.push(LayoutItem {
                rect: *r,
                node: kid,
                first_child: 0,
                child_count: 0,
                surface: s,
                color_ext: leaf_color(tree, kid),
                depth: it.depth + 1,
            });
        }
        items[i - 1].first_child = first;
        items[i - 1].child_count = rects.len() as u32;
    }
    Layout { items, width, height }
}

fn leaf_color(tree: &Tree, id: NodeId) -> u32 {
    let n = tree.node(id);
    if n.is_dir() { tree.dominant_ext(id) } else if n.is_file() { n.ext } else { NO_EXT }
}

/// Adds a parabolic ridge over `r` to the cushion surface (WinDirStat's formulation, which
/// keeps the edge slope independent of the tile size).
fn add_ridge(s: &mut [f32; 4], r: Rect, h: f32) {
    let w = r.width();
    if w > 0.0 {
        let wf = 4.0 * h / w;
        s[2] += wf * (r.x1 + r.x0);
        s[0] -= wf;
    }
    let hh = r.height();
    if hh > 0.0 {
        let hf = 4.0 * h / hh;
        s[3] += hf * (r.y1 + r.y0);
        s[1] -= hf;
    }
}

/// Squarified subdivision of `rect` into areas proportional to `values` (sorted descending).
/// Pushes exactly `values.len()` rectangles, in order, that tile `rect` without gaps.
pub fn squarify(values: &[f64], rect: Rect, out: &mut Vec<Rect>) {
    let n = values.len();
    let total: f64 = values.iter().sum();
    let (mut x0, mut y0, x1, y1) = (f64::from(rect.x0), f64::from(rect.y0), f64::from(rect.x1), f64::from(rect.y1));
    if n == 0 {
        return;
    }
    if total <= 0.0 || x1 <= x0 || y1 <= y0 {
        out.extend(std::iter::repeat_n(Rect::new(rect.x0, rect.y0, rect.x0, rect.y0), n));
        return;
    }
    let scale = (x1 - x0) * (y1 - y0) / total;
    let mut i = 0;
    while i < n {
        let (w, h) = (x1 - x0, y1 - y0);
        let short = w.min(h).max(f64::MIN_POSITIVE);
        let biggest = values[i] * scale;
        let mut row = 0.0;
        let mut best = f64::INFINITY;
        let mut j = i;
        while j < n {
            let a = values[j] * scale;
            let s = row + a;
            let worst = if a > 0.0 {
                f64::max(short * short * biggest / (s * s), (s * s) / (short * short * a))
            } else {
                f64::INFINITY
            };
            if j > i && worst > best {
                break;
            }
            best = worst;
            row = s;
            j += 1;
        }
        let last_row = j == n;
        if w >= h {
            // Column along the left edge.
            let t = if last_row { w } else { (row / h).min(w) };
            let mut y = y0;
            for (k, v) in values.iter().enumerate().take(j).skip(i) {
                let hh = if k + 1 == j { y1 - y } else { v * scale / t };
                out.push(Rect::new(x0 as f32, y as f32, (x0 + t) as f32, (y + hh) as f32));
                y += hh;
            }
            x0 += t;
        } else {
            // Row along the top edge.
            let t = if last_row { h } else { (row / w).min(h) };
            let mut x = x0;
            for (k, v) in values.iter().enumerate().take(j).skip(i) {
                let ww = if k + 1 == j { x1 - x } else { v * scale / t };
                out.push(Rect::new(x as f32, y0 as f32, (x + ww) as f32, (y0 + t) as f32));
                x += ww;
            }
            y0 += t;
        }
        i = j;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ScanNode, flags};

    fn rects_for(values: &[f64], rect: Rect) -> Vec<Rect> {
        let mut out = Vec::new();
        squarify(values, rect, &mut out);
        out
    }

    #[test]
    fn areas_are_proportional_and_tile_the_rect() {
        let values = [500.0, 250.0, 120.0, 60.0, 40.0, 20.0, 7.0, 3.0];
        let rect = Rect::new(10.0, 20.0, 810.0, 620.0);
        let out = rects_for(&values, rect);
        assert_eq!(out.len(), values.len());
        let total: f64 = values.iter().sum();
        let mut area_sum = 0.0;
        for (v, r) in values.iter().zip(&out) {
            let expect = v / total * f64::from(rect.area());
            assert!((f64::from(r.area()) - expect).abs() / expect < 0.01, "{r:?} area {} vs {expect}", r.area());
            assert!(r.x0 >= rect.x0 - 0.01 && r.x1 <= rect.x1 + 0.01 && r.y0 >= rect.y0 - 0.01 && r.y1 <= rect.y1 + 0.01);
            area_sum += f64::from(r.area());
        }
        assert!((area_sum - f64::from(rect.area())).abs() < 1.0);
        for (i, a) in out.iter().enumerate() {
            for b in &out[i + 1..] {
                let ox = (a.x1.min(b.x1) - a.x0.max(b.x0)).max(0.0);
                let oy = (a.y1.min(b.y1) - a.y0.max(b.y0)).max(0.0);
                assert!(ox * oy < 0.01, "overlap between {a:?} and {b:?}");
            }
        }
    }

    #[test]
    fn aspect_ratios_are_reasonable() {
        let values = vec![1.0; 100];
        let out = rects_for(&values, Rect::new(0.0, 0.0, 1000.0, 1000.0));
        let worst = out.iter().map(|r| (r.width() / r.height()).max(r.height() / r.width())).fold(0.0f32, f32::max);
        assert!(worst < 3.0, "worst aspect ratio {worst}");
    }

    #[test]
    fn single_and_degenerate() {
        let r = Rect::new(0.0, 0.0, 100.0, 50.0);
        assert_eq!(rects_for(&[42.0], r), vec![r]);
        assert_eq!(rects_for(&[0.0, 0.0], r).len(), 2);
        assert_eq!(rects_for(&[1.0], Rect::new(0.0, 0.0, 0.0, 10.0)).len(), 1);
        assert!(rects_for(&[], r).is_empty());
    }

    fn tree() -> Tree {
        let file = |n: &str, s: u64| ScanNode { name: n.into(), size: s, flags: 0, children: vec![] };
        let mut many: Vec<ScanNode> = (0..5000).map(|i| file(&format!("tiny{i}.log"), 1)).collect();
        many.push(file("huge.bin", 10_000_000));
        Tree::from_scan(
            "R:\\",
            ScanNode {
                name: "R".into(),
                size: 0,
                flags: flags::DIR,
                children: vec![
                    ScanNode { name: "d".into(), size: 0, flags: flags::DIR, children: many },
                    file("a.mp4", 5_000_000),
                    file("zero", 0),
                ],
            },
        )
    }

    fn named(t: &Tree, name: &str) -> NodeId {
        t.preorder(t.root(), None).map(|(id, _)| id).find(|&id| &*t.node(id).name == name).unwrap()
    }

    #[test]
    fn every_file_gets_its_own_tile_however_small() {
        let t = tree();
        let lay = layout(&t, t.root(), 400.0, 300.0);
        // The root, d, a.mp4 and all 5,001 files in d (each tiny file is under 0.01 px²).
        assert_eq!(lay.items.len(), 3 + 5001);
        // Zero-size nodes get no tile.
        assert!(lay.items.iter().all(|i| &*t.node(i.node).name != "zero"));
        // Leaves tile the full area.
        let leaf_area: f32 = lay.items.iter().filter(|i| i.is_leaf()).map(|i| i.rect.area()).sum();
        assert!((leaf_area - 400.0 * 300.0).abs() < 1.0);
        // Wherever you point, the tile is a file, never the folder around it.
        for y in (0..300).step_by(7) {
            for x in (0..400).step_by(7) {
                let i = lay.hit_test(x as f32 + 0.5, y as f32 + 0.5).unwrap();
                let n = t.node(lay.items[i].node);
                assert!(n.is_file(), "({x}, {y}) shows {}", n.name);
            }
        }
        // find() maps a file, large or tiny, to its own tile.
        for name in ["huge.bin", "tiny4999.log"] {
            let id = named(&t, name);
            let (idx, exact) = lay.find(&t, id).unwrap();
            assert!(exact, "{name}");
            assert_eq!(lay.items[idx].node, id);
        }
        let (idx, _) = lay.find(&t, named(&t, "huge.bin")).unwrap();
        let r = lay.items[idx].rect;
        assert_eq!(lay.hit_test((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0), Some(idx));
        assert_eq!(lay.hit_test(-1.0, 5.0), None);
    }

    #[test]
    fn thin_folders_are_split_and_dot_sized_ones_are_not() {
        let file = |n: &str, s: u64| ScanNode { name: n.into(), size: s, flags: 0, children: vec![] };
        let dir = |n: &str, children| ScanNode { name: n.into(), size: 0, flags: flags::DIR, children };
        let t = Tree::from_scan(
            "R:\\",
            dir(
                "R",
                vec![
                    dir("strip", vec![file("s1.bin", 600), file("s2.bin", 400)]),
                    dir("dot", vec![file("d1.bin", 2), file("d2.bin", 1)]),
                    file("big.bin", 299_000),
                ],
            ),
        );
        // A folder drawn 2 px wide still shows its files.
        let strip = named(&t, "strip");
        let lay = layout(&t, strip, 2.0, 500.0);
        assert_eq!(lay.items.len(), 3);
        assert_eq!(lay.find(&t, named(&t, "s2.bin")).map(|(_, exact)| exact), Some(true));
        // A folder smaller than about 2 x 2 px is one tile; its files map to it.
        let lay = layout(&t, t.root(), 300.0, 100.0);
        let dot = named(&t, "dot");
        let (idx, exact) = lay.find(&t, named(&t, "d1.bin")).unwrap();
        assert_eq!((lay.items[idx].node, exact), (dot, false));
        assert!(lay.items[idx].is_leaf());
    }
}
