//! Merges parsed chunks into a record index (sizes, flags, parent -> children links) and builds
//! the final [`Tree`] from it.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use super::chunk::ChunkOut;
use super::record::ROOT_RECORD;
use crate::model::{NodeId, Tree, flags};
use crate::scan::Progress;

const F_IN_USE: u8 = 1;
const F_DIR: u8 = 2;
const F_SURROGATE: u8 = 4;

/// Records below this number are NTFS metafiles ($MFT, $LogFile, $Bitmap, $Extend, ...).
const FIRST_USER_RECORD: u32 = 16;
/// `$BadClus`: its `$Bad` stream is sparse with a real size equal to the volume size.
const BADCLUS_RECORD: u32 = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Link {
    pub child: u32,
    pub parent: u32,
    pub name_start: u32,
    pub name_len: u32,
}

#[derive(Debug, Default)]
pub struct MftIndex {
    seq: Vec<u16>,
    flags: Vec<u8>,
    size: Vec<u64>,
    stream_alloc: HashMap<u32, u64>,
    /// Valid links grouped by parent: children of `p` are `links[child_start[p]..child_start[p+1]]`.
    links: Vec<Link>,
    child_start: Vec<u32>,
    /// Links whose parent is missing, stale or not a directory.
    orphans: Vec<Link>,
    names: Vec<u16>,
    pub bad_records: u64,
}

impl MftIndex {
    pub fn record_count(&self) -> usize {
        self.flags.len()
    }

    pub fn seq(&self, record: u32) -> u16 {
        self.seq.get(record as usize).copied().unwrap_or(0)
    }

    pub fn children(&self, record: u32) -> &[Link] {
        let r = record as usize;
        match (self.child_start.get(r), self.child_start.get(r + 1)) {
            (Some(&a), Some(&b)) => &self.links[a as usize..b as usize],
            _ => &[],
        }
    }

    pub fn name(&self, link: &Link) -> &[u16] {
        let s = link.name_start as usize;
        &self.names[s..s + link.name_len as usize]
    }

    fn is_traversable_dir(&self, record: u32) -> bool {
        self.flags.get(record as usize).is_some_and(|&f| f & F_DIR != 0 && f & F_SURROGATE == 0)
    }

    /// Resolves path components (UTF-16) from the root directory to a directory record.
    /// Tries an exact match first, then a case-insensitive one.
    pub fn resolve(&self, components: &[Vec<u16>]) -> Option<u32> {
        let mut cur = ROOT_RECORD as u32;
        for comp in components {
            let kids = self.children(cur);
            let found = kids
                .iter()
                .find(|l| self.is_traversable_dir(l.child) && self.name(l) == comp.as_slice())
                .or_else(|| kids.iter().find(|l| self.is_traversable_dir(l.child) && eq_ignore_case(self.name(l), comp)))?;
            cur = found.child;
        }
        Some(cur)
    }

    /// Adds the subtree below directory `root_record` to `tree` (under `tree.root()`).
    /// For whole-volume scans, metafiles are grouped under `<NTFS metadata>` and records with
    /// broken parent links under `<Orphaned>`.
    pub fn build_tree(
        &self,
        root_record: u32,
        tree: &mut Tree,
        whole_volume: bool,
        progress: &Progress,
    ) -> Result<(), super::MftError> {
        let mut visited = vec![false; self.record_count()];
        if let Some(v) = visited.get_mut(root_record as usize) {
            *v = true;
        }
        let root_node = tree.root();
        let meta_node =
            whole_volume.then(|| tree.add_child(root_node, "<NTFS metadata>", 0, flags::DIR | flags::GROUP));
        let mut stack: Vec<(u32, NodeId, bool)> = vec![(root_record, root_node, false)];
        let mut b = Builder { index: self, tree, visited: &mut visited, progress, added: 0 };

        while let Some((rec, node, in_meta)) = stack.pop() {
            for link in self.children(rec) {
                let to_meta = !in_meta && rec == ROOT_RECORD as u32 && link.child < FIRST_USER_RECORD;
                let (parent, meta) = match (to_meta, meta_node) {
                    (true, Some(m)) => (m, true),
                    _ => (node, in_meta),
                };
                if let Some(dir) = b.add(parent, link, meta)? {
                    stack.push((link.child, dir, meta));
                }
            }
        }
        if whole_volume && !self.orphans.is_empty() {
            let orphan_node = b.tree.add_child(root_node, "<Orphaned>", 0, flags::DIR | flags::NO_PATH);
            for link in &self.orphans {
                if let Some(dir) = b.add(orphan_node, link, false)? {
                    stack.push((link.child, dir, false));
                }
            }
            while let Some((rec, node, meta)) = stack.pop() {
                for link in self.children(rec) {
                    if let Some(dir) = b.add(node, link, meta)? {
                        stack.push((link.child, dir, meta));
                    }
                }
            }
        }
        Ok(())
    }
}

