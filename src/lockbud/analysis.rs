use charon_lib::ast::*;
use charon_lib::export::CrateData;
use charon_lib::ullbc_ast;
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::VecDeque;

use crate::lockbud::callgraph::CallGraph;
use crate::lockbud::condvar;
use crate::lockbud::types::*;

/// Per-function gen/kill maps and analysis results.
#[derive(Debug, Default)]
pub struct FunctionAnalysis {
    pub gen_map: FxHashMap<Location, FxHashSet<LockGuardId>>,
    pub kill_map: FxHashMap<Location, FxHashSet<LockGuardId>>,
    /// Live lockguards at the exit of each block.
    pub exit_states: FxHashMap<ullbc_ast::BlockId, LiveLockGuards>,
    /// Live lockguards before each call to a specific callee.
    pub callsite_states: FxHashMap<(ullbc_ast::BlockId, FunDeclId), LiveLockGuards>,
}

/// Intra- and inter-procedural analysis.
pub struct Analyzer<'a> {
    crate_data: &'a CrateData,
    callgraph: &'a CallGraph,
    lockguards: &'a LockGuardMap,
    pub analyses: FxHashMap<FunDeclId, FunctionAnalysis>,
    /// (a, b): lockguard a is live when b is acquired.
    pub relations: FxHashSet<(LockGuardId, LockGuardId)>,
    /// Context (live lockguards) at the entry of each function.
    pub contexts: FxHashMap<FunDeclId, LiveLockGuards>,
    /// Live lockguards before each condvar API call.
    /// Key: (caller_fun_id, callsite_loc, callee_fun_id)
    pub condvar_callsite_states: FxHashMap<(FunDeclId, Location, FunDeclId), LiveLockGuards>,
    condvar_callsites: Option<&'a condvar::CondvarCallSites>,
}

impl<'a> Analyzer<'a> {
    pub fn new(
        crate_data: &'a CrateData,
        callgraph: &'a CallGraph,
        lockguards: &'a LockGuardMap,
    ) -> Self {
        Self {
            crate_data,
            callgraph,
            lockguards,
            analyses: FxHashMap::default(),
            relations: FxHashSet::default(),
            contexts: FxHashMap::default(),
            condvar_callsite_states: FxHashMap::default(),
            condvar_callsites: None,
        }
    }

    pub fn with_condvar_callsites(mut self, condvar_callsites: &'a condvar::CondvarCallSites) -> Self {
        self.condvar_callsites = Some(condvar_callsites);
        self
    }

    pub fn analyze(&mut self) {
        // Step 1: Build per-function gen/kill maps.
        self.build_gen_kill_maps();

        // Step 2: Inter-procedural fixed-point propagation.
        self.interproc_propagation();

        // Step 3: Intra-procedural dataflow to compute exit states, callsite states, and relations.
        self.intraprocedural_analysis();
    }

    fn build_gen_kill_maps(&mut self) {
        let krate = &self.crate_data.translated;
        for (fun_id, fun_decl) in krate.fun_decls.iter_indexed() {
            let Body::Unstructured(_body) = &fun_decl.body else {
                continue;
            };
            let mut analysis = FunctionAnalysis::default();
            for (guard_id, info) in self.lockguards.iter() {
                if guard_id.fun_id != fun_id {
                    continue;
                }
                for loc in &info.gen_locs {
                    analysis.gen_map.entry(*loc).or_default().insert(*guard_id);
                }
                for loc in &info.kill_locs {
                    analysis.kill_map.entry(*loc).or_default().insert(*guard_id);
                }
            }
            self.analyses.insert(fun_id, analysis);
        }
    }

    fn interproc_propagation(&mut self) {
        // Initialize contexts with empty live sets.
        let all_nodes: FxHashSet<FunDeclId> = self
            .callgraph
            .nodes()
            .into_iter()
            .chain(krate_fun_ids(self.crate_data))
            .collect();
        for fun_id in &all_nodes {
            self.contexts.insert(*fun_id, LiveLockGuards::new());
        }

        let mut worklist: VecDeque<FunDeclId> = all_nodes.into_iter().collect();

        while let Some(fun_id) = worklist.pop_front() {
            let context = self.contexts[&fun_id].clone();

            // For each callee, propagate the context + callsite state.
            // We need the callsite states, which depend on intraprocedural analysis.
            // To bootstrap, we do a combined approach:
            //   - If the function has lockguards, we compute its exit state using the current context.
            //   - Then for each callee, we union the exit state into callee's context.
            // We trigger intraprocedural analysis for `fun_id` here.
            let body_has_locks = self
                .lockguards
                .keys()
                .any(|gid| gid.fun_id == fun_id);

            if body_has_locks || !context.is_empty() {
                let exit_state = self.compute_function_state(fun_id, &context);
                if let Some(callees) = self.callgraph.edges.get(&fun_id) {
                    for &callee in callees {
                        let changed = self
                            .contexts
                            .get_mut(&callee)
                            .unwrap()
                            .union(&exit_state);
                        if changed {
                            worklist.push_back(callee);
                        }
                    }
                }
            } else {
                // No body locks and empty context: just pass context through to callees.
                if let Some(callees) = self.callgraph.edges.get(&fun_id) {
                    for &callee in callees {
                        let changed = self
                            .contexts
                            .get_mut(&callee)
                            .unwrap()
                            .union(&context);
                        if changed {
                            worklist.push_back(callee);
                        }
                    }
                }
            }
        }
    }

    fn intraprocedural_analysis(&mut self) {
        // Re-run intraprocedural analysis with the fixed contexts to populate
        // exit_states, callsite_states, and relations.
        let krate = &self.crate_data.translated;
        let fun_ids: Vec<FunDeclId> = krate.fun_decls.iter_indexed().map(|(id, _)| id).collect();
        for fun_id in fun_ids {
            let context = self.contexts.get(&fun_id).cloned().unwrap_or_default();
            self.compute_function_state(fun_id, &context);
        }
    }

