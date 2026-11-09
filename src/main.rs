use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use dirstat::cli::{ReportOptions, print_report};
use dirstat::scan::{self, Cancelled, Method, Progress};
use dirstat::util::{format_count, format_size};
use dirstat::{export, gui, platform};

/// Disk usage analyzer with a cushion treemap (WinDirStat-style). Run without arguments to open
/// the window.
#[derive(Parser)]
#[command(name = "dirstat", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Open the graphical interface (the default), optionally scanning PATHS right away
    /// (several drives or folders are combined into one view).
    Gui {
        paths: Vec<PathBuf>,
        #[arg(long, value_enum, default_value_t = Method::Auto)]
        method: Method,
    },
    /// Scan a drive or folder and print a report (and optionally export CSV/JSON).
    Scan(ScanArgs),
}

#[derive(Args)]
struct ScanArgs {
    /// Drives or folders to scan, e.g. C:\ or D:\Projects. Several are combined into one report.
    #[arg(required = true, num_args = 1..)]
    paths: Vec<PathBuf>,
    /// auto = MFT when elevated on NTFS, else walk.
    #[arg(long, value_enum, default_value_t = Method::Auto)]
    method: Method,
    /// Entries listed per folder.
    #[arg(long, default_value_t = 20)]
    top: usize,
    /// Folder levels to list.
    #[arg(long, default_value_t = 1)]
    depth: usize,
    /// File types listed.
    #[arg(long, default_value_t = 15)]
    extensions: usize,
    /// Largest files listed.
    #[arg(long, default_value_t = 10)]
    files: usize,
    /// Write every file and folder to a CSV file.
    #[arg(long, value_name = "FILE")]
    csv: Option<PathBuf>,
    /// Limit CSV rows to this many levels below the root.
    #[arg(long, value_name = "N")]
    csv_depth: Option<usize>,
    /// Write the tree as nested JSON.
    #[arg(long, value_name = "FILE")]
    json: Option<PathBuf>,
    /// Limit JSON nesting to this many levels below the root.
    #[arg(long, value_name = "N")]
    json_depth: Option<usize>,
    /// Print scan statistics (throughput, MFT records, skipped records).
    #[arg(long)]
    stats: bool,
    /// Do not show live progress on stderr.
    #[arg(long, short)]
    quiet: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        None => run_gui(Vec::new(), Method::Auto),
        Some(Command::Gui { paths, method }) => run_gui(paths, method),
        Some(Command::Scan(args)) => match run_scan(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) if e.is::<Cancelled>() => ExitCode::from(130),
            Err(e) => {
                eprintln!("error: {e:#}");
                ExitCode::FAILURE
            }
        },
    }
}

fn run_gui(paths: Vec<PathBuf>, method: Method) -> ExitCode {
    platform::detach_console_if_owned();
    match gui::run(paths, method) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("dirstat")
                .set_description(format!("{e:#}"))
                .show();
            ExitCode::FAILURE
        }
    }
}

fn run_scan(args: ScanArgs) -> anyhow::Result<()> {
    let progress = Arc::new(Progress::new());
    let show_progress = !args.quiet && std::io::stderr().is_terminal();
    let paths = args.paths.clone();
    let method = args.method;
    let worker = {
        let progress = progress.clone();
        std::thread::spawn(move || scan::scan_many(&paths, method, &progress))
    };
    while !worker.is_finished() {
        std::thread::sleep(Duration::from_millis(200));
        if show_progress {
            let total = progress.records_total.load(Ordering::Relaxed);
            let records = (progress.records_done.load(Ordering::Relaxed) * 100)
                .checked_div(total)
                .map(|pct| format!("  MFT {pct}%"))
                .unwrap_or_default();
            eprint!(
                "\r\x1b[2K{} files, {} folders, {}{}",
                format_count(progress.files.load(Ordering::Relaxed)),
                format_count(progress.dirs.load(Ordering::Relaxed)),
                format_size(progress.bytes.load(Ordering::Relaxed)),
                records
            );
        }
    }
    if show_progress {
        eprint!("\r\x1b[2K");
    }
    let tree = worker.join().map_err(|_| anyhow::anyhow!("scan thread panicked"))??;

    let mut out = std::io::stdout().lock();
    let opts = ReportOptions { top: args.top, depth: args.depth, extensions: args.extensions, files: args.files };
    print_report(&tree, &opts, &mut out)?;
    if args.stats {
        let secs = tree.info.elapsed.as_secs_f64().max(1e-9);
        let root = tree.node(tree.root());
        let items = root.files + root.dirs;
        writeln!(out, "\nStatistics:")?;
        writeln!(out, "  method        {}", tree.info.method)?;
        writeln!(out, "  elapsed       {:.3} s", secs)?;
        writeln!(out, "  items         {} ({:.0}/s)", format_count(items), items as f64 / secs)?;
        let records = progress.records_total.load(Ordering::Relaxed);
        if records > 0 {
            writeln!(out, "  MFT records   {}", format_count(records))?;
            writeln!(out, "  bad records   {}", format_count(tree.info.bad_records))?;
        }
        writeln!(out, "  unreadable    {}", format_count(tree.info.unreadable))?;
        writeln!(out, "  tree nodes    {}", format_count(tree.arena_len() as u64))?;
    }
    if let Some(csv) = &args.csv {
        let file = std::io::BufWriter::new(std::fs::File::create(csv)?);
        export::write_csv(&tree, tree.root(), args.csv_depth, file)?;
        writeln!(out, "\nWrote {}", csv.display())?;
    }
    if let Some(json) = &args.json {
        let file = std::io::BufWriter::new(std::fs::File::create(json)?);
        export::write_json(&tree, tree.root(), args.json_depth, file)?;
        writeln!(out, "Wrote {}", json.display())?;
    }
    Ok(())
}
