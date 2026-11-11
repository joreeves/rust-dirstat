//! Headless text reports for `rust-dirstat scan` and `rust-dirstat diff`.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::io::{self, Write};

use crate::diff::{Diff, Side, Status, format_delta};
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

#[derive(Clone, Debug)]
pub struct DiffReportOptions {
    /// Differences listed per folder.
    pub top: usize,
    /// Folder levels to expand.
    pub depth: usize,
    /// Largest file differences listed.
    pub files: usize,
}

/// Prints both locations, the totals per kind of difference, the folders with the most bytes
/// that differ and the largest file differences.
pub fn print_diff_report(diff: &Diff, opts: &DiffReportOptions, out: &mut impl Write) -> io::Result<()> {
    for (label, loc) in [("Left: ", &diff.left), ("Right:", &diff.right)] {
        write!(
            out,
            "{label} {}  {} in {} files, {} folders ({}",
            loc.root.display(),
            format_size(loc.size),
            format_count(loc.files),
            format_count(loc.dirs),
            loc.info.method
        )?;
        if let Some(note) = &loc.info.note {
            write!(out, "; {note}")?;
        }
        writeln!(out, ")")?;
        if loc.info.unreadable > 0 {
            writeln!(out, "Warning: {} folders could not be read", format_count(loc.info.unreadable))?;
        }
    }
    let (l, r) = (diff.left.size, diff.right.size);
    let net = match r.cmp(&l) {
        std::cmp::Ordering::Greater => format!("Right is {} larger", format_size(r - l)),
        std::cmp::Ordering::Less => format!("Right is {} smaller", format_size(l - r)),
        std::cmp::Ordering::Equal => "Both have the same total size".to_owned(),
    };
    writeln!(out, "{net}. Compared in {:.2} s.", diff.elapsed.as_secs_f64())?;
    if diff.changes() == 0 {
        writeln!(out, "\nNo differences: the same items with the same sizes on both sides.")?;
        return Ok(());
    }

    writeln!(out, "\n{:<16} {:>11} {:>11}", "", "Items", "Size")?;
    for status in [Status::OnlyLeft, Status::OnlyRight, Status::LargerLeft, Status::LargerRight, Status::Same] {
        let t = diff.totals[status.index()];
        writeln!(out, "{:<16} {:>11} {:>11}", status.label(), format_count(t.items), format_size(t.bytes))?;
    }

    writeln!(out, "\nDifferences, most bytes first:")?;
    writeln!(out, "{:>11} {:>11} {:>12}  Name", "Left", "Right", "Difference")?;
    print_diff_children(diff, diff.tree.root(), 0, opts, out)?;

    if opts.files > 0 {
        writeln!(out, "\nLargest file differences:")?;
        for id in largest_differences(diff, opts.files) {
            let e = diff.entry(id);
            writeln!(out, "{:>12}  {:<15}  {}", format_delta(e.left, e.right), e.status.label(), diff.display_path(id))?;
        }
    }
    Ok(())
}

fn print_diff_children(diff: &Diff, dir: NodeId, level: usize, opts: &DiffReportOptions, out: &mut impl Write) -> io::Result<()> {
    let tree = &diff.tree;
    let children: Vec<NodeId> = tree.node(dir).children.iter().copied().filter(|&c| diff.entry(c).changes > 0).collect();
    for &c in children.iter().take(opts.top) {
        let n = tree.node(c);
        let e = diff.entry(c);
        let name = if n.is_dir() { format!("{}\\", n.name) } else { n.name.to_string() };
        let size = |side: Side| if e.on(side) { format_size(e.size(side)) } else { "-".to_owned() };
        let what = match e.status {
            Status::LargerLeft | Status::LargerRight | Status::Changed if n.is_dir() => match e.changes {
                1 => "1 difference".to_owned(),
                n => format!("{} differences", format_count(u64::from(n))),
            },
            status => status.label().to_lowercase(),
        };
        writeln!(
            out,
            "{:>11} {:>11} {:>12}  {}{}  ({what})",
            size(Side::Left),
            size(Side::Right),
            format_delta(e.left, e.right),
            "  ".repeat(level),
            name
        )?;
        if n.is_dir() && level + 1 < opts.depth {
            print_diff_children(diff, c, level + 1, opts, out)?;
        }
    }
    if children.len() > opts.top {
        writeln!(out, "{:>11} {:>11} {:>12}  {}… {} more", "", "", "", "  ".repeat(level), format_count((children.len() - opts.top) as u64))?;
    }
    Ok(())
}

/// The `n` files (and links) with the most bytes that differ, largest first.
pub fn largest_differences(diff: &Diff, n: usize) -> Vec<NodeId> {
    let tree = &diff.tree;
    let mut heap: BinaryHeap<Reverse<(u64, NodeId)>> = BinaryHeap::with_capacity(n + 1);
    for (id, _) in tree.preorder(tree.root(), None) {
        let node = tree.node(id);
        if node.is_dir() || diff.entry(id).status == Status::Same {
            continue;
        }
        heap.push(Reverse((node.size, id)));
        if heap.len() > n {
            heap.pop();
        }
    }
    let mut v: Vec<_> = heap.into_iter().map(|Reverse((s, id))| (s, id)).collect();
    v.sort_by_key(|&(size, id)| (Reverse(size), id));
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

    #[test]
    fn diff_report_lists_totals_and_differences() {
        let file = |n: &str, s: u64| ScanNode { name: n.into(), size: s, flags: 0, children: vec![] };
        let dir = |n: &str, children| ScanNode { name: n.into(), size: 0, flags: flags::DIR, children };
        let left = Tree::from_scan(
            "L:\\",
            dir("L", vec![file("same.txt", 5), dir("Photos", vec![file("a.jpg", 2000), file("b.jpg", 100)]), file("old.iso", 9000)]),
        );
        let right = Tree::from_scan(
            "R:\\",
            dir("R", vec![file("same.txt", 5), dir("photos", vec![file("a.jpg", 2000), file("b.jpg", 600), file("c.jpg", 50)])]),
        );
        let d = Diff::new(&left, &right);
        let opts = DiffReportOptions { top: 1, depth: 2, files: 2 };
        let mut out = Vec::new();
        print_diff_report(&d, &opts, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Right is 8.25 KiB smaller"), "{text}");
        assert!(text.contains(&format!("{:<16} {:>11} {:>11}", "Only in left", 1, "8.79 KiB")), "{text}");
        // Only the biggest difference per folder (top 1), then a count of the rest.
        assert!(text.contains("  old.iso  (only in left)"), "{text}");
        assert!(text.contains("… 1 more"), "{text}");
        assert!(!text.contains("same.txt"), "identical items are left out: {text}");
        let lines: Vec<&str> = text.lines().skip_while(|l| !l.starts_with("Largest file")).collect();
        assert!(lines[1].contains("-8.79 KiB") && lines[1].contains("old.iso"), "{text}");
        assert!(lines[2].contains("+500 B") && lines[2].contains("Photos\\b.jpg"), "{text}");

        let same = Diff::new(&left, &left);
        let mut out = Vec::new();
        print_diff_report(&same, &opts, &mut out).unwrap();
        assert!(String::from_utf8(out).unwrap().contains("No differences"));
    }
}
