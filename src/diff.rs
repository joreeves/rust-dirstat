//! Comparing two scans ("diff mode").
//!
//! [`Diff::new`] matches two trees by path and builds one combined tree whose node sizes are the
//! bytes that differ. Identical items get size 0, so the usual tree table sorts folders by how
//! much differs inside them and the treemap shows only the differences.
//!
//! Matching rules:
//! - Items match by name, ignoring case as Windows does.
//! - A file and a folder with the same name are different items.
//! - Files are compared by size only, so two files of the same size count as the same.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::model::{NodeId, ScanInfo, Tree, flags};
use crate::scan::{self, Cancelled, Method, Progress};

/// One of the two compared locations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn label(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
        }
    }
}

/// How an item differs between the two sides.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    OnlyLeft,
    OnlyRight,
    /// On both sides, larger on the left.
    LargerLeft,
    /// On both sides, larger on the right.
    LargerRight,
    /// A folder with the same total size on both sides but different contents.
    Changed,
    /// The same size on both sides (for a folder: nothing inside differs).
    Same,
}

impl Status {
    pub const ALL: [Status; 6] =
        [Status::OnlyLeft, Status::OnlyRight, Status::LargerLeft, Status::LargerRight, Status::Changed, Status::Same];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(i: u32) -> Option<Status> {
        Self::ALL.get(i as usize).copied()
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::OnlyLeft => "Only in left",
            Status::OnlyRight => "Only in right",
            Status::LargerLeft => "Larger in left",
            Status::LargerRight => "Larger in right",
            Status::Changed => "Contents differ",
            Status::Same => "Same size",
        }
    }

    /// Short machine-readable name (CSV export).
    pub fn slug(self) -> &'static str {
        match self {
            Status::OnlyLeft => "only_left",
            Status::OnlyRight => "only_right",
            Status::LargerLeft => "larger_left",
            Status::LargerRight => "larger_right",
            Status::Changed => "contents_differ",
            Status::Same => "same",
        }
    }

    /// Treemap and legend color. Warm colors mean the left side has more, cool colors mean the
    /// right side has more (avoiding a red/green pair).
    pub fn rgb(self) -> [u8; 3] {
        match self {
            Status::OnlyLeft => [229, 57, 53],     // red
            Status::LargerLeft => [251, 160, 0],   // amber
            Status::OnlyRight => [30, 136, 229],   // blue
            Status::LargerRight => [0, 188, 212],  // cyan
            Status::Changed => [156, 39, 176],     // purple
            Status::Same => [158, 158, 158],       // gray
        }
    }

    fn of_sizes(left: u64, right: u64) -> Status {
        match left.cmp(&right) {
            Ordering::Greater => Status::LargerLeft,
            Ordering::Less => Status::LargerRight,
            Ordering::Equal => Status::Same,
        }
    }
}

/// Comparison data for one node of [`Diff::tree`].
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    /// Size on the left (for a folder: its total), 0 when it is only on the right.
    pub left: u64,
    pub right: u64,
    /// Items in the subtree that differ: files, links and empty folders that aren't
    /// [`Status::Same`].
    pub changes: u32,
    pub status: Status,
}

impl Entry {
    pub fn on(&self, side: Side) -> bool {
        match side {
            Side::Left => self.status != Status::OnlyRight,
            Side::Right => self.status != Status::OnlyLeft,
        }
    }

    pub fn size(&self, side: Side) -> u64 {
        match side {
            Side::Left => self.left,
            Side::Right => self.right,
        }
    }
}

/// One compared location.
#[derive(Clone, Debug)]
pub struct Location {
    pub root: PathBuf,
    pub size: u64,
    pub files: u64,
    pub dirs: u64,
    pub info: ScanInfo,
}

impl Location {
    fn of(tree: &Tree) -> Self {
        let root = tree.node(tree.root());
        Self { root: tree.root_path().to_path_buf(), size: root.size, files: root.files, dirs: root.dirs, info: tree.info.clone() }
    }
}

/// Items and bytes per [`Status`], counting files, links and empty folders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Total {
    pub items: u64,
    /// Bytes on the side that has more: the item's size if it is on one side only, otherwise the
    /// size difference (or the size, for [`Status::Same`]).
    pub bytes: u64,
}

/// Two locations matched item by item.
#[derive(Clone, Debug)]
pub struct Diff {
    /// The union of both sides. A node's size is the number of bytes that differ in it: the
    /// size difference of a file on both sides, the whole size of an item on one side only, and
    /// for a folder the sum over its contents.
    pub tree: Tree,
    entries: Vec<Entry>,
    pub left: Location,
    pub right: Location,
    pub totals: [Total; Status::ALL.len()],
    /// Scanning (both sides at once) and matching.
    pub elapsed: Duration,
}

