use charon_lib::ast::*;
use rustc_hash::FxHashSet;
use std::cmp::Ordering;

/// Uniquely identify a LockGuard in a crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LockGuardId {
    pub fun_id: FunDeclId,
    pub local: LocalId,
}

impl LockGuardId {
    pub fn new(fun_id: FunDeclId, local: LocalId) -> Self {
        Self { fun_id, local }
    }
}

/// The possibility of deadlock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeadlockPossibility {
    Probably,
    Possibly,
    Unlikely,
    Unknown,
}

impl PartialOrd for DeadlockPossibility {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        use DeadlockPossibility::*;
        match (*self, *other) {
            (Probably, Probably)
            | (Possibly, Possibly)
            | (Unlikely, Unlikely)
            | (Unknown, Unknown) => Some(Ordering::Equal),
            (Probably, _) | (Possibly, Unlikely) | (Possibly, Unknown) | (Unlikely, Unknown) => {
                Some(Ordering::Greater)
            }
            (_, Probably) | (Unlikely, Possibly) | (Unknown, Possibly) | (Unknown, Unlikely) => {
                Some(Ordering::Less)
            }
        }
    }
}

/// LockGuardKind with the data type it protects.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LockGuardTy {
    StdMutex(Ty),
    ParkingLotMutex(Ty),
    SpinMutex(Ty),
    StdRwLockRead(Ty),
    StdRwLockWrite(Ty),
    ParkingLotRead(Ty),
    ParkingLotWrite(Ty),
    SpinRead(Ty),
    SpinWrite(Ty),
    Unknown,
}

