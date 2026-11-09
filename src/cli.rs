//! Headless text report for `dirstat scan`.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::io::{self, Write};

use crate::model::{NodeId, Tree};
use crate::util::{format_count, format_size, percent};

#[derive(Clone, Debug)]
pub struct ReportOptions {
    /// Children listed per directory.
    pub top: usize,
    /// Directory levels to expand.
    pub depth: usize,
    /// Extensions listed.
    pub extensions: usize,
    /// Largest files listed.
    pub files: usize,
}

pub fn print_report(tree: &Tree, opts: &ReportOptions, out: &mut impl Write) -> io::Result<()> {
    let root = tree.node(tree.root());
    let info = &tree.info;
    let title = tree.path_of(tree.root()).map(|p| p.display().to_string()).unwrap_or_else(|| root.name.to_string());
    writeln!(
        out,
        "{}  {} in {} files, {} folders",
        title,
        format_size(root.size),
        format_count(root.files),
        format_count(root.dirs)
    )?;
    write!(out, "Scanned with {} in {:.2} s", info.method, info.elapsed.as_secs_f64())?;
    if let Some(note) = &info.note {
        write!(out, " ({note})")?;
    }
    writeln!(out)?;
    if info.unreadable > 0 {
        writeln!(out, "Warning: {} folders could not be read", format_count(info.unreadable))?;
    }

    if opts.depth > 0 && opts.top > 0 {
        writeln!(out, "\nLargest entries:")?;
        writeln!(out, "{:>11} {:>7} {:>11}  Name", "Size", "%", "Files")?;
        print_children(tree, tree.root(), 0, opts, out)?;
    }

    if opts.extensions > 0 {
        writeln!(out, "\nFile types:")?;
        writeln!(out, "{:>11} {:>7} {:>11}  Extension", "Size", "%", "Files")?;
        for e in tree.exts().iter().filter(|e| e.files > 0).take(opts.extensions) {
            writeln!(
                out,
                "{:>11} {:>6.1}% {:>11}  {}",
                format_size(e.bytes),
                percent(e.bytes, root.size),
                format_count(e.files),
                e.display_name()
            )?;
        }
    }

    if opts.files > 0 {
        writeln!(out, "\nLargest files:")?;
        for id in largest_files(tree, opts.files) {
            let n = tree.node(id);
            let path = tree.path_of(id).map(|p| p.display().to_string()).unwrap_or_else(|| n.name.to_string());
            writeln!(out, "{:>11}  {}", format_size(n.size), path)?;
        }
    }
    Ok(())
}

fn print_children(tree: &Tree, dir: NodeId, level: usize, opts: &ReportOptions, out: &mut impl Write) -> io::Result<()> {
    let parent = tree.node(dir);
    let children = &parent.children;
    for &c in children.iter().take(opts.top) {
        let n = tree.node(c);
        let name = if n.is_dir() { format!("{}\\", n.name) } else { n.name.to_string() };
        let files = if n.is_dir() { format_count(n.files) } else { String::new() };
        writeln!(
            out,
            "{:>11} {:>6.1}% {:>11}  {}{}",
            format_size(n.size),
            percent(n.size, parent.size),
            files,
            "  ".repeat(level),
            name
        )?;
        if n.is_dir() && level + 1 < opts.depth {
            print_children(tree, c, level + 1, opts, out)?;
        }
    }
    if children.len() > opts.top {
        let rest: u64 = children[opts.top..].iter().map(|&c| tree.node(c).size).sum();
        writeln!(
            out,
            "{:>11} {:>6.1}% {:>11}  {}… {} more",
            format_size(rest),
            percent(rest, parent.size),
            "",
            "  ".repeat(level),
            format_count((children.len() - opts.top) as u64)
        )?;
    }
    Ok(())
}

/// The `n` largest files in the tree, largest first.
pub fn largest_files(tree: &Tree, n: usize) -> Vec<NodeId> {
    let mut heap: BinaryHeap<Reverse<(u64, NodeId)>> = BinaryHeap::with_capacity(n + 1);
    for (id, _) in tree.preorder(tree.root(), None) {
        let node = tree.node(id);
        if !node.is_file() {
            continue;
        }
        heap.push(Reverse((node.size, id)));
        if heap.len() > n {
            heap.pop();
        }
    }
    let mut v: Vec<_> = heap.into_iter().map(|Reverse((s, id))| (s, id)).collect();
    v.sort_by_key(|&(size, _)| Reverse(size));
    v.into_iter().map(|(_, id)| id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ScanNode, flags};

    #[test]
    fn report_mentions_largest_items() {
        let file = |n: &str, s: u64| ScanNode { name: n.into(), size: s, flags: 0, children: vec![] };
        let t = Tree::from_scan(
            "C:\\x",
            ScanNode {
                name: "x".into(),
                size: 0,
                flags: flags::DIR,
                children: vec![
                    file("a.iso", 5000),
                    ScanNode { name: "d".into(), size: 0, flags: flags::DIR, children: vec![file("b.mp4", 3000), file("c.txt", 1)] },
                    file("e.txt", 2),
                ],
            },
        );
        let opts = ReportOptions { top: 2, depth: 2, extensions: 5, files: 2 };
        let mut out = Vec::new();
        print_report(&t, &opts, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("a.iso"));
        assert!(text.contains("  b.mp4"));
        assert!(text.contains("… 1 more"));
        assert!(text.contains(".iso"));
        let ids = largest_files(&t, 2);
        assert_eq!(ids.iter().map(|&i| &*t.node(i).name).collect::<Vec<_>>(), ["a.iso", "b.mp4"]);
    }
}
