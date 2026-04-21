//! Generic inter-procedural Gen/Kill dataflow engine.
//!
//! This module provides a generic framework for inter-procedural dataflow analysis
//! over ULLBC. It is parameterized over an entity type `T` and supports detector-specific
//! relation building and callsite hooks via traits.

use charon_lib::ast::*;
use charon_lib::export::CrateData;
use charon_lib::ullbc_ast;
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::VecDeque;

use crate::lockbud::callgraph::CallGraph;
use crate::lockbud::types::Location;

/// Marker trait for entities that can be tracked by the dataflow engine.
pub trait DataflowEntity:
    Clone + Copy + std::fmt::Debug + PartialEq + Eq + std::hash::Hash + Ord + Send + Sync + 'static
{
}

/// A set of live entities at a program point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveSet<T: DataflowEntity>(pub FxHashSet<T>);

impl<T: DataflowEntity> Default for LiveSet<T> {
    fn default() -> Self {
        Self(FxHashSet::default())
    }
}

impl<T: DataflowEntity> LiveSet<T> {
    pub fn new() -> Self {
        Self(FxHashSet::default())
    }

    pub fn insert(&mut self, id: T) -> bool {
        self.0.insert(id)
    }

    pub fn remove(&mut self, id: &T) -> bool {
        self.0.remove(id)
    }

    /// Union `other` into self. Returns true if self changed.
    pub fn union(&mut self, other: &Self) -> bool {
        let old_len = self.0.len();
        self.0.extend(&other.0);
        old_len != self.0.len()
    }

    /// Remove all elements of `other` from self. Returns true if self changed.
    pub fn difference(&mut self, other: &Self) -> bool {
        let old_len = self.0.len();
        for id in &other.0 {
            self.0.remove(id);
        }
        old_len != self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.0.iter()
    }
}

/// Gen/kill maps for a single function.
#[derive(Debug)]
pub struct GenKillMaps<T: DataflowEntity> {
    pub gen_map: FxHashMap<Location, FxHashSet<T>>,
    pub kill_map: FxHashMap<Location, FxHashSet<T>>,
}

impl<T: DataflowEntity> Default for GenKillMaps<T> {
    fn default() -> Self {
        Self {
            gen_map: FxHashMap::default(),
            kill_map: FxHashMap::default(),
        }
    }
}

/// Per-function analysis results.
#[derive(Debug)]
pub struct FunctionAnalysis<T: DataflowEntity> {
    pub gen_kill: GenKillMaps<T>,
    /// Live entities at the exit of each block.
    pub exit_states: FxHashMap<ullbc_ast::BlockId, LiveSet<T>>,
    /// Live entities before each call to a specific callee.
    pub callsite_states: FxHashMap<(ullbc_ast::BlockId, FunDeclId), LiveSet<T>>,
}

impl<T: DataflowEntity> Default for FunctionAnalysis<T> {
    fn default() -> Self {
        Self {
            gen_kill: GenKillMaps::default(),
            exit_states: FxHashMap::default(),
            callsite_states: FxHashMap::default(),
        }
    }
}

/// Trait for building relations when entities are generated.
pub trait RelationBuilder<T: DataflowEntity> {
    /// Called when entities in `gen_set` are generated while `state` entities are live.
    /// Returns the set of relations to record.
    fn build_relations(
        &mut self,
        state: &LiveSet<T>,
        gen_set: &FxHashSet<T>,
    ) -> FxHashSet<(T, T)>;
}

/// Trait for hooking into callsites during analysis.
pub trait CallsiteHook<T: DataflowEntity> {
    /// Called at each callsite with the current live state.
    fn on_callsite(
        &mut self,
        caller: FunDeclId,
        callee: FunDeclId,
        loc: Location,
        state: &LiveSet<T>,
    );
}

/// A no-op relation builder that records no relations.
pub struct NoOpRelationBuilder;

impl<T: DataflowEntity> RelationBuilder<T> for NoOpRelationBuilder {
    fn build_relations(
        &mut self,
        _state: &LiveSet<T>,
        _gen_set: &FxHashSet<T>,
    ) -> FxHashSet<(T, T)> {
        FxHashSet::default()
    }
}