/// A child of a folder on one side, keyed for matching.
struct Key<'a> {
    name: Cow<'a, str>,
    dir: bool,
    id: NodeId,
}

fn keys<'a>(tree: &'a Tree, dir: NodeId, out: &mut Vec<Key<'a>>) {
    out.clear();
    out.extend(tree.node(dir).children.iter().map(|&id| {
        let n = tree.node(id);
        let name = if n.name.chars().any(char::is_uppercase) { Cow::Owned(n.name.to_lowercase()) } else { Cow::Borrowed(&*n.name) };
        Key { name, dir: n.is_dir(), id }
    }));
    // Names that differ only in case on the same side pair up in name order.
    out.sort_unstable_by(|a, b| key_order(a, b).then_with(|| tree.node(a.id).name.cmp(&tree.node(b.id).name)));
}

fn key_order(a: &Key<'_>, b: &Key<'_>) -> Ordering {
    a.name.cmp(&b.name).then(a.dir.cmp(&b.dir))
}

/// Flags a combined node keeps from its source nodes.
const KEPT_FLAGS: u8 = flags::DIR | flags::LINK | flags::UNREADABLE | flags::GROUP | flags::NO_PATH;

struct Builder {
    tree: Tree,
    entries: Vec<Entry>,
}

impl Builder {
    fn add(&mut self, parent: NodeId, name: &str, size: u64, node_flags: u8, entry: Entry) -> NodeId {
        let id = self.tree.add_child(parent, name, size, node_flags & KEPT_FLAGS);
        debug_assert_eq!(id.index(), self.entries.len());
        self.entries.push(entry);
        id
    }

    /// Copies `from` and everything below it, which exists on one side only.
    fn add_one_sided(&mut self, src: &Tree, from: NodeId, parent: NodeId, side: Side) {
        let mut stack = vec![(from, parent)];
        while let Some((id, parent)) = stack.pop() {
            let n = src.node(id);
            let (left, right, status) = match side {
                Side::Left => (n.size, 0, Status::OnlyLeft),
                Side::Right => (0, n.size, Status::OnlyRight),
            };
            let new = self.add(parent, &n.name, n.size, n.flags, Entry { left, right, changes: 0, status });
            stack.extend(n.children.iter().map(|&c| (c, new)));
        }
    }
}

impl Diff {
    /// Matches `left` and `right` item by item (see the module docs).
    pub fn new(left: &Tree, right: &Tree) -> Self {
        let start = Instant::now();
        let (lroot, rroot) = (left.node(left.root()), right.node(right.root()));
        let mut tree = Tree::new(PathBuf::new());
        let root = tree.root();
        tree.set_name(root, &format!("{}  ↔  {}", left.root_path().display(), right.root_path().display()));
        let mut b = Builder {
            tree,
            entries: vec![Entry { left: lroot.size, right: rroot.size, changes: 0, status: Status::of_sizes(lroot.size, rroot.size) }],
        };

        let (mut lkeys, mut rkeys) = (Vec::new(), Vec::new());
        let mut stack = vec![(left.root(), right.root(), root)];
        while let Some((l, r, parent)) = stack.pop() {
            keys(left, l, &mut lkeys);
            keys(right, r, &mut rkeys);
            let (mut i, mut j) = (0, 0);
            loop {
                let order = match (lkeys.get(i), rkeys.get(j)) {
                    (Some(a), Some(b)) => key_order(a, b),
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => break,
                };
                match order {
                    Ordering::Less => {
                        b.add_one_sided(left, lkeys[i].id, parent, Side::Left);
                        i += 1;
                    }
                    Ordering::Greater => {
                        b.add_one_sided(right, rkeys[j].id, parent, Side::Right);
                        j += 1;
                    }
                    Ordering::Equal => {
                        let (lc, rc) = (lkeys[i].id, rkeys[j].id);
                        let (ln, rn) = (left.node(lc), right.node(rc));
                        let entry = Entry { left: ln.size, right: rn.size, changes: 0, status: Status::of_sizes(ln.size, rn.size) };
                        // A folder's size comes from its contents once the tree is finalized.
                        let size = if ln.is_dir() { 0 } else { ln.size.abs_diff(rn.size) };
                        let id = b.add(parent, &ln.name, size, ln.flags | rn.flags, entry);
                        if ln.is_dir() {
                            stack.push((lc, rc, id));
                        }
                        i += 1;
                        j += 1;
                    }
                }
            }
        }

        let Builder { mut tree, mut entries } = b;
        tree.finalize();
        // Children are always added after their parent, so going backwards through the arena
        // sees every child before its parent.
        let mut totals = [Total::default(); Status::ALL.len()];
        for i in (0..entries.len()).rev() {
            let id = NodeId(i as u32);
            let n = tree.node(id);
            let e = &mut entries[i];
            if n.children.is_empty() {
                e.changes = u32::from(e.status != Status::Same);
                let t = &mut totals[e.status.index()];
                t.items += 1;
                t.bytes += if e.status == Status::Same { e.left } else { e.left.abs_diff(e.right) };
            } else if e.status == Status::Same && e.changes > 0 {
                e.status = Status::Changed;
            }
            if let Some(p) = n.parent {
                let changes = entries[i].changes;
                entries[p.index()].changes = entries[p.index()].changes.saturating_add(changes);
            }
        }
        let elapsed = left.info.elapsed.max(right.info.elapsed) + start.elapsed();
        Self { tree, entries, left: Location::of(left), right: Location::of(right), totals, elapsed }
    }

