use charon_lib::ast::*;
use charon_lib::export::CrateData;
use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use petgraph::{Directed, Graph};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::lockbud::analysis::Analyzer;
use crate::lockbud::callgraph::CallGraph;
use crate::lockbud::condvar::{self, CondvarApi, ParkingLotCondvarApi, StdCondvarApi};
use crate::lockbud::report::{
    CondvarDeadlockDiagnosis, DeadlockDiagnosis, Report, ReportContent, WaitNotifyLocks,
};
use crate::lockbud::types::*;

/// Detect doublelock, conflictlock, and condvar misuse from the analyzed relations.
pub struct DeadlockDetector<'a> {
    crate_data: &'a CrateData,
    callgraph: &'a CallGraph,
    lockguards: &'a LockGuardMap,
    analyzer: &'a Analyzer<'a>,
    condvar_callsites: &'a condvar::CondvarCallSites,
}

impl<'a> DeadlockDetector<'a> {
    pub fn new(
        crate_data: &'a CrateData,
        callgraph: &'a CallGraph,
        lockguards: &'a LockGuardMap,
        analyzer: &'a Analyzer<'a>,
        condvar_callsites: &'a condvar::CondvarCallSites,
    ) -> Self {
        Self {
            crate_data,
            callgraph,
            lockguards,
            analyzer,
            condvar_callsites,
        }
    }

    pub fn detect(&self) -> Vec<Report> {
        let mut reports = Vec::new();
        let mut conflict_graph = ConflictLockGraph::new();
        let mut relation_to_node: FxHashMap<(LockGuardId, LockGuardId), NodeIndex> =
            FxHashMap::default();

        // Phase 1: Detect doublelock.
        for (a, b) in &self.analyzer.relations {
            // Skip relations involving std library internals.
            if !self.is_user_guard(a) || !self.is_user_guard(b) {
                continue;
            }
            let (possibility, reason) = self.deadlock_possibility(a, b);
            match possibility {
                DeadlockPossibility::Probably => {
                    let diagnosis = self.diagnose_relation(a, b);
                    reports.push(Report::DoubleLock(ReportContent::new(
                        "DoubleLock".to_string(),
                        format!("{:?}", possibility),
                        diagnosis,
                        "The first lock is not released when acquiring the second lock"
                            .to_string(),
                    )));
                }
                DeadlockPossibility::Possibly => {
                    let diagnosis = self.diagnose_relation(a, b);
                    reports.push(Report::DoubleLock(ReportContent::new(
                        "DoubleLock".to_string(),
                        format!("{:?}", possibility),
                        diagnosis,
                        "The first lock is not released when acquiring the second lock"
                            .to_string(),
                    )));
                }
                _ if reason != NotDeadlockReason::SameSpan => {
                    let node = conflict_graph.add_node((*a, *b));
                    relation_to_node.insert((*a, *b), node);
                }
                _ => {}
            }
        }

        // Phase 2: Detect conflictlock via cycle detection.
        for ((_, a), node1) in &relation_to_node {
            for ((b, _), node2) in &relation_to_node {
                if node1 == node2 {
                    continue; // skip self-loops
                }
                let (possibility, _) = self.deadlock_possibility(a, b);
                if matches!(
                    possibility,
                    DeadlockPossibility::Probably | DeadlockPossibility::Possibly
                ) {
                    conflict_graph.add_edge(*node1, *node2, possibility);
                }
            }
        }

        let cycles = conflict_graph.simple_cycles();
        for cycle in cycles {
            let diagnosis: Vec<_> = cycle
                .into_iter()
                .map(|node| {
                    let (a, b) = conflict_graph.node_weight(node).unwrap();
                    self.diagnose_relation(a, b)
                })
                .collect();
            reports.push(Report::ConflictLock(ReportContent::new(
                "ConflictLock".to_string(),
                "Possibly".to_string(),
                diagnosis,
                "Locks mutually wait for each other to form a cycle".to_string(),
            )));
        }

        // Phase 3: Detect condvar misuse.
        reports.extend(self.detect_condvar_misuse());

        reports
    }

    fn is_user_guard(&self, guard: &LockGuardId) -> bool {
        let krate = &self.crate_data.translated;
        let Some(decl) = krate.fun_decls.get(guard.fun_id) else {
            return false;
        };
        let name = format_name(&decl.item_meta.name);
        !name.starts_with("std::")
            && !name.starts_with("core::")
            && !name.starts_with("alloc::")
            && !name.starts_with("rustc")
    }