/// A no-op callsite hook.
pub struct NoOpCallsiteHook;

impl<T: DataflowEntity> CallsiteHook<T> for NoOpCallsiteHook {
    fn on_callsite(
        &mut self,
        _caller: FunDeclId,
        _callee: FunDeclId,
        _loc: Location,
        _state: &LiveSet<T>,
    ) {
    }
}

/// Generic inter-procedural Gen/Kill dataflow engine.
pub struct DataflowEngine<'a, T: DataflowEntity> {
    crate_data: &'a CrateData,
    callgraph: &'a CallGraph,
    pub analyses: FxHashMap<FunDeclId, FunctionAnalysis<T>>,
    /// Context (live entities) at the entry of each function.
    pub contexts: FxHashMap<FunDeclId, LiveSet<T>>,
    /// Relations discovered during analysis.
    pub relations: FxHashSet<(T, T)>,
}

impl<'a, T: DataflowEntity> DataflowEngine<'a, T> {
    pub fn new(crate_data: &'a CrateData, callgraph: &'a CallGraph) -> Self {
        Self {
            crate_data,
            callgraph,
            analyses: FxHashMap::default(),
            contexts: FxHashMap::default(),
            relations: FxHashSet::default(),
        }
    }

    /// Register gen/kill maps for a function before running analysis.
    pub fn add_function_gen_kill(&mut self, fun_id: FunDeclId, gen_kill: GenKillMaps<T>) {
        self.analyses.insert(
            fun_id,
            FunctionAnalysis {
                gen_kill,
                exit_states: FxHashMap::default(),
                callsite_states: FxHashMap::default(),
            },
        );
    }

    /// Run the full analysis pipeline.
    pub fn analyze(
        &mut self,
        relation_builder: &mut dyn RelationBuilder<T>,
        callsite_hook: &mut dyn CallsiteHook<T>,
    ) {
        self.interproc_propagation(relation_builder, callsite_hook);
        self.intraprocedural_analysis(relation_builder, callsite_hook);
    }

    /// Convenience method that runs analysis with no-op hooks.
    pub fn analyze_no_hooks(&mut self) {
        let mut rb = NoOpRelationBuilder;
        let mut ch = NoOpCallsiteHook;
        self.analyze(&mut rb, &mut ch);
    }

