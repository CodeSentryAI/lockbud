use charon_lib::ast::*;
use charon_lib::export::CrateData;
use charon_lib::ullbc_ast;
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::VecDeque;

/// A simple callgraph over ULLBC.
/// Nodes are FunDeclId; edges are directed from caller to callee.
#[derive(Debug, Default)]
pub struct CallGraph {
    /// adjacency: caller -> set of callees
    pub edges: FxHashMap<FunDeclId, FxHashSet<FunDeclId>>,
    /// reverse edges: callee -> set of callers
    pub reverse_edges: FxHashMap<FunDeclId, FxHashSet<FunDeclId>>,
    /// callsites: (caller, callee) -> list of block ids where the call happens
    pub callsites: FxHashMap<(FunDeclId, FunDeclId), Vec<ullbc_ast::BlockId>>,
}

impl CallGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn build(crate_data: &CrateData) -> Self {
        let krate = &crate_data.translated;
        let mut cg = CallGraph::new();
        for (fun_id, fun_decl) in krate.fun_decls.iter_indexed() {
            let Body::Unstructured(body) = &fun_decl.body else {
                continue;
            };
            for (block_id, block) in body.body.iter_indexed() {
                if let ullbc_ast::TerminatorKind::Call { call, .. } = &block.terminator.kind {
                    if let FnOperand::Regular(fn_ptr) = &call.func {
                        if let FnPtrKind::Fun(FunId::Regular(callee_id)) = fn_ptr.kind.as_ref() {
                            cg.add_edge(fun_id, *callee_id, block_id);
                        }
                    }
                }
            }
        }
        cg
    }

    fn add_edge(
        &mut self,
        caller: FunDeclId,
        callee: FunDeclId,
        block_id: ullbc_ast::BlockId,
    ) {
        self.edges.entry(caller).or_default().insert(callee);
        self.reverse_edges.entry(callee).or_default().insert(caller);
        self.callsites
            .entry((caller, callee))
            .or_default()
            .push(block_id);
    }

    pub fn nodes(&self) -> Vec<FunDeclId> {
        let mut set = FxHashSet::default();
        for &id in self.edges.keys() {
            set.insert(id);
        }
        for &id in self.reverse_edges.keys() {
            set.insert(id);
        }
        set.into_iter().collect()
    }

    pub fn callees(&self, fun_id: FunDeclId) -> impl Iterator<Item = FunDeclId> + '_ {
        self.edges
            .get(&fun_id)
            .into_iter()
            .flat_map(|s| s.iter().copied())
    }

    pub fn callers(&self, fun_id: FunDeclId) -> impl Iterator<Item = FunDeclId> + '_ {
        self.reverse_edges
            .get(&fun_id)
            .into_iter()
            .flat_map(|s| s.iter().copied())
    }

    /// Returns true if `source` can reach `target` via call edges.
    pub fn can_reach(&self, source: FunDeclId, target: FunDeclId) -> bool {
        if source == target {
            return true;
        }
        let mut visited = FxHashSet::default();
        let mut queue = VecDeque::new();
        queue.push_back(source);
        visited.insert(source);
        while let Some(node) = queue.pop_front() {
            if let Some(nexts) = self.edges.get(&node) {
                for &next in nexts {
                    if next == target {
                        return true;
                    }
                    if visited.insert(next) {
                        queue.push_back(next);
                    }
                }
            }
        }
        false
    }

    /// Returns true if there exists a node that can reach both `a` and `b`.
    pub fn share_common_ancestor(&self, a: FunDeclId, b: FunDeclId) -> bool {
        if a == b {
            return true;
        }
        // Reverse-reachable sets from a and b.
        let rev_a = self.reverse_reachable(a);
        let rev_b = self.reverse_reachable(b);
        !rev_a.is_disjoint(&rev_b)
    }

    fn reverse_reachable(&self, start: FunDeclId) -> FxHashSet<FunDeclId> {
        let mut visited = FxHashSet::default();
        let mut queue = VecDeque::new();
        queue.push_back(start);
        visited.insert(start);
        while let Some(node) = queue.pop_front() {
            if let Some(preds) = self.reverse_edges.get(&node) {
                for &pred in preds {
                    if visited.insert(pred) {
                        queue.push_back(pred);
                    }
                }
            }
        }
        visited
    }

    /// Returns all simple paths from `source` to `target` up to a depth limit.
    pub fn all_simple_paths(
        &self,
        source: FunDeclId,
        target: FunDeclId,
        max_depth: usize,
    ) -> Vec<Vec<FunDeclId>> {
        let mut results = Vec::new();
        let mut stack = vec![(source, vec![source])];
        while let Some((node, path)) = stack.pop() {
            if node == target && path.len() > 1 {
                results.push(path.clone());
            }
            if path.len() >= max_depth {
                continue;
            }
            if let Some(nexts) = self.edges.get(&node) {
                for &next in nexts {
                    if !path.contains(&next) {
                        let mut new_path = path.clone();
                        new_path.push(next);
                        stack.push((next, new_path));
                    }
                }
            }
        }
        results
    }
}
