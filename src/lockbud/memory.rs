//! Memory bug detector ported to ULLBC.
//! Detects InvalidFree and UseAfterFree patterns.

use charon_lib::ast::*;
use charon_lib::export::CrateData;
use charon_lib::ullbc_ast;
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::VecDeque;

use crate::lockbud::report::{InvalidFreeDiagnosis, Report, ReportContent, UseAfterFreeDiagnosis};
use crate::lockbud::types::Location;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UninitApi {
    Uninitialized,
    MaybeUninit,
    AssumeInit,
    MaybeUninitWrite,
    PtrWrite,
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

fn classify_uninit_api(func: &FnOperand, krate: &TranslatedCrate) -> Option<UninitApi> {
    match func {
        FnOperand::Regular(fn_ptr) => match fn_ptr.kind.as_ref() {
            FnPtrKind::Fun(FunId::Regular(fun_id)) => {
                let decl = krate.fun_decls.get(*fun_id)?;
                let name = format_name(&decl.item_meta.name);
                if name.contains("MaybeUninit")
                    && (name.contains("::uninit") || name.contains("::zeroed"))
                {
                    Some(UninitApi::MaybeUninit)
                } else if name.contains("MaybeUninit") && name.contains("::write") {
                    Some(UninitApi::MaybeUninitWrite)
                } else if name.contains("MaybeUninit") && name.contains("::assume_init") {
                    Some(UninitApi::AssumeInit)
                } else if name.contains("mem::uninitialized") || name.contains("mem::zeroed") {
                    Some(UninitApi::Uninitialized)
                } else if name.contains("as_mut_ptr") && name.contains("MaybeUninit") {
                    Some(UninitApi::PtrWrite)
                } else {
                    None
                }
            }
            _ => None,
        },
        FnOperand::Dynamic(_) => None,
    }
}

fn is_mem_drop(func: &FnOperand, krate: &TranslatedCrate) -> bool {
    match func {
        FnOperand::Regular(fn_ptr) => match fn_ptr.kind.as_ref() {
            FnPtrKind::Fun(FunId::Regular(fun_id)) => {
                let Some(decl) = krate.fun_decls.get(*fun_id) else {
                    return false;
                };
                let name = format_name(&decl.item_meta.name);
                name.starts_with("std::mem::drop") || name.starts_with("core::mem::drop")
            }
            _ => false,
        },
        FnOperand::Dynamic(_) => false,
    }
}

fn is_drop_in_place(func: &FnOperand, krate: &TranslatedCrate) -> bool {
    match func {
        FnOperand::Regular(fn_ptr) => match fn_ptr.kind.as_ref() {
            FnPtrKind::Fun(FunId::Regular(fun_id)) => {
                let Some(decl) = krate.fun_decls.get(*fun_id) else {
                    return false;
                };
                let name = format_name(&decl.item_meta.name);
                name.contains("ptr::drop_in_place")
            }
            _ => false,
        },
        FnOperand::Dynamic(_) => false,
    }
}

/// Information about a drop (auto or manual).
#[derive(Clone, Debug)]
pub struct DropSite {
    pub loc: Location,
    pub place: Place,
}

/// Collect all drop sites in a function: auto drops (TerminatorKind::Drop) and manual drops (mem::drop calls).
fn collect_drops(fun_decl: &FunDecl, krate: &TranslatedCrate) -> Vec<DropSite> {
    let mut drops = Vec::new();
    let Body::Unstructured(body) = &fun_decl.body else {
        return drops;
    };
    for (block_id, block) in body.body.iter_indexed() {
        let term_loc = Location::new(block_id, block.statements.len());
        match &block.terminator.kind {
            ullbc_ast::TerminatorKind::Drop { place, .. } => {
                drops.push(DropSite {
                    loc: term_loc,
                    place: place.clone(),
                });
            }
            ullbc_ast::TerminatorKind::Call { call, .. } => {
                if is_mem_drop(&call.func, krate) {
                    if let Some(Operand::Move(p) | Operand::Copy(p)) = call.args.first() {
                        drops.push(DropSite {
                            loc: term_loc,
                            place: p.clone(),
                        });
                    }
                } else if is_drop_in_place(&call.func, krate) {
                    if let Some(Operand::Move(p) | Operand::Copy(p)) = call.args.first() {
                        drops.push(DropSite {
                            loc: term_loc,
                            place: p.clone(),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    drops
}

/// Check if a type is "simple" (scalar/primitive).
fn is_simple_ty(ty: &Ty) -> bool {
    match ty.kind() {
        TyKind::Literal(lit_ty) => match lit_ty {
            LiteralTy::Bool
            | LiteralTy::Char
            | LiteralTy::Int(_)
            | LiteralTy::UInt(_)
            | LiteralTy::Float(_) => true,
        },
        TyKind::Ref(_, inner, _) => is_simple_ty(inner),
        TyKind::RawPtr(inner, _) => is_simple_ty(inner),
        _ => false,
    }
}

/// A callsite of an uninit API.
#[derive(Clone, Debug)]
pub struct UninitCallSite {
    pub api: UninitApi,
    pub loc: Location,
    pub dest: Option<Place>,
    pub first_arg: Option<Place>,
}

pub type UninitCallSites = FxHashMap<FunDeclId, Vec<UninitCallSite>>;

pub fn collect_uninit_callsites(crate_data: &CrateData) -> UninitCallSites {
    let mut result = UninitCallSites::default();
    let krate = &crate_data.translated;
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
                if let Some(api) = classify_uninit_api(&call.func, krate) {
                    let dest = call.dest.local_id().map(|l| Place {
                        kind: PlaceKind::Local(l),
                        ty: call.dest.ty().clone(),
                    });
                    let first_arg = call.args.first().and_then(|op| match op {
                        Operand::Copy(p) | Operand::Move(p) => Some(p.clone()),
                        Operand::Const(_) => None,
                    });
                    result.entry(fun_id).or_default().push(UninitCallSite {
                        api,
                        loc: term_loc,
                        dest,
                        first_arg,
                    });
                }
            }
        }
    }
    result
}

pub fn detect_invalid_free(
    crate_data: &CrateData,
    uninit_callsites: &UninitCallSites,
) -> Vec<Report> {
    let mut reports = Vec::new();
    let krate = &crate_data.translated;

    for (fun_id, callsites) in uninit_callsites {
        let Some(fun_decl) = krate.fun_decls.get(*fun_id) else {
            continue;
        };
        let Body::Unstructured(body) = &fun_decl.body else {
            continue;
        };
        let drops = collect_drops(fun_decl, krate);

        // Process mem::uninitialized() calls.
        for site in callsites.iter().filter(|s| matches!(s.api, UninitApi::Uninitialized)) {
            if let Some(dest) = &site.dest {
                if is_simple_ty(&dest.ty) {
                    continue;
                }
                // Check if any drop aliases with dest.
                if any_drop_aliases(dest, &drops, body) {
                    let span = span_for_loc(body, site.loc);
                    let ty_str = format!("{:?}", dest.ty);
                    let diagnosis = InvalidFreeDiagnosis {
                        ty: ty_str,
                        uninit_span: format!("{:?}", span),
                        assume_init_span: None,
                    };
                    reports.push(Report::InvalidFree(ReportContent::new(
                        "InvalidFree".to_owned(),
                        "Possibly".to_owned(),
                        diagnosis,
                        "Call mem::uninitialized() on a non-simple type and the value is dropped"
                            .to_owned(),
                    )));
                }
            }
        }

        // Collect MaybeUninit-related calls.
        let maybe_uninits: Vec<_> = callsites
            .iter()
            .filter(|s| matches!(s.api, UninitApi::MaybeUninit))
            .collect();
        let writes: Vec<_> = callsites
            .iter()
            .filter(|s| matches!(s.api, UninitApi::MaybeUninitWrite | UninitApi::PtrWrite))
            .collect();
        let assume_inits: Vec<_> = callsites
            .iter()
            .filter(|s| matches!(s.api, UninitApi::AssumeInit))
            .collect();

        for uninit in &maybe_uninits {
            let Some(uninit_dest) = &uninit.dest else { continue };
            for assume in &assume_inits {
                let Some(assume_dest) = &assume.dest else { continue };
                let Some(assume_arg) = &assume.first_arg else { continue };
                // Types should match (simple check: same type name string or exact match).
                if uninit_dest.ty != assume_arg.ty {
                    continue;
                }
                if is_simple_ty(&assume_dest.ty) {
                    continue;
                }
                // Check if any drop aliases with the assume_init result.
                if !any_drop_aliases(assume_dest, &drops, body) {
                    continue;
                }
                // Check if there is a write in between on the same object.
                let has_write_between = writes.iter().any(|w| {
                    let Some(write_arg) = &w.first_arg else { return false };
                    // Write arg should alias with the uninit dest.
                    places_may_alias(write_arg, uninit_dest)
                        && is_reachable(uninit.loc, w.loc, body)
                        && is_reachable(w.loc, assume.loc, body)
                });
                if has_write_between {
                    continue;
                }
                let uninit_span = span_for_loc(body, uninit.loc);
                let assume_span = span_for_loc(body, assume.loc);
                let diagnosis = InvalidFreeDiagnosis {
                    ty: format!("{:?}", assume_dest.ty),
                    uninit_span: format!("{:?}", uninit_span),
                    assume_init_span: Some(format!("{:?}", assume_span)),
                };
                reports.push(Report::InvalidFree(ReportContent::new(
                    "InvalidFree".to_owned(),
                    "Possibly".to_owned(),
                    diagnosis,
                    "MaybeUninit::uninit() followed by assume_init() without write on non-simple type, and dropped".to_owned(),
                )));
            }
        }
    }

    reports
}

fn any_drop_aliases(place: &Place, drops: &[DropSite], body: &ullbc_ast::ExprBody) -> bool {
    let start_local = match place.local_id() {
        Some(l) => l,
        None => return false,
    };
    let reachable = assignment_closure(start_local, body);
    drops.iter().any(|d| match d.place.local_id() {
        Some(l) => reachable.contains(&l),
        None => false,
    })
}

/// Compute all locals that `start` flows into via simple assignments
/// (`dest = Use(Move(src))` or `dest = Use(Copy(src))`), `Ref`, `RawPtr`,
/// and `Aggregate` operands.
fn assignment_closure(start: LocalId, body: &ullbc_ast::ExprBody) -> FxHashSet<LocalId> {
    let mut closure = FxHashSet::default();
    closure.insert(start);
    let mut changed = true;
    while changed {
        changed = false;
        for block in &body.body {
            for stmt in &block.statements {
                if let ullbc_ast::StatementKind::Assign(dest, rvalue) = &stmt.kind {
                    let Some(dest_local) = dest.local_id() else { continue };
                    if closure.contains(&dest_local) {
                        continue;
                    }
                    let src_locals: Vec<LocalId> = match rvalue {
                        ullbc_ast::Rvalue::Use(Operand::Move(p) | Operand::Copy(p)) => {
                            p.local_id().into_iter().collect()
                        }
                        ullbc_ast::Rvalue::Ref { place, .. } => {
                            place.local_id().into_iter().collect()
                        }
                        ullbc_ast::Rvalue::RawPtr { place, .. } => {
                            place.local_id().into_iter().collect()
                        }
                        ullbc_ast::Rvalue::Aggregate(_, ops) => ops
                            .iter()
                            .filter_map(|op| match op {
                                Operand::Move(p) | Operand::Copy(p) => p.local_id(),
                                _ => None,
                            })
                            .collect(),
                        _ => vec![],
                    };
                    if src_locals.iter().any(|src| closure.contains(src)) {
                        closure.insert(dest_local);
                        changed = true;
                    }
                }
            }
        }
    }
    closure
}

/// Very simple alias check: exact match, or same base local.
fn places_may_alias(a: &Place, b: &Place) -> bool {
    if a == b {
        return true;
    }
    match (&a.kind, &b.kind) {
        (PlaceKind::Local(la), PlaceKind::Local(lb)) => la == lb,
        _ => false,
    }
}

fn span_for_loc(body: &ullbc_ast::ExprBody, loc: Location) -> Span {
    let block = &body.body[loc.block];
    if loc.statement_index < block.statements.len() {
        block.statements[loc.statement_index].span
    } else {
        block.terminator.span
    }
}

fn is_reachable(from: Location, to: Location, body: &ullbc_ast::ExprBody) -> bool {
    if from.block == to.block {
        return from.statement_index <= to.statement_index;
    }
    let mut visited = FxHashSet::default();
    let mut queue = VecDeque::new();
    queue.push_back(from.block);
    visited.insert(from.block);
    while let Some(curr) = queue.pop_front() {
        if curr == to.block {
            return true;
        }
        let block = &body.body[curr];
        for succ in terminator_successors(&block.terminator.kind) {
            if visited.insert(*succ) {
                queue.push_back(*succ);
            }
        }
    }
    false
}

fn terminator_successors(term: &ullbc_ast::TerminatorKind) -> Vec<&ullbc_ast::BlockId> {
    use ullbc_ast::TerminatorKind::*;
    match term {
        Goto { target } => vec![target],
        Switch { targets, .. } => match targets {
            ullbc_ast::SwitchTargets::If(t1, t2) => vec![t1, t2],
            ullbc_ast::SwitchTargets::SwitchInt(_, vals, default) => {
                let mut v: Vec<_> = vals.iter().map(|(_, b)| b).collect();
                v.push(default);
                v
            }
        },
        Call { target, on_unwind, .. } => vec![target, on_unwind],
        Drop { target, on_unwind, .. } => vec![target, on_unwind],
        Assert { target, on_unwind, .. } => vec![target, on_unwind],
        Return | Abort(_) | UnwindResume => vec![],
    }
}

// ============================================================================
// UseAfterFree detector (simplified intra-procedural)
// ============================================================================

/// Detect use-after-free by tracking raw ptr locals and their uses after drops.
pub fn detect_use_after_free(crate_data: &CrateData) -> Vec<Report> {
    let mut reports = Vec::new();
    let krate = &crate_data.translated;

    for (fun_id, fun_decl) in krate.fun_decls.iter_indexed() {
        let Body::Unstructured(body) = &fun_decl.body else {
            continue;
        };
        if !fun_decl.item_meta.is_local {
            continue;
        }

        // Collect raw ptr locals.
        let raw_ptrs: FxHashSet<LocalId> = body
            .locals
            .locals
            .iter_indexed()
            .filter_map(|(local, ldecl)| {
                if is_raw_ptr(&ldecl.ty) {
                    Some(local)
                } else {
                    None
                }
            })
            .collect();
        if raw_ptrs.is_empty() {
            continue;
        }

        let drops = collect_drops(fun_decl, krate);

        // Build use map for each local.
        let uses = collect_uses(body);

        for raw_local in &raw_ptrs {
            // Find what this raw ptr points to by tracking assignments.
            let pointees = find_pointees(*raw_local, body);
            for pointee in &pointees {
                for drop in &drops {
                    let mut aliases = places_may_alias(pointee, &drop.place);
                    if !aliases {
                        if let Some(drop_local) = drop.place.local_id() {
                            let drop_pointees = resolve_drop_pointees(drop_local, body);
                            for dp in &drop_pointees {
                                if places_may_alias(pointee, dp) {
                                    aliases = true;
                                    break;
                                }
                            }
                        }
                    }
                    if aliases {
                        // Find uses of raw_local after the drop.
                        let raw_uses = uses.get(raw_local).cloned().unwrap_or_default();
                        for use_loc in raw_uses {
                            if is_reachable(drop.loc, use_loc, body) {
                                let use_span = span_for_loc(body, use_loc);
                                let drop_span = span_for_loc(body, drop.loc);
                                let diagnosis = UseAfterFreeDiagnosis {
                                    raw_ptr_local: raw_local.index(),
                                    use_span: format!("{:?}", use_span),
                                    drop_span: format!("{:?}", drop_span),
                                    explanation:
                                        "Raw ptr is used after the pointed value is dropped"
                                            .to_owned(),
                                };
                                reports.push(Report::UseAfterFree(ReportContent::new(
                                    "UseAfterFree".to_owned(),
                                    "Possibly".to_owned(),
                                    diagnosis,
                                    "Raw ptr is used after the pointed value is dropped"
                                        .to_owned(),
                                )));
                            }
                        }
                    }
                }
            }
        }
    }

    reports
}

fn is_raw_ptr(ty: &Ty) -> bool {
    matches!(ty.kind(), TyKind::RawPtr(_, _))
}

/// Collect all use locations for each local.
fn collect_uses(body: &ullbc_ast::ExprBody) -> FxHashMap<LocalId, Vec<Location>> {
    let mut uses: FxHashMap<LocalId, Vec<Location>> = FxHashMap::default();
    for (block_id, block) in body.body.iter_indexed() {
        for (stmt_idx, stmt) in block.statements.iter().enumerate() {
            let loc = Location::new(block_id, stmt_idx);
            match &stmt.kind {
                ullbc_ast::StatementKind::Assign(_, rvalue) => {
                    collect_rvalue_locals(rvalue, &mut uses, loc);
                }
                _ => {}
            }
        }
        let term_loc = Location::new(block_id, block.statements.len());
        match &block.terminator.kind {
            ullbc_ast::TerminatorKind::Call { call, .. } => {
                for arg in &call.args {
                    if let Some(local) = operand_local(arg) {
                        uses.entry(local).or_default().push(term_loc);
                    }
                }
            }
            ullbc_ast::TerminatorKind::Switch { discr, .. } => {
                if let Some(local) = operand_local(discr) {
                    uses.entry(local).or_default().push(term_loc);
                }
            }
            ullbc_ast::TerminatorKind::Drop { place, .. } => {
                if let Some(local) = place.local_id() {
                    uses.entry(local).or_default().push(term_loc);
                }
            }
            _ => {}
        }
    }
    uses
}

fn collect_rvalue_locals(
    rvalue: &ullbc_ast::Rvalue,
    uses: &mut FxHashMap<LocalId, Vec<Location>>,
    loc: Location,
) {
    match rvalue {
        ullbc_ast::Rvalue::Use(op) => {
            if let Some(local) = operand_local(op) {
                uses.entry(local).or_default().push(loc);
            }
        }
        ullbc_ast::Rvalue::Ref { place, .. } => {
            if let Some(local) = place.local_id() {
                uses.entry(local).or_default().push(loc);
            }
        }
        ullbc_ast::Rvalue::BinaryOp(_, op1, op2) => {
            if let Some(local) = operand_local(op1) {
                uses.entry(local).or_default().push(loc);
            }
            if let Some(local) = operand_local(op2) {
                uses.entry(local).or_default().push(loc);
            }
        }
        ullbc_ast::Rvalue::UnaryOp(_, op) => {
            if let Some(local) = operand_local(op) {
                uses.entry(local).or_default().push(loc);
            }
        }
        ullbc_ast::Rvalue::Aggregate(_, ops) => {
            for op in ops {
                if let Some(local) = operand_local(op) {
                    uses.entry(local).or_default().push(loc);
                }
            }
        }
        ullbc_ast::Rvalue::RawPtr { place, .. } => {
            if let Some(local) = place.local_id() {
                uses.entry(local).or_default().push(loc);
            }
        }
        ullbc_ast::Rvalue::Repeat(op, _, _) => {
            if let Some(local) = operand_local(op) {
                uses.entry(local).or_default().push(loc);
            }
        }
        ullbc_ast::Rvalue::Discriminant(place) => {
            if let Some(local) = place.local_id() {
                uses.entry(local).or_default().push(loc);
            }
        }
        ullbc_ast::Rvalue::Len(place, _, _) => {
            if let Some(local) = place.local_id() {
                uses.entry(local).or_default().push(loc);
            }
        }
        ullbc_ast::Rvalue::NullaryOp(_, _) => {}
        ullbc_ast::Rvalue::ShallowInitBox(op, _) => {
            if let Some(local) = operand_local(op) {
                uses.entry(local).or_default().push(loc);
            }
        }
    }
}

fn operand_local(op: &Operand) -> Option<LocalId> {
    match op {
        Operand::Copy(place) | Operand::Move(place) => place.local_id(),
        Operand::Const(_) => None,
    }
}

/// Find places that a raw ptr local may point to by scanning assignments.
fn find_pointees(raw_local: LocalId, body: &ullbc_ast::ExprBody) -> Vec<Place> {
    let mut pointees = Vec::new();
    for block in &body.body {
        for stmt in &block.statements {
            if let ullbc_ast::StatementKind::Assign(dest, rvalue) = &stmt.kind {
                if let Some(dest_local) = dest.local_id() {
                    if dest_local == raw_local {
                        if let ullbc_ast::Rvalue::Ref { place, .. } = rvalue {
                            pointees.push(place.clone());
                        }
                        if let ullbc_ast::Rvalue::RawPtr { place, .. } = rvalue {
                            pointees.push(place.clone());
                        }
                        if let ullbc_ast::Rvalue::Use(Operand::Copy(p) | Operand::Move(p)) = rvalue
                        {
                            // Propagate through copy/move.
                            pointees.push(p.clone());
                        }
                    }
                }
            }
        }
        // Terminators can also assign to the raw_local.
        if let ullbc_ast::TerminatorKind::Call { call, .. } = &block.terminator.kind {
            if let Some(dest_local) = call.dest.local_id() {
                if dest_local == raw_local {
                    // Heuristic: first arg of functions returning raw ptrs is often the source.
                    if let Some(Operand::Copy(p) | Operand::Move(p)) = call.args.first() {
                        pointees.push(p.clone());
                    }
                }
            }
        }
    }
    pointees
}

/// Resolve what a drop-local (often a pointer/referential local in ULLBC) points to.
/// Charon may lower `drop(x)` into `drop(ptr_to_x)`, so we trace ptr/ref origins.
fn resolve_drop_pointees(drop_local: LocalId, body: &ullbc_ast::ExprBody) -> Vec<Place> {
    let mut pointees = Vec::new();
    for block in &body.body {
        for stmt in &block.statements {
            if let ullbc_ast::StatementKind::Assign(dest, rvalue) = &stmt.kind {
                if let Some(dest_local) = dest.local_id() {
                    if dest_local == drop_local {
                        if let ullbc_ast::Rvalue::Ref { place, .. } = rvalue {
                            pointees.push(place.clone());
                        }
                        if let ullbc_ast::Rvalue::RawPtr { place, .. } = rvalue {
                            pointees.push(place.clone());
                        }
                    }
                }
            }
        }
        if let ullbc_ast::TerminatorKind::Call { call, .. } = &block.terminator.kind {
            if let Some(dest_local) = call.dest.local_id() {
                if dest_local == drop_local {
                    if let Some(Operand::Copy(p) | Operand::Move(p)) = call.args.first() {
                        pointees.push(p.clone());
                    }
                }
            }
        }
    }
    pointees
}
