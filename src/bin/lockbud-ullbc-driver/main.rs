#![feature(rustc_private)]
#![feature(iterator_try_collect)]
#![feature(iter_array_chunks)]
#![feature(impl_trait_in_bindings)]
#![feature(if_let_guard)]

extern crate rustc_driver;
extern crate rustc_hir;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_public;
extern crate rustc_session;

use std::{env, fmt, panic, path::PathBuf};

use charon_lib::{export::CrateData, logger, transform::Pass};
use obol_lib::args::{self, CliOpts};
use obol_lib::ObolError;
use obol_lib::driver;
use obol_lib::translate;

fn transformation_passes(_opts: &CliOpts) -> Vec<Pass> {
    use Pass::*;
    use charon_lib::transform::*;
    vec![
        // post-translation
        NonBody(&add_missing_info::compute_short_names::Transform),
        UnstructuredBody(&finish_translation::insert_ptr_metadata::Transform),
        UnstructuredBody(&finish_translation::insert_assign_return_unit::Transform),
        NonBody(&finish_translation::insert_storage_lives::Transform),
        // ullbc
        UnstructuredBody(&resugar::move_asserts_to_statements::Transform),
        UnstructuredBody(&control_flow::merge_goto_chains::Transform),
        UnstructuredBody(&resugar::reconstruct_fallible_operations::Transform),
        UnstructuredBody(&resugar::reconstruct_asserts::Transform),
        UnstructuredBody(&resugar::inline_local_panic_functions::Transform),
        UnstructuredBody(&simplify_output::simplify_constants::Transform),
        UnstructuredBody(&simplify_output::remove_unit_locals::Transform),
        UnstructuredBody(&normalize::filter_unreachable_blocks::Transform),
        UnstructuredBody(&simplify_output::update_block_indices::Transform),
        // finalising
        NonBody(&simplify_output::remove_unused_locals::Transform),
        NonBody(&simplify_output::remove_nops::Transform),
        NonBody(&add_missing_info::reorder_decls::Transform),
    ]
}

/// Run obol. Returns the number of warnings generated.
fn run_obol(options: args::CliOpts) -> Result<usize, ObolError> {
    // Run the driver machinery.
    let Some(mut ctx) = driver::run_rustc_driver(&options)? else {
        // We didn't run charon.
        return Ok(0);
    };

    // The bulk of the translation is done, we no longer need to interact with rustc internals. We
    // run several passes that simplify the items and cleanup the bodies.
    for pass in transformation_passes(&options) {
        pass.run(&mut ctx);
    }

    let error_count = ctx.errors.borrow().error_count;

    if options.print_ullbc {
        println!("\n// Final ULLBC:\n\n{ctx}\n");
    }

    let crate_data = CrateData::new(ctx);

    // Run lockbud analysis directly on ULLBC without serializing to file.
    obol_lib::lockbud::run(&crate_data, options.report_file.as_deref());

    let path = options.dest_file.clone().or_else(|| {
        let crate_name = &crate_data.translated.crate_name;
        let extension = "ullbc";
        Some(PathBuf::from(format!("{crate_name}.{extension}")))
    });

    if let Some(path) = path {
        crate_data
            .serialize_to_file(&path)
            .expect("Failed to write smir.json to output")
    }

    Ok(error_count)
}

fn main() {
    // Initialize the logger
    logger::initialize_logger();

    // Retrieve the Charon options by deserializing them from the environment variable
    // (cargo-charon serialized the arguments and stored them in a specific environment
    // variable before calling cargo with RUSTC_WRAPPER=charon-driver).
    let options: args::CliOpts = match env::var(args::OBOL_ARGS) {
        Ok(opts) => serde_json::from_str(opts.as_str()).unwrap(),
        Err(_) => Default::default(),
    };

    // Catch any and all panics coming from charon to display a clear error.
    let res = panic::catch_unwind(move || run_obol(options))
        .map_err(|_| ObolError::Panic)
        .flatten();

    match res {
        Ok(warn_count) => {
            if warn_count != 0 {
                let msg = format!("The extraction generated {} warnings", warn_count);
                eprintln!("warning: {}", msg);
            }
        }
        Err(err) => {
            log::error!("{err}");
            let exit_code = match err {
                ObolError::ObolError(_) | ObolError::Serialize => 1,
                ObolError::RustcError => 2,
                // This is a real panic, exit with the standard rust panic error code.
                ObolError::Panic => 101,
            };
            std::process::exit(exit_code);
        }
    }
}
