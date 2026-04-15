#![feature(rustc_private)]
#![feature(iterator_try_collect)]
#![feature(iter_array_chunks)]

use anyhow::Result;
use clap::Parser;
use obol_lib::args::{CliOpts, OBOL_ARGS, ObolCli};
use std::{env, process::ExitStatus};

use crate::toolchain::toolchain_path;

mod toolchain;

fn main() -> Result<()> {
    let opts = ObolCli::parse();

    let res = match opts {
        ObolCli::Rustc(opts) => translate_without_cargo(opts)?,
        ObolCli::Cargo(opts) => translate_with_cargo(opts)?,
        ObolCli::ToolchainPath => {
            let path = toolchain_path()?;
            println!("{}", path.display());
            ExitStatus::default()
        }
        ObolCli::Version(opts) => {
            let v = if opts.charon {
                charon_lib::VERSION
            } else {
                obol_lib::VERSION
            };
            println!("{v}");
            ExitStatus::default()
        }
    };

    handle_exit_status(res)
}

fn translate_with_cargo(mut options: CliOpts) -> Result<ExitStatus> {
    options.validate()?;
    ensure_rustup();

    let mut cmd = toolchain::in_toolchain("cargo")?;
    cmd.env("RUSTC_WRAPPER", toolchain::driver_path());
    cmd.env("OBOL_USING_CARGO", "1");
    cmd.env_remove("CARGO_PRIMARY_PACKAGE");
    if cfg!(target_os = "macos") {
        let mut lib = toolchain_path()?;
        lib.push("lib");
        cmd.env("DYLD_LIBRARY_PATH", lib);
    }

    // When a test target is requested (--test <name>) or unit tests in src are
    // requested (--lib), we need `cargo test --no-run` so that the library is
    // fully built (with codegen) before the test binary is compiled.
    // `cargo build --test` would cause
    // obol-driver to suppress codegen for the library, making the test binary
    // unable to link against it.
    let is_specified = |arg: &str| options.spread.iter().any(|input| input.starts_with(arg));
    if is_specified("--test") || is_specified("--lib") {
        cmd.arg("test");
        cmd.arg("--no-run");
        // Signal to obol-driver that we are building a test target so it knows
        // not to translate (and suppress codegen for) the library crate.
        cmd.env("OBOL_BUILDING_TEST", "1");
    } else {
        cmd.arg("build");
    }

    if !is_specified("--target") {
        // Make sure the build target is explicitly set. This is needed to detect which crates are
        // proc-macro/build-script in `obol-driver`.
        cmd.arg("--target");
        cmd.arg(&get_rustc_version()?.host);
    }

    cmd.args(std::mem::take(&mut options.spread));
    cmd.env(OBOL_ARGS, serde_json::to_string(&options).unwrap());

    Ok(cmd
        .spawn()
        .expect("could not run cargo")
        .wait()
        .expect("failed to wait for cargo?"))
}

fn translate_without_cargo(mut options: CliOpts) -> Result<ExitStatus> {
    options.validate()?;
    ensure_rustup();

    let mut cmd = toolchain::driver_cmd()?;

    let is_specified = |arg| options.spread.iter().any(|input| input.starts_with(arg));
    if !is_specified("--target") {
        // Make sure the build target is explicitly set. This is needed to detect which crates are
        // proc-macro/build-script in `obol-driver`.
        cmd.arg("--target");
        cmd.arg(&get_rustc_version()?.host);
    }

    cmd.args(std::mem::take(&mut options.spread));
    cmd.env(OBOL_ARGS, serde_json::to_string(&options).unwrap());
    Ok(cmd
        .spawn()
        .expect("could not run obol-driver")
        .wait()
        .expect("failed to wait for obol-driver?"))
}

fn get_rustc_version() -> Result<rustc_version::VersionMeta> {
    let cmd = toolchain::driver_cmd()?;
    let rustc_version = rustc_version::VersionMeta::for_command(cmd).unwrap_or_else(|err| {
        panic!("failed to determine underlying rustc version of Obol:\\n{err:?}",)
    });
    Ok(rustc_version)
}

fn ensure_rustup() {
    // FIXME: when using rustup, ensure the toolchain has the right components installed.
    let use_rustup = which::which("rustup").is_ok();
    // This is set by the nix develop environment and the nix builder; in both cases the toolchain
    // is set up in `\$PATH` and the driver should be correctly dynamically linked.
    let correct_toolchain_is_in_path = env::var("OBOL_TOOLCHAIN_IS_IN_PATH").is_ok();

    if !use_rustup && !correct_toolchain_is_in_path {
        panic!(
            "Can't find `rustup`; please install it with your system package manager \\
            or from https://rustup.rs . \\
            If you are using nix, make sure to be in the flake-defined environment \\
            using `nix develop`.",
        )
    }
}

fn handle_exit_status(exit_status: ExitStatus) -> Result<()> {
    if exit_status.success() {
        Ok(())
    } else {
        let code = exit_status.code().unwrap_or(-1);
        // Rethrow the exit code
        std::process::exit(code);
    }
}
