use charon_lib::ast::*;
use charon_lib::export::CrateData;
use charon_lib::ullbc_ast;
use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};

/// Identify whether a type is a known lock guard type by inspecting the type declaration name.
fn is_lock_guard_ty(ty: &Ty, crate_data: &TranslatedCrate) -> Option<LockGuardKind> {
    match ty.kind() {
        TyKind::Adt(adt_ref) => {
            let type_decl_id = match adt_ref.id {
                TypeId::Adt(id) => id,
                _ => return None,
            };
            let decl = crate_data.type_decls.get(type_decl_id)?;
            let name = format_name(&decl.item_meta.name);
            if name.contains("MutexGuard") {
                Some(LockGuardKind::MutexGuard)
            } else if name.contains("RwLockReadGuard") {
                Some(LockGuardKind::RwLockReadGuard)
            } else if name.contains("RwLockWriteGuard") {
                Some(LockGuardKind::RwLockWriteGuard)
            } else {
                None
            }
        }
        _ => None,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum LockGuardKind {
    MutexGuard,
    RwLockReadGuard,
    RwLockWriteGuard,
    Unknown, // Used when we detect lock() but the guard type is not directly available.
}

/// We track locks by the `LocalId` that holds the call result.
/// The lock "receiver" is the place on which `lock()` is called.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ActiveLock {
    /// The local that stores the result of the `lock()` call (may be `Result<Guard>`).
    guard_local: LocalId,
    /// The local/place that is the receiver of `lock()`.
    receiver_local: LocalId,
    kind: LockGuardKind,
}

impl Hash for ActiveLock {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.guard_local.hash(state);
        self.receiver_local.hash(state);
        self.kind.hash(state);
    }
}