    pub fn entry(&self, id: NodeId) -> &Entry {
        &self.entries[id.index()]
    }

    pub fn location(&self, side: Side) -> &Location {
        match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
        }
    }

    /// Number of differing items (files, links and empty folders).
    pub fn changes(&self) -> u32 {
        self.entry(self.tree.root()).changes
    }

    /// The status that colors a treemap tile for `id`: its own for a file, otherwise that of
    /// the largest difference inside it.
    pub fn color_status(&self, id: NodeId) -> Status {
        self.entry(self.tree.dominant_leaf(id)).status
    }

    /// Path of `id` relative to the compared roots (empty for the root itself).
    pub fn relative_path(&self, id: NodeId) -> Option<PathBuf> {
        self.tree.path_under(Path::new(""), id)
    }

    /// Path of `id` on one side, if it exists there.
    pub fn path(&self, id: NodeId, side: Side) -> Option<PathBuf> {
        if !self.entry(id).on(side) {
            return None;
        }
        self.tree.path_under(&self.location(side).root, id)
    }

    /// `relative_path`, or the node's name where there is no path (the root, synthetic nodes).
    pub fn display_path(&self, id: NodeId) -> String {
        match self.relative_path(id) {
            Some(p) if !p.as_os_str().is_empty() => p.display().to_string(),
            _ => self.tree.node(id).name.to_string(),
        }
    }
}

/// Signed size difference from left to right, e.g. `+1.50 GiB`, `-20 KiB` or `0 B`.
pub fn format_delta(left: u64, right: u64) -> String {
    use crate::util::format_size;
    match right.cmp(&left) {
        Ordering::Greater => format!("+{}", format_size(right - left)),
        Ordering::Less => format!("-{}", format_size(left - right)),
        Ordering::Equal => "0 B".to_owned(),
    }
}