    fn interproc_propagation(
        &mut self,
        relation_builder: &mut dyn RelationBuilder<T>,
        callsite_hook: &mut dyn CallsiteHook<T>,
    ) {
        // Initialize contexts with empty live sets.
        let all_nodes: FxHashSet<FunDeclId> = self
            .callgraph
            .nodes()
            .into_iter()
            .chain(krate_fun_ids(self.crate_data))
            .collect();
        for fun_id in &all_nodes {
            self.contexts.insert(*fun_id, LiveSet::new());
        }

        let mut worklist: VecDeque<FunDeclId> = all_nodes.into_iter().collect();

        while let Some(fun_id) = worklist.pop_front() {
            let context = self.contexts[&fun_id].clone();

            let body_has_entities = self
                .analyses
                .get(&fun_id)
                .map(|a| !a.gen_kill.gen_map.is_empty() || !a.gen_kill.kill_map.is_empty())
                .unwrap_or(false);

            if body_has_entities || !context.is_empty() {
                let exit_state =
                    self.compute_function_state(fun_id, &context, relation_builder, callsite_hook);
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
                // No body entities and empty context: just pass context through to callees.
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

    fn intraprocedural_analysis(
        &mut self,
        relation_builder: &mut dyn RelationBuilder<T>,
        callsite_hook: &mut dyn CallsiteHook<T>,
    ) {
        // Re-run intraprocedural analysis with the fixed contexts to populate
        // exit_states, callsite_states, and relations.
        let krate = &self.crate_data.translated;
        let fun_ids: Vec<FunDeclId> = krate.fun_decls.iter_indexed().map(|(id, _)| id).collect();
        for fun_id in fun_ids {
            let context = self.contexts.get(&fun_id).cloned().unwrap_or_default();
            self.compute_function_state(fun_id, &context, relation_builder, callsite_hook);
        }
    }

    /// Compute the state for a single function given an entry context.
    /// Returns the union of all exit states (for Return terminators).
    fn compute_function_state(
        &mut self,
        fun_id: FunDeclId,
        context: &LiveSet<T>,
        relation_builder: &mut dyn RelationBuilder<T>,
        callsite_hook: &mut dyn CallsiteHook<T>,
    ) -> LiveSet<T> {
        let krate = &self.crate_data.translated;
        let Some(fun_decl) = krate.fun_decls.get(fun_id) else {
            return LiveSet::new();
        };
        let Body::Unstructured(body) = &fun_decl.body else {
            return LiveSet::new();
        };
        let Some(analysis) = self.analyses.get(&fun_id) else {
            return LiveSet::new();
        };

        // Clone maps so we don't borrow self.analyses for the whole loop.
        let gen_map = analysis.gen_kill.gen_map.clone();
        let kill_map = analysis.gen_kill.kill_map.clone();

        let mut entry_states: FxHashMap<ullbc_ast::BlockId, LiveSet<T>> = FxHashMap::default();
        let mut worklist: VecDeque<ullbc_ast::BlockId> = VecDeque::new();
        let mut callsite_updates: FxHashMap<(ullbc_ast::BlockId, FunDeclId), LiveSet<T>> =
            FxHashMap::default();

        entry_states.insert(ullbc_ast::START_BLOCK_ID, context.clone());
        worklist.push_back(ullbc_ast::START_BLOCK_ID);

        while let Some(block_id) = worklist.pop_front() {
            let block = &body.body[block_id];
            let mut state = entry_states.get(&block_id).cloned().unwrap_or_default();

            // Process statements.
            for (stmt_idx, _stmt) in block.statements.iter().enumerate() {
                let loc = Location::new(block_id, stmt_idx);
                let relation = apply_gen_kill(
                    &mut state,
                    gen_map.get(&loc),
                    kill_map.get(&loc),
                    relation_builder,
                );
                self.relations.extend(relation);
            }

            // Process terminator.
            let term_loc = Location::new(block_id, block.statements.len());
            let relation = apply_gen_kill(
                &mut state,
                gen_map.get(&term_loc),
                kill_map.get(&term_loc),
                relation_builder,
            );
            self.relations.extend(relation);

            // Record callsite states for known callees.
            if let ullbc_ast::TerminatorKind::Call { call, .. } = &block.terminator.kind
                && let FnOperand::Regular(fn_ptr) = &call.func
                && let FnPtrKind::Fun(FunId::Regular(callee_id)) = fn_ptr.kind.as_ref()
            {
                callsite_updates
                    .entry((block_id, *callee_id))
                    .or_default()
                    .union(&state);
                callsite_hook.on_callsite(fun_id, *callee_id, term_loc, &state);
            }

            // Propagate to successors.
            let successors = terminator_successors(&block.terminator.kind);
            for succ in successors {
                let is_new = !entry_states.contains_key(succ);
                let changed = entry_states.entry(*succ).or_default().union(&state);
                if changed || is_new {
                    worklist.push_back(*succ);
                }
            }
        }

        // Gather exit states (Return terminators).
        let mut exit_state = LiveSet::new();
        for (block_id, block) in body.body.iter_indexed() {
            if matches!(block.terminator.kind, ullbc_ast::TerminatorKind::Return)
                && let Some(state) = entry_states.get(&block_id)
            {
                exit_state.union(state);
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

fn apply_gen_kill<T: DataflowEntity>(
    state: &mut LiveSet<T>,
    gen_set: Option<&FxHashSet<T>>,
    kill: Option<&FxHashSet<T>>,
    relation_builder: &mut dyn RelationBuilder<T>,
) -> FxHashSet<(T, T)> {
    if let Some(kill) = kill {
        let kill_set = LiveSet(kill.iter().copied().collect());
        state.difference(&kill_set);
    }
    let mut relations = FxHashSet::default();
    if let Some(gset) = gen_set {
        relations = relation_builder.build_relations(state, gset);
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
