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
pub mod util;