pub fn detect(crate_data: &CrateData) {
    let krate = &crate_data.translated;
    for (fun_id, fun_decl) in krate.fun_decls.iter_indexed() {
        let Body::Unstructured(body) = &fun_decl.body else {
            continue;
        };

        // Find locals that are lock guards (for precise type info).
        let guard_locals: BTreeSet<LocalId> = body
            .locals
            .locals
            .iter_indexed()
            .filter_map(|(id, local)| is_lock_guard_ty(&local.ty, krate).map(|_| id))
            .collect();

        // Build a borrow map for the whole function body.
        let mut borrow_map: HashMap<LocalId, LocalId> = HashMap::new();
        for block in body.body.iter() {
            for stmt in &block.statements {
                match &stmt.kind {
                    ullbc_ast::StatementKind::Assign(dest, rvalue) => {
                        if let Rvalue::Ref { place, .. } = rvalue {
                            if let Some(base) = place.local_id() {
                                if let Some(id) = dest.local_id() {
                                    borrow_map.insert(id, base);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // DFS over the CFG to find double locks.
        let mut visited: HashMap<(ullbc_ast::BlockId, BTreeSet<ActiveLock>), ()> = HashMap::new();
        let initial_state = BTreeSet::new();
        dfs(
            krate,
            fun_id,
            &fun_decl.item_meta.name,
            body,
            &guard_locals,
            ullbc_ast::START_BLOCK_ID,
            initial_state,
            &mut visited,
        );
    }
}

fn dfs(
    krate: &TranslatedCrate,
    _fun_id: FunDeclId,
    fun_name: &Name,
    body: &ullbc_ast::ExprBody,
    _guard_locals: &BTreeSet<LocalId>,
    block_id: ullbc_ast::BlockId,
    mut state: BTreeSet<ActiveLock>,
    visited: &mut HashMap<(ullbc_ast::BlockId, BTreeSet<ActiveLock>), ()>,
) {
    let key = (block_id, state.clone());
    if visited.contains_key(&key) {
        return;
    }
    visited.insert(key, ());

    let block = &body.body[block_id];

    println!(
        "  [BLOCK-ENTER] {} block={:?} state_len={}",
        format_name(fun_name), block_id, state.len()
    );

        // Build a borrow map: borrow_temp_local -> base_local of the borrowed place.
        let mut borrow_map: HashMap<LocalId, LocalId> = HashMap::new();
        for stmt in &block.statements {
            match &stmt.kind {
                ullbc_ast::StatementKind::StorageDead(_) => {}
                ullbc_ast::StatementKind::Assign(dest, rvalue) => {
                if let Rvalue::Ref { place, .. } = rvalue {
                    if let Some(base) = place.local_id() {
                        if let Some(id) = dest.local_id() {
                            borrow_map.insert(id, base);
                        }
                    }
                }
                }
                _ => {}
            }
        }

    // Process terminator.
    match &block.terminator.kind {
        ullbc_ast::TerminatorKind::Call { call, target, on_unwind: _ } => {
            if is_lock_call(&call.func, krate) {
                // Extract receiver from first argument.
                if let Some(receiver) = call.args.first() {
                    println!("  [LOCK-CALL-ARG] receiver operand: {:?}", receiver);
                    let receiver_local = operand_to_local_id(receiver);
                    let guard_local = place_to_local_id(&call.dest);

                    if let (Some(rl), Some(gl)) = (receiver_local, guard_local) {
                        // Resolve through borrow map if the receiver is a borrow temporary.
                        let resolved_rl = *borrow_map.get(&rl).unwrap_or(&rl);

                        println!(
                            "  [LOCK-CALL] receiver={:?} (base={:?}) guard={:?} in {} block {:?}",
                            rl, resolved_rl, gl, format_name(fun_name), block_id
                        );
                        // we warn. This works well for simple test cases.
                        for active in &state {
                            let active_resolved = *borrow_map.get(&active.receiver_local).unwrap_or(&active.receiver_local);
                            // Relaxed: same base receiver or same guard local means double lock
                            if active_resolved == resolved_rl || active.guard_local == gl {
                                let fun_name_str = format_name(fun_name);
                                println!(
                                    "[DOUBLE_LOCK] {:?} in function {} (block {:?})",
                                    active.kind, fun_name_str, block_id
                                );
                                println!(
                                    "  -> Acquiring lock on local {:?} while already holding a guard in local {:?}",
                                    rl, active.guard_local
                                );
                            }
                        }

                        // Determine kind from destination local type if possible.
                        let kind = body
                            .locals
                            .locals
                            .get(gl)
                            .and_then(|l| is_lock_guard_ty(&l.ty, krate))
                            .unwrap_or(LockGuardKind::Unknown);

                        state.insert(ActiveLock {
                            guard_local: gl,
                            receiver_local: rl,
                            kind,
                        });
                    }
                }
            }

            // Continue to target block.
            dfs(
                krate, _fun_id, fun_name, body, _guard_locals, *target, state, visited,
            );
        }
        ullbc_ast::TerminatorKind::Drop { place, target, on_unwind: _, .. } => {
            if let Some(dropped_local) = place.local_id() {
                state.retain(|l| l.guard_local != dropped_local);
            }
            dfs(
                krate, _fun_id, fun_name, body, _guard_locals, *target, state, visited,
            );
        }
        ullbc_ast::TerminatorKind::Goto { target } => {
            dfs(
                krate, _fun_id, fun_name, body, _guard_locals, *target, state, visited,
            );
        }
        ullbc_ast::TerminatorKind::Switch { targets, .. } => {
            for target in targets.targets() {
                dfs(
                    krate,
                    _fun_id,
                    fun_name,
                    body,
                    _guard_locals,
                    target,
                    state.clone(),
                    visited,
                );
            }
        }
        ullbc_ast::TerminatorKind::Return => {}
        ullbc_ast::TerminatorKind::Abort(_) => {}
        ullbc_ast::TerminatorKind::UnwindResume => {}
        ullbc_ast::TerminatorKind::Assert { target, on_unwind, .. } => {
            dfs(
                krate, _fun_id, fun_name, body, _guard_locals, *target, state.clone(), visited,
            );
            dfs(
                krate,
                _fun_id,
                fun_name,
                body,
                _guard_locals,
                *on_unwind,
                state,
                visited,
            );
        }
    }
}

/// Check if a function operand refers to `lock()`.
fn is_lock_call(func: &FnOperand, krate: &TranslatedCrate) -> bool {
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

fn operand_to_local_id(op: &Operand) -> Option<LocalId> {
    match op {
        Operand::Copy(place) | Operand::Move(place) => place.local_id(),
        Operand::Const(_) => None,
    }
}

fn place_to_local_id(place: &Place) -> Option<LocalId> {
    place.local_id()
}