    /// Detect condvar misuse.
    /// For each pair of wait/notify on the same condvar, check if there are
    /// aliasing lockguards before wait and notify that do not alias with the
    /// mutex guard passed to wait.
    fn detect_condvar_misuse(&self) -> Vec<Report> {
        let mut reports = Vec::new();
        let mut std_wait: FxHashMap<(FunDeclId, Location, FunDeclId), (Place, Place)> =
            FxHashMap::default();
        let mut std_notify: FxHashMap<(FunDeclId, Location, FunDeclId), Place> =
            FxHashMap::default();
        let mut parking_lot_wait: FxHashMap<(FunDeclId, Location, FunDeclId), (Place, Place)> =
            FxHashMap::default();
        let mut parking_lot_notify: FxHashMap<(FunDeclId, Location, FunDeclId), Place> =
            FxHashMap::default();

        let krate = &self.crate_data.translated;

        for (key, api) in self.condvar_callsites {
            let (caller_id, loc, _callee_id) = *key;
            let Some(decl) = krate.fun_decls.get(caller_id) else {
                continue;
            };
            let Body::Unstructured(body) = &decl.body else {
                continue;
            };
            let block = &body.body[loc.block];
            let term = &block.terminator;
            let ullbc_ast::TerminatorKind::Call { call, .. } = &term.kind else {
                continue;
            };

            // Build a local -> ref_place map for this function so we can resolve
            // temporary locals created by `&place` back to their underlying place.
            let ref_places = self.build_ref_places_for_function(caller_id);

            match api {
                CondvarApi::Std(StdCondvarApi::Wait(_)) => {
                    if let (Some(condvar_place), Some(mutex_place)) =
                        (call.args.get(0), call.args.get(1))
                    {
                        if let (Operand::Copy(cp) | Operand::Move(cp), Operand::Move(mp)) =
                            (condvar_place, mutex_place)
                        {
                            let resolved_condvar = resolve_place_through_refs(cp, &ref_places);
                            let resolved_mutex = resolve_place_through_refs(mp, &ref_places);
                            std_wait.insert(*key, (resolved_condvar.clone(), resolved_mutex.clone()));
                        }
                    }
                }
                CondvarApi::Std(StdCondvarApi::Notify(_)) => {
                    if let Some(condvar_place) = call.args.get(0) {
                        if let Operand::Copy(cp) | Operand::Move(cp) = condvar_place {
                            let resolved = resolve_place_through_refs(cp, &ref_places);
                            std_notify.insert(*key, resolved.clone());
                        }
                    }
                }
                CondvarApi::ParkingLot(ParkingLotCondvarApi::Wait(_)) => {
                    if let (Some(condvar_place), Some(mutex_ref_place)) =
                        (call.args.get(0), call.args.get(1))
                    {
                        if let (Operand::Copy(cp) | Operand::Move(cp), Operand::Move(mrp)) =
                            (condvar_place, mutex_ref_place)
                        {
                            let resolved_condvar = resolve_place_through_refs(cp, &ref_places);
                            let resolved_mutex = resolve_place_through_refs(mrp, &ref_places);
                            parking_lot_wait.insert(*key, (resolved_condvar.clone(), resolved_mutex.clone()));
                        }
                    }
                }
                CondvarApi::ParkingLot(ParkingLotCondvarApi::Notify(_)) => {
                    if let Some(condvar_place) = call.args.get(0) {
                        if let Operand::Copy(cp) | Operand::Move(cp) = condvar_place {
                            let resolved = resolve_place_through_refs(cp, &ref_places);
                            parking_lot_notify.insert(*key, resolved.clone());
                        }
                    }
                }
            }
        }

        // Check std::sync::Condvar
        for (wait_key, (condvar_ref1, mutex_guard1)) in std_wait.iter() {
            for (notify_key, condvar_ref2) in std_notify.iter() {
                if condvar_ref1 == condvar_ref2 {
                    let wait_fun = wait_key.0;
                    let notify_fun = notify_key.0;
                    if wait_fun == notify_fun {
                        if let Some(report) = self.check_condvar_pair(
                            *wait_key,
                            *notify_key,
                            mutex_guard1,
                            true,
                            &std_wait,
                            &std_notify,
                        ) {
                            reports.push(report);
                        }
                    } else if self.callgraph.can_reach(wait_fun, notify_fun)
                        || self.callgraph.can_reach(notify_fun, wait_fun)
                        || self.callgraph.share_common_ancestor(wait_fun, notify_fun)
                    {
                        if let Some(report) = self.check_condvar_pair_interproc(
                            *wait_key,
                            *notify_key,
                            mutex_guard1,
                            true,
                        ) {
                            reports.push(report);
                        }
                    }
                }
            }
        }

        // Check parking_lot::Condvar
        for (wait_key, (condvar_ref1, mutex_guard_ref1)) in parking_lot_wait.iter() {
            for (notify_key, condvar_ref2) in parking_lot_notify.iter() {
                if condvar_ref1 == condvar_ref2 {
                    let wait_fun = wait_key.0;
                    let notify_fun = notify_key.0;
                    if wait_fun == notify_fun {
                        if let Some(report) = self.check_condvar_pair(
                            *wait_key,
                            *notify_key,
                            mutex_guard_ref1,
                            false,
                            &parking_lot_wait,
                            &parking_lot_notify,
                        ) {
                            reports.push(report);
                        }
                    } else if self.callgraph.can_reach(wait_fun, notify_fun)
                        || self.callgraph.can_reach(notify_fun, wait_fun)
                        || self.callgraph.share_common_ancestor(wait_fun, notify_fun)
                    {
                        if let Some(report) = self.check_condvar_pair_interproc(
                            *wait_key,
                            *notify_key,
                            mutex_guard_ref1,
                            false,
                        ) {
                            reports.push(report);
                        }
                    }
                }
            }
        }

        reports
    }