pub fn format_name(name: &Name) -> String {
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

impl LockGuardTy {
    pub fn from_ty(ty: &Ty, crate_data: &TranslatedCrate) -> Option<Self> {
        match ty.kind() {
            TyKind::Adt(adt_ref) => {
                let type_decl_id = match adt_ref.id {
                    TypeId::Adt(id) => id,
                    _ => return None,
                };
                let decl = crate_data.type_decls.get(type_decl_id)?;
                let path = format_name(&decl.item_meta.name);
                // quick fail
                if !path.contains("MutexGuard")
                    && !path.contains("RwLockReadGuard")
                    && !path.contains("RwLockWriteGuard")
                {
                    return None;
                }
                let first_part = path.split('<').next()?;
                if first_part.contains("MutexGuard") {
                    if first_part.contains("async")
                        || first_part.contains("tokio")
                        || first_part.contains("future")
                        || first_part.contains("loom")
                    {
                        None
                    } else if first_part.contains("spin") {
                        Some(LockGuardTy::SpinMutex(extract_first_type_arg_from_name(&decl.item_meta.name)?))
                    } else if first_part.contains("lock_api") || first_part.contains("parking_lot")
                    {
                        Some(LockGuardTy::ParkingLotMutex(extract_second_type_arg_from_name(&decl.item_meta.name)?))
                    } else {
                        Some(LockGuardTy::StdMutex(extract_first_type_arg_from_name(&decl.item_meta.name)?))
                    }
                } else if first_part.contains("RwLockReadGuard") {
                    if first_part.contains("async")
                        || first_part.contains("tokio")
                        || first_part.contains("future")
                        || first_part.contains("loom")
                    {
                        None
                    } else if first_part.contains("spin") {
                        Some(LockGuardTy::SpinRead(extract_first_type_arg_from_name(&decl.item_meta.name)?))
                    } else if first_part.contains("lock_api") || first_part.contains("parking_lot")
                    {
                        Some(LockGuardTy::ParkingLotRead(extract_second_type_arg_from_name(&decl.item_meta.name)?))
                    } else {
                        Some(LockGuardTy::StdRwLockRead(extract_first_type_arg_from_name(&decl.item_meta.name)?))
                    }
                } else if first_part.contains("RwLockWriteGuard") {
                    if first_part.contains("async")
                        || first_part.contains("tokio")
                        || first_part.contains("future")
                        || first_part.contains("loom")
                    {
                        None
                    } else if first_part.contains("spin") {
                        Some(LockGuardTy::SpinWrite(extract_first_type_arg_from_name(&decl.item_meta.name)?))
                    } else if first_part.contains("lock_api") || first_part.contains("parking_lot")
                    {
                        Some(LockGuardTy::ParkingLotWrite(extract_second_type_arg_from_name(&decl.item_meta.name)?))
                    } else {
                        Some(LockGuardTy::StdRwLockWrite(extract_first_type_arg_from_name(&decl.item_meta.name)?))
                    }
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub fn deadlock_with(&self, other: &Self) -> DeadlockPossibility {
        use LockGuardTy::*;
        match (self, other) {
            (StdMutex(a), StdMutex(b))
            | (ParkingLotMutex(a), ParkingLotMutex(b))
            | (SpinMutex(a), SpinMutex(b))
            | (StdRwLockWrite(a), StdRwLockWrite(b))
            | (StdRwLockWrite(a), StdRwLockRead(b))
            | (StdRwLockRead(a), StdRwLockWrite(b))
            | (ParkingLotWrite(a), ParkingLotWrite(b))
            | (ParkingLotWrite(a), ParkingLotRead(b))
            | (ParkingLotRead(a), ParkingLotWrite(b))
            | (SpinWrite(a), SpinWrite(b))
            | (SpinWrite(a), SpinRead(b))
            | (SpinRead(a), SpinWrite(b))
                if a == b =>
            {
                DeadlockPossibility::Probably
            }
            (StdRwLockRead(a), StdRwLockRead(b)) | (ParkingLotRead(a), ParkingLotRead(b))
                if a == b =>
            {
                DeadlockPossibility::Possibly
            }
            _ => DeadlockPossibility::Unlikely,
        }
    }
}

fn extract_first_type_arg_from_name(name: &Name) -> Option<Ty> {
    for elem in &name.name {
        if let PathElem::Instantiated(binder) = elem {
            let types = &binder.skip_binder.types;
            let id: TypeVarId = 0_usize.into();
            return types.get(id).cloned();
        }
    }
    None
}

fn extract_second_type_arg_from_name(name: &Name) -> Option<Ty> {
    for elem in &name.name {
        if let PathElem::Instantiated(binder) = elem {
            let types = &binder.skip_binder.types;
            let id: TypeVarId = 1_usize.into();
            return types.get(id).cloned();
        }
    }
    None
}

fn extract_first_type_arg(ty: &Ty) -> Option<Ty> {
    match ty.kind() {
        TyKind::Adt(adt_ref) => {
            let substs = &adt_ref.generics.types;
            let id: TypeVarId = 0_usize.into();
            substs.get(id).cloned()
        }
        _ => None,
    }
}

fn extract_second_type_arg(ty: &Ty) -> Option<Ty> {
    match ty.kind() {
        TyKind::Adt(adt_ref) => {
            let substs = &adt_ref.generics.types;
            let id: TypeVarId = 1_usize.into();
            substs.get(id).cloned()
        }
        _ => None,
    }
}

/// A location inside a function body: (block, statement_index).
/// statement_index == statements.len() means the terminator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Location {
    pub block: charon_lib::ullbc_ast::BlockId,
    pub statement_index: usize,
}

impl Location {
    pub fn new(block: charon_lib::ullbc_ast::BlockId, statement_index: usize) -> Self {
        Self {
            block,
            statement_index,
        }
    }
}

/// The lockguard info.
#[derive(Clone, Debug)]
pub struct LockGuardInfo {
    pub lockguard_ty: LockGuardTy,
    pub span: Span,
    pub gen_locs: Vec<Location>,
    pub kill_locs: Vec<Location>,
    /// The place that is the receiver of the `lock()` call.
    pub receiver_place: Option<Place>,
    /// Whether this guard was generated by a `read_recursive()` call.
    pub is_recursive_read: bool,
    /// If this guard is an alias of another guard (via move/copy),
    /// record the original guard id.
    pub alias_of: Option<LockGuardId>,
}

impl LockGuardInfo {
    pub fn new(lockguard_ty: LockGuardTy, span: Span) -> Self {
        Self {
            lockguard_ty,
            span,
            gen_locs: Vec::new(),
            kill_locs: Vec::new(),
            receiver_place: None,
            is_recursive_read: false,
            alias_of: None,
        }
    }
}

pub type LockGuardMap = std::collections::HashMap<LockGuardId, LockGuardInfo>;

/// Set of live lockguards.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveLockGuards(pub FxHashSet<LockGuardId>);

impl LiveLockGuards {
    pub fn new() -> Self {
        Self(FxHashSet::default())
    }
    pub fn insert(&mut self, id: LockGuardId) -> bool {
        self.0.insert(id)
    }
    pub fn remove(&mut self, id: &LockGuardId) -> bool {
        self.0.remove(id)
    }
    pub fn union(&mut self, other: &Self) -> bool {
        let old_len = self.0.len();
        self.0.extend(&other.0);
        old_len != self.0.len()
    }
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
    pub fn iter(&self) -> impl Iterator<Item = &LockGuardId> {
        self.0.iter()
    }
}
