//! Run the rustc compiler with our custom options and hooks.
use crate::ObolError;
use crate::translate::translate_crate;
use charon_lib::transform::TransformCtx;
use crate::args::CliOpts;
use rustc_driver::{Callbacks, Compilation};
use rustc_interface::Config;
use rustc_interface::interface::Compiler;
use rustc_middle::ty::TyCtxt;
use rustc_session::config::{OutputType, OutputTypes};
use std::ops::Deref;
use std::{env, fmt};

/// Helper that runs the compiler and catches its fatal errors.
fn run_compiler_with_callbacks(
    args: Vec<String>,
    callbacks: &mut (dyn Callbacks + Send),
) -> Result<(), ObolError> {
    rustc_driver::catch_fatal_errors(|| rustc_driver::run_compiler(&args, callbacks))
        .map_err(|_| ObolError::RustcError)
}

/// Tweak options to get usable MIR even for foreign crates.
fn set_mir_options(config: &mut Config) {
    config.opts.unstable_opts.always_encode_mir = true;
    config.opts.unstable_opts.mir_opt_level = Some(0);
    config.opts.unstable_opts.mir_emit_retag = true;
    config.opts.unstable_opts.mir_preserve_ub = true;
    let disabled_mir_passes = [
        "RemoveStorageMarkers",
        "CheckAlignment",
        "CheckNull",
        "CheckEnums",
    ];
    for pass in disabled_mir_passes {
        config
            .opts
            .unstable_opts
            .mir_enable_passes
            .push((pass.to_owned(), false));
    }
}

/// Don't even try to codegen. This avoids errors due to checking if the output filename is
/// available (despite the fact that we won't emit it because we stop compilation early).
fn set_no_codegen(config: &mut Config) {
    config.opts.unstable_opts.no_codegen = true;
    // Only emit metadata.
    config.opts.output_types = OutputTypes::new(&[(OutputType::Metadata, None)]);
}

// We use a static to be able to pass data to `override_queries`.

fn setup_compiler(config: &mut Config, do_translate: bool) {
    if do_translate {
        set_no_codegen(config);
    }
    set_mir_options(config);
}

/// Run the rustc driver with our custom hooks. Returns `None` if the crate was not compiled with
/// charon (e.g. because it was a dependency). Otherwise returns the translated crate, ready for
/// post-processing transformations.
pub fn run_rustc_driver(options: &CliOpts) -> Result<Option<TransformCtx>, ObolError> {
    // Retreive the command-line arguments pased to `charon_driver`. The first arg is the path to
    // the current executable, we skip it.
    let mut compiler_args: Vec<String> = env::args().skip(1).collect();

    // When called using cargo, we tell cargo to use `charon-driver` by setting the `RUSTC_WRAPPER`
    // env var. This uses `charon-driver` for all the crates being compiled.
    // We may however not want to be calling charon on all crates; `CARGO_PRIMARY_PACKAGE` tells us
    // whether the crate was specifically selected or is a dependency.
    let is_workspace_dependency =
        env::var("OBOL_USING_CARGO").is_ok() && !env::var("CARGO_PRIMARY_PACKAGE").is_ok();
    // Determines if we are being invoked to build a crate for the "target" architecture, in
    // contrast to the "host" architecture. Host crates are for build scripts and proc macros and
    // still need to be built like normal; target crates need to be processed by Charon.
    //
    // Currently, we detect this by checking for "--target=", which is never set for host crates.
    // This matches what Miri does, which hopefully makes it reliable enough. This relies on us
    // always invoking cargo itself with `--target`, which `charon` ensures.
    let is_target = arg_values(&compiler_args, "--target").next().is_some();
    // When building test binaries with `cargo test --no-run`, cargo first compiles the library
    // crate (primary package, with codegen) and then the test binary. We only want to translate
    // the actual test binary (identified by the `--test` flag that cargo passes to rustc), not the
    // library. If we suppressed codegen for the library, the test binary could not link against it.
    // `OBOL_BUILDING_TEST` is set by `obol` when it detects `--test` in the spread args.
    let is_building_test_target = env::var("OBOL_BUILDING_TEST").is_ok();
    let is_test_binary = compiler_args.iter().any(|a| a == "--test");
    // Whether this is the crate we want to translate.
    let is_selected_crate =
        !is_workspace_dependency && is_target && (!is_building_test_target || is_test_binary);

    let output = if !is_selected_crate {
        // Run the compiler normally.
        run_compiler_with_callbacks(
            compiler_args,
            &mut RunCompilerNormallyCallbacks { _options: options },
        )?;
        None
    } else {
        for extra_flag in options.spread.iter().cloned() {
            compiler_args.push(extra_flag);
        }

        // Call the Rust compiler with our custom callback.
        let mut callback = ObolCallbacks {
            options,
            transform_ctx: None,
        };

        run_compiler_with_callbacks(compiler_args, &mut callback)?;
        // If `transform_ctx` is not set here, there was a fatal error.
        let ctx = callback.transform_ctx.ok_or(ObolError::RustcError)?;
        Some(ctx)
    };
    Ok(output)
}