    fn build_ref_places_for_function(
        &self,
        fun_id: FunDeclId,
    ) -> rustc_hash::FxHashMap<LocalId, Place> {
        let mut ref_places = rustc_hash::FxHashMap::default();
        let krate = &self.crate_data.translated;
        let Some(decl) = krate.fun_decls.get(fun_id) else {
            return ref_places;
        };
        let Body::Unstructured(body) = &decl.body else {
            return ref_places;
        };
        for block in &body.body {
            for stmt in &block.statements {
                if let ullbc_ast::StatementKind::Assign(dest, rvalue) = &stmt.kind {
                    if let Some(dest_local) = dest.local_id() {
                        if let ullbc_ast::Rvalue::Ref { place: ref_place, .. } = rvalue {
                            ref_places.insert(dest_local, ref_place.clone());
                        }
                    }
                }
            }
        }
        ref_places
    }

    fn check_condvar_pair(
        &self,
        wait_key: (FunDeclId, Location, FunDeclId),
        notify_key: (FunDeclId, Location, FunDeclId),
        mutex_guard_place: &Place,
        is_std_condvar: bool,
        _wait_map: &FxHashMap<(FunDeclId, Location, FunDeclId), (Place, Place)>,
        _notify_map: &FxHashMap<(FunDeclId, Location, FunDeclId), Place>,
    ) -> Option<Report> {
        let live1 = self
            .analyzer
            .condvar_callsite_states
            .get(&wait_key)?
            .clone();
        let live2 = self
            .analyzer
            .condvar_callsite_states
            .get(&notify_key)?
            .clone();

        let live1_ids: Vec<_> = live1.0.iter().copied().collect();
        let live2_ids: Vec<_> = live2.0.iter().copied().collect();

        let mut aliased_pairs = Vec::new();
        for g1 in &live1_ids {
            for g2 in &live2_ids {
                let info1 = match self.lockguards.get(g1) {
                    Some(i) => i,
                    None => continue,
                };
                let info2 = match self.lockguards.get(g2) {
                    Some(i) => i,
                    None => continue,
                };
                // Check if the two guards alias (same receiver place).
                let alias = match (&info1.receiver_place, &info2.receiver_place) {
                    (Some(r1), Some(r2)) if r1 == r2 => true,
                    _ => false,
                };
                if !alias {
                    continue;
                }
                // Check deadlock possibility between the two guards.
                let (possibility, _) = self.deadlock_possibility(g1, g2);
                if possibility > DeadlockPossibility::Unlikely {
                    aliased_pairs.push((g1, g2));
                }
            }
        }

        // Filter out pairs where g1 aliases with the mutex guard passed to wait.
        let mut no_mutex_guards = Vec::new();
        for (g1, g2) in aliased_pairs {
            let g1_info = self.lockguards.get(g1).unwrap();
            let aliases_mutex = match (&g1_info.receiver_place, mutex_guard_place.local_id()) {
                (Some(rp), Some(mg_local)) => {
                    // For std condvar, the mutex guard place itself is compared.
                    // For parking_lot, the mutex guard ref place's local is compared.
                    match &rp.kind {
                        PlaceKind::Local(l) if *l == mg_local => true,
                        _ => false,
                    }
                }
                _ => false,
            };
            if !aliases_mutex {
                no_mutex_guards.push((g1, g2));
            }
        }

        if no_mutex_guards.is_empty() {
            return None;
        }

        let diagnosis =
            self.diagnose_condvar_deadlock(wait_key, notify_key, is_std_condvar, &no_mutex_guards);
        Some(Report::CondvarDeadlock(ReportContent::new(
            "CondvarDeadlock".to_string(),
            "Possibly".to_string(),
            diagnosis,
            "The same lock before Condvar::wait and notify".to_string(),
        )))
    }