struct Builder<'a> {
    index: &'a MftIndex,
    tree: &'a mut Tree,
    visited: &'a mut [bool],
    progress: &'a Progress,
    added: u64,
}

impl Builder<'_> {
    /// Adds the node for `link` under `parent`. Returns the new node id if it is a directory
    /// whose children still need to be added.
    fn add(&mut self, parent: NodeId, link: &Link, in_meta: bool) -> Result<Option<NodeId>, super::MftError> {
        self.added += 1;
        if self.added.is_multiple_of(65_536) && self.progress.is_cancelled() {
            return Err(super::MftError::Cancelled);
        }
        let idx = self.index;
        let c = link.child as usize;
        let f = idx.flags.get(c).copied().unwrap_or(0);
        let name = String::from_utf16_lossy(idx.name(link));
        if f & F_SURROGATE != 0 {
            self.tree.add_child(parent, &name, 0, flags::LINK);
            return Ok(None);
        }
        if f & F_DIR != 0 {
            if std::mem::replace(&mut self.visited[c], true) {
                return Ok(None);
            }
            self.progress.dirs.fetch_add(1, Ordering::Relaxed);
            return Ok(Some(self.tree.add_child(parent, &name, 0, flags::DIR)));
        }
        let mut size = idx.size[c];
        if in_meta && link.child != BADCLUS_RECORD {
            size += idx.stream_alloc.get(&link.child).copied().unwrap_or(0);
        }
        self.progress.files.fetch_add(1, Ordering::Relaxed);
        self.progress.bytes.fetch_add(size, Ordering::Relaxed);
        self.tree.add_child(parent, &name, size, 0);
        Ok(None)
    }
}

/// Combines parsed chunks into an index over `record_count` records.
pub fn merge(chunks: Vec<ChunkOut>, record_count: u64) -> MftIndex {
    let n = record_count as usize;
    let mut idx = MftIndex {
        seq: vec![0; n],
        flags: vec![0; n],
        size: vec![0; n],
        ..Default::default()
    };
    for c in &chunks {
        idx.bad_records += c.bad;
        for b in &c.bases {
            let i = b.record as usize;
            if i >= n {
                continue;
            }
            idx.seq[i] = b.seq;
            idx.flags[i] = F_IN_USE | if b.is_dir { F_DIR } else { 0 } | if b.surrogate { F_SURROGATE } else { 0 };
            idx.size[i] = b.data_size.unwrap_or(0);
            if b.stream_alloc > 0 {
                *idx.stream_alloc.entry(b.record).or_default() += b.stream_alloc;
            }
        }
    }
    for c in &chunks {
        for p in &c.patches {
            let Ok(i) = usize::try_from(p.base.record()) else { continue };
            if i >= n || idx.flags[i] & F_IN_USE == 0 || idx.seq[i] != p.base.seq() {
                continue;
            }
            if let Some(size) = p.data_size {
                idx.size[i] = size;
            }
            if p.surrogate {
                idx.flags[i] |= F_SURROGATE;
            }
            if p.stream_alloc > 0 {
                *idx.stream_alloc.entry(i as u32).or_default() += p.stream_alloc;
            }
        }
    }

    let mut valid = Vec::new();
    for c in &chunks {
        for l in &c.links {
            let child = l.child as usize;
            if child >= n || idx.flags[child] & F_IN_USE == 0 {
                continue;
            }
            if l.child_seq.is_some_and(|s| idx.seq[child] != s) {
                continue;
            }
            let parent = l.parent.record();
            if parent == ROOT_RECORD && child as u64 == ROOT_RECORD {
                continue; // the root directory names itself "." in itself
            }
            let name = c.name(l);
            let link = Link {
                child: l.child,
                parent: u32::try_from(parent).unwrap_or(u32::MAX),
                name_start: idx.names.len() as u32,
                name_len: name.len() as u32,
            };
            idx.names.extend_from_slice(name);
            let p = parent as usize;
            let parent_ok = p < n
                && idx.flags[p] & F_IN_USE != 0
                && idx.flags[p] & F_DIR != 0
                && (l.parent.seq() == 0 || idx.seq[p] == l.parent.seq());
            if parent_ok { valid.push(link) } else { idx.orphans.push(link) }
        }
    }
    drop(chunks);

    // Counting sort of links by parent (CSR layout).
    let mut start = vec![0u32; n + 1];
    for l in &valid {
        start[l.parent as usize + 1] += 1;
    }
    for i in 0..n {
        start[i + 1] += start[i];
    }
    let mut fill = start.clone();
    let mut links = vec![Link::default(); valid.len()];
    for l in valid {
        let slot = &mut fill[l.parent as usize];
        links[*slot as usize] = l;
        *slot += 1;
    }
    idx.links = links;
    idx.child_start = start;
    idx
}