/// The callbacks for Charon
pub struct ObolCallbacks<'a> {
    options: &'a CliOpts,
    /// This is to be filled during the extraction; it contains the translated crate.
    transform_ctx: Option<TransformCtx>,
}
impl<'a> Callbacks for ObolCallbacks<'a> {
    fn config(&mut self, config: &mut Config) {
        setup_compiler(config, true);
    }

    /// The MIR is modified in place: borrow-checking requires the "promoted" MIR, which causes the
    /// "built" MIR (which results from the conversion to HIR to MIR) to become unaccessible.
    /// Because we require built MIR at the moment, we hook ourselves before MIR-based analysis
    /// passes.
    fn after_expansion<'tcx>(&mut self, compiler: &Compiler, tcx: TyCtxt<'tcx>) -> Compilation {
        // Set up our own `DefId` debug routine.
        rustc_hir::def_id::DEF_ID_DEBUG
            .swap(&(def_id_debug as fn(_, &mut fmt::Formatter<'_>) -> _));

        let transform_ctx = rustc_public::rustc_internal::run(tcx, || {
            translate_crate::translate(
                &self.options,
                tcx,
                compiler.sess.opts.sysroot.path().to_owned(),
            )
        });
        if let Ok(transform_ctx) = transform_ctx {
            self.transform_ctx = Some(transform_ctx);
        }
        Compilation::Continue
    }
    fn after_analysis<'tcx>(&mut self, _: &Compiler, _: TyCtxt<'tcx>) -> Compilation {
        // Don't continue to codegen etc.
        Compilation::Stop
    }
}

/// Dummy callbacks used to run the compiler normally when we shouldn't be analyzing the crate.
pub struct RunCompilerNormallyCallbacks<'a> {
    _options: &'a CliOpts,
}
impl<'a> Callbacks for RunCompilerNormallyCallbacks<'a> {
    fn config(&mut self, config: &mut Config) {
        setup_compiler(config, false);
    }
}

/// Returns the values of the command-line options that match `find_arg`. The options are built-in
/// to be of the form `--arg=value` or `--arg value`.
fn arg_values<'a, T: Deref<Target = str>>(
    args: &'a [T],
    needle: &'a str,
) -> impl Iterator<Item = &'a str> {
    struct ArgFilter<'a, T> {
        args: std::slice::Iter<'a, T>,
        needle: &'a str,
    }
    impl<'a, T: Deref<Target = str>> Iterator for ArgFilter<'a, T> {
        type Item = &'a str;
        fn next(&mut self) -> Option<Self::Item> {
            while let Some(arg) = self.args.next() {
                let mut split_arg = arg.splitn(2, '=');
                if split_arg.next() == Some(self.needle) {
                    return match split_arg.next() {
                        // `--arg=value` form
                        arg @ Some(_) => arg,
                        // `--arg value` form
                        None => self.args.next().map(|x| x.deref()),
                    };
                }
            }
            None
        }
    }
    ArgFilter {
        args: args.iter(),
        needle,
    }
}

/// Custom `DefId` debug routine that doesn't print unstable values like ids and hashes.
fn def_id_debug(def_id: rustc_hir::def_id::DefId, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    rustc_middle::ty::tls::with_opt(|opt_tcx| {
        if let Some(tcx) = opt_tcx {
            let crate_name = if def_id.is_local() {
                tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE)
            } else {
                tcx.cstore_untracked().crate_name(def_id.krate)
            };
            write!(
                f,
                "{}{}",
                crate_name,
                tcx.def_path(def_id).to_string_no_crate_verbose()
            )?;
        } else {
            write!(f, "<can't access `tcx` to print `DefId` path>")?;
        }
        Ok(())
    })
}