    /// Inter-procedural variant of check_condvar_pair that unions the callsite states
    /// with the propagated function entry contexts.
    fn check_condvar_pair_interproc(
        &self,
        wait_key: (FunDeclId, Location, FunDeclId),
        notify_key: (FunDeclId, Location, FunDeclId),
        mutex_guard_place: &Place,
        is_std_condvar: bool,
    ) -> Option<Report> {
        let (wait_fun, _, _) = wait_key;
        let (notify_fun, _, _) = notify_key;

        let mut live1 = self
            .analyzer
            .condvar_callsite_states
            .get(&wait_key)
            .cloned()
            .unwrap_or_default();
        let mut live2 = self
            .analyzer
            .condvar_callsite_states
            .get(&notify_key)
            .cloned()
            .unwrap_or_default();

        // Union with the propagated function entry contexts.
        if let Some(ctx) = self.analyzer.contexts.get(&wait_fun) {
            live1.union(ctx);
        }
        if let Some(ctx) = self.analyzer.contexts.get(&notify_fun) {
            live2.union(ctx);
        }

        let live1_ids: Vec<_> = live1.0.iter().copied().collect();
        let live2_ids: Vec<_> = live2.0.iter().copied().collect();

        let mut aliased_pairs = Vec::new();
        for g1 in &live1_ids {
            for g2 in &live2_ids {
                let info1 = match self.lockguards.get(g1) {
                    Some(i) => i,
                    None => continue,
                };
                let info2 = match self.lockguards.get(g2) {
                    Some(i) => i,
                    None => continue,
                };
                let alias = match (&info1.receiver_place, &info2.receiver_place) {
                    (Some(r1), Some(r2)) if r1 == r2 => true,
                    _ => false,
                };
                if !alias {
                    continue;
                }
                let (possibility, _) = self.deadlock_possibility(g1, g2);
                if possibility > DeadlockPossibility::Unlikely {
                    aliased_pairs.push((g1, g2));
                }
            }
        }

        // Filter out pairs where g1 aliases with the mutex guard passed to wait.
        let mut no_mutex_guards = Vec::new();
        for (g1, g2) in aliased_pairs {
            let g1_info = self.lockguards.get(g1).unwrap();
            let aliases_mutex = match (&g1_info.receiver_place, mutex_guard_place.local_id()) {
                (Some(rp), Some(mg_local)) => match &rp.kind {
                    PlaceKind::Local(l) if *l == mg_local => true,
                    _ => false,
                },
                _ => false,
            };
            if !aliases_mutex {
                no_mutex_guards.push((g1, g2));
            }
        }

        if no_mutex_guards.is_empty() {
            return None;
        }

        let diagnosis =
            self.diagnose_condvar_deadlock(wait_key, notify_key, is_std_condvar, &no_mutex_guards);
        Some(Report::CondvarDeadlock(ReportContent::new(
            "CondvarDeadlock".to_string(),
            "Possibly".to_string(),
            diagnosis,
            "The same lock before Condvar::wait and notify".to_string(),
        )))
    }

