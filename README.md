# dirstat

A WinDirStat-style disk usage analyzer written in Rust. It scans a drive or folder, shows a sortable directory tree and a file-type breakdown, and draws a cushion-shaded treemap where every file is a tile sized by its bytes and colored by its extension.

- **Fast NTFS scan.** When run as administrator on an NTFS drive, dirstat reads the Master File Table directly from the raw volume. It never walks directories, so a scan takes seconds even with millions of files. Otherwise it falls back to a parallel directory walk, which needs no special rights.
- **GUI** built with egui/eframe:
  - tree table with % of parent, size, file and folder counts
  - file-type list
  - treemap with hover, click-to-select, double-click to zoom
  - right-click menu: Open, Reveal in Explorer, Copy path, Rescan folder, Move to Recycle Bin
  - CSV/JSON export
- **CLI** for scripting: a text report plus CSV/JSON export.

## Build

```powershell
cargo build --release
# binary: target\release\dirstat.exe
```

Requires Rust 1.88+ (edition 2024). Developed on Windows; the walker, treemap, CLI and GUI also build on other platforms, but the MFT scanner and Explorer integration are Windows-only.

## Usage

```powershell
dirstat                                  # open the window
dirstat gui D:\Projects                  # open and scan right away
dirstat gui C:\ D:\                      # scan several drives/folders into one view
dirstat gui C:\ --method mft             # force the MFT scanner (no fallback)

dirstat scan D:\Projects                 # text report
dirstat scan C:\ D:\ --top 10            # one report covering both drives
dirstat scan C:\ --depth 2 --top 10 --files 20 --extensions 10 --stats
dirstat scan D:\ --csv d.csv --json d.json --json-depth 3
```

**Several drives at once.** In the window, tick drives on the welcome screen or in the **Drives** menu, then click **Scan selected drives**, or click **Scan all local drives**. The drives appear side by side under one virtual root (`C:\ + D:\`). The tree, file types and treemap all cover the combined total. If a drive can't be scanned, it is skipped and the status bar says why. **Rescan** scans the same set again.

`--method auto|mft|walk`:
- `auto` (the default) uses the MFT when it can and otherwise falls back to the walk. The reason for any fallback appears in the report and the status bar, for example `MFT unavailable: not elevated`.
- `mft` fails instead of falling back.

Keys in the tree:
- **↑/↓** move the selection.
- **→/←** expand or collapse; **←** on a file selects its parent.
- **Enter** opens a file or expands a folder.
- **Delete** asks before moving the item to the Recycle Bin.

## How sizes are counted

- **Logical size.** Sizes are each file's logical size (what Explorer calls *Size*, not *Size on disk*), the same as WinDirStat's size column.
- **Hard links** count once per link, so the MFT and walk scans give the same totals.
- **Symlinks, junctions and mount points** are listed as zero-size entries and never followed.
- **MFT whole-drive scans:**
  - NTFS metadata files (`$MFT`, `$LogFile`, `$Extend\…`) appear under `<NTFS metadata>`. Files whose parent folder is missing or stale appear under `<Orphaned>`.
  - The MFT is read as it is on disk, so changes made in the last few seconds may not be visible yet.

## Code map

| Path | What it does |
|---|---|
| `src/model.rs` | Arena tree (`Tree`, `NodeId`): bottom-up totals, largest-first order, per-extension stats, remove/graft for deletes and rescans |
| `src/scan/walk.rs` | Parallel `std::fs` walker (rayon) |
| `src/scan/mft/` | Raw MFT reader. `boot`, `fixup`, `runs`, `record`, `chunk` are pure parsers with no panics on bad data. `io` does the aligned volume reads and finds the `$MFT` (including when it's fragmented through `$ATTRIBUTE_LIST`). `index` joins records into the tree. |
| `src/treemap/` | Squarified layout and van Wijk cushion shading into an RGBA buffer |
| `src/gui/` | eframe app; the treemap is rendered on a background thread and uploaded as a texture |
| `src/export.rs`, `src/cli.rs` | CSV/JSON writers and the text report |

## Tests

```powershell
cargo test                        # unit, integration and headless GUI tests (egui_kittest)
cargo clippy --all-targets -- -D warnings
```

The MFT parser is tested without admin rights. The tests feed it hand-built FILE records and a small in-memory NTFS volume image that checks read alignment and covers a fragmented `$MFT`, an `$ATTRIBUTE_LIST`, extension records, junctions, and records past the initialized size. They also fuzz the parser.

Two further tests compare the MFT scanner with the walker on real data, so they need an **elevated** terminal:

```powershell
cargo test --release --test mft_admin -- --ignored --nocapture
```

- **Fixture comparison:** builds a set of test files (hard links, alternate streams, a compressed file, a junction, Unicode names, deep nesting), then requires both scans to match exactly.
- **System32 comparison:** scans `C:\Windows\System32` both ways and allows at most 0.1% of entries to differ.
