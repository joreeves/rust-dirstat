//! CSV and JSON export of a scanned tree, and CSV export of a comparison.

use std::io::Write;
use std::path::MAIN_SEPARATOR;

use serde::Serialize;
use serde::ser::{SerializeMap, SerializeSeq, Serializer};

use crate::diff::{Diff, Side};
use crate::model::{NodeId, Tree, flags};

fn kind(tree: &Tree, id: NodeId) -> &'static str {
    let n = tree.node(id);
    if n.is_link() {
        "link"
    } else if n.is_dir() {
        "dir"
    } else {
        "file"
    }
}

fn extension(tree: &Tree, id: NodeId) -> &str {
    let n = tree.node(id);
    if n.is_file() { tree.exts().get(n.ext as usize).map_or("", |e| &e.name) } else { "" }
}

fn join(parent: &str, name: &str) -> String {
    if parent.ends_with(['\\', '/']) { format!("{parent}{name}") } else { format!("{parent}{MAIN_SEPARATOR}{name}") }
}

/// Writes one CSV row per node under `from` (pre-order, largest first), down to `max_depth`.
/// Synthetic grouping nodes do not add a path component; `<Orphaned>` does.
pub fn write_csv<W: Write>(tree: &Tree, from: NodeId, max_depth: Option<usize>, out: W) -> anyhow::Result<()> {
    let mut w = csv::Writer::from_writer(out);
    w.write_record(["path", "type", "size_bytes", "files", "folders", "extension", "depth"])?;
    let base_path = tree.path_of(from).map(|p| p.display().to_string());
    // A multi-drive tree's virtual root has no path; its children ("C:\", "D:\") are absolute.
    let virtual_root = base_path.is_none() && from == tree.root();
    let base = base_path.unwrap_or_else(|| tree.node(from).name.to_string());
    let mut paths: Vec<String> = Vec::new();
    for (id, depth) in tree.preorder(from, max_depth) {
        let n = tree.node(id);
        paths.truncate(depth);
        let path = match paths.last() {
            None => base.clone(),
            Some(_) if virtual_root && depth == 1 => n.name.to_string(),
            Some(parent) if n.flags & flags::GROUP != 0 => parent.clone(),
            Some(parent) => join(parent, &n.name),
        };
        let (files, folders) = if n.is_dir() { (n.files.to_string(), n.dirs.to_string()) } else { (String::new(), String::new()) };
        w.write_record([
            path.as_str(),
            kind(tree, id),
            &n.size.to_string(),
            &files,
            &folders,
            extension(tree, id),
            &depth.to_string(),
        ])?;
        paths.push(path);
    }
    w.flush()?;
    Ok(())
}

/// Writes one CSV row per item of a comparison that differs, in pre-order with the biggest
/// difference first, after a row for the two roots (path `.`). Paths are relative to the roots;
/// sizes are empty for the side an item is missing from.
pub fn write_diff_csv<W: Write>(diff: &Diff, out: W) -> anyhow::Result<()> {
    let mut w = csv::Writer::from_writer(out);
    w.write_record(["path", "type", "status", "left_bytes", "right_bytes", "difference_bytes", "depth"])?;
    let tree = &diff.tree;
    let mut stack = vec![(tree.root(), 0usize)];
    while let Some((id, depth)) = stack.pop() {
        let e = diff.entry(id);
        let path = if id == tree.root() { ".".to_owned() } else { diff.display_path(id) };
        let size = |side: Side| if e.on(side) { e.size(side).to_string() } else { String::new() };
        let difference = (i128::from(e.right) - i128::from(e.left)).to_string();
        w.write_record([
            path.as_str(),
            kind(tree, id),
            e.status.slug(),
            &size(Side::Left),
            &size(Side::Right),
            &difference,
            &depth.to_string(),
        ])?;
        let children = &tree.node(id).children;
        stack.extend(children.iter().rev().filter(|&&c| diff.entry(c).changes > 0).map(|&c| (c, depth + 1)));
    }
    w.flush()?;
    Ok(())
}

/// Writes the tree under `from` as nested JSON, down to `max_depth`.
pub fn write_json<W: Write>(tree: &Tree, from: NodeId, max_depth: Option<usize>, out: W) -> anyhow::Result<()> {
    let doc = JsonDoc { tree, from, max_depth };
    let mut ser = serde_json::Serializer::new(out);
    doc.serialize(&mut ser)?;
    Ok(())
}

struct JsonDoc<'a> {
    tree: &'a Tree,
    from: NodeId,
    max_depth: Option<usize>,
}

impl Serialize for JsonDoc<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let info = &self.tree.info;
        let mut m = s.serialize_map(None)?;
        let root = self.tree.path_of(self.from).map(|p| p.display().to_string());
        m.serialize_entry("root", &root)?;
        m.serialize_entry("method", info.method)?;
        m.serialize_entry("note", &info.note)?;
        m.serialize_entry("elapsed_ms", &(info.elapsed.as_millis() as u64))?;
        m.serialize_entry("tree", &JsonNode { tree: self.tree, id: self.from, depth_left: self.max_depth })?;
        m.end()
    }
}

struct JsonNode<'a> {
    tree: &'a Tree,
    id: NodeId,
    depth_left: Option<usize>,
}