/// Case-insensitive comparison of UTF-16 names using simple per-unit uppercasing (NTFS compares
/// names through its `$UpCase` table, which maps each UTF-16 unit independently).
fn eq_ignore_case(a: &[u16], b: &[u16]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| x == y || upcase(x) == upcase(y))
}

fn upcase(u: u16) -> u16 {
    match char::from_u32(u32::from(u)) {
        Some(c) => {
            let mut up = c.to_uppercase();
            match (up.next(), up.next()) {
                (Some(single), None) if (single as u32) <= 0xFFFF => single as u16,
                _ => u,
            }
        }
        None => u, // surrogate half
    }
}

#[cfg(test)]
mod tests {
    use super::super::chunk::parse_chunk;
    use super::super::record::{FileRef, RECORD_IN_USE, RECORD_IS_DIRECTORY};
    use super::super::testutil::RecordBuilder;
    use super::*;

    const RS: usize = 1024;

    /// Builds a chunk from records described as (number, seq, flags, builder closure).
    fn chunk(count: u64, recs: Vec<RecordBuilder>) -> ChunkOut {
        let mut buf = vec![0u8; count as usize * RS];
        for rb in recs {
            let n = rb.record_number() as usize;
            buf[n * RS..(n + 1) * RS].copy_from_slice(&rb.build());
        }
        parse_chunk(&mut buf, 0, RS)
    }

    fn dir(n: u32, seq: u16, parent: u64, pseq: u16, name: &str) -> RecordBuilder {
        let mut rb = RecordBuilder::new(RS, n, seq);
        rb.flags(RECORD_IN_USE | RECORD_IS_DIRECTORY).file_name(parent, pseq, 1, name, 0x10, 0);
        rb
    }

    fn file(n: u32, seq: u16, parent: u64, pseq: u16, name: &str, size: usize) -> RecordBuilder {
        let mut rb = RecordBuilder::new(RS, n, seq);
        rb.flags(RECORD_IN_USE).file_name(parent, pseq, 1, name, 0, 0).resident_data("", &vec![0; size]);
        rb
    }

    fn sample() -> MftIndex {
        let root = {
            let mut rb = dir(5, 5, 5, 5, ".");
            rb.flags(RECORD_IN_USE | RECORD_IS_DIRECTORY);
            rb
        };
        let mft = {
            let mut rb = RecordBuilder::new(RS, 0, 1);
            rb.flags(RECORD_IN_USE).file_name(5, 5, 3, "$MFT", 0, 0).non_resident_data("", 0, 9, 40960, 40960, 40960, None, &[0x11, 10, 0x10, 0]);
            rb
        };
        let secure = {
            let mut rb = RecordBuilder::new(RS, 9, 9);
            rb.flags(RECORD_IN_USE).file_name(5, 5, 3, "$Secure", 0, 0).non_resident_data("$SDS", 0, 0, 4096, 300, 300, None, &[0x11, 1, 0x40, 0]);
            rb
        };
        let hardlinked = {
            let mut rb = file(22, 1, 20, 1, "a.txt", 10);
            rb.file_name(21, 1, 1, "a-link.txt", 0, 0);
            rb
        };
        let ext = {
            // Extension record of 23 holding a second hard link and the $DATA size.
            let mut rb = RecordBuilder::new(RS, 24, 1);
            rb.flags(RECORD_IN_USE).base(FileRef::new(23, 2)).file_name(5, 5, 1, "big.bin", 0, 0).non_resident_data(
                "",
                0,
                24,
                102400,
                100000,
                100000,
                None,
                &[0x11, 25, 0x50, 0],
            );
            rb
        };
        let base23 = {
            let mut rb = RecordBuilder::new(RS, 23, 2);
            rb.flags(RECORD_IN_USE).file_name(20, 1, 1, "big.bin", 0, 0);
            rb
        };
        let stale_ext = {
            let mut rb = RecordBuilder::new(RS, 25, 1);
            rb.flags(RECORD_IN_USE).base(FileRef::new(23, 1)).resident_data("", &[0; 7]);
            rb
        };
        let orphan = file(26, 1, 27, 1, "lost.txt", 3); // parent 27 is a file
        let parent_is_file = file(27, 1, 5, 5, "plain", 1);
        let stale_parent = file(28, 1, 20, 7, "stale.txt", 4); // 20 has seq 1
        let cycle_a = dir(29, 1, 30, 1, "cycA");
        let cycle_b = dir(30, 1, 29, 1, "cycB");
        let junction = {
            let mut rb = RecordBuilder::new(RS, 31, 1);
            rb.flags(RECORD_IN_USE | RECORD_IS_DIRECTORY).file_name(20, 1, 1, "junction", 0x410, 0xA000_0003);
            rb
        };
        let c = chunk(
            40,
            vec![
                root,
                mft,
                secure,
                dir(20, 1, 5, 5, "Users"),
                dir(21, 1, 20, 1, "Other"),
                hardlinked,
                base23,
                ext,
                stale_ext,
                orphan,
                parent_is_file,
                stale_parent,
                cycle_a,
                cycle_b,
                junction,
            ],
        );
        assert_eq!(c.bad, 0);
        merge(vec![c], 40)
    }