    fn diagnose_condvar_deadlock(
        &self,
        wait_key: (FunDeclId, Location, FunDeclId),
        notify_key: (FunDeclId, Location, FunDeclId),
        is_std_condvar: bool,
        aliased_pairs: &[(&LockGuardId, &LockGuardId)],
    ) -> CondvarDeadlockDiagnosis {
        let (caller_id1, loc1, _callee_id1) = wait_key;
        let (caller_id2, loc2, _callee_id2) = notify_key;
        let krate = &self.crate_data.translated;

        let wait_span = krate
            .fun_decls
            .get(caller_id1)
            .and_then(|d| {
                if let Body::Unstructured(body) = &d.body {
                    Some(format!("{:?}", body.body[loc1.block].terminator.span))
                } else {
                    None
                }
            })
            .unwrap_or_default();
        let notify_span = krate
            .fun_decls
            .get(caller_id2)
            .and_then(|d| {
                if let Body::Unstructured(body) = &d.body {
                    Some(format!("{:?}", body.body[loc2.block].terminator.span))
                } else {
                    None
                }
            })
            .unwrap_or_default();

        let deadlocks = aliased_pairs
            .iter()
            .map(|(a, b)| {
                let a_info = &self.lockguards[a];
                let b_info = &self.lockguards[b];
                WaitNotifyLocks::new(
                    format!("{:?}", a_info.lockguard_ty),
                    format!("{:?}", a_info.span),
                    format!("{:?}", b_info.lockguard_ty),
                    format!("{:?}", b_info.span),
                )
            })
            .collect::<Vec<_>>();

        if is_std_condvar {
            CondvarDeadlockDiagnosis::new(
                "std::sync::Condvar::wait".to_owned(),
                wait_span,
                "std::sync::Condvar::notify".to_owned(),
                notify_span,
                deadlocks,
            )
        } else {
            CondvarDeadlockDiagnosis::new(
                "parking_lot::Condvar::wait".to_owned(),
                wait_span,
                "parking_lot::Condvar::notify".to_owned(),
                notify_span,
                deadlocks,
            )
        }
    }

    fn deadlock_possibility(
        &self,
        a: &LockGuardId,
        b: &LockGuardId,
    ) -> (DeadlockPossibility, NotDeadlockReason) {
        let a_info = match self.lockguards.get(a) {
            Some(i) => i,
            None => return (DeadlockPossibility::Unlikely, NotDeadlockReason::UnknownGuard),
        };
        let b_info = match self.lockguards.get(b) {
            Some(i) => i,
            None => return (DeadlockPossibility::Unlikely, NotDeadlockReason::UnknownGuard),
        };

        // Same span heuristic: loops/recursion often cause same-span false positives.
        if a_info.span == b_info.span {
            return (DeadlockPossibility::Unlikely, NotDeadlockReason::SameSpan);
        }

        let ty_possibility = a_info.lockguard_ty.deadlock_with(&b_info.lockguard_ty);
        if matches!(ty_possibility, DeadlockPossibility::Unlikely) {
            return (DeadlockPossibility::Unlikely, NotDeadlockReason::TypeMismatch);
        }

        // Recursive read locks generated by `read_recursive()` are safe.
        if matches!(
            (&a_info.lockguard_ty, &b_info.lockguard_ty),
            (LockGuardTy::StdRwLockRead(_), LockGuardTy::StdRwLockRead(_))
                | (LockGuardTy::ParkingLotRead(_), LockGuardTy::ParkingLotRead(_))
                | (LockGuardTy::SpinRead(_), LockGuardTy::SpinRead(_))
        ) {
            if a_info.is_recursive_read || b_info.is_recursive_read {
                if let (Some(ra), Some(rb)) = (&a_info.receiver_place, &b_info.receiver_place) {
                    if ra == rb {
                        return (DeadlockPossibility::Unlikely, NotDeadlockReason::RecursiveRead);
                    }
                }
            }
        }

        // Simplified alias analysis.
        let alias = match (&a_info.receiver_place, &b_info.receiver_place) {
            (Some(ra), Some(rb)) if ra == rb => DeadlockPossibility::Probably,
            (Some(ra), Some(rb)) => {
                // Two distinct locals without projection are definitely different objects.
                match (&ra.kind, &rb.kind) {
                    (PlaceKind::Local(la), PlaceKind::Local(lb)) if la != lb => {
                        DeadlockPossibility::Unlikely
                    }
                    _ => DeadlockPossibility::Possibly,
                }
            }
            _ => DeadlockPossibility::Possibly,
        };

        let possibility = match (ty_possibility, alias) {
            (DeadlockPossibility::Probably, DeadlockPossibility::Probably) => {
                DeadlockPossibility::Probably
            }
            (DeadlockPossibility::Probably, DeadlockPossibility::Possibly)
            | (DeadlockPossibility::Possibly, DeadlockPossibility::Probably)
            | (DeadlockPossibility::Possibly, DeadlockPossibility::Possibly) => {
                DeadlockPossibility::Possibly
            }
            _ => DeadlockPossibility::Unlikely,
        };

        (possibility, NotDeadlockReason::TrueDeadlock)
    }