impl Serialize for JsonNode<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let n = self.tree.node(self.id);
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("name", &*n.name)?;
        m.serialize_entry("type", kind(self.tree, self.id))?;
        m.serialize_entry("size", &n.size)?;
        if n.is_dir() {
            m.serialize_entry("files", &n.files)?;
            m.serialize_entry("folders", &n.dirs)?;
            if n.is_unreadable() {
                m.serialize_entry("unreadable", &true)?;
            }
            if !n.children.is_empty() && self.depth_left != Some(0) {
                m.serialize_entry("children", &JsonChildren { tree: self.tree, id: self.id, depth_left: self.depth_left.map(|d| d - 1) })?;
            }
        } else if n.is_file() {
            m.serialize_entry("extension", extension(self.tree, self.id))?;
        }
        m.end()
    }
}

struct JsonChildren<'a> {
    tree: &'a Tree,
    id: NodeId,
    depth_left: Option<usize>,
}

impl Serialize for JsonChildren<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let children = &self.tree.node(self.id).children;
        let mut seq = s.serialize_seq(Some(children.len()))?;
        for &c in children {
            seq.serialize_element(&JsonNode { tree: self.tree, id: c, depth_left: self.depth_left })?;
        }
        seq.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ScanNode;

    fn sample() -> Tree {
        let file = |n: &str, s: u64| ScanNode { name: n.into(), size: s, flags: 0, children: vec![] };
        Tree::from_scan(
            "C:\\data",
            ScanNode {
                name: "data".into(),
                size: 0,
                flags: flags::DIR,
                children: vec![
                    file("b, with comma.TXT", 7),
                    ScanNode { name: "sub".into(), size: 0, flags: flags::DIR, children: vec![file("a.bin", 20)] },
                ],
            },
        )
    }

    #[test]
    fn csv_rows() {
        let t = sample();
        let mut out = Vec::new();
        write_csv(&t, t.root(), None, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let sep = MAIN_SEPARATOR;
        let expected = format!(
            "path,type,size_bytes,files,folders,extension,depth\n\
             C:\\data,dir,27,2,1,,0\n\
             C:\\data{sep}sub,dir,20,1,0,,1\n\
             C:\\data{sep}sub{sep}a.bin,file,20,,,bin,2\n\
             \"C:\\data{sep}b, with comma.TXT\",file,7,,,txt,1\n"
        );
        assert_eq!(text, expected);
        let mut shallow = Vec::new();
        write_csv(&t, t.root(), Some(1), &mut shallow).unwrap();
        assert_eq!(String::from_utf8(shallow).unwrap().lines().count(), 4);
    }

    #[test]
    fn csv_paths_for_multi_drive_tree() {
        let file = |n: &str, s: u64| ScanNode { name: n.into(), size: s, flags: 0, children: vec![] };
        let c = Tree::from_scan("C:\\", ScanNode { name: "C".into(), size: 0, flags: flags::DIR, children: vec![file("a.txt", 5)] });
        let d = Tree::from_scan("D:\\", ScanNode { name: "D".into(), size: 0, flags: flags::DIR, children: vec![file("b.bin", 9)] });
        let t = Tree::combine(vec![c, d]);
        let mut out = Vec::new();
        write_csv(&t, t.root(), None, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let paths: Vec<&str> = text.lines().skip(1).map(|l| l.split(',').next().unwrap()).collect();
        assert_eq!(paths, ["C:\\ + D:\\", "D:\\", "D:\\b.bin", "C:\\", "C:\\a.txt"]);
    }

    #[test]
    fn diff_csv_lists_only_differences() {
        let file = |n: &str, s: u64| ScanNode { name: n.into(), size: s, flags: 0, children: vec![] };
        let dir = |n: &str, children| ScanNode { name: n.into(), size: 0, flags: flags::DIR, children };
        let left = Tree::from_scan("L:\\", dir("L", vec![file("same.txt", 5), dir("sub", vec![file("a.bin", 20)]), file("old.log", 3)]));
        let right = Tree::from_scan("R:\\", dir("R", vec![file("same.txt", 5), dir("sub", vec![file("a.bin", 32)])]));
        let d = Diff::new(&left, &right);
        let mut out = Vec::new();
        write_diff_csv(&d, &mut out).unwrap();
        let sep = MAIN_SEPARATOR;
        let expected = format!(
            "path,type,status,left_bytes,right_bytes,difference_bytes,depth\n\
             .,dir,larger_right,28,37,9,0\n\
             sub,dir,larger_right,20,32,12,1\n\
             sub{sep}a.bin,file,larger_right,20,32,12,2\n\
             old.log,file,only_left,3,,-3,1\n"
        );
        assert_eq!(String::from_utf8(out).unwrap(), expected);
    }

    #[test]
    fn json_structure() {
        let t = sample();
        let mut out = Vec::new();
        write_json(&t, t.root(), None, &mut out).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["root"], "C:\\data");
        assert_eq!(v["tree"]["size"], 27);
        assert_eq!(v["tree"]["children"][0]["name"], "sub");
        assert_eq!(v["tree"]["children"][0]["children"][0]["extension"], "bin");
        let mut out = Vec::new();
        write_json(&t, t.root(), Some(1), &mut out).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert!(v["tree"]["children"][0].get("children").is_none());
    }
}
