#![feature(rustc_private)]

use anyhow::Result;
use clap::Parser;
use obol_lib::args::{CliOpts, OBOL_ARGS};
use obol_lib::lockbud::config::{DetectorKind, LockbudConfig};
use std::{env, path::PathBuf, process::ExitStatus};

mod cli;

fn main() -> Result<()> {
    // Detect if invoked as a cargo subcommand (cargo-lockbud).
    let args: Vec<String> = env::args().collect();
    let is_cargo_subcommand = args
        .get(0)
        .map(|a| {
            PathBuf::from(a)
                .file_stem()
                .map(|s| s == "cargo-lockbud")
                .unwrap_or(false)
        })
        .unwrap_or(false);

    let cli_args: Vec<String> = if is_cargo_subcommand {
        // cargo passes: cargo-lockbud lockbud [args...]
        // Keep a synthetic binary name and skip the subcommand name.
        let mut v = vec!["lockbud".to_string()];
        v.extend(args.into_iter().skip(2));
        v
    } else {
        // clap::parse_from expects the first arg to be the binary name.
        args
    };

    let opts = cli::LockbudCli::parse_from(cli_args);

    let mut config = LockbudConfig::new();
    if let Some(ref k) = opts.kind {
        config.kinds = k.split(',').map(|s| s.trim().to_string()).filter_map(|s| DetectorKind::from_str(&s)).collect();
    }
    config.report_file = opts.report_file.clone();

    if let Some(ref list) = opts.crate_list {
        let crates: Vec<String> = list.split(',').map(|s| s.trim().to_string()).collect();
        if opts.blacklist {
            config.crate_blacklist = Some(crates);
        } else {
            config.crate_whitelist = Some(crates);
        }
    }

    // Determine a known destination for the .ullbc file.
    let ullbc_path = opts.dest_file.clone().unwrap_or_else(|| {
        let pid = std::process::id();
        PathBuf::from(format!("/tmp/lockbud_{pid}.ullbc"))
    });

    // Run translation then detection.
    let res = run_lockbud(opts, &config, &ullbc_path)?;
    handle_exit_status(res)
}

fn run_lockbud(
    opts: cli::LockbudCli,
    config: &LockbudConfig,
    ullbc_path: &PathBuf,
) -> Result<ExitStatus> {
    ensure_rustup();

    // Build Obol options for translation.
    let mut obol_opts = CliOpts::default();
    obol_opts.dest_file = Some(ullbc_path.clone());

    // Invoke cargo with obol-driver as RUSTC_WRAPPER.
    let mut cmd = obol_lib::toolchain::in_toolchain("cargo")?;
    cmd.env("RUSTC_WRAPPER", obol_lib::toolchain::driver_path());
    cmd.env("OBOL_USING_CARGO", "1");
    cmd.env_remove("CARGO_PRIMARY_PACKAGE");

    if cfg!(target_os = "macos") {
        let mut lib = obol_lib::toolchain::toolchain_path()?;
        lib.push("lib");
        cmd.env("DYLD_LIBRARY_PATH", lib);
    }

    let is_specified = |arg: &str| opts.cargo_args.iter().any(|input| input.starts_with(arg));
    if is_specified("--test") || is_specified("--lib") {
        cmd.arg("test");
        cmd.arg("--no-run");
        cmd.env("OBOL_BUILDING_TEST", "1");
    } else {
        cmd.arg("build");
    }

    if !is_specified("--target") {
        cmd.arg("--target");
        cmd.arg(&get_rustc_version()?.host);
    }

    cmd.args(opts.cargo_args);
    cmd.env(OBOL_ARGS, serde_json::to_string(&obol_opts).unwrap());

    let status = cmd
        .spawn()
        .expect("could not run cargo")
        .wait()
        .expect("failed to wait for cargo?");

    if !status.success() {
        return Ok(status);
    }

    // Translation succeeded; run detection on the .ullbc file.
    if !ullbc_path.exists() {
        eprintln!("error: ULLBC file not found at {:?}", ullbc_path);
        return Ok(ExitStatus::default());
    }

    let file = std::fs::File::open(ullbc_path)?;
    let crate_data: charon_lib::export::CrateData = match serde_json::from_reader(file) {
        Ok(data) => data,
        Err(e) => {
            eprintln!("error: failed to deserialize ULLBC: {}", e);
            return Ok(ExitStatus::default());
        }
    };

    obol_lib::lockbud::run(&crate_data, config);

    // Clean up temporary .ullbc unless user specified a dest-file.
    if opts.dest_file.is_none() {
        let _ = std::fs::remove_file(ullbc_path);
    }

    Ok(status)
}

fn get_rustc_version() -> Result<rustc_version::VersionMeta> {
    let cmd = obol_lib::toolchain::driver_cmd()?;
    let rustc_version = rustc_version::VersionMeta::for_command(cmd).unwrap_or_else(|err| {
        panic!("failed to determine underlying rustc version:\n{err:?}")
    });
    Ok(rustc_version)
}

fn ensure_rustup() {
    let use_rustup = which::which("rustup").is_ok();
    let correct_toolchain_is_in_path = env::var("OBOL_TOOLCHAIN_IS_IN_PATH").is_ok();

    if !use_rustup && !correct_toolchain_is_in_path {
        panic!(
            "Can't find `rustup`; please install it with your system package manager \
            or from https://rustup.rs . \
            If you are using nix, make sure to be in the flake-defined environment \
            using `nix develop`.",
        )
    }
}

fn handle_exit_status(exit_status: ExitStatus) -> Result<()> {
    if exit_status.success() {
        Ok(())
    } else {
        let code = exit_status.code().unwrap_or(-1);
        std::process::exit(code);
    }
}