    fn tree_of(idx: &MftIndex, root: u32, whole: bool) -> Tree {
        let mut tree = Tree::new("X:\\");
        idx.build_tree(root, &mut tree, whole, &Progress::new()).unwrap();
        tree.finalize();
        tree
    }

    fn paths(tree: &Tree) -> Vec<(String, u64)> {
        let mut v: Vec<_> = tree
            .preorder(tree.root(), None)
            .skip(1)
            .map(|(id, _)| {
                let names: Vec<_> = tree.ancestry(id).iter().skip(1).map(|&a| tree.node(a).name.to_string()).collect();
                (names.join("/"), tree.node(id).size)
            })
            .collect();
        v.sort();
        v
    }

    #[test]
    fn whole_volume_tree() {
        let idx = sample();
        let t = tree_of(&idx, 5, true);
        let p = paths(&t);
        let expect = |path: &str, size: u64| {
            assert!(p.contains(&(path.to_string(), size)), "missing {path} ({size}) in {p:#?}");
        };
        expect("<NTFS metadata>/$MFT", 40960);
        expect("<NTFS metadata>/$Secure", 4096); // named $SDS stream counted on disk for metafiles
        expect("Users/a.txt", 10);
        expect("Users/Other/a-link.txt", 10); // hard links count once per link
        expect("Users/big.bin", 100000); // size from extension record, stale extension ignored
        expect("big.bin", 100000); // second link stored in the extension record
        expect("plain", 1);
        expect("Users/junction", 0);
        expect("<Orphaned>/lost.txt", 3);
        expect("<Orphaned>/stale.txt", 4);
        assert!(t.preorder(t.root(), None).any(|(id, _)| t.node(id).is_link()));
        // The cycle is unreachable from the root and has no orphan link, so it is skipped.
        assert!(!p.iter().any(|(path, _)| path.contains("cyc")));
        // "." self link of the root is dropped.
        assert!(!p.iter().any(|(path, _)| path == "."));
    }

    #[test]
    fn subdirectory_tree_and_resolution() {
        let idx = sample();
        let users: Vec<u16> = "users".encode_utf16().collect();
        let other: Vec<u16> = "Other".encode_utf16().collect();
        assert_eq!(idx.resolve(std::slice::from_ref(&users)), Some(20));
        assert_eq!(idx.resolve(&[users.clone(), other]), Some(21));
        assert_eq!(idx.resolve(&["nope".encode_utf16().collect()]), None);
        // Junctions and files are not traversable.
        assert_eq!(idx.resolve(&[users.clone(), "junction".encode_utf16().collect()]), None);
        assert_eq!(idx.resolve(&["plain".encode_utf16().collect()]), None);
        let t = tree_of(&idx, 20, false);
        assert_eq!(t.node(t.root()).size, 10 + 10 + 100000);
        assert!(!paths(&t).iter().any(|(p, _)| p.starts_with('<')));
    }

    #[test]
    fn exact_match_preferred_over_case_insensitive() {
        let c = chunk(
            30,
            vec![
                {
                    let mut rb = dir(5, 5, 5, 5, ".");
                    rb.flags(RECORD_IN_USE | RECORD_IS_DIRECTORY);
                    rb
                },
                dir(20, 1, 5, 5, "data"),
                dir(21, 1, 5, 5, "Data"),
            ],
        );
        let idx = merge(vec![c], 30);
        assert_eq!(idx.resolve(&["Data".encode_utf16().collect()]), Some(21));
        assert_eq!(idx.resolve(&["data".encode_utf16().collect()]), Some(20));
        assert_eq!(idx.resolve(&["DATA".encode_utf16().collect()]).map(|r| r == 20 || r == 21), Some(true));
    }

    #[test]
    fn upcase_rules() {
        assert!(eq_ignore_case(&"Straße".encode_utf16().collect::<Vec<_>>(), &"STRAßE".encode_utf16().collect::<Vec<_>>()));
        assert!(eq_ignore_case(&"ÄÖü".encode_utf16().collect::<Vec<_>>(), &"äöÜ".encode_utf16().collect::<Vec<_>>()));
        assert!(!eq_ignore_case(&"ab".encode_utf16().collect::<Vec<_>>(), &"abc".encode_utf16().collect::<Vec<_>>()));
    }
}
