//! Demo of using stable_analysis callgraph
//!
//! This demonstrates how to build and use a call graph with only
//! stable_mir_wrapper types.

#![feature(rustc_private)]

extern crate rustc_driver;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_hir;
extern crate rustc_public;

use rustc_middle::ty::{TyCtxt};
use rustc_public::external_crates;
use rustc_session::config::ErrorOutputType;
use rustc_session::EarlyDiagCtxt;
use log::info;
use std::env;
use std::process::ExitCode;
use std::sync::Mutex;

use stable_mir_wrapper::{
    ClosureKind, CompilerError, ConstOperand, CrateDef, CrateItem, Instance, MonoItem, Operand, RigidTy, TerminatorKind, all_local_items, local_crate, run_with_tcx,
    Body,
};
use stable_mir_wrapper::ItemKind;

mod stable_analysis;
use stable_analysis::{CallGraph, Node, closure_analysis, LockDetector, LockReport, LockBugKind, MirVisitorTypeCollector};

// Global variable to store analysis results
static ANALYSIS_RESULT: Mutex<Option<String>> = Mutex::new(None);

fn main() -> ExitCode {
    let args: Vec<_> = env::args().collect();

    // Initialize loggers
    let handler = EarlyDiagCtxt::new(ErrorOutputType::default());
    if std::env::var("RUSTC_LOG").is_ok() {
        rustc_driver::init_rustc_env_logger(&handler);
    }
    if std::env::var("LOCKBUD_LOG").is_ok() {
        let e = env_logger::Env::new()
            .filter("LOCKBUD_LOG")
            .write_style("LOCKBUD_LOG_STYLE");
        env_logger::init_from_env(e);
    }

    let result = run_with_tcx!(&args, demo_callgraph);
    match result {
        Ok(_) => {
            // Print the analysis result
            let result = ANALYSIS_RESULT.lock().unwrap();
            if let Some(output) = result.as_ref() {
                println!("{}", output);
            }
            ExitCode::SUCCESS
        }
        Err(CompilerError::Skipped | CompilerError::Interrupted(_)) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

// fn resolve_callee(operand: &Operand) {
//     match operand {
//         Operand::Constant(ConstOperand { span, user_ty, const_ }) => {
//             let kind = const_.ty.kind();
//             format!("Callee kind: {:?}", kind);
//         }
//         _ => { /* Other operand kinds */ }
//     }
// }

fn demo_callgraph(_tcx: TyCtxt) -> std::ops::ControlFlow<()> {
    let local_crate = local_crate();
    // Only analyze local crates, not dependencies
    // List of crate names to analyze
    const CRATE_NAMES: &[&str] = &[
        "intra",
        "inter",
        "conflict",
        "conflict_inter",
        "lock_closure",
        "static_ref",
        "tikv_wrapper",
    ];

    let crate_normalized = local_crate.name.replace("-", "_");
    if !CRATE_NAMES.iter().any(|&name| crate_normalized.contains(name)) {
        return std::ops::ControlFlow::Continue(());
    }
    info!("Analyzing crate: {}", local_crate.name);

    let mut output = String::new();

    output.push_str(&format!("=== Stable Call Graph Analysis ===\n"));
    output.push_str(&format!("Crate: {}\n", local_crate.name));
    output.push_str("\n");

    // Collect all local items
    let items: Vec<CrateItem> = all_local_items().into_iter().collect();

    // Build the call graph
    let mut callgraph = CallGraph::new();
    callgraph.analyze(&items);

    output.push_str(&format!("Call Graph Statistics:\n"));
    output.push_str(&format!("  Total functions: {}\n", callgraph.node_count()));
    output.push_str(&format!("  Total calls: {}\n", callgraph.edge_count()));
    output.push_str("\n");

    output.push_str("=== Call Graph Details ===\n");
    output.push_str("\n");

    // For each caller, list callees and callsites
    let mut caller_count = 0;
    for node in callgraph.nodes() {
        let caller_item = &node.0;

        // Only process functions, not statics or global asm
        let caller_instance = match caller_item {
            stable_mir_wrapper::MonoItem::Fn(instance) => instance,
            _ => continue,
        };

        let caller_name = caller_instance.name();

        // Skip foreign items, very long internal names, and std library functions
        // Only show local functions or closures
        if caller_instance.is_foreign_item()
            || caller_name.len() > 200
            || caller_name.starts_with("std::")
            || caller_name.starts_with("alloc::")
            || caller_name.starts_with("core::")
            || caller_name.starts_with("<std::")
            || caller_name.starts_with("<alloc::")
            || caller_name.starts_with("<core::") {
            continue;
        }

        // Get callees
        let callees = callgraph.successors(caller_item);
        if callees.is_empty() {
            continue;
        }

        caller_count += 1;
        output.push_str(&format!("Caller: {}\n", caller_name));
        output.push_str(&format!("  Callees: {}\n", callees.len()));

        // Find callsites in the caller's body
        if let Some(body) = caller_instance.body() {
            let locals = body.locals();

            // For each callee, find its callsite
            for callee_item in &callees {
                let callee_name = match callee_item {
                    stable_mir_wrapper::MonoItem::Fn(instance) => instance.name(),
                    stable_mir_wrapper::MonoItem::Static(def) => def.name(),
                    stable_mir_wrapper::MonoItem::GlobalAsm(_) => "<global_asm>".to_string(),
                };

                output.push_str(&format!("    -> {}\n", callee_name));

                // Search for the callsite in the body
                let mut callsite_found = false;
                for (bb_idx, bb) in body.blocks.iter().enumerate() {
                    // Check terminator for calls
                    if let TerminatorKind::Call { func, .. } = &bb.terminator.kind {
                        if let Ok(func_ty) = func.ty(locals) {
                            if let Some(RigidTy::FnDef(fn_def, _)) = func_ty.kind().rigid() {
                                let callee_from_terminator = fn_def.name();
                                // Check if this terminator matches our callee
                                if callee_from_terminator == callee_name
                                    || callee_from_terminator.contains(&callee_name)
                                    || callee_name.contains(&callee_from_terminator) {
                                    output.push_str(&format!("       Callsite: block={}, terminator\n", bb_idx));
                                    callsite_found = true;
                                }
                            }
                        }
                    }

                    // Check drop glue
                    if let TerminatorKind::Drop { place, .. } = &bb.terminator.kind {
                        if let Ok(place_ty) = place.ty(locals) {
                            let drop_instance = Instance::resolve_drop_in_place(place_ty);
                            let drop_name = drop_instance.name();
                            if drop_name == callee_name || drop_name.contains(&callee_name) {
                                output.push_str(&format!("       Callsite: block={}, drop\n", bb_idx));
                                callsite_found = true;
                            }
                        }
                    }
                }

                if !callsite_found {
                    output.push_str(&format!("       Callsite: <not found in body>\n"));
                }
            }
        }

        output.push_str("\n");

        // Limit output for readability
        if caller_count >= 20 {
            output.push_str("... (output truncated after 20 callers)\n");
            break;
        }
    }

    // Special section for closures
    output.push_str("\n=== Closure Summary ===\n");
    output.push_str("\n");

    // Set expected closures based on crate name
    let expected_closures: Vec<&str> = if local_crate.name.contains("lock_closure") ||
                                         local_crate.name.contains("lock-closure") {
        vec![
            "one_closure_one_caller::{closure#0}",
            "two_closures::{closure#0}",
            "two_closures::{closure#1}",
        ]
    } else {
        vec![]  // No expected closures for other crates
    };

    let mut found_closures = Vec::new();

    for node in callgraph.nodes() {
        let item = &node.0;
        if let stable_mir_wrapper::MonoItem::Fn(instance) = item {
            let name = instance.name();

            // Check if this is one of our expected closures
            for expected in &expected_closures {
                if name.contains(expected) {
                    found_closures.push(name.clone());
                    output.push_str(&format!("✓ Found closure: {}\n", name));

                    // Find callers
                    let callers = callgraph.predecessors(item);
                    output.push_str(&format!("  Called by: {} function(s)\n", callers.len()));

                    for caller in &callers {
                        if let stable_mir_wrapper::MonoItem::Fn(caller_instance) = caller {
                            let caller_name = caller_instance.name();
                            output.push_str(&format!("    - {}\n", caller_name));

                            // Find callsite location
                            find_and_print_closure_callsite(*caller_instance, &name, &mut output);
                        }
                    }
                    output.push_str("\n");
                }
            }
        }
    }

    // Report missing closures
    for expected in &expected_closures {
        if !found_closures.iter().any(|name| name.contains(expected)) {
            output.push_str(&format!("✗ Missing closure: {}\n", expected));
        }
    }

    output.push_str(&format!("\nFound {} out of {} expected closures\n", found_closures.len(), expected_closures.len()));

    // Thread spawn summary
    output.push_str("\n=== Thread Spawn Summary ===\n");
    output.push_str("\n");

    let mut spawn_count = 0;
    for node in callgraph.nodes() {
        let item = &node.0;
        if let stable_mir_wrapper::MonoItem::Fn(instance) = item {
            if instance.name().contains("thread::spawn") {
                spawn_count += 1;
            }
        }
    }

    output.push_str(&format!("Total thread::spawn calls found: {}\n", spawn_count));

    // Quick lock usage check
    output.push_str("\n=== Lock Type Analysis ===\n");
    output.push_str("\n");

    let quick_scanner = stable_analysis::QuickLockScanner::new();

    // Fast check: does this crate use locks?
    let uses_locks = quick_scanner.crate_uses_locks(&items);

    if !uses_locks {
        output.push_str("No lock types (Mutex, RwLock) found in this crate\n");
        output.push_str("Skipping detailed lock analysis.\n");
        output.push_str("\n");
    } else {
        // Locks found - do detailed analysis
        let lock_info = quick_scanner.scan_lock_types(&items);

        output.push_str(&format!("Lock usage detected!\n"));
        output.push_str(&format!("Functions using locks: {}\n", lock_info.instances_with_locks.len()));
        output.push_str(&format!("Unique lock types found: {}\n", lock_info.lock_types_found.len()));

        // Show which functions use locks
        if !lock_info.instances_with_locks.is_empty() {
            output.push_str("\nFunctions:\n");
            for instance_name in &lock_info.instances_with_locks {
                output.push_str(&format!("  - {}\n", instance_name));
            }
        }
        output.push_str("\n");

        // Detailed type collection using original collector
        let mut type_collector = stable_analysis::lock_types::LockTypeCollector::new();
        type_collector.analyze_crate(&items);
        output.push_str(&type_collector.format_summary());

        // MirVisitor-based analysis (finds types in expressions too)
        output.push_str("\n--- MirVisitor Type Collection ---\n");
        output.push_str("(Finds types in expressions, not just locals)\n");
        output.push_str("\n");

        let mut visitor_collected_count = 0;
        for item in &items {
            if let Ok(instance) = Instance::try_from(*item) {
                let instance_name = instance.name();
                // Skip very long names and std library functions
                if instance_name.len() > 200 || instance_name.starts_with("std::") {
                    continue;
                }

                if let Some(body) = instance.body() {
                    let mut visitor = MirVisitorTypeCollector::new(&body, instance_name.clone());
                    visitor.analyze();

                    if visitor.has_lock_types() {
                        visitor_collected_count += 1;
                        output.push_str(&visitor.format_summary());
                        output.push_str("\n");

                        // Limit output
                        if visitor_collected_count >= 5 {
                            output.push_str("... (output truncated after 5 instances)\n");
                            break;
                        }
                    }
                }
            }
        }

        if visitor_collected_count == 0 {
            output.push_str("No lock types found by MirVisitor (unexpected!)\n");
        }
    }

    output.push_str("\n");

    // Lock detection summary
    output.push_str("\n=== Lock Deadlock Detection ===\n");
    output.push_str("\n");

    let mut detector = LockDetector::new();
    let reports = detector.detect(&items, &callgraph);

    output.push_str(&format!("Total potential deadlocks found: {}\n", reports.len()));

    for (idx, report) in reports.iter().enumerate() {
        output.push_str(&format!("\n[Deadlock #{} - {:?}]\n", idx + 1, report.kind));
        output.push_str(&format!("Confidence: {}\n", report.possibility));
        output.push_str(&format!("┌─ First Lock:\n"));
        output.push_str(&format!("│   Type: {}\n", report.first_lock.lock_type));
        output.push_str(&format!("│   Function: {}\n", report.first_lock.function));
        if let Some(var_name) = &report.first_lock.var_name {
            output.push_str(&format!("│   Variable: {}\n", var_name));
        }
        if let Some(source_loc) = &report.first_lock.source_loc {
            output.push_str(&format!("│   Location: {}\n", source_loc));
        }
        output.push_str(&format!("│\n"));
        output.push_str(&format!("├─ Second Lock:\n"));
        output.push_str(&format!("│   Type: {}\n", report.second_lock.lock_type));
        output.push_str(&format!("│   Function: {}\n", report.second_lock.function));
        if let Some(var_name) = &report.second_lock.var_name {
            output.push_str(&format!("│   Variable: {}\n", var_name));
        }
        if let Some(source_loc) = &report.second_lock.source_loc {
            output.push_str(&format!("│   Location: {}\n", source_loc));
        }
        output.push_str(&format!("│\n"));
        output.push_str(&format!("└─ Deadlock Path:\n"));
        for step in &report.callchain {
            output.push_str(&format!("    {}\n", step));
        }
    }

    // Store result
    *ANALYSIS_RESULT.lock().unwrap() = Some(output);

    std::ops::ControlFlow::Continue(())
}

/// Find and print the callsite where a caller invokes a specific closure
fn find_and_print_closure_callsite(caller: Instance, closure_name: &str, output: &mut String) {
    if let Some(body) = caller.body() {
        let locals = body.locals();

        for (bb_idx, bb) in body.blocks.iter().enumerate() {
            // Check terminator for calls
            if let TerminatorKind::Call { func, ref args, .. } = &bb.terminator.kind {
                if let Ok(func_ty) = func.ty(locals) {
                    if let Some(RigidTy::FnDef(fn_def, _)) = func_ty.kind().rigid() {
                        let callee_name = fn_def.name();
                        if callee_name.contains("closure") || callee_name.contains(closure_name) {
                            output.push_str(&format!("      Callsite: block={}, terminator\n", bb_idx));

                            // Try to extract closure from arguments
                            for (arg_idx, arg) in args.iter().enumerate() {
                                if let Ok(arg_ty) = arg.ty(locals) {
                                    if let Some(RigidTy::Closure(closure_def, _)) = arg_ty.kind().rigid() {
                                        output.push_str(&format!("      Argument {}: Closure type {:?}\n", arg_idx, closure_def));
                                    }
                                }
                            }
                            return;
                        }
                    }
                }
            }
        }
        output.push_str("      Callsite: <not found>\n");
    }
}
