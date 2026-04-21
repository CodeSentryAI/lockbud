//! Lockguard-specific wrapper around the generic dataflow engine.

use charon_lib::ast::*;
use charon_lib::export::CrateData;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::lockbud::callgraph::CallGraph;
use crate::lockbud::condvar;
use crate::lockbud::engine::{DataflowEngine, GenKillMaps, LiveSet, RelationBuilder, CallsiteHook};
use crate::lockbud::types::*;

/// Lockguard-specific relation builder.
pub struct LockGuardRelationBuilder;

impl RelationBuilder<LockGuardId> for LockGuardRelationBuilder {
    fn build_relations(
        &mut self,
        state: &LiveSet<LockGuardId>,
        gen_set: &FxHashSet<LockGuardId>,
    ) -> FxHashSet<(LockGuardId, LockGuardId)> {
        let mut relations = FxHashSet::default();
        for s in state.iter() {
            for g in gen_set.iter() {
                if s != g {
                    relations.insert((*s, *g));
                }
            }
        }
        relations
    }
}

/// Lockguard-specific callsite hook for condvar analysis.
pub struct LockGuardCallsiteHook<'a> {
    pub condvar_callsites: Option<&'a condvar::CondvarCallSites>,
    pub condvar_callsite_states: &'a mut FxHashMap<(FunDeclId, Location, FunDeclId), LiveLockGuards>,
}

impl<'a> CallsiteHook<LockGuardId> for LockGuardCallsiteHook<'a> {
    fn on_callsite(
        &mut self,
        caller: FunDeclId,
        callee: FunDeclId,
        loc: Location,
        state: &LiveSet<LockGuardId>,
    ) {
        if let Some(cv) = self.condvar_callsites
            && cv.contains_key(&(caller, loc, callee))
        {
            self.condvar_callsite_states
                .insert((caller, loc, callee), state.clone());
        }
    }
}

/// Intra- and inter-procedural analysis for lockguards.
/// Thin wrapper around the generic `DataflowEngine<LockGuardId>`.
pub struct Analyzer<'a> {
    engine: DataflowEngine<'a, LockGuardId>,
    /// Live lockguards before each condvar API call.
    pub condvar_callsite_states: FxHashMap<(FunDeclId, Location, FunDeclId), LiveLockGuards>,
    condvar_callsites: Option<&'a condvar::CondvarCallSites>,
}

impl<'a> Analyzer<'a> {
    pub fn new(
        crate_data: &'a CrateData,
        callgraph: &'a CallGraph,
        lockguards: &'a LockGuardMap,
    ) -> Self {
        let mut engine = DataflowEngine::new(crate_data, callgraph);

        // Build per-function gen/kill maps from lockguards.
        let krate = &crate_data.translated;
        for (fun_id, fun_decl) in krate.fun_decls.iter_indexed() {
            let Body::Unstructured(_body) = &fun_decl.body else {
                continue;
            };
            let mut gen_kill = GenKillMaps::default();
            for (guard_id, info) in lockguards.iter() {
                if guard_id.fun_id != fun_id {
                    continue;
                }
                for loc in &info.gen_locs {
                    gen_kill.gen_map.entry(*loc).or_default().insert(*guard_id);
                }
                for loc in &info.kill_locs {
                    gen_kill.kill_map.entry(*loc).or_default().insert(*guard_id);
                }
            }
            engine.add_function_gen_kill(fun_id, gen_kill);
        }

        Self {
            engine,
            condvar_callsite_states: FxHashMap::default(),
            condvar_callsites: None,
        }
    }

    pub fn with_condvar_callsites(mut self, condvar_callsites: &'a condvar::CondvarCallSites) -> Self {
        self.condvar_callsites = Some(condvar_callsites);
        self
    }

    pub fn analyze(&mut self) {
        let mut relation_builder = LockGuardRelationBuilder;
        let mut callsite_hook = LockGuardCallsiteHook {
            condvar_callsites: self.condvar_callsites,
            condvar_callsite_states: &mut self.condvar_callsite_states,
        };
        self.engine.analyze(&mut relation_builder, &mut callsite_hook);
    }

    /// Relations discovered during analysis: (a, b) means lockguard a is live when b is acquired.
    pub fn relations(&self) -> &FxHashSet<(LockGuardId, LockGuardId)> {
        &self.engine.relations
    }

    /// Context (live lockguards) at the entry of each function.
    pub fn contexts(&self) -> &FxHashMap<FunDeclId, LiveLockGuards> {
        &self.engine.contexts
    }

    /// Per-function analysis results.
    pub fn analyses(
        &self,
    ) -> &FxHashMap<FunDeclId, crate::lockbud::engine::FunctionAnalysis<LockGuardId>> {
        &self.engine.analyses
    }
}