    fn diagnose_relation(&self, a: &LockGuardId, b: &LockGuardId) -> DeadlockDiagnosis {
        let a_info = self.lockguards.get(a).unwrap();
        let b_info = self.lockguards.get(b).unwrap();
        let callchains = self.track_callchains(a.fun_id, b.fun_id);
        DeadlockDiagnosis::new(
            format!("{:?}", a_info.lockguard_ty),
            format!("{:?}", a_info.span),
            format!("{:?}", b_info.lockguard_ty),
            format!("{:?}", b_info.span),
            callchains,
        )
    }

    fn track_callchains(&self, source: FunDeclId, target: FunDeclId) -> Vec<Vec<Vec<String>>> {
        let paths = self.callgraph.all_simple_paths(source, target, 6);
        let krate = &self.crate_data.translated;
        paths
            .into_iter()
            .map(|path| {
                path.windows(2)
                    .map(|window| {
                        let caller = window[0];
                        let callee = window[1];
                        let blocks = self
                            .callgraph
                            .callsites
                            .get(&(caller, callee))
                            .cloned()
                            .unwrap_or_default();
                        let caller_decl = krate.fun_decls.get(caller);
                        blocks
                            .into_iter()
                            .filter_map(|block_id| {
                                caller_decl.and_then(|decl| {
                                    if let Body::Unstructured(body) = &decl.body {
                                        let term = &body.body[block_id].terminator;
                                        Some(format!("{:?}", term.span))
                                    } else {
                                        None
                                    }
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotDeadlockReason {
    TrueDeadlock,
    SameSpan,
    TypeMismatch,
    UnknownGuard,
    RecursiveRead,
}

struct ConflictLockGraph {
    graph: Graph<(LockGuardId, LockGuardId), DeadlockPossibility, Directed>,
}

/// Resolve a place by following `Ref` chains through temporary locals.
fn resolve_place_through_refs(
    place: &Place,
    ref_places: &rustc_hash::FxHashMap<LocalId, Place>,
) -> Place {
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

impl ConflictLockGraph {
    fn new() -> Self {
        Self {
            graph: Graph::new(),
        }
    }

    fn add_node(&mut self, relation: (LockGuardId, LockGuardId)) -> NodeIndex {
        self.graph.add_node(relation)
    }

    fn add_edge(&mut self, from: NodeIndex, to: NodeIndex, weight: DeadlockPossibility) {
        self.graph.add_edge(from, to, weight);
    }

    fn node_weight(&self, node: NodeIndex) -> Option<&(LockGuardId, LockGuardId)> {
        self.graph.node_weight(node)
    }

    /// Find simple cycles in the graph using Johnson's algorithm approximation:
    /// enumerate all simple paths and check if they form a cycle.
    fn simple_cycles(&self) -> Vec<Vec<NodeIndex>> {
        let mut cycles = Vec::new();
        let mut seen_cycles: FxHashSet<Vec<NodeIndex>> = FxHashSet::default();
        for start in self.graph.node_indices() {
            let mut stack = vec![(start, vec![start])];
            while let Some((node, path)) = stack.pop() {
                if path.len() > 1 && node == start {
                    // Remove the trailing duplicate of start to get the true cycle nodes.
                    let cycle_nodes = &path[..path.len() - 1];
                    let mut normalized: Vec<_> = cycle_nodes.to_vec();
                    // Rotate to smallest element to canonicalize.
                    let min_pos = normalized
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, n)| n.index())
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    normalized.rotate_left(min_pos);
                    // Also consider the reversed cycle to avoid duplicates from opposite traversal.
                    let mut reversed = normalized.clone();
                    reversed.reverse();
                    let rev_min_pos = reversed
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, n)| n.index())
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    reversed.rotate_left(rev_min_pos);
                    let canonical = if normalized <= reversed {
                        normalized
                    } else {
                        reversed
                    };
                    if seen_cycles.insert(canonical.clone()) {
                        cycles.push(cycle_nodes.to_vec());
                    }
                    continue;
                }
                if path.len() >= 6 {
                    continue;
                }
                for edge in self.graph.edges_directed(node, petgraph::Direction::Outgoing) {
                    let next = edge.target();
                    if next == start || !path.contains(&next) {
                        let mut new_path = path.clone();
                        new_path.push(next);
                        stack.push((next, new_path));
                    }
                }
            }
        }
        cycles
    }
}
