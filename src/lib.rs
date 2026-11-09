//! dirstat: a WinDirStat-style disk usage analyzer.
//!
//! The library holds everything that does not need a window: the scanners (a parallel directory
//! walker and a raw NTFS MFT reader), the tree model, treemap layout/rendering, and exporters.
//! The binary (`main.rs`) adds the CLI and the egui front end.

pub mod cli;
pub mod colors;
pub mod export;
pub mod gui;
pub mod model;
pub mod platform;
pub mod scan;
pub mod treemap;
pub mod update;
pub mod util;

/// Name shown to people: window title, welcome screen, installer and Start menu.
/// (The executable and CLI command stay `dirstat`.)
pub const APP_NAME: &str = "rust-dirstat";