/// Scans both locations at the same time. If one side fails, the other is cancelled and that
/// failure is returned.
pub fn scan_both(paths: [&Path; 2], method: Method, progress: [&Progress; 2]) -> anyhow::Result<[Tree; 2]> {
    let results = std::thread::scope(|s| {
        let jobs = [0, 1].map(|i| {
            std::thread::Builder::new()
                .name(format!("scan-{}", ["left", "right"][i]))
                .spawn_scoped(s, move || {
                    let result = scan::scan(paths[i], method, progress[i]);
                    if result.is_err() {
                        progress[1 - i].cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    result
                })
        });
        jobs.map(|job| match job {
            Ok(handle) => handle.join().unwrap_or_else(|_| Err(anyhow::anyhow!("the scan thread panicked"))),
            Err(e) => Err(e.into()),
        })
    });
    let [left, right] = results;
    let side = |result: anyhow::Result<Tree>, side: Side| {
        result.map_err(|e| if e.is::<Cancelled>() { e } else { e.context(format!("could not scan the {} side", side.label())) })
    };
    match (side(left, Side::Left), side(right, Side::Right)) {
        (Ok(l), Ok(r)) => Ok([l, r]),
        // Prefer the failure that made the other side stop.
        (Err(e), Err(f)) if e.is::<Cancelled>() => Err(f),
        (Err(e), _) | (_, Err(e)) => Err(e),
    }
}

/// Scans both locations and compares them.
pub fn compare(paths: [&Path; 2], method: Method, progress: [&Progress; 2]) -> anyhow::Result<Diff> {
    let start = Instant::now();
    let [left, right] = scan_both(paths, method, progress)?;
    let mut diff = Diff::new(&left, &right);
    diff.elapsed = start.elapsed();
    Ok(diff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ScanNode;

    fn file(name: &str, size: u64) -> ScanNode {
        ScanNode { name: name.into(), size, flags: 0, children: vec![] }
    }

    fn dir(name: &str, children: Vec<ScanNode>) -> ScanNode {
        ScanNode { name: name.into(), size: 0, flags: flags::DIR, children }
    }

    fn find(d: &Diff, path: &str) -> NodeId {
        d.tree
            .preorder(d.tree.root(), None)
            .map(|(id, _)| id)
            .find(|&id| d.relative_path(id).is_some_and(|p| p == Path::new(path)))
            .unwrap_or_else(|| panic!("{path} not in the comparison"))
    }

    fn sample() -> Diff {
        let left = Tree::from_scan(
            "L:\\",
            dir("L", vec![
                dir("Same", vec![file("a.txt", 10), file("b.txt", 20)]),
                dir("Photos", vec![file("one.jpg", 1000), file("two.jpg", 2000), file("gone.jpg", 500)]),
                file("Readme.TXT", 100),
                file("big.iso", 5000),
                dir("OldStuff", vec![file("x.bin", 300), dir("empty", vec![])]),
                file("clash", 7),
                dir("Moved", vec![file("m1.bin", 50), file("m2.bin", 50)]),
            ]),
        );
        let right = Tree::from_scan(
            "R:\\",
            dir("R", vec![
                dir("Same", vec![file("a.txt", 10), file("b.txt", 20)]),
                dir("photos", vec![file("ONE.jpg", 1000), file("two.jpg", 2500), file("new.jpg", 800)]),
                file("readme.txt", 100),
                file("big.iso", 4000),
                dir("NewStuff", vec![file("y.bin", 40)]),
                dir("clash", vec![file("inner.dat", 9)]),
                dir("Moved", vec![file("m1.bin", 60), file("m2.bin", 40)]),
            ]),
        );
        Diff::new(&left, &right)
    }

    #[test]
    fn matches_by_name_ignoring_case_and_sizes_the_differences() {
        let d = sample();
        let t = &d.tree;
        let root = d.entry(t.root());
        assert_eq!((root.left, root.right), (9037, 8579));
        assert_eq!(root.status, Status::LargerLeft);

        // Identical folders are kept with size 0 and no changes.
        let same = find(&d, "Same");
        assert_eq!((t.node(same).size, d.entry(same).changes, d.entry(same).status), (0, 0, Status::Same));

        // Case differences still match; the left name is kept.
        let photos = find(&d, "Photos");
        let e = d.entry(photos);
        assert_eq!((e.left, e.right, e.status), (3500, 4300, Status::LargerRight));
        assert_eq!(d.entry(find(&d, "Photos\\one.jpg")).status, Status::Same);
        let two = find(&d, "Photos\\two.jpg");
        assert_eq!((t.node(two).size, d.entry(two).status), (500, Status::LargerRight));
        assert_eq!(d.entry(find(&d, "Photos\\gone.jpg")).status, Status::OnlyLeft);
        assert_eq!(d.entry(find(&d, "Photos\\new.jpg")).status, Status::OnlyRight);
        // Folder size = bytes that differ inside: 500 + 500 + 800.
        assert_eq!(t.node(photos).size, 1800);
        assert_eq!(e.changes, 3);
        assert_eq!(d.entry(find(&d, "Readme.TXT")).status, Status::Same);

        let big = find(&d, "big.iso");
        assert_eq!((t.node(big).size, d.entry(big).status), (1000, Status::LargerLeft));

        // One-sided folders are copied whole, empty folders included.
        let old = find(&d, "OldStuff");
        assert_eq!((t.node(old).size, d.entry(old).status, d.entry(old).changes), (300, Status::OnlyLeft, 2));
        assert_eq!(d.entry(find(&d, "OldStuff\\empty")).status, Status::OnlyLeft);
        assert_eq!(d.entry(find(&d, "NewStuff\\y.bin")).status, Status::OnlyRight);

        // A file and a folder with the same name are two different items.
        let clashes: Vec<Status> =
            t.node(t.root()).children.iter().filter(|&&c| &*t.node(c).name == "clash").map(|&c| d.entry(c).status).collect();
        assert_eq!(clashes.len(), 2);
        assert!(clashes.contains(&Status::OnlyLeft) && clashes.contains(&Status::OnlyRight));

        // Same total, different contents.
        let moved = find(&d, "Moved");
        assert_eq!((d.entry(moved).status, t.node(moved).size, d.entry(moved).changes), (Status::Changed, 20, 2));

        // Children sort by bytes that differ, so the biggest difference comes first.
        let first = t.node(t.root()).children[0];
        assert_eq!(&*t.node(first).name, "Photos");
        assert_eq!(t.node(t.root()).size, 1800 + 1000 + 300 + 40 + 7 + 9 + 20);
    }

    #[test]
    fn totals_count_items_and_bytes_per_status() {
        let d = sample();
        let total = |s: Status| d.totals[s.index()];
        // gone.jpg, x.bin, empty, clash (file)
        assert_eq!(total(Status::OnlyLeft), Total { items: 4, bytes: 500 + 300 + 7 });
        // new.jpg, y.bin, clash\inner.dat
        assert_eq!(total(Status::OnlyRight), Total { items: 3, bytes: 800 + 40 + 9 });
        assert_eq!(total(Status::LargerLeft), Total { items: 2, bytes: 1000 + 10 });
        assert_eq!(total(Status::LargerRight), Total { items: 2, bytes: 500 + 10 });
        assert_eq!(total(Status::Same), Total { items: 4, bytes: 10 + 20 + 1000 + 100 });
        assert_eq!(total(Status::Changed), Total::default());
        assert_eq!(d.changes(), 11);
        // Net difference = what right gains minus what it lacks.
        let net = |s: Status| total(s).bytes as i64;
        assert_eq!(
            net(Status::OnlyRight) + net(Status::LargerRight) - net(Status::OnlyLeft) - net(Status::LargerLeft),
            d.right.size as i64 - d.left.size as i64
        );
    }

    #[test]
    fn paths_on_each_side() {
        let d = sample();
        let two = find(&d, "Photos\\two.jpg");
        assert_eq!(d.path(two, Side::Left).unwrap(), Path::new("L:\\Photos\\two.jpg"));
        assert_eq!(d.path(two, Side::Right).unwrap(), Path::new("R:\\Photos\\two.jpg"));
        let gone = find(&d, "Photos\\gone.jpg");
        assert!(d.path(gone, Side::Right).is_none());
        assert_eq!(d.display_path(two), "Photos\\two.jpg");
        assert_eq!(d.display_path(d.tree.root()), "L:\\  ↔  R:\\");
        assert_eq!(d.left.root, Path::new("L:\\"));
        assert_eq!(d.right.files, 11);
    }

    #[test]
    fn the_treemap_color_of_a_folder_is_its_largest_difference() {
        let d = sample();
        assert_eq!(d.color_status(find(&d, "Photos")), Status::OnlyRight); // new.jpg, 800
        assert_eq!(d.color_status(find(&d, "big.iso")), Status::LargerLeft);
    }

    #[test]
    fn identical_trees_have_no_differences() {
        let make = || Tree::from_scan("X:\\", dir("X", vec![file("a", 1), dir("d", vec![file("b", 2)])]));
        let d = Diff::new(&make(), &make());
        assert_eq!(d.changes(), 0);
        assert_eq!(d.tree.node(d.tree.root()).size, 0);
        assert_eq!(d.entry(d.tree.root()).status, Status::Same);
        assert_eq!(d.totals[Status::Same.index()], Total { items: 2, bytes: 3 });
    }

    #[test]
    fn deltas_are_signed() {
        assert_eq!(format_delta(100, 2148), "+2.00 KiB");
        assert_eq!(format_delta(1024, 0), "-1.00 KiB");
        assert_eq!(format_delta(5, 5), "0 B");
    }

    #[test]
    fn scans_two_folders_and_reports_a_missing_one() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(a.path().join("same.txt"), b"hello").unwrap();
        std::fs::write(b.path().join("same.txt"), b"HELLO").unwrap();
        std::fs::write(b.path().join("extra.bin"), vec![0u8; 1000]).unwrap();
        let p = [Progress::new(), Progress::new()];
        let d = compare([a.path(), b.path()], Method::Walk, [&p[0], &p[1]]).unwrap();
        assert_eq!(d.changes(), 1);
        assert_eq!(d.totals[Status::OnlyRight.index()], Total { items: 1, bytes: 1000 });
        assert_eq!(d.totals[Status::Same.index()].items, 1);

        let missing = a.path().join("missing");
        let err = compare([missing.as_path(), b.path()], Method::Walk, [&p[0], &p[1]]).unwrap_err();
        assert!(!err.is::<Cancelled>());
        assert!(format!("{err:#}").contains("missing"), "{err:#}");
    }
}
