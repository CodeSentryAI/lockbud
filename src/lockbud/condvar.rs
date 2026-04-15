//! Condvar API detection for std::sync::Condvar and parking_lot::Condvar.

use charon_lib::ast::*;
use charon_lib::export::CrateData;

use crate::lockbud::types::Location;

#[derive(Clone, Copy, Debug)]
pub enum CondvarApi {
    Std(StdCondvarApi),
    ParkingLot(ParkingLotCondvarApi),
}

#[derive(Clone, Copy, Debug)]
pub enum StdCondvarApi {
    Wait(StdWait),
    Notify(StdNotify),
}

#[derive(Clone, Copy, Debug)]
pub enum StdWait {
    Wait,
    WaitTimeout,
    WaitTimeoutMs,
    WaitTimeoutWhile,
    WaitWhile,
}

#[derive(Clone, Copy, Debug)]
pub enum StdNotify {
    NotifyAll,
    NotifyOne,
}

#[derive(Clone, Copy, Debug)]
pub enum ParkingLotCondvarApi {
    Wait(ParkingLotWait),
    Notify(ParkingLotNotify),
}

#[derive(Clone, Copy, Debug)]
pub enum ParkingLotWait {
    Wait,
    WaitFor,
    WaitUntil,
    WaitWhile,
    WaitWhileFor,
    WaitWhileUntil,
}

#[derive(Clone, Copy, Debug)]
pub enum ParkingLotNotify {
    NotifyAll,
    NotifyOne,
}

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

pub fn classify_condvar_api(func: &FnOperand, krate: &TranslatedCrate) -> Option<CondvarApi> {
    match func {
        FnOperand::Regular(fn_ptr) => match fn_ptr.kind.as_ref() {
            FnPtrKind::Fun(FunId::Regular(fun_id)) => {
                let decl = krate.fun_decls.get(*fun_id)?;
                let name = format_name(&decl.item_meta.name);
                const STD_CONDVAR: &str = "std::sync::Condvar::";
                const PARKING_LOT_CONDVAR: &str = "parking_lot::Condvar::";
                if let Some(tail) = name.strip_prefix(STD_CONDVAR) {
                    let api = if tail.starts_with("wait::") {
                        StdCondvarApi::Wait(StdWait::Wait)
                    } else if tail.starts_with("wait_timeout::") {
                        StdCondvarApi::Wait(StdWait::WaitTimeout)
                    } else if tail.starts_with("wait_timeout_ms::") {
                        StdCondvarApi::Wait(StdWait::WaitTimeoutMs)
                    } else if tail.starts_with("wait_timeout_while::") {
                        StdCondvarApi::Wait(StdWait::WaitTimeoutWhile)
                    } else if tail.starts_with("wait_while::") {
                        StdCondvarApi::Wait(StdWait::WaitWhile)
                    } else if tail == "notify_all" {
                        StdCondvarApi::Notify(StdNotify::NotifyAll)
                    } else if tail == "notify_one" {
                        StdCondvarApi::Notify(StdNotify::NotifyOne)
                    } else {
                        return None;
                    };
                    Some(CondvarApi::Std(api))
                } else if let Some(tail) = name.strip_prefix(PARKING_LOT_CONDVAR) {
                    let api = if tail.starts_with("wait::") {
                        ParkingLotCondvarApi::Wait(ParkingLotWait::Wait)
                    } else if tail.starts_with("wait_for::") {
                        ParkingLotCondvarApi::Wait(ParkingLotWait::WaitFor)
                    } else if tail.starts_with("wait_until::") {
                        ParkingLotCondvarApi::Wait(ParkingLotWait::WaitUntil)
                    } else if tail.starts_with("wait_while::") {
                        ParkingLotCondvarApi::Wait(ParkingLotWait::WaitWhile)
                    } else if tail.starts_with("wait_while_for::") {
                        ParkingLotCondvarApi::Wait(ParkingLotWait::WaitWhileFor)
                    } else if tail.starts_with("wait_while_until::") {
                        ParkingLotCondvarApi::Wait(ParkingLotWait::WaitWhileUntil)
                    } else if tail == "notify_all" {
                        ParkingLotCondvarApi::Notify(ParkingLotNotify::NotifyAll)
                    } else if tail == "notify_one" {
                        ParkingLotCondvarApi::Notify(ParkingLotNotify::NotifyOne)
                    } else {
                        return None;
                    };
                    Some(CondvarApi::ParkingLot(api))
                } else {
                    None
                }
            }
            _ => None,
        },
        FnOperand::Dynamic(_) => None,
    }
}

/// Collector for condvar API calls.
/// Maps (caller_fun_id, callsite_loc, callee_fun_id) -> CondvarApi.
pub type CondvarCallSites = rustc_hash::FxHashMap<(FunDeclId, Location, FunDeclId), CondvarApi>;

pub fn collect_condvar_callsites(krate: &CrateData) -> CondvarCallSites {
    let mut result = CondvarCallSites::default();
    let krate = &krate.translated;
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
                if let Some(api) = classify_condvar_api(&call.func, krate) {
                    if let FnOperand::Regular(fn_ptr) = &call.func {
                        if let FnPtrKind::Fun(FunId::Regular(callee_id)) = fn_ptr.kind.as_ref() {
                            result.insert((fun_id, term_loc, *callee_id), api);
                        }
                    }
                }
            }
        }
    }
    result
}
