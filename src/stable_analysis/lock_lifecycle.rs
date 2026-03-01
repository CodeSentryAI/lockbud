//! MutexGuard lifecycle tracking using MirVisitor
//!
//! This module tracks the lifecycle of MutexGuard instances through their
//! creation, moves, and drops.

use std::collections::HashMap;

use stable_mir_wrapper::{
    Body, Instance, MirVisitor, Local, Terminator, TerminatorKind,
    Statement, StatementKind, Rvalue, Operand, Place,
};

use crate::stable_analysis::lock_types::{LockGuardId, LockGuardInfo, LockGuardTy, MirLocation};

/// Collector for tracking MutexGuard lifecycle events
pub struct LockLifecycleCollector<'a> {
    instance: Instance,
    pub body: &'a Body,
    pub lockguards: HashMap<LockGuardId, LockGuardInfo>,
    // Tracking fields for visitor
    current_block: Option<stable_mir_wrapper::BasicBlockIdx>,
    current_location: Option<MirLocation>,
}

impl<'a> LockLifecycleCollector<'a> {
    pub fn new(instance: Instance, body: &'a Body) -> Self {
        Self {
            instance,
            body,
            lockguards: HashMap::new(),
            current_block: None,
            current_location: None,
        }
    }

    /// Main analysis entry point
    pub fn analyze(&mut self) {
        // Phase 1: Collect all locals with MutexGuard type
        self.collect_guard_locals();

        // Phase 2: Track lifecycle events via MirVisitor
        self.visit_body(self.body);

        // Phase 3: Determine guarded basic blocks
        self.compute_guarded_blocks();
    }

    /// Phase 1: Identify all locals with MutexGuard type
    fn collect_guard_locals(&mut self) {
        let func_name = self.instance.name();

        for (local, local_decl) in self.body.local_decls() {
            let local_ty = local_decl.ty;

            if let Some(lockguard_ty) = LockGuardTy::from_ty(&local_ty) {
                let id = LockGuardId::new(self.instance, local);

                let span_str = format!("{:?}", local_decl);

                let mut info = LockGuardInfo::new(lockguard_ty, span_str, func_name.clone());
                // Note: Variable name extraction from debug info not yet implemented
                // Source location extraction from spans not yet implemented
                info.var_name = None;
                info.source_loc = None;

                self.lockguards.insert(id, info);
            }
        }
    }

    /// Phase 3: Determine which basic blocks each guard protects
    fn compute_guarded_blocks(&mut self) {
        // For each guard, find which blocks it guards
        // A guard is live from its gen_loc to its kill_loc
        // We need to track all blocks reachable between gen and kill

        // Collect guard_ids first to avoid borrow checker issues
        let guard_ids: Vec<_> = self.lockguards.keys().copied().collect();

        for guard_id in guard_ids {
            let pairs = {
                let info = self.lockguards.get(&guard_id).unwrap();
                // Pair up gen_locs with kill_locs (they come in chronological order)
                let mut pairs = Vec::new();
                let mut kill_iter = info.kill_locs.iter().peekable();

                for gen_loc in &info.gen_locs {
                    // Find the matching kill_loc
                    while let Some(&kill_loc) = kill_iter.peek() {
                        if kill_loc.block >= gen_loc.block {
                            pairs.push((gen_loc.clone(), kill_loc.clone()));
                            break;
                        } else {
                            kill_iter.next();
                        }
                    }
                }
                pairs
            };

            // For each gen-kill pair, find all blocks in that range
            for (gen, kill) in pairs {
                self.mark_blocks_as_guarded(guard_id, gen.block, kill.block);
            }
        }
    }

    /// Mark all blocks reachable from start to end as guarded
    fn mark_blocks_as_guarded(
        &mut self,
        guard_id: LockGuardId,
        start: stable_mir_wrapper::BasicBlockIdx,
        end: stable_mir_wrapper::BasicBlockIdx,
    ) {
        // Perform forward traversal from start to end
        let mut worklist = vec![start];
        let mut visited = std::collections::HashSet::new();

        while let Some(bb) = worklist.pop() {
            if !visited.insert(bb) {
                continue;
            }

            // Record this block as guarded
            if let Some(info) = self.lockguards.get_mut(&guard_id) {
                let loc = MirLocation { block: bb, statement_index: 0 };
                info.guarded_blocks.push((bb, loc));
            }

            // Stop if we reached the end block
            if bb == end {
                continue;
            }

            // Add successors
            if let Some(terminator) = self.body.blocks.get(bb).map(|b| &b.terminator) {
                for successor in terminator.successors() {
                    worklist.push(successor);
                }
            }
        }
    }

    /// Check if a function call is a mutex lock operation
    fn is_mutex_lock_call(&self, func: &Operand) -> bool {
        // Extract function type
        let func_ty = match func {
            Operand::Copy(place) | Operand::Move(place) => {
                place.ty(self.body.locals()).ok()
            }
            Operand::Constant(_) => None,
        };

        // Check if it's a Mutex::lock() call
        if let Some(ty) = func_ty {
            if let Some(rigid) = ty.kind().rigid() {
                match rigid {
                    stable_mir_wrapper::RigidTy::FnDef(fn_def, _) => {
                        let name = format!("{:?}", fn_def);
                        return name.contains("Mutex::lock")
                            || name.contains("MutexGuard::")
                            || name.contains("RwLock::read")
                            || name.contains("RwLock::write");
                    }
                    _ => {}
                }
            }
        }
        false
    }
}

impl MirVisitor for LockLifecycleCollector<'_> {
    fn visit_body(&mut self, body: &Body) {
        for (bb_idx, bb) in body.blocks.iter().enumerate() {
            self.current_block = Some(bb_idx);

            // Visit statements
            for (stmt_idx, stmt) in bb.statements.iter().enumerate() {
                self.current_location = Some(MirLocation::new(bb_idx, stmt_idx));
                // Create a dummy Location - the Location parameter is not used in our implementation
                let loc = unsafe { std::mem::zeroed() };
                self.visit_statement(stmt, loc);
            }

            // Visit terminator
            self.current_location = Some(MirLocation::new(bb_idx, 999)); // Terminator at end
            let loc = unsafe { std::mem::zeroed() };
            self.visit_terminator(&bb.terminator, loc);

            self.current_block = None;
            self.current_location = None;
        }
    }

    fn visit_terminator(&mut self, terminator: &Terminator, _location: stable_mir_wrapper::Location) {
        let mir_loc = self.current_location.unwrap_or_else(|| MirLocation::new(0, 0));

        match &terminator.kind {
            TerminatorKind::Call { func, destination, .. } => {
                let dest_local = destination.local;

                // Check if destination is a guard we're tracking
                let guard_id = LockGuardId::new(self.instance, dest_local);

                if self.lockguards.contains_key(&guard_id) {
                    // This is a lock call (the destination is a known guard)
                    if let Some(info) = self.lockguards.get_mut(&guard_id) {
                        info.gen_locs.push(mir_loc);
                    }
                }
            }

            TerminatorKind::Drop { place, .. } => {
                // Check if we're dropping a MutexGuard
                let local = place.local;
                let guard_id = LockGuardId::new(self.instance, local);

                if self.lockguards.contains_key(&guard_id) {
                    if let Some(info) = self.lockguards.get_mut(&guard_id) {
                        info.kill_locs.push(mir_loc);
                    }
                }
            }

            _ => {}
        }

        self.super_terminator(terminator, _location);
    }

    fn visit_statement(&mut self, statement: &Statement, _location: stable_mir_wrapper::Location) {
        let mir_loc = self.current_location.unwrap_or_else(|| MirLocation::new(0, 0));

        // Track assignments that might create or move guards
        match &statement.kind {
            StatementKind::Assign(place, rvalue) => {
                // Check if this assigns to a guard local
                let dest_local = place.local;
                let guard_id = LockGuardId::new(self.instance, dest_local);

                if self.lockguards.contains_key(&guard_id) {
                    // Check if this is a move from another guard
                    if let Rvalue::Use(operand) = rvalue {
                        if let Operand::Move(source) | Operand::Copy(source) = operand {
                            let source_local = source.local;
                            let source_id = LockGuardId::new(self.instance, source_local);

                            if self.lockguards.contains_key(&source_id) {
                                // This is a move - update both guards
                                if let Some(info) = self.lockguards.get_mut(&guard_id) {
                                    info.gen_locs.push(mir_loc);
                                    info.move_gen_locs.push(mir_loc);
                                }
                            }
                        }
                    } else {
                        // Direct assignment (likely from lock call result)
                        if let Some(info) = self.lockguards.get_mut(&guard_id) {
                            info.gen_locs.push(mir_loc);
                        }
                    }
                }
            }

            StatementKind::FakeRead(_, _) => {
                // Track moves through FakeRead
            }

            _ => {}
        }

        self.super_statement(statement, _location);
    }
}
