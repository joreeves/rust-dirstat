# Changelog

All notable changes to rust-dirstat are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Compare two drives or folders, for example a disk and its backup. Click **↔ Compare…**,
  choose the two locations, and rust-dirstat scans both at once. It then shows:
  - what is only on one side and what differs in size, with the sizes on each side;
  - totals for each kind of difference;
  - a treemap of just the bytes that differ, colored by how they differ.

  Items are matched by path, and files are compared by size. Right-click an item to reveal it
  or copy its path on either side. You can export the differences to CSV. From a terminal,
  `rust-dirstat diff LEFT RIGHT` prints the same comparison as a report.

## [0.4.0] - 2026-11-10

### Added

- Updates install themselves. When a newer release is out, a copy installed with setup.exe
  shows **Update to vX.Y.Z** in the status bar. Clicking it downloads the installer, closes
  the app, upgrades it (keeping your options) and opens it again. `rust-dirstat update` does
  the same from a terminal. Portable copies still link to the release page.

## [0.3.0] - 2026-11-09

### Added

- An app icon: treemap blocks in rust, copper and amber. It shows on the window and taskbar,
  on `rust-dirstat.exe` in File Explorer, on the Start menu shortcut and on the installer.
- Version details on `rust-dirstat.exe` (shown in its Properties and in Task Manager).

### Fixed

- The hover highlight no longer trails the mouse pointer. The window now shows each frame as
  soon as it is drawn, instead of waiting for the display's next refresh. This matters most
  over Remote Desktop, where the display refreshes only about 30 times a second.
- Mouse-wheel scrolling moves the lists a full step at once, instead of easing in over several
  frames while the highlight lags behind.
- The treemap no longer lumps a folder's smallest files into one tile that looked like a single
  file but selected the folder. Every file now has its own tile, however small, so pointing at
  one shows and selects that file.

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

[Unreleased]: https://github.com/joreeves/rust-dirstat/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/joreeves/rust-dirstat/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/joreeves/rust-dirstat/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/joreeves/rust-dirstat/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/joreeves/rust-dirstat/releases/tag/v0.1.0
