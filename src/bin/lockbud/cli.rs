use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "lockbud", about = "Statically detect bugs in Rust programs")]
pub struct LockbudCli {
    /// Detector kind to run (deadlock, atomicity_violation, memory, panic, channel).
    /// If omitted, all detectors run.
    #[arg(short = 'k', long = "kind", value_name = "KIND")]
    pub kind: Option<String>,

    /// Comma-separated crate names for whitelist/blacklist.
    #[arg(short = 'l')]
    pub crate_list: Option<String>,

    /// Treat -l as a blacklist instead of whitelist.
    #[arg(short = 'b')]
    pub blacklist: bool,

    /// Output path for JSON reports.
    #[arg(long = "report-file")]
    pub report_file: Option<PathBuf>,

    /// Destination path for the intermediate .ullbc file.
    #[arg(long = "dest-file")]
    pub dest_file: Option<PathBuf>,

    /// Additional args passed to cargo (e.g., `--release`, `--target`).
    #[arg(last = true)]
    pub cargo_args: Vec<String>,
}
