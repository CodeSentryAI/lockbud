#![feature(rustc_private)]
#![feature(assert_matches)]

extern crate rustc_driver;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_hir;
extern crate rustc_public; // Required for the run_with_tcx! macro

use rustc_middle::ty::TyCtxt;
use std::ops::ControlFlow;
use std::process::ExitCode;

// Use stable-mir-wrapper for types instead of rustc_public directly
use stable_mir_wrapper::{
    // MIR types
    Body,
    Instance, MonoItem, StaticDef,
    TerminatorKind, Operand,
    // Type types
    TyKind, RigidTy,
    // Crate types
    CrateItem, ItemKind, CrateDef,
    // Crate queries (from the wrapper)
    local_crate, all_local_items, entry_fn,
    // Error type
    CompilerError,
};

// Use run_with_tcx macro from rustc_public (it needs the extern crate)
use rustc_public::run_with_tcx;

fn main() -> ExitCode {
    let rustc_args: Vec<_> = std::env::args().collect();
    let result = run_with_tcx!(&rustc_args, demo_analysis);
    match result {
        Ok(_) | Err(CompilerError::Skipped | CompilerError::Interrupted(_)) => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}

fn demo_analysis<'tcx>(_tcx: TyCtxt<'tcx>) -> ControlFlow<()> {
    let local_crate = local_crate();
    const CRATE_NAME: &str = "static_ref";
    if local_crate.name != CRATE_NAME {
        return ControlFlow::Continue(());
    }

    eprintln!("crate: {}", local_crate.name);

    // Get entry function
    if let Some(entry) = entry_fn() {
        eprintln!("entry_fn {}", entry.name());
    }

    let mut local_cnt = 0;
    for local_item in all_local_items() {
        match local_item.kind() {
            ItemKind::Fn => {
                if let Ok(instance) = Instance::try_from(local_item) {
                    let x = MonoItem::from(instance);
                    eprintln!("{:?}", x);
                    local_cnt += 1;

                    // Try to get the body
                    if let Some(body) = local_item.body() {
                        eprintln!("  - Body has {} basic blocks", body.blocks.len());

                        // Analyze the body for function calls
                        for bb in &body.blocks {
                            if let TerminatorKind::Call { func, .. } = &bb.terminator.kind {
                                match func {
                                    Operand::Constant(c) => {
                                        if let TyKind::RigidTy(rigid_ty) = c.ty().kind() {
                                            if let RigidTy::FnDef(def, ref args) = rigid_ty {
                                                if let Ok(callee_instance) = Instance::resolve(def, args) {
                                                    eprintln!("  - callee: {}", callee_instance.name());
                                                }
                                            }
                                        }
                                    }
                                    Operand::Copy(_) => {}
                                    Operand::Move(_) => {}
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
            ItemKind::Static => {
                if let Ok(static_def) = StaticDef::try_from(local_item) {
                    let y = MonoItem::from(static_def);
                    eprintln!("{:?}", y);
                    local_cnt += 1;
                }
            }
            ItemKind::Const => {
                eprintln!("const {:?}", local_item);
            }
            ItemKind::Ctor(_) => {}
        }
    }

    eprintln!("local_cnt: {}", local_cnt);

    ControlFlow::Continue(())
}