    /// Compute the state for a single function given an entry context.
    /// Returns the union of all exit states (for Return terminators).
    fn compute_function_state(&mut self, fun_id: FunDeclId, context: &LiveLockGuards) -> LiveLockGuards {
        let krate = &self.crate_data.translated;
        let Some(fun_decl) = krate.fun_decls.get(fun_id) else {
            return LiveLockGuards::new();
        };
        let Body::Unstructured(body) = &fun_decl.body else {
            return LiveLockGuards::new();
        };
        let Some(analysis) = self.analyses.get(&fun_id) else {
            return LiveLockGuards::new();
        };

        // Clone maps so we don't borrow self.analyses for the whole loop.
        let gen_map = analysis.gen_map.clone();
        let kill_map = analysis.kill_map.clone();

        let mut entry_states: FxHashMap<ullbc_ast::BlockId, LiveLockGuards> = FxHashMap::default();
        let mut visited: FxHashSet<ullbc_ast::BlockId> = FxHashSet::default();
        let mut worklist: VecDeque<ullbc_ast::BlockId> = VecDeque::new();
        let mut callsite_updates: FxHashMap<(ullbc_ast::BlockId, FunDeclId), LiveLockGuards> =
            FxHashMap::default();

        entry_states.insert(ullbc_ast::START_BLOCK_ID, context.clone());
        worklist.push_back(ullbc_ast::START_BLOCK_ID);

        while let Some(block_id) = worklist.pop_front() {
            let block = &body.body[block_id];
            let mut state = entry_states.get(&block_id).cloned().unwrap_or_default();

            if fun_id.index() == 1 {
                log::info!("compute_function_state fun_id={:?} block={:?} entry_state={:?}", fun_id, block_id, state.0.iter().map(|g| g.local).collect::<Vec<_>>());
            }

            // Process statements.
            for (stmt_idx, _stmt) in block.statements.iter().enumerate() {
                let loc = Location::new(block_id, stmt_idx);
                let relation = apply_gen_kill(
                    &mut state,
                    gen_map.get(&loc),
                    kill_map.get(&loc),
                );
                if fun_id.index() == 1 && !relation.is_empty() {
                    log::info!("  stmt {} at {:?} relations={:?}", stmt_idx, loc, relation.iter().map(|(a,b)| (a.local, b.local)).collect::<Vec<_>>());
                }
                self.relations.extend(relation);
            }

            // Process terminator.
            let term_loc = Location::new(block_id, block.statements.len());
            let relation = apply_gen_kill(
                &mut state,
                gen_map.get(&term_loc),
                kill_map.get(&term_loc),
            );
            if fun_id.index() == 1 && !relation.is_empty() {
                log::info!("  term at {:?} relations={:?}", term_loc, relation.iter().map(|(a,b)| (a.local, b.local)).collect::<Vec<_>>());
            }
            self.relations.extend(relation);

            // Record callsite states for known callees.
            if let ullbc_ast::TerminatorKind::Call { call, .. } = &block.terminator.kind {
                if let FnOperand::Regular(fn_ptr) = &call.func {
                    if let FnPtrKind::Fun(FunId::Regular(callee_id)) = fn_ptr.kind.as_ref() {
                        callsite_updates
                            .entry((block_id, *callee_id))
                            .or_default()
                            .union(&state);
                        // Record condvar callsite state if this is a condvar API call.
                        if let Some(cv) = self.condvar_callsites {
                            if cv.contains_key(&(fun_id, term_loc, *callee_id)) {
                                self.condvar_callsite_states
                                    .insert((fun_id, term_loc, *callee_id), state.clone());
                            }
                        }
                    }
                }
            }

            // Propagate to successors.
            let successors = terminator_successors(&block.terminator.kind);
            for succ in successors {
                let is_new = !entry_states.contains_key(succ);
                let changed = entry_states
                    .entry(*succ)
                    .or_default()
                    .union(&state);
                if changed || is_new {
                    worklist.push_back(*succ);
                }
            }
        }

        // Gather exit states (Return terminators).
        let mut exit_state = LiveLockGuards::new();
        for (block_id, block) in body.body.iter_indexed() {
            if matches!(block.terminator.kind, ullbc_ast::TerminatorKind::Return) {
                if let Some(state) = entry_states.get(&block_id) {
                    exit_state.union(state);
                }
            }
        }

        if let Some(a) = self.analyses.get_mut(&fun_id) {
            a.exit_states = entry_states;
            for (k, v) in callsite_updates {
                a.callsite_states.entry(k).or_default().union(&v);
            }
        }

        exit_state
    }
}

fn krate_fun_ids(crate_data: &CrateData) -> impl Iterator<Item = FunDeclId> + '_ {
    crate_data.translated.fun_decls.iter_indexed().map(|(id, _)| id)
}

fn apply_gen_kill(
    state: &mut LiveLockGuards,
    gen_set: Option<&FxHashSet<LockGuardId>>,
    kill: Option<&FxHashSet<LockGuardId>>,
) -> FxHashSet<(LockGuardId, LockGuardId)> {
    if let Some(kill) = kill {
        let kill_set = LiveLockGuards(kill.iter().copied().collect());
        state.difference(&kill_set);
    }
    let mut relations = FxHashSet::default();
    if let Some(gset) = gen_set {
        for s in state.0.iter() {
            for g in gset.iter() {
                relations.insert((*s, *g));
            }
        }
        for g in gset.iter() {
            state.insert(*g);
        }
    }
    relations
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
