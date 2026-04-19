use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub const OBOL_ARGS: &str = "OBOL_ARGS";

#[derive(Debug, clap::Parser)]
#[clap(name = "Obol")]
pub enum ObolCli {
    /// Runs Obol on a single rust file (and the modules it references, if any).
    Rustc(CliOpts),
    /// Runs obol on a cargo project.
    Cargo(CliOpts),
    /// Print the path to the rustc toolchain used by obol.
    ToolchainPath,
    /// Print the version of Obol (or Charon, if flag provided)
    Version(VersionOpts),
}

#[derive(Debug, Default, Clone, clap::Parser, Serialize, Deserialize)]
#[serde(default)]
pub struct CliOpts {
    /// The destination file. By default `<dest_dir>/<crate_name>.ullbc`.
    #[clap(long = "dest-file", value_parser)]
    pub dest_file: Option<PathBuf>,
    #[clap(
        long = "print-ullbc",
        help = "Print the ULLBC after applying the micro-passes."
    )]
    pub print_ullbc: bool,
    #[clap(
        long = "start-from",
        help = "List of function names that count as entry points to generate; if none are specified, main is used."
    )]
    pub start_from: Vec<String>,
    #[clap(
        long = "start-from-attribute",
        help = "List of attributes (e.g. `kani::proof`) that count as entry points to generate; empty by default."
    )]
    pub start_from_attribute: Vec<String>,
    #[clap(
        long = "start-from-pub",
        help = "Whether to start translation from all public items."
    )]
    pub start_from_pub: bool,
    #[clap(
        long = "opaque",
        help = "List of item names to keep opaque during translation"
    )]
    pub opaque: Vec<String>,
    /// Args that are passed to the underlying tool (`rustc` or `cargo` depending on `--cargo`).
    #[arg(last = true)]
    pub spread: Vec<String>,
}

#[derive(Debug, Default, Clone, clap::Parser, Serialize, Deserialize)]
pub struct VersionOpts {
    #[clap(long = "charon", help = "Print Charon version instead of Obol.")]
    pub charon: bool,
}

impl CliOpts {
    pub fn validate(&mut self) -> Result<()> {
        Ok(())
    }
}
