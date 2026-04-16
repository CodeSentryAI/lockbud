//! Panic detector ported to ULLBC.
//! Detects calls to panic APIs (unwrap, expect, panic!, assert!, etc.).

use charon_lib::ast::*;
use charon_lib::export::CrateData;
use charon_lib::ullbc_ast;
use rustc_hash::FxHashSet;

use crate::lockbud::report::{PanicDiagnosis, Report, ReportContent};
use crate::lockbud::types::Location;

fn format_name(name: &Name) -> String {
    name.name
        .iter()
        .map(|elem| match elem {
            PathElem::Ident(s, _) => s.clone(),
            PathElem::Impl(_) => "{impl}".to_string(),
            PathElem::Instantiated(_) => "{inst}".to_string(),
        })
        .collect::<Vec<_>>()
        .join("::")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanicApi {
    ResultUnwrap,
    ResultExpect,
    OptionUnwrap,
    OptionExpect,
    PanicFmt,
    AssertFailed,
    Panic,
}

fn classify_panic_api(func: &FnOperand, krate: &TranslatedCrate) -> Option<PanicApi> {
    match func {
        FnOperand::Regular(fn_ptr) => match fn_ptr.kind.as_ref() {
            FnPtrKind::Fun(FunId::Regular(fun_id)) => {
                let decl = krate.fun_decls.get(*fun_id)?;
                let name = format_name(&decl.item_meta.name);
                // Result/Option unwrap/expect
                if name.contains("Result") && name.contains("::unwrap") {
                    return Some(PanicApi::ResultUnwrap);
                }
                if name.contains("Result") && name.contains("::expect") {
                    return Some(PanicApi::ResultExpect);
                }
                if name.contains("Option") && name.contains("::unwrap") {
                    return Some(PanicApi::OptionUnwrap);
                }
                if name.contains("Option") && name.contains("::expect") {
                    return Some(PanicApi::OptionExpect);
                }
                // Panic/assert internals
                if name.contains("panic_fmt") || name.contains("panic_display") {
                    return Some(PanicApi::PanicFmt);
                }
                if name.contains("assert_failed") {
                    return Some(PanicApi::AssertFailed);
                }
                if name.contains("panicking::panic") && !name.contains("panic_fmt") {
                    return Some(PanicApi::Panic);
                }
                // Also catch `core::panicking::panic_*` and `std::panic::...` entry points
                if name.starts_with("core::panicking::panic_")
                    || name.starts_with("std::panicking::panic_")
                {
                    if name.contains("assert") {
                        return Some(PanicApi::AssertFailed);
                    }
                    return Some(PanicApi::Panic);
                }
                None
            }
            _ => None,
        },
        FnOperand::Dynamic(_) => None,
    }
}

fn panic_api_name(api: PanicApi) -> &'static str {
    match api {
        PanicApi::ResultUnwrap => "Result::unwrap",
        PanicApi::ResultExpect => "Result::expect",
        PanicApi::OptionUnwrap => "Option::unwrap",
        PanicApi::OptionExpect => "Option::expect",
        PanicApi::PanicFmt => "panic!",
        PanicApi::AssertFailed => "assert!",
        PanicApi::Panic => "panic",
    }
}

/// Detect panic calls in local functions.
pub fn detect_panics(crate_data: &CrateData) -> Vec<Report> {
    let mut reports = Vec::new();
    let krate = &crate_data.translated;
    let mut seen: FxHashSet<(FunDeclId, Location)> = FxHashSet::default();

    for (fun_id, fun_decl) in krate.fun_decls.iter_indexed() {
        let Body::Unstructured(body) = &fun_decl.body else {
            continue;
        };
        if !fun_decl.item_meta.is_local {
            continue;
        }
        for (block_id, block) in body.body.iter_indexed() {
            let term_loc = Location::new(block_id, block.statements.len());
            if let ullbc_ast::TerminatorKind::Call { call, .. } = &block.terminator.kind {
                if let Some(api) = classify_panic_api(&call.func, krate) {
                    if seen.insert((fun_id, term_loc)) {
                        let fn_name = format_name(&fun_decl.item_meta.name);
                        let span = block.terminator.span;
                        let diagnosis = PanicDiagnosis {
                            fn_name,
                            panic_api: panic_api_name(api).to_owned(),
                            callsite_span: format!("{:?}", span),
                        };
                        reports.push(Report::Panic(ReportContent::new(
                            "Panic".to_owned(),
                            "Possibly".to_owned(),
                            diagnosis,
                            "Call to a panic API that may panic at runtime".to_owned(),
                        )));
                    }
                }
            }
        }
    }

    reports
}
