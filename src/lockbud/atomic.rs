//! Atomicity violation detector ported to ULLBC.
//! Detects patterns where an atomic store is control/data dependent on an atomic load
//! on the same object, without an intervening read-write (compare_exchange) operation.

use charon_lib::ast::*;
use charon_lib::export::CrateData;
use charon_lib::ullbc_ast;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::lockbud::callgraph::CallGraph;
use crate::lockbud::report::{AtomicityViolationDiagnosis, Report, ReportContent};
use crate::lockbud::types::Location;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtomicApi {
    Read,
    Write,
    ReadWrite,
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

fn classify_atomic_api(func: &FnOperand, krate: &TranslatedCrate) -> Option<AtomicApi> {
    match func {
        FnOperand::Regular(fn_ptr) => match fn_ptr.kind.as_ref() {
            FnPtrKind::Fun(FunId::Regular(fun_id)) => {
                let decl = krate.fun_decls.get(*fun_id)?;
                let name = format_name(&decl.item_meta.name);
                if !name.starts_with("std::sync::atomic") && !name.starts_with("core::sync::atomic")
                {
                    return None;
                }
                if name.contains("::load") {
                    Some(AtomicApi::Read)
                } else if name.contains("::store") {
                    Some(AtomicApi::Write)
                } else if name.contains("compare") || name.contains("fetch_") {
                    Some(AtomicApi::ReadWrite)
                } else {
                    None
                }
            }
            _ => None,
        },
        FnOperand::Dynamic(_) => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DependenceKind {
    Control,
    Data,
    Both,
}

/// A callsite of an atomic API.
#[derive(Clone, Debug)]
pub struct AtomicCallSite {
    pub api: AtomicApi,
    pub loc: Location,
    /// The place of the atomic receiver (self arg).
    pub receiver: Option<Place>,
    /// The destination local of the call.
    pub dest: Option<LocalId>,
    /// The value operand for store (if write).
    pub value_op: Option<Operand>,
}

/// Collect atomic API callsites per function.
pub type AtomicCallSites = FxHashMap<FunDeclId, Vec<AtomicCallSite>>;

pub fn collect_atomic_callsites(crate_data: &CrateData) -> AtomicCallSites {
    let mut result = AtomicCallSites::default();
    let krate = &crate_data.translated;
    for (fun_id, fun_decl) in krate.fun_decls.iter_indexed() {
        let Body::Unstructured(body) = &fun_decl.body else {
            continue;
        };
        if !fun_decl.item_meta.is_local {
            continue;
        }
        let ref_places = build_ref_places(body);
        for (block_id, block) in body.body.iter_indexed() {
            let term_loc = Location::new(block_id, block.statements.len());
            if let ullbc_ast::TerminatorKind::Call { call, .. } = &block.terminator.kind {
                if let Some(api) = classify_atomic_api(&call.func, krate) {
                    let receiver = call.args.first().and_then(|arg| match arg {
                        Operand::Copy(p) | Operand::Move(p) => {
                            Some(resolve_place_through_refs(p, &ref_places))
                        }
                        Operand::Const(_) => None,
                    });
                    let dest = call.dest.local_id();
                    let value_op = if matches!(api, AtomicApi::Write) {
                        call.args.get(1).cloned()
                    } else {
                        None
                    };
                    result.entry(fun_id).or_default().push(AtomicCallSite {
                        api,
                        loc: term_loc,
                        receiver,
                        dest,
                        value_op,
                    });
                }
            }
        }
    }
    result
}

fn build_ref_places(body: &ullbc_ast::ExprBody) -> FxHashMap<LocalId, Place> {
    let mut map = FxHashMap::default();
    for block in &body.body {
        for stmt in &block.statements {
            if let ullbc_ast::StatementKind::Assign(dest, rvalue) = &stmt.kind {
                if let Some(dest_local) = dest.local_id() {
                    if let ullbc_ast::Rvalue::Ref { place, .. } = rvalue {
                        map.insert(dest_local, place.clone());
                    }
                }
            }
        }
    }
    map
}

fn resolve_place_through_refs(place: &Place, ref_places: &FxHashMap<LocalId, Place>) -> Place {
    match &place.kind {
        PlaceKind::Local(local) => {
            if let Some(underlying) = ref_places.get(local) {
                resolve_place_through_refs(underlying, ref_places)
            } else {
                place.clone()
            }
        }
        PlaceKind::Projection(inner, elem) => {
            let resolved = resolve_place_through_refs(inner, ref_places);
            Place {
                kind: PlaceKind::Projection(Box::new(resolved), elem.clone()),
                ty: place.ty.clone(),
            }
        }
        PlaceKind::Global(_) => place.clone(),
    }
}

/// Detect atomicity violations intra- and inter-procedurally.
pub fn detect_atomicity_violations(
    crate_data: &CrateData,
    atomic_callsites: &AtomicCallSites,
    callgraph: &CallGraph,
) -> Vec<Report> {
    let mut reports = Vec::new();
    let krate = &crate_data.translated;
    let mut reported_pairs: FxHashSet<((FunDeclId, Location), (FunDeclId, Location))> =
        FxHashSet::default();

    // Phase 1: Intra-procedural analysis.
    for (fun_id, callsites) in atomic_callsites {
        let Some(fun_decl) = krate.fun_decls.get(*fun_id) else {
            continue;
        };
        let Body::Unstructured(body) = &fun_decl.body else {
            continue;
        };

        let reads: Vec<_> = callsites
            .iter()
            .filter(|c| matches!(c.api, AtomicApi::Read))
            .collect();
        let writes: Vec<_> = callsites
            .iter()
            .filter(|c| matches!(c.api, AtomicApi::Write))
            .collect();
        let read_writes: Vec<_> = callsites
            .iter()
            .filter(|c| matches!(c.api, AtomicApi::ReadWrite))
            .collect();

        let def_use = build_def_use(body);
        let dominance = build_dominance(body);

        for read in &reads {
            for write in &writes {
                if !same_receiver(read.receiver.as_ref(), write.receiver.as_ref()) {
                    continue;
                }

                let Some(read_dest) = read.dest else { continue };
                let dep = compute_dependence(
                    read_dest,
                    read.loc,
                    write.loc,
                    write.value_op.as_ref(),
                    body,
                    &def_use,
                    &dominance,
                );
                let Some(dep_kind) = dep else { continue };

                let has_interlock = read_writes.iter().any(|rw| {
                    same_receiver(rw.receiver.as_ref(), read.receiver.as_ref())
                        && is_between(rw.loc, read.loc, write.loc, body, &dominance)
                });
                if has_interlock {
                    continue;
                }

                reported_pairs.insert(((*fun_id, read.loc), (*fun_id, write.loc)));

                let fn_name = format_name(&fun_decl.item_meta.name);
                let read_span = span_for_loc(body, read.loc);
                let write_span = span_for_loc(body, write.loc);
                let diagnosis = AtomicityViolationDiagnosis {
                    fn_name,
                    atomic_reader: format!("{:?}", read_span),
                    atomic_writer: format!("{:?}", write_span),
                    dep_kind: format!("{:?}", dep_kind),
                };
                reports.push(Report::AtomicityViolation(ReportContent::new(
                    "AtomicityViolation".to_owned(),
                    "Possibly".to_owned(),
                    diagnosis,
                    "atomic::store is data/control dependent on atomic::load".to_owned(),
                )));
            }
        }
    }

    // Phase 2: Inter-procedural analysis.
    // Collect all reads and writes globally.
    let mut all_reads: Vec<(FunDeclId, &AtomicCallSite)> = Vec::new();
    let mut all_writes: Vec<(FunDeclId, &AtomicCallSite)> = Vec::new();
    let mut all_read_writes: Vec<(FunDeclId, &AtomicCallSite)> = Vec::new();

    for (fun_id, callsites) in atomic_callsites {
        for c in callsites {
            match c.api {
                AtomicApi::Read => all_reads.push((*fun_id, c)),
                AtomicApi::Write => all_writes.push((*fun_id, c)),
                AtomicApi::ReadWrite => all_read_writes.push((*fun_id, c)),
            }
        }
    }

    for (read_fun, read) in &all_reads {
        for (write_fun, write) in &all_writes {
            if read_fun == write_fun {
                continue; // already handled intra-procedurally
            }
            if !same_receiver(read.receiver.as_ref(), write.receiver.as_ref()) {
                continue;
            }
            let pair = ((*read_fun, read.loc), (*write_fun, write.loc));
            if reported_pairs.contains(&pair) {
                continue;
            }
            // Check callgraph reachability.
            if !callgraph.can_reach(*read_fun, *write_fun)
                && !callgraph.can_reach(*write_fun, *read_fun)
                && !callgraph.share_common_ancestor(*read_fun, *write_fun)
            {
                continue;
            }
            // Conservatively skip if there's an inter-procedural read-write on the same receiver.
            // We approximate by checking if any read-write function is reachable from either side.
            let has_interlock = all_read_writes.iter().any(|(rw_fun, rw)| {
                same_receiver(rw.receiver.as_ref(), read.receiver.as_ref())
                    && (callgraph.can_reach(*read_fun, *rw_fun)
                        || callgraph.can_reach(*write_fun, *rw_fun)
                        || callgraph.can_reach(*rw_fun, *read_fun)
                        || callgraph.can_reach(*rw_fun, *write_fun))
            });
            if has_interlock {
                continue;
            }

            reported_pairs.insert(pair);

            let read_fn_decl = krate.fun_decls.get(*read_fun);
            let write_fn_decl = krate.fun_decls.get(*write_fun);
            let fn_name = format!(
                "{} -> {}",
                read_fn_decl.map(|d| format_name(&d.item_meta.name)).unwrap_or_default(),
                write_fn_decl.map(|d| format_name(&d.item_meta.name)).unwrap_or_default()
            );
            let read_span = read_fn_decl
                .and_then(|d| {
                    if let Body::Unstructured(body) = &d.body {
                        Some(format!("{:?}", span_for_loc(body, read.loc)))
                    } else {
                        None
                    }
                })
                .unwrap_or_default();
            let write_span = write_fn_decl
                .and_then(|d| {
                    if let Body::Unstructured(body) = &d.body {
                        Some(format!("{:?}", span_for_loc(body, write.loc)))
                    } else {
                        None
                    }
                })
                .unwrap_or_default();
            let diagnosis = AtomicityViolationDiagnosis {
                fn_name,
                atomic_reader: read_span,
                atomic_writer: write_span,
                dep_kind: "InterProc".to_owned(),
            };
            reports.push(Report::AtomicityViolation(ReportContent::new(
                "AtomicityViolation".to_owned(),
                "Possibly".to_owned(),
                diagnosis,
                "atomic::store in callee is reachable from atomic::load in caller".to_owned(),
            )));
        }
    }

    reports
}

fn same_receiver(a: Option<&Place>, b: Option<&Place>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a == b,
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

/// Build a map of local -> locations where it is defined/assigned.
fn build_def_use(body: &ullbc_ast::ExprBody) -> FxHashMap<LocalId, Vec<Location>> {
    let mut map: FxHashMap<LocalId, Vec<Location>> = FxHashMap::default();
    for (block_id, block) in body.body.iter_indexed() {
        for (stmt_idx, stmt) in block.statements.iter().enumerate() {
            let loc = Location::new(block_id, stmt_idx);
            if let ullbc_ast::StatementKind::Assign(dest, _) = &stmt.kind {
                if let Some(local) = dest.local_id() {
                    map.entry(local).or_default().push(loc);
                }
            }
        }
        // Terminator calls also define their dest.
        let term_loc = Location::new(block_id, block.statements.len());
        if let ullbc_ast::TerminatorKind::Call { call, .. } = &block.terminator.kind {
            if let Some(local) = call.dest.local_id() {
                map.entry(local).or_default().push(term_loc);
            }
        }
    }
    map
}

/// Simple dominance information: block -> set of blocks that dominate it.
fn build_dominance(body: &ullbc_ast::ExprBody) -> FxHashMap<ullbc_ast::BlockId, FxHashSet<ullbc_ast::BlockId>> {
    let mut dom = FxHashMap::default();
    let all_blocks: FxHashSet<_> = body.body.iter_indexed().map(|(id, _)| id).collect();
    for (id, _) in body.body.iter_indexed() {
        if id == ullbc_ast::START_BLOCK_ID {
            let mut set = FxHashSet::default();
            set.insert(id);
            dom.insert(id, set);
        } else {
            dom.insert(id, all_blocks.clone());
        }
    }
    let preds = build_predecessors(body);
    let mut changed = true;
    while changed {
        changed = false;
        for (id, _) in body.body.iter_indexed() {
            if id == ullbc_ast::START_BLOCK_ID {
                continue;
            }
            let pred_sets: Vec<_> = preds
                .get(&id)
                .into_iter()
                .flat_map(|v| v.iter())
                .filter_map(|p| dom.get(p).cloned())
                .collect();
            if let Some(first) = pred_sets.first() {
                let mut new_set = first.clone();
                for s in &pred_sets[1..] {
                    new_set.retain(|x| s.contains(x));
                }
                new_set.insert(id);
                let old = dom.get(&id).cloned().unwrap_or_default();
                if old != new_set {
                    dom.insert(id, new_set);
                    changed = true;
                }
            }
        }
    }
    dom
}

fn build_predecessors(body: &ullbc_ast::ExprBody) -> FxHashMap<ullbc_ast::BlockId, Vec<ullbc_ast::BlockId>> {
    let mut preds: FxHashMap<ullbc_ast::BlockId, Vec<ullbc_ast::BlockId>> = FxHashMap::default();
    for (block_id, block) in body.body.iter_indexed() {
        for succ in terminator_successors(&block.terminator.kind) {
            preds.entry(*succ).or_default().push(block_id);
        }
    }
    preds
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

/// Compute whether the write at `write_loc` is data/control dependent on `read_local` defined at `read_loc`.
fn compute_dependence(
    read_local: LocalId,
    read_loc: Location,
    write_loc: Location,
    write_value: Option<&Operand>,
    body: &ullbc_ast::ExprBody,
    def_use: &FxHashMap<LocalId, Vec<Location>>,
    dominance: &FxHashMap<ullbc_ast::BlockId, FxHashSet<ullbc_ast::BlockId>>,
) -> Option<DependenceKind> {
    // Data dependence: the write's value operand uses read_local or a local derived from it.
    let mut data_dep_locals = FxHashSet::default();
    data_dep_locals.insert(read_local);

    // Propagate through assignments in the body (simple forward dataflow).
    // We iterate until fixed point for the function body.
    let mut changed = true;
    while changed {
        changed = false;
        for (block_id, block) in body.body.iter_indexed() {
            for (stmt_idx, stmt) in block.statements.iter().enumerate() {
                let loc = Location::new(block_id, stmt_idx);
                // Only consider statements after the read.
                if !is_after(loc, read_loc) {
                    continue;
                }
                if let ullbc_ast::StatementKind::Assign(dest, rvalue) = &stmt.kind {
                    if let Some(dest_local) = dest.local_id() {
                        if data_dep_locals.contains(&dest_local) {
                            continue; // already known
                        }
                        if rvalue_uses_local(rvalue, &data_dep_locals) {
                            if data_dep_locals.insert(dest_local) {
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
    }

    let is_data_dep = write_value.map_or(false, |op| operand_uses_local(op, &data_dep_locals));

    // Control dependence: a use of read_local (or derived) appears in a Switch condition
    // that controls whether write_loc is reachable.
    let mut is_control_dep = false;
    for (block_id, block) in body.body.iter_indexed() {
        // Check if this block's terminator is a switch influenced by read_local.
        if let ullbc_ast::TerminatorKind::Switch { discr, .. } = &block.terminator.kind {
            if operand_uses_local(discr, &data_dep_locals) {
                // Check if write_loc is in a successor that is control-dependent on this switch.
                if is_control_dependent(write_loc.block, block_id, body, dominance) {
                    is_control_dep = true;
                    break;
                }
            }
        }
    }

    match (is_data_dep, is_control_dep) {
        (true, true) => Some(DependenceKind::Both),
        (true, false) => Some(DependenceKind::Data),
        (false, true) => Some(DependenceKind::Control),
        (false, false) => None,
    }
}

fn rvalue_uses_local(rvalue: &ullbc_ast::Rvalue, locals: &FxHashSet<LocalId>) -> bool {
    match rvalue {
        ullbc_ast::Rvalue::Use(op) => operand_uses_local(op, locals),
        ullbc_ast::Rvalue::Ref { place, .. } => place_uses_local(place, locals),
        ullbc_ast::Rvalue::BinaryOp(_, op1, op2) => {
            operand_uses_local(op1, locals) || operand_uses_local(op2, locals)
        }
        ullbc_ast::Rvalue::UnaryOp(_, op) => operand_uses_local(op, locals),
        ullbc_ast::Rvalue::Aggregate(_, ops) => ops.iter().any(|op| operand_uses_local(op, locals)),
        ullbc_ast::Rvalue::Discriminant(place) => place_uses_local(place, locals),
        ullbc_ast::Rvalue::Len(place, _, _) => place_uses_local(place, locals),
        ullbc_ast::Rvalue::NullaryOp(_, _) => false,
        ullbc_ast::Rvalue::RawPtr { place, .. } => place_uses_local(place, locals),
        ullbc_ast::Rvalue::Repeat(op, _, _) => operand_uses_local(op, locals),
        ullbc_ast::Rvalue::ShallowInitBox(op, _) => operand_uses_local(op, locals),
    }
}

fn operand_uses_local(op: &Operand, locals: &FxHashSet<LocalId>) -> bool {
    match op {
        Operand::Copy(place) | Operand::Move(place) => place_uses_local(place, locals),
        Operand::Const(_) => false,
    }
}

fn place_uses_local(place: &Place, locals: &FxHashSet<LocalId>) -> bool {
    match &place.kind {
        PlaceKind::Local(local) => locals.contains(local),
        PlaceKind::Projection(inner, _) => place_uses_local(inner, locals),
        PlaceKind::Global(_) => false,
    }
}

fn is_after(loc: Location, after: Location) -> bool {
    if loc.block == after.block {
        loc.statement_index > after.statement_index
    } else {
        // Simplified: different blocks; we conservatively say true and let dominance filter.
        true
    }
}

/// Check if `target_block` is control-dependent on `branch_block`.
/// Simplified: target_block is in a successor path of branch_block but not all successors.
fn is_control_dependent(
    target_block: ullbc_ast::BlockId,
    branch_block: ullbc_ast::BlockId,
    body: &ullbc_ast::ExprBody,
    dominance: &FxHashMap<ullbc_ast::BlockId, FxHashSet<ullbc_ast::BlockId>>,
) -> bool {
    if target_block == branch_block {
        return false;
    }
    let branch_dominates = dominance
        .get(&target_block)
        .map(|set| set.contains(&branch_block))
        .unwrap_or(false);
    if !branch_dominates {
        return false;
    }
    // Check that branch_block does not dominate all successors of itself
    // (i.e., it's actually a branch).
    let block = &body.body[branch_block];
    let succs = terminator_successors(&block.terminator.kind);
    if succs.len() <= 1 {
        return false;
    }
    true
}

/// Check if `mid` is between `start` and `end` in control flow.
fn is_between(
    mid: Location,
    start: Location,
    end: Location,
    _body: &ullbc_ast::ExprBody,
    dominance: &FxHashMap<ullbc_ast::BlockId, FxHashSet<ullbc_ast::BlockId>>,
) -> bool {
    // Simple heuristic: mid block is dominated by start block and dominates end block,
    // or at least they're in a reasonable order.
    let start_dominates_mid = dominance
        .get(&mid.block)
        .map(|set| set.contains(&start.block))
        .unwrap_or(false);
    let mid_dominates_end = dominance
        .get(&end.block)
        .map(|set| set.contains(&mid.block))
        .unwrap_or(false);
    start_dominates_mid || mid_dominates_end
}
