//! The scanned directory tree, stored as an arena of nodes indexed by [`NodeId`].
//!
//! Both scanners produce a [`Tree`]: the walker by flattening its owned [`ScanNode`] tree, the
//! MFT scanner by calling [`Tree::add_child`] directly. After building, [`Tree::finalize`] sums
//! sizes bottom-up, sorts children largest-first and builds the per-extension statistics.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Index of a node in a [`Tree`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

impl NodeId {
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// `Node::ext` value for directories and other nodes that have no extension slot.
pub const NO_EXT: u32 = u32::MAX;

/// Bit flags stored in [`Node::flags`].
pub mod flags {
    /// A directory (has children, or could have).
    pub const DIR: u8 = 1;
    /// A directory whose contents could not be listed.
    pub const UNREADABLE: u8 = 2;
    /// A symlink, junction or mount point. Never followed; shown as a zero-size leaf.
    pub const LINK: u8 = 4;
    /// A synthetic grouping node (e.g. `<NTFS metadata>`) that is skipped when building paths.
    pub const GROUP: u8 = 8;
    /// A synthetic node whose descendants have no reliable path (e.g. `<Orphaned>`).
    pub const NO_PATH: u8 = 16;
}

#[derive(Clone, Debug)]
pub struct Node {
    pub name: Box<str>,
    pub parent: Option<NodeId>,
    /// Children, sorted largest first after [`Tree::finalize`].
    pub children: Vec<NodeId>,
    /// Logical size in bytes (for directories: the sum over the subtree).
    pub size: u64,
    /// Number of files in the subtree (1 for a file, 0 for a link).
    pub files: u64,
    /// Number of directories in the subtree, excluding this node.
    pub dirs: u64,
    /// Index into [`Tree::exts`], or [`NO_EXT`].
    pub ext: u32,
    pub flags: u8,
    /// Set when the node was deleted from the tree (it stays in the arena but is unreachable).
    pub removed: bool,
}

impl Node {
    #[inline]
    pub fn is_dir(&self) -> bool {
        self.flags & flags::DIR != 0
    }
    #[inline]
    pub fn is_link(&self) -> bool {
        self.flags & flags::LINK != 0
    }
    #[inline]
    pub fn is_unreadable(&self) -> bool {
        self.flags & flags::UNREADABLE != 0
    }
    #[inline]
    pub fn is_synthetic(&self) -> bool {
        self.flags & (flags::GROUP | flags::NO_PATH) != 0
    }
    /// A regular file (not a directory and not a link).
    #[inline]
    pub fn is_file(&self) -> bool {
        self.flags & (flags::DIR | flags::LINK) == 0
    }
}

/// Totals for one file extension.
#[derive(Clone, Debug, Default)]
pub struct ExtStat {
    /// Lowercase extension without the dot; empty for files without an extension.
    pub name: Box<str>,
    pub bytes: u64,
    pub files: u64,
}

impl ExtStat {
    pub fn display_name(&self) -> String {
        if self.name.is_empty() { "(no extension)".to_owned() } else { format!(".{}", self.name) }
    }
}

/// How a tree was produced.
#[derive(Clone, Debug, Default)]
pub struct ScanInfo {
    /// "MFT" or "walk".
    pub method: &'static str,
    /// Why the preferred method was not used, if a fallback happened.
    pub note: Option<String>,
    pub elapsed: Duration,
    /// Directories that could not be listed.
    pub unreadable: u64,
    /// MFT records skipped because they were malformed or torn.
    pub bad_records: u64,
}

/// An owned tree as produced by the directory walker before it is flattened into a [`Tree`].
#[derive(Debug, Default, Clone)]
pub struct ScanNode {
    pub name: String,
    pub size: u64,
    pub flags: u8,
    pub children: Vec<ScanNode>,
}

#[derive(Clone, Debug)]
pub struct Tree {
    nodes: Vec<Node>,
    root: NodeId,
    root_path: PathBuf,
    exts: Vec<ExtStat>,
    ext_map: HashMap<Box<str>, u32>,
    pub info: ScanInfo,
}

impl Tree {
    /// Creates a tree containing only a root directory for `root_path`.
    pub fn new(root_path: impl Into<PathBuf>) -> Self {
        let root_path = root_path.into();
        let root = Node {
            name: root_path.display().to_string().into_boxed_str(),
            parent: None,
            children: Vec::new(),
            size: 0,
            files: 0,
            dirs: 0,
            ext: NO_EXT,
            flags: flags::DIR,
            removed: false,
        };
        Self {
            nodes: vec![root],
            root: NodeId(0),
            root_path,
            exts: Vec::new(),
            ext_map: HashMap::new(),
            info: ScanInfo::default(),
        }
    }

