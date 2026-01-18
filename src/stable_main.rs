#![feature(rustc_private)]
#![feature(assert_matches)]

extern crate rustc_driver;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_public;
extern crate rustc_hir;

use rustc_hir::def_id::LOCAL_CRATE;
use rustc_middle::ty::TyCtxt;
use rustc_public::mir::mono::Instance;
use rustc_public::mir::mono::MonoItem;
use rustc_public::mir::mono::StaticDef;
use rustc_public::mir::Body;
use rustc_public::mir::Operand;
use rustc_public::mir::TerminatorKind;
use rustc_public::ty::RigidTy;
use rustc_public::ty::TyKind;
use rustc_public::CompilerError;
use rustc_public::run_with_tcx;
use rustc_public::CrateDef;
use rustc_public::CrateItem;
use rustc_public::ItemKind;
use std::collections::HashMap;
use std::collections::HashSet;
use std::ops::ControlFlow;
use std::process::ExitCode;

fn main() -> ExitCode {
    let rustc_args: Vec<_> = std::env::args().collect();
    let result = run_with_tcx!(&rustc_args, demo_analysis);
    match result {
        Ok(_) | Err(CompilerError::Skipped | CompilerError::Interrupted(_)) => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}

fn demo_analysis<'tcx>(tcx: TyCtxt<'tcx>) -> ControlFlow<()> {
    let local_crate = rustc_public::local_crate();
    const CRATE_NAME: &str = "static_ref";
    if local_crate.name != CRATE_NAME {
        return ControlFlow::Continue(());
    }
    // let cgus = tcx.collect_and_partition_mono_items(()).codegen_units;
    // eprintln!("{}", cgus.len());

    // let mut cnt = 0;
    // for (idx, cgu) in cgus.iter().enumerate() {
    //     cnt += cgu.items().len();
    //     // for (item, _) in cgu.items() {
    //     //     // if item.krate() == LOCAL_CRATE {
    //     //         // eprintln!("{:?}", item.def_id());
    //     //     // }
    //     // }
    //     //    println!("local: {}", cgu.items().len());
    //     // }
    // }
    // eprintln!("cnt {}", cnt);
    
    let local_crate = rustc_public::local_crate();
    if local_crate.name == CRATE_NAME {
        eprintln!("crate: {}", local_crate.name);
        
        eprintln!("entry_fn {}", rustc_public::entry_fn().unwrap().name());
        let mut local_cnt = 0;
        for local_item in rustc_public::all_local_items() {
            // eprintln!("local_item: {:?}", local_item);
            // if let Ok(instance) = Instance::try_from(local_item) {
            //     // not GlobalAsm
            // }
            if let ItemKind::Fn = local_item.kind() {
                if let Ok(instance) = Instance::try_from(local_item) {
                    let x = MonoItem::from(instance);
                    eprintln!("{:?}", x);
                }
            }
            if let ItemKind::Static = local_item.kind() {
                // eprintln!("static: {:?}", local_item);
                if let Ok(static_def) = StaticDef::try_from(local_item) {
                    let y = MonoItem::from(static_def);
                    eprintln!("{:?}", y);
                }
            }
            if let ItemKind::Const = local_item.kind() {
                eprintln!("const {:?}", local_item);
            }
            if let ItemKind::Ctor(ctor_kind) = local_item.kind() {
                eprintln!("ctor: {:?}", ctor_kind);
            }




           

            


            // local_cnt += 1;
            // if let Some(body) = local_item.body() {
            //     for bb in body.blocks {
            //         if let TerminatorKind::Call { func, args, destination, target, unwind } = bb.terminator.kind {
            //             match func {
            //                 Operand::Constant(c) => {
            //                     if let TyKind::RigidTy(rigid_ty) = c.ty().kind() {
            //                         if let RigidTy::FnDef(def, ref args) = rigid_ty {
            //                             if let Ok(instance) = Instance::resolve(def, args) {
            //                                 eprintln!("callee: {}", instance.name());
            //                                 local_cnt += 1;
            //                             }
            //                         }

            //                     }
            //                 }
            //                 Operand::Copy(_) => {

            //                 }
            //                 Operand::Move(_) => {

            //                 }
            //             }
            //         }
            //     }
            // }
            
        }
        // eprintln!("local_cnt: {}", local_cnt);
        // for trait_impl in rustc_public::all_trait_impls() {
        //     eprintln!("trait_impl: {:?}", trait_impl);
        // }
        // eprintln!("impls_cnt: {}", rustc_public::all_trait_impls().len());
        // for trait_decl in rustc_public::all_trait_decls() {
        //     eprintln!("trait_decl: {:?}", trait_decl);
        // }
        // eprintln!("decls_cnt: {}", rustc_public::all_trait_decls().len());
        // for external_crate in rustc_public::external_crates() {
        //     eprintln!("external crate: {:?}", external_crate);

        //     // for fn_def in external_crate.fn_defs() {
        //     //     eprintln!("{:?}", fn_def);
        //     // }
        //     eprintln!("{}", external_crate.fn_defs().len());
        //     eprintln!("{}", external_crate.statics().len());
        //     // eprintln!("{}", external_crate..len());
        // }
    }
    ControlFlow::Continue(())
}