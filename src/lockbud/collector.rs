use charon_lib::ast::*;
use charon_lib::export::CrateData;
use charon_lib::ullbc_ast;

use crate::lockbud::types::*;

/// Collect lockguard info for each function.
pub struct LockGuardCollector<'a> {
    crate_data: &'a CrateData,
    pub lockguards: LockGuardMap,
}

impl<'a> LockGuardCollector<'a> {
    pub fn new(crate_data: &'a CrateData) -> Self {
        Self {
            crate_data,
            lockguards: LockGuardMap::new(),
        }
    }

    pub fn collect(&mut self) {
        let krate = &self.crate_data.translated;
        for (fun_id, fun_decl) in krate.fun_decls.iter_indexed() {
            // Only collect lockguards from the local crate, mirroring lockbud's `-l` filter.
            if !fun_decl.item_meta.is_local {
                continue;
            }
            let Body::Unstructured(body) = &fun_decl.body else {
                continue;
            };
            // Step 1: Identify lockguard locals by type.
            for (local_id, local) in body.locals.locals.iter_indexed() {
                if let Some(lg_ty) = LockGuardTy::from_ty(&local.ty, krate) {
                    let guard_id = LockGuardId::new(fun_id, local_id);
                    let info = LockGuardInfo::new(lg_ty, local.span);
                    self.lockguards.insert(guard_id, info);
                }
            }

            // Track locals that transitively wrap a lockguard and can be traced back
            // to a receiver (e.g., Result<Guard,_>, Option<Guard>, Guard itself).
            // Maps container local -> receiver place.
            let mut guard_containers: rustc_hash::FxHashMap<LocalId, Place> =
                rustc_hash::FxHashMap::default();

            // Track Ref chains: local created by `&place` -> place.
            let mut ref_places: rustc_hash::FxHashMap<LocalId, Place> =
                rustc_hash::FxHashMap::default();

            // Track move/copy chains: local assigned from a place -> place.
            let mut move_sources: rustc_hash::FxHashMap<LocalId, Place> =
                rustc_hash::FxHashMap::default();

            // Helper: recursively resolve a place through move_sources and ref_places.
            fn resolve_place(
                place: &Place,
                move_sources: &rustc_hash::FxHashMap<LocalId, Place>,
                ref_places: &rustc_hash::FxHashMap<LocalId, Place>,
                visited: &mut rustc_hash::FxHashSet<LocalId>,
            ) -> Place {
                match &place.kind {
                    PlaceKind::Local(local) => {
                        if !visited.insert(*local) {
                            return place.clone();
                        }
                        if let Some(src) = move_sources.get(local) {
                            return resolve_place(src, move_sources, ref_places, visited);
                        }
                        if let Some(src) = ref_places.get(local) {
                            return resolve_place(src, move_sources, ref_places, visited);
                        }
                        place.clone()
                    }
                    PlaceKind::Projection(inner, elem) => {
                        let resolved_inner = resolve_place(inner, move_sources, ref_places, visited);
                        Place {
                            kind: PlaceKind::Projection(Box::new(resolved_inner), elem.clone()),
                            ty: place.ty.clone(),
                        }
                    }
                    PlaceKind::Global(_) => place.clone(),
                }
            }

            // Helper: resolve receiver place for an operand.
            let resolve_arg_place =
                |arg: &Operand,
                 ref_places: &rustc_hash::FxHashMap<LocalId, Place>,
                 move_sources: &rustc_hash::FxHashMap<LocalId, Place>,
                 guard_containers: &rustc_hash::FxHashMap<LocalId, Place>,
                 lockguards: &LockGuardMap,
                 fun_id: FunDeclId|
                 -> Option<Place> {
                    match arg {
                        Operand::Copy(place) | Operand::Move(place) => {
                            let mut visited = rustc_hash::FxHashSet::default();
                            let resolved = resolve_place(place, move_sources, ref_places, &mut visited);
                            if let Some(local) = resolved.local_id() {
                                if let Some(p) = guard_containers.get(&local) {
                                    let mut v2 = rustc_hash::FxHashSet::default();
                                    return Some(resolve_place(p, move_sources, ref_places, &mut v2));
                                }
                                if let Some(info) =
                                    lockguards.get(&LockGuardId::new(fun_id, local))
                                {
                                    if let Some(ref rp) = info.receiver_place {
                                        let mut v2 = rustc_hash::FxHashSet::default();
                                        return Some(resolve_place(rp, move_sources, ref_places, &mut v2));
                                    }
                                }
                            }
                            Some(resolved)
                        }
                        Operand::Const(_) => None,
                    }
                };

            // Step 2: Walk the body to find gen/kill locs.
            for (block_id, block) in body.body.iter_indexed() {
                for (stmt_idx, stmt) in block.statements.iter().enumerate() {
                    let loc = Location::new(block_id, stmt_idx);
                    match &stmt.kind {
                        ullbc_ast::StatementKind::Assign(dest, rvalue) => {
                            if let Some(dest_local) = dest.local_id() {
                                // Propagate container mapping for moves/copies of containers.
                                if let Rvalue::Use(Operand::Move(src) | Operand::Copy(src)) = rvalue
                                {
                                    if let Some(dest_local) = dest.local_id() {
                                        move_sources.insert(dest_local, src.clone());
                                        if let Some(src_local) = src.local_id() {
                                            if let Some(p) = guard_containers.get(&src_local) {
                                                guard_containers.insert(dest_local, p.clone());
                                            }
                                        }
                                    }
                                }
                                // Record ref places for alias resolution.
                                if let Rvalue::Ref { place, .. } = rvalue {
                                    ref_places.insert(dest_local, place.clone());
                                }
                                let guard_id = LockGuardId::new(fun_id, dest_local);
                                // Do NOT treat a mere move/copy of an existing lockguard
                                // as a new gen (e.g., temp copies for function arguments).
                                let is_alias = match rvalue {
                                    Rvalue::Use(Operand::Move(src) | Operand::Copy(src)) => {
                                        src.local_id().map(|src_local| {
                                            self.lockguards.contains_key(&LockGuardId::new(
                                                fun_id, src_local,
                                            ))
                                        }).unwrap_or(false)
                                    }
                                    _ => false,
                                };
                                if !is_alias {
                                    if let Some(info) = self.lockguards.get_mut(&guard_id) {
                                        // This is a gen location.
                                        info.gen_locs.push(loc);
                                        // Try to extract receiver if this is a lock() call.
                                        if let Rvalue::Use(Operand::Move(place)) = rvalue {
                                            info.receiver_place = Some(place.clone());
                                        }
                                        // Also try Ref receiver patterns.
                                        if let Rvalue::Ref { place, .. } = rvalue {
                                            info.receiver_place = Some(place.clone());
                                        }
                                    }
                                }
                            }
                            // If the rvalue moves a lockguard into another local,
                            // treat the source as killed at this loc.
                            if let Rvalue::Use(Operand::Move(src_place)) = rvalue {
                                if let Some(src_local) = src_place.local_id() {
                                    let src_id = LockGuardId::new(fun_id, src_local);
                                    if let Some(info) = self.lockguards.get_mut(&src_id) {
                                        info.kill_locs.push(loc);
                                    }
                                }
                            }
                        }
                        ullbc_ast::StatementKind::StorageDead(local) => {
                            let guard_id = LockGuardId::new(fun_id, *local);
                            if let Some(info) = self.lockguards.get_mut(&guard_id) {
                                info.kill_locs.push(loc);
                            }
                        }
                        _ => {}
                    }
                }

                // Step 3: Terminator-level gen (lock() calls) and kill (drop).
                let term_loc = Location::new(block_id, block.statements.len());
                match &block.terminator.kind {
                    ullbc_ast::TerminatorKind::Call { call, .. } => {
                        // For is_lock_call, receiver is the first argument.
                        let lock_recv = call.args.first().and_then(|arg| {
                            resolve_arg_place(arg, &ref_places, &move_sources, &guard_containers, &self.lockguards, fun_id)
                        });

                        // For other calls, resolve receiver from arguments using containers/known-guards.
                        let resolved_recv = call
                            .args
                            .iter()
                            .filter_map(|arg| {
                                resolve_arg_place(arg, &ref_places, &move_sources, &guard_containers, &self.lockguards, fun_id)
                            })
                            .next();

                        if is_lock_call(&call.func, krate) {
                            if let Some(dest_local) = call.dest.local_id() {
                                let guard_id = LockGuardId::new(fun_id, dest_local);
                                if let Some(info) = self.lockguards.get_mut(&guard_id) {
                                    info.gen_locs.push(term_loc);
                                    if let Some(r) = lock_recv {
                                        info.receiver_place = Some(r);
                                    }
                                } else if let Some(r) = lock_recv {
                                    guard_containers.insert(dest_local, r);
                                }
                            }
                        }
                        let is_rec_read = is_read_recursive_call(&call.func, krate);
                        // Any call that returns a lockguard is also a gen.
                        if let Some(dest_local) = call.dest.local_id() {
                            let guard_id = LockGuardId::new(fun_id, dest_local);
                            if let Some(info) = self.lockguards.get_mut(&guard_id) {
                                info.gen_locs.push(term_loc);
                                if is_rec_read {
                                    info.is_recursive_read = true;
                                }
                                if let Some(r) = resolved_recv {
                                    info.receiver_place = Some(r);
                                }
                            } else if let Some(r) = resolved_recv {
                                // dest is not a guard but receives a container/guard;
                                // propagate so downstream unwrap() can resolve receiver.
                                guard_containers.insert(dest_local, r);
                            }
                        }
                    }
                    ullbc_ast::TerminatorKind::Drop { place, .. } => {
                        if let Some(local) = place.local_id() {
                            let guard_id = LockGuardId::new(fun_id, local);
                            if let Some(info) = self.lockguards.get_mut(&guard_id) {
                                info.kill_locs.push(term_loc);
                            }
                        }
                    }
                    _ => {}
                }
            }

            // Step 4: Resolve receiver places by following move/ref chains.
            for (guard_id, info) in self.lockguards.iter_mut() {
                if guard_id.fun_id != fun_id {
                    continue;
                }
                if let Some(ref rp) = info.receiver_place {
                    let mut visited = rustc_hash::FxHashSet::default();
                    info.receiver_place = Some(resolve_place(rp, &move_sources, &ref_places, &mut visited));
                }
            }
        }
    }
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

pub fn is_lock_call(func: &FnOperand, krate: &TranslatedCrate) -> bool {
    match func {
        FnOperand::Regular(fn_ptr) => match fn_ptr.kind.as_ref() {
            FnPtrKind::Fun(FunId::Regular(fun_id)) => {
                let decl = krate.fun_decls.get(*fun_id);
                if let Some(decl) = decl {
                    let name = format_name(&decl.item_meta.name);
                    name.contains("::lock") && !name.contains("lock_contended")
                } else {
                    false
                }
            }
            _ => false,
        },
        FnOperand::Dynamic(_) => false,
    }
}

pub fn is_read_recursive_call(func: &FnOperand, krate: &TranslatedCrate) -> bool {
    match func {
        FnOperand::Regular(fn_ptr) => match fn_ptr.kind.as_ref() {
            FnPtrKind::Fun(FunId::Regular(fun_id)) => {
                let decl = krate.fun_decls.get(*fun_id);
                if let Some(decl) = decl {
                    let name = format_name(&decl.item_meta.name);
                    name.contains("read_recursive")
                } else {
                    false
                }
            }
            _ => false,
        },
        FnOperand::Dynamic(_) => false,
    }
}

fn operand_to_local_id(op: &Operand) -> Option<LocalId> {
    match op {
        Operand::Copy(place) | Operand::Move(place) => place.local_id(),
        Operand::Const(_) => None,
    }
}