    /// Builds and finalizes a tree from a walker result whose root corresponds to `root_path`.
    pub fn from_scan(root_path: impl Into<PathBuf>, scan: ScanNode) -> Self {
        let mut tree = Self::new(root_path);
        let root = tree.root;
        tree.nodes[root.index()].flags |= scan.flags & flags::UNREADABLE;
        for child in scan.children {
            tree.add_scan_tree(root, child);
        }
        tree.finalize();
        tree
    }

    #[inline]
    pub fn root(&self) -> NodeId {
        self.root
    }

    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    #[inline]
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.index()]
    }

    /// Number of nodes in the arena (including removed and unreachable ones).
    pub fn arena_len(&self) -> usize {
        self.nodes.len()
    }

    /// Extension statistics, sorted by total bytes (largest first) after [`Tree::finalize`].
    pub fn exts(&self) -> &[ExtStat] {
        &self.exts
    }

    /// Appends a child under `parent`. Sizes of ancestors are not updated until
    /// [`Tree::finalize`] (or the `remove`/`graft` helpers) run.
    pub fn add_child(&mut self, parent: NodeId, name: &str, size: u64, node_flags: u8) -> NodeId {
        let id = NodeId(u32::try_from(self.nodes.len()).expect("more than 4 billion nodes"));
        let ext = if node_flags & flags::DIR != 0 { NO_EXT } else { self.intern_ext(name) };
        self.nodes.push(Node {
            name: name.into(),
            parent: Some(parent),
            children: Vec::new(),
            size: if node_flags & flags::DIR != 0 { 0 } else { size },
            files: 0,
            dirs: 0,
            ext,
            flags: node_flags,
            removed: false,
        });
        self.nodes[parent.index()].children.push(id);
        id
    }

    /// Adds an owned walker subtree under `parent` and returns the id of its top node.
    pub fn add_scan_tree(&mut self, parent: NodeId, node: ScanNode) -> NodeId {
        let ScanNode { name, size, flags: f, children } = node;
        let top = self.add_child(parent, &name, size, f);
        let mut stack = vec![(top, children)];
        while let Some((pid, children)) = stack.pop() {
            for child in children {
                let ScanNode { name, size, flags: f, children } = child;
                let cid = self.add_child(pid, &name, size, f);
                if !children.is_empty() {
                    stack.push((cid, children));
                }
            }
        }
        top
    }

    fn intern_ext(&mut self, name: &str) -> u32 {
        let ext = extension_of(name);
        if let Some(&idx) = self.ext_map.get(ext.as_str()) {
            return idx;
        }
        let idx = self.exts.len() as u32;
        self.exts.push(ExtStat { name: ext.clone().into_boxed_str(), bytes: 0, files: 0 });
        self.ext_map.insert(ext.into_boxed_str(), idx);
        idx
    }

    /// Sums sizes and counts bottom-up, sorts children and rebuilds extension statistics.
    pub fn finalize(&mut self) {
        self.recompute_subtree(self.root);
        self.rebuild_ext_stats();
    }

    /// Recomputes sizes/counts and child order for the subtree rooted at `id`.
    fn recompute_subtree(&mut self, id: NodeId) {
        let order = self.preorder_ids(id);
        for &nid in order.iter().rev() {
            let i = nid.index();
            if !self.nodes[i].is_dir() {
                let n = &mut self.nodes[i];
                n.files = u64::from(n.is_file());
                n.dirs = 0;
                continue;
            }
            let mut children = std::mem::take(&mut self.nodes[i].children);
            let (mut size, mut files, mut dirs) = (0u64, 0u64, 0u64);
            for &c in &children {
                let cn = &self.nodes[c.index()];
                size += cn.size;
                files += cn.files;
                dirs += cn.dirs + u64::from(cn.is_dir());
            }
            self.sort_children(&mut children);
            let n = &mut self.nodes[i];
            n.children = children;
            n.size = size;
            n.files = files;
            n.dirs = dirs;
        }
    }

    fn sort_children(&self, children: &mut [NodeId]) {
        children.sort_unstable_by(|&a, &b| {
            let (na, nb) = (&self.nodes[a.index()], &self.nodes[b.index()]);
            nb.size.cmp(&na.size).then_with(|| na.name.cmp(&nb.name))
        });
    }

    fn resort_children_of(&mut self, id: NodeId) {
        let mut children = std::mem::take(&mut self.nodes[id.index()].children);
        self.sort_children(&mut children);
        self.nodes[id.index()].children = children;
    }

    /// All reachable node ids under `id` (inclusive), parents before children.
    fn preorder_ids(&self, id: NodeId) -> Vec<NodeId> {
        let mut order = Vec::new();
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            order.push(n);
            stack.extend(self.nodes[n.index()].children.iter().copied());
        }
        order
    }

    /// Recounts bytes/files per extension over the reachable tree and re-sorts the table so
    /// index 0 is the extension with the most bytes. Node `ext` indices are remapped.
    pub fn rebuild_ext_stats(&mut self) {
        for e in &mut self.exts {
            e.bytes = 0;
            e.files = 0;
        }
        let mut stack = vec![self.root];
        while let Some(id) = stack.pop() {
            let n = &self.nodes[id.index()];
            if n.is_file() && n.ext != NO_EXT {
                let e = &mut self.exts[n.ext as usize];
                e.bytes += n.size;
                e.files += 1;
            }
            stack.extend(n.children.iter().copied());
        }
        let mut order: Vec<u32> = (0..self.exts.len() as u32).collect();
        order.sort_by(|&a, &b| {
            let (ea, eb) = (&self.exts[a as usize], &self.exts[b as usize]);
            eb.bytes.cmp(&ea.bytes).then_with(|| eb.files.cmp(&ea.files)).then_with(|| ea.name.cmp(&eb.name))
        });
        if order.iter().enumerate().all(|(i, &o)| i as u32 == o) {
            return;
        }
        let mut remap = vec![0u32; order.len()];
        for (new, &old) in order.iter().enumerate() {
            remap[old as usize] = new as u32;
        }
        let old_exts = std::mem::take(&mut self.exts);
        let mut slots: Vec<Option<ExtStat>> = old_exts.into_iter().map(Some).collect();
        self.exts = order.iter().map(|&old| slots[old as usize].take().unwrap_or_default()).collect();
        for n in &mut self.nodes {
            if n.ext != NO_EXT {
                n.ext = remap[n.ext as usize];
            }
        }
        for v in self.ext_map.values_mut() {
            *v = remap[*v as usize];
        }
    }

    /// Ancestors of `id` from the root down to and including `id`.
    pub fn ancestry(&self, id: NodeId) -> Vec<NodeId> {
        let mut chain = vec![id];
        let mut cur = id;
        while let Some(p) = self.nodes[cur.index()].parent {
            chain.push(p);
            cur = p;
        }
        chain.reverse();
        chain
    }

    /// Depth below the root (root = 0).
    pub fn depth_of(&self, id: NodeId) -> usize {
        self.ancestry(id).len() - 1
    }

    /// `true` if `id` is `ancestor` or lies below it.
    pub fn is_descendant_of(&self, id: NodeId, ancestor: NodeId) -> bool {
        let mut cur = Some(id);
        while let Some(c) = cur {
            if c == ancestor {
                return true;
            }
            cur = self.nodes[c.index()].parent;
        }
        false
    }

    /// `true` if `id` is still attached to the tree (not removed and not replaced by a rescan).
    pub fn is_reachable(&self, id: NodeId) -> bool {
        if id.index() >= self.nodes.len() {
            return false;
        }
        let mut cur = id;
        loop {
            let n = &self.nodes[cur.index()];
            if n.removed {
                return false;
            }
            match n.parent {
                None => return cur == self.root,
                Some(p) => {
                    if !self.nodes[p.index()].children.contains(&cur) {
                        return false;
                    }
                    cur = p;
                }
            }
        }
    }

    /// The filesystem path of a node, or `None` for nodes inside a synthetic `NO_PATH` group.
    pub fn path_of(&self, id: NodeId) -> Option<PathBuf> {
        let chain = self.ancestry(id);
        let mut path = self.root_path.clone();
        for &n in chain.iter().skip(1) {
            let node = &self.nodes[n.index()];
            if node.flags & flags::NO_PATH != 0 {
                return None;
            }
            if node.flags & flags::GROUP != 0 {
                continue;
            }
            path.push(&*node.name);
        }
        // The virtual root of a multi-drive tree has no path of its own.
        (!path.as_os_str().is_empty()).then_some(path)
    }

    /// Combines separately scanned trees (e.g. several drives) under one virtual root. Each
    /// part becomes a child named after its root path, so paths below it stay absolute.
    pub fn combine(parts: Vec<Tree>) -> Tree {
        let name = parts.iter().map(|t| t.root_path.display().to_string()).collect::<Vec<_>>().join(" + ");
        let mut tree = Tree::new(PathBuf::new());
        let root = tree.root;
        tree.nodes[root.index()].name = name.into_boxed_str();
        tree.nodes[root.index()].flags |= flags::GROUP;
        let methods: Vec<&'static str> = parts.iter().map(|t| t.info.method).collect();
        for part in &parts {
            tree.append(root, part);
            tree.info.elapsed += part.info.elapsed;
            tree.info.unreadable += part.info.unreadable;
            tree.info.bad_records += part.info.bad_records;
        }
        let notes: Vec<String> = parts
            .iter()
            .filter_map(|t| t.info.note.as_ref().map(|n| format!("{}: {n}", t.root_path.display())))
            .collect();
        tree.info.note = (!notes.is_empty()).then(|| notes.join("; "));
        tree.info.method = match methods.first() {
            Some(&first) if methods.iter().all(|&m| m == first) => first,
            _ => "MFT + walk",
        };
        tree.finalize();
        tree
    }

    /// Copies the reachable tree of `other` (including its root) under `parent`.
    fn append(&mut self, parent: NodeId, other: &Tree) {
        let mut stack = vec![(other.root, parent)];
        while let Some((src, dst_parent)) = stack.pop() {
            let n = other.node(src);
            let id = self.add_child(dst_parent, &n.name, n.size, n.flags);
            stack.extend(n.children.iter().map(|&c| (c, id)));
        }
    }

    /// Extension index of the largest file reached by following the largest child downwards;
    /// used to color directories that are too small to subdivide in the treemap.
    pub fn dominant_ext(&self, id: NodeId) -> u32 {
        let mut cur = id;
        loop {
            let n = &self.nodes[cur.index()];
            if !n.is_dir() {
                return n.ext;
            }
            match n.children.first() {
                Some(&c) => cur = c,
                None => return NO_EXT,
            }
        }
    }

    /// Detaches `id` (and its subtree) from the tree, updating ancestor totals.
    /// Returns `false` for the root or an already removed node.
    pub fn remove(&mut self, id: NodeId) -> bool {
        if id == self.root || self.nodes[id.index()].removed {
            return false;
        }
        let Some(parent) = self.nodes[id.index()].parent else { return false };
        let n = &self.nodes[id.index()];
        let (size, files, dirs) = (n.size, n.files, n.dirs + u64::from(n.is_dir()));
        self.nodes[id.index()].removed = true;
        self.nodes[parent.index()].children.retain(|&c| c != id);
        let mut cur = Some(parent);
        while let Some(a) = cur {
            let an = &mut self.nodes[a.index()];
            an.size -= size;
            an.files -= files;
            an.dirs -= dirs;
            cur = an.parent;
            self.resort_children_of(a);
        }
        self.rebuild_ext_stats();
        true
    }

    /// Replaces the contents of directory `id` with a freshly scanned subtree (from the walker),
    /// updating ancestor totals. The old children stay in the arena but become unreachable.
    pub fn graft(&mut self, id: NodeId, scan: ScanNode) {
        let old = &self.nodes[id.index()];
        let (old_size, old_files, old_dirs) = (old.size, old.files, old.dirs);
        {
            let n = &mut self.nodes[id.index()];
            n.children.clear();
            n.flags = (n.flags & !flags::UNREADABLE) | (scan.flags & flags::UNREADABLE);
        }
        for child in scan.children {
            self.add_scan_tree(id, child);
        }
        self.recompute_subtree(id);
        let new = &self.nodes[id.index()];
        let (new_size, new_files, new_dirs) = (new.size, new.files, new.dirs);
        let mut cur = self.nodes[id.index()].parent;
        while let Some(a) = cur {
            let an = &mut self.nodes[a.index()];
            an.size = an.size - old_size + new_size;
            an.files = an.files - old_files + new_files;
            an.dirs = an.dirs - old_dirs + new_dirs;
            cur = an.parent;
            self.resort_children_of(a);
        }
        self.rebuild_ext_stats();
    }

    /// Pre-order traversal from `from` (children visited largest first), yielding
    /// `(id, depth relative to from)`. Descends at most `max_depth` levels below `from`.
    pub fn preorder(&self, from: NodeId, max_depth: Option<usize>) -> PreOrder<'_> {
        PreOrder { tree: self, stack: vec![(from, 0)], max_depth }
    }
}

