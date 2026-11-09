# Changelog

All notable changes to rust-dirstat are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-11-09

### Changed

- The command and executable are now `rust-dirstat` (`rust-dirstat.exe`), matching the app's
  name. For example, `dirstat scan C:\` is now `rust-dirstat scan C:\`.
- Upgrading with the installer removes the old `dirstat.exe`. The Start menu shortcut and
  the PATH entry carry over.

## [0.1.0] - 2026-11-08

First release.

### Added

- Window with a directory tree (size, % of parent, file and folder counts), a file-type list
  and a cushion-shaded treemap colored by file type.
- Fast scans of whole NTFS drives by reading the Master File Table directly when run as
  administrator. Otherwise a parallel folder walk is used, and the reason is shown.
- Scan several drives at once: select them on the welcome screen or in the Drives menu, then
  click Scan selected drives (or Scan all local drives). They appear side by side in one view.
- Treemap: hover for details, click to select, double-click to zoom in, and click a file type
  to highlight its tiles.
- Right-click actions: Open, Reveal in Explorer, Copy path, Rescan folder, Move to Recycle Bin.
- Export to CSV or JSON.
- Command-line mode (`dirstat scan`) that prints a report and can export CSV or JSON.
- Installer (`rust-dirstat-vX.Y.Z-windows-x64-setup.exe`): per-user install without admin
  rights, Start menu shortcut, optional PATH entry, in-place upgrades and an uninstaller. A
  portable zip is also published.
- Update check: the window checks GitHub for a newer release at startup and links to it from
  the status bar. `dirstat check-update` does the same from a terminal. Set
  `DIRSTAT_NO_UPDATE_CHECK=1` to turn it off.

[Unreleased]: https://github.com/joreeves/rust-dirstat/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/joreeves/rust-dirstat/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/joreeves/rust-dirstat/releases/tag/v0.1.0