/// Iterator returned by [`Tree::preorder`].
pub struct PreOrder<'a> {
    tree: &'a Tree,
    stack: Vec<(NodeId, usize)>,
    max_depth: Option<usize>,
}

impl Iterator for PreOrder<'_> {
    type Item = (NodeId, usize);

    fn next(&mut self) -> Option<Self::Item> {
        let (id, depth) = self.stack.pop()?;
        if self.max_depth.is_none_or(|m| depth < m) {
            let children = &self.tree.node(id).children;
            self.stack.extend(children.iter().rev().map(|&c| (c, depth + 1)));
        }
        Some((id, depth))
    }
}

/// Lowercase extension of a file name: the text after the last dot, if the dot is not the
/// first character (so `.gitignore` has no extension, matching `Path::extension`).
pub fn extension_of(name: &str) -> String {
    match name.rfind('.') {
        Some(pos) if pos > 0 && pos + 1 < name.len() => name[pos + 1..].to_lowercase(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, size: u64) -> ScanNode {
        ScanNode { name: name.into(), size, flags: 0, children: vec![] }
    }
    fn dir(name: &str, children: Vec<ScanNode>) -> ScanNode {
        ScanNode { name: name.into(), size: 0, flags: flags::DIR, children }
    }

    fn sample() -> Tree {
        Tree::from_scan(
            "R:\\root",
            dir(
                "root",
                vec![
                    file("a.txt", 10),
                    dir("sub", vec![file("b.TXT", 100), file("c.bin", 5), dir("empty", vec![])]),
                    file("noext", 1),
                    ScanNode { name: "link".into(), size: 0, flags: flags::LINK, children: vec![] },
                ],
            ),
        )
    }

    fn find(tree: &Tree, name: &str) -> NodeId {
        tree.preorder(tree.root(), None).map(|(id, _)| id).find(|&id| &*tree.node(id).name == name).unwrap()
    }

    #[test]
    fn finalize_sums_and_sorts() {
        let t = sample();
        let root = t.node(t.root());
        assert_eq!(root.size, 116);
        assert_eq!(root.files, 4);
        assert_eq!(root.dirs, 2);
        let names: Vec<_> = root.children.iter().map(|&c| &*t.node(c).name).collect();
        assert_eq!(names, ["sub", "a.txt", "noext", "link"]);
        let sub = t.node(find(&t, "sub"));
        assert_eq!((sub.size, sub.files, sub.dirs), (105, 2, 1));
    }

    #[test]
    fn ext_stats_sorted_and_case_insensitive() {
        let t = sample();
        let exts = t.exts();
        assert_eq!(&*exts[0].name, "txt");
        assert_eq!((exts[0].bytes, exts[0].files), (110, 2));
        assert_eq!(&*exts[1].name, "bin");
        assert_eq!(&*exts[2].name, "");
        let b = t.node(find(&t, "b.TXT"));
        assert_eq!(b.ext, 0);
    }

    #[test]
    fn remove_updates_ancestors_and_exts() {
        let mut t = sample();
        let b = find(&t, "b.TXT");
        assert!(t.remove(b));
        assert!(!t.remove(b));
        let root = t.node(t.root());
        assert_eq!((root.size, root.files), (16, 3));
        let sub = t.node(find(&t, "sub"));
        assert_eq!((sub.size, sub.files), (5, 1));
        // "a.txt" (10) is now larger than "sub" (5).
        assert_eq!(&*t.node(t.node(t.root()).children[0]).name, "a.txt");
        assert_eq!(&*t.exts()[0].name, "txt");
        assert_eq!(t.exts()[0].bytes, 10);
        assert!(!t.is_reachable(b));
        let sub_id = find(&t, "sub");
        let c = find(&t, "c.bin");
        assert!(t.is_reachable(c));
        assert!(t.remove(sub_id));
        assert!(!t.is_reachable(c), "descendants of a removed node are unreachable");
        assert_eq!(t.node(t.root()).dirs, 0);
        assert!(!t.remove(t.root()));
    }

    #[test]
    fn graft_replaces_subtree() {
        let mut t = sample();
        let sub = find(&t, "sub");
        let old_child = find(&t, "b.TXT");
        t.graft(sub, dir("sub", vec![file("new.dat", 1000), dir("d", vec![file("x.txt", 1)])]));
        assert!(!t.is_reachable(old_child));
        assert!(t.is_reachable(sub));
        let root = t.node(t.root());
        assert_eq!(root.size, 10 + 1 + 1001);
        assert_eq!(root.files, 2 + 2);
        assert_eq!(root.dirs, 2);
        assert_eq!(&*t.exts()[0].name, "dat");
        assert!(t.preorder(t.root(), None).all(|(id, _)| &*t.node(id).name != "b.TXT"));
    }

    #[test]
    fn paths_skip_groups_and_stop_at_no_path() {
        let mut t = Tree::new("C:\\");
        let root = t.root();
        let meta = t.add_child(root, "<NTFS metadata>", 0, flags::DIR | flags::GROUP);
        let mft = t.add_child(meta, "$MFT", 5, 0);
        let orphans = t.add_child(root, "<Orphaned>", 0, flags::DIR | flags::NO_PATH);
        let lost = t.add_child(orphans, "lost.txt", 1, 0);
        t.finalize();
        assert_eq!(t.path_of(mft).unwrap(), PathBuf::from("C:\\$MFT"));
        assert_eq!(t.path_of(lost), None);
        assert_eq!(t.path_of(root).unwrap(), PathBuf::from("C:\\"));
    }

    #[test]
    fn combine_puts_drives_under_a_virtual_root() {
        let mut c = Tree::from_scan("C:\\", dir("C", vec![file("pagefile.sys", 100), dir("Users", vec![file("a.txt", 5)])]));
        c.info.method = "MFT";
        c.info.unreadable = 1;
        let mut d = Tree::from_scan("D:\\", dir("D", vec![file("movie.mkv", 1000)]));
        d.info.method = "walk";
        d.info.note = Some("MFT unavailable".into());
        let t = Tree::combine(vec![c, d]);
        let root = t.node(t.root());
        assert_eq!(&*root.name, "C:\\ + D:\\");
        assert!(root.is_synthetic());
        assert_eq!((root.size, root.files, root.dirs), (1105, 3, 3));
        assert_eq!(t.path_of(t.root()), None);
        let names: Vec<_> = root.children.iter().map(|&c| &*t.node(c).name).collect();
        assert_eq!(names, ["D:\\", "C:\\"]);
        assert_eq!(t.path_of(find(&t, "a.txt")).unwrap(), PathBuf::from("C:\\Users\\a.txt"));
        assert_eq!(t.path_of(root.children[0]).unwrap(), PathBuf::from("D:\\"));
        assert_eq!(t.info.method, "MFT + walk");
        assert_eq!(t.info.unreadable, 1);
        assert_eq!(t.info.note.as_deref(), Some("D:\\: MFT unavailable"));
        assert_eq!(&*t.exts()[0].name, "mkv");
    }

    #[test]
    fn preorder_depth_limit() {
        let t = sample();
        let shallow: Vec<_> = t.preorder(t.root(), Some(1)).collect();
        assert_eq!(shallow.len(), 5);
        assert_eq!(t.preorder(t.root(), None).count(), 8);
    }

    #[test]
    fn extensions() {
        assert_eq!(extension_of("a.TXT"), "txt");
        assert_eq!(extension_of("archive.tar.gz"), "gz");
        assert_eq!(extension_of(".gitignore"), "");
        assert_eq!(extension_of("noext"), "");
        assert_eq!(extension_of("trailing."), "");
    }
}
