//! Type definitions for Mutex deadlock detection using stable-mir
//!
//! This module provides the core types for identifying and tracking MutexGuards
//! across the program.

use std::collections::HashMap;
use std::fmt;

use log::debug;
use stable_mir_wrapper::{
    Instance, Local, BasicBlockIdx, Ty, TyKind, RigidTy,
};

/// MIR Location tracking block and statement indices
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MirLocation {
    pub block: BasicBlockIdx,
    pub statement_index: usize,
}

impl MirLocation {
    pub fn new(block: BasicBlockIdx, statement_index: usize) -> Self {
        Self { block, statement_index }
    }
}

impl std::cmp::PartialOrd for MirLocation {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match (self.block.cmp(&other.block), self.statement_index.cmp(&other.statement_index)) {
            (std::cmp::Ordering::Equal, std::cmp::Ordering::Equal) => Some(std::cmp::Ordering::Equal),
            (std::cmp::Ordering::Equal, ord) | (ord, std::cmp::Ordering::Equal) => Some(ord),
            (ord, _) => Some(ord),
        }
    }
}

impl std::cmp::Ord for MirLocation {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.partial_cmp(other).unwrap_or(std::cmp::Ordering::Equal)
    }
}

/// Unique identifier for a LockGuard across the crate
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LockGuardId {
    pub instance: Instance,
    pub local: Local,
}

impl LockGuardId {
    pub fn new(instance: Instance, local: Local) -> Self {
        Self { instance, local }
    }
}

impl fmt::Display for LockGuardId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}::{}", self.instance.name(), self.local)
    }
}

/// Supported Mutex types
///
/// Each variant contains the protected data type T from MutexGuard<T>
#[derive(Clone, Debug, PartialEq)]
pub enum LockGuardTy {
    /// std::sync::MutexGuard<T>
    StdMutex(Ty),
    /// parking_lot::MutexGuard<T>
    ParkingLotMutex(Ty),
    /// spin::MutexGuard<T>
    SpinMutex(Ty),
    /// std::sync::RwLockReadGuard<T>
    StdRwLockRead(Ty),
    /// std::sync::RwLockWriteGuard<T>
    StdRwLockWrite(Ty),
    /// parking_lot::RwLockReadGuard<T>
    ParkingLotRead(Ty),
    /// parking_lot::RwLockWriteGuard<T>
    ParkingLotWrite(Ty),
    /// spin::RwLockReadGuard<T>
    SpinRead(Ty),
    /// spin::RwLockWriteGuard<T>
    SpinWrite(Ty),
}

impl LockGuardTy {
    /// Extract LockGuard type from a Ty
    ///
    /// Returns None if not a MutexGuard type
    pub fn from_ty(ty: &Ty) -> Option<Self> {
        // Try to get a string representation for quick filtering
        let type_str = format!("{:?}", ty);

        // Quick fail if not a lock type
        if !type_str.contains("MutexGuard")
            && !type_str.contains("RwLockReadGuard")
            && !type_str.contains("RwLockWriteGuard")
        {
            return None;
        }

        // Pattern match on TyKind
        match ty.kind() {
            TyKind::RigidTy(RigidTy::Adt(adt_def, args)) => {
                // Get type path - we need to check the type name
                let path = format!("{:?}", adt_def);

                // Extract first part before '<' to get the base type name
                let first_part = path.split('<').next().unwrap_or(&path);

                if first_part.contains("MutexGuard") {
                    // Check for async/loom (not supported)
                    if first_part.contains("async")
                        || first_part.contains("tokio")
                        || first_part.contains("future")
                        || first_part.contains("loom")
                    {
                        return None;
                    }

                    // Extract type parameter
                    // - std::sync::MutexGuard<T>: first generic arg
                    // - parking_lot::MutexGuard<RawMutex, T>: second generic arg
                    // - spin::MutexGuard<T>: first generic arg
                    let inner_ty = if first_part.contains("spin") {
                        Self::extract_generic_arg(&args, 0)?
                    } else if first_part.contains("lock_api") || first_part.contains("parking_lot") {
                        Self::extract_generic_arg(&args, 1)?
                    } else {
                        // std::sync::Mutex or wrapper by default
                        Self::extract_generic_arg(&args, 0)?
                    };

                    if first_part.contains("spin") {
                        Some(LockGuardTy::SpinMutex(inner_ty))
                    } else if first_part.contains("lock_api") || first_part.contains("parking_lot") {
                        Some(LockGuardTy::ParkingLotMutex(inner_ty))
                    } else {
                        Some(LockGuardTy::StdMutex(inner_ty))
                    }
                } else if first_part.contains("RwLockReadGuard") {
                    // Check for async/loom (not supported)
                    if first_part.contains("async")
                        || first_part.contains("tokio")
                        || first_part.contains("future")
                        || first_part.contains("loom")
                    {
                        return None;
                    }

                    let inner_ty = if first_part.contains("spin") {
                        Self::extract_generic_arg(&args, 0)?
                    } else if first_part.contains("lock_api") || first_part.contains("parking_lot") {
                        Self::extract_generic_arg(&args, 1)?
                    } else {
                        Self::extract_generic_arg(&args, 0)?
                    };

                    if first_part.contains("spin") {
                        Some(LockGuardTy::SpinRead(inner_ty))
                    } else if first_part.contains("lock_api") || first_part.contains("parking_lot") {
                        Some(LockGuardTy::ParkingLotRead(inner_ty))
                    } else {
                        Some(LockGuardTy::StdRwLockRead(inner_ty))
                    }
                } else if first_part.contains("RwLockWriteGuard") {
                    // Check for async/loom (not supported)
                    if first_part.contains("async")
                        || first_part.contains("tokio")
                        || first_part.contains("future")
                        || first_part.contains("loom")
                    {
                        return None;
                    }

                    let inner_ty = if first_part.contains("spin") {
                        Self::extract_generic_arg(&args, 0)?
                    } else if first_part.contains("lock_api") || first_part.contains("parking_lot") {
                        Self::extract_generic_arg(&args, 1)?
                    } else {
                        Self::extract_generic_arg(&args, 0)?
                    };

                    if first_part.contains("spin") {
                        Some(LockGuardTy::SpinWrite(inner_ty))
                    } else if first_part.contains("lock_api") || first_part.contains("parking_lot") {
                        Some(LockGuardTy::ParkingLotWrite(inner_ty))
                    } else {
                        Some(LockGuardTy::StdRwLockWrite(inner_ty))
                    }
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Extract the nth generic argument from GenericArgs
    fn extract_generic_arg(args: &stable_mir_wrapper::GenericArgs, index: usize) -> Option<Ty> {
        // For now, use string parsing since GenericArgs doesn't expose
        // direct access to type arguments in stable-mir
        let args_str = format!("{:?}", args);

        // Extract type arguments between < and >
        let args_str = args_str.strip_prefix("GenericArgs(")?.strip_suffix(")")?;
        let types_part = args_str.split(['[', ']']).nth(1)?;

        // Parse the types (comma-separated)
        let types: Vec<&str> = types_part.split(',').map(|s| s.trim()).collect();

        if index < types.len() {
            // Return a placeholder Ty - in practice, this would need proper parsing
            // For now, we'll need a different approach
            None  // Placeholder
        } else {
            None
        }
    }

    /// Check if two lock types can deadlock with each other
    pub fn deadlock_with(&self, other: &Self) -> DeadlockPossibility {
        use LockGuardTy::*;

        match (self, other) {
            // Same mutex type with same data type
            (StdMutex(a), StdMutex(b))
            | (ParkingLotMutex(a), ParkingLotMutex(b))
            | (SpinMutex(a), SpinMutex(b))
                if Self::same_ty(a, b) => DeadlockPossibility::Probably,

            // Read/write lock interactions
            (StdRwLockWrite(a), StdRwLockRead(b))
            | (StdRwLockRead(a), StdRwLockWrite(b))
                if Self::same_ty(a, b) => DeadlockPossibility::Probably,

            // Read-read is possibly deadlock (platform-dependent)
            (StdRwLockRead(a), StdRwLockRead(b))
                if Self::same_ty(a, b) => DeadlockPossibility::Possibly,

            _ => DeadlockPossibility::Unlikely,
        }
    }

    /// Compare two types for equality (string-based comparison)
    fn same_ty(a: &Ty, b: &Ty) -> bool {
        format!("{:?}", a) == format!("{:?}", b)
    }
}

impl fmt::Display for LockGuardTy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LockGuardTy::StdMutex(ty) => write!(f, "MutexGuard<{:?}>", ty),
            LockGuardTy::ParkingLotMutex(ty) => write!(f, "MutexGuard<{:?}>", ty),
            LockGuardTy::SpinMutex(ty) => write!(f, "MutexGuard<{:?}>", ty),
            LockGuardTy::StdRwLockRead(ty) => write!(f, "RwLockReadGuard<{:?}>", ty),
            LockGuardTy::StdRwLockWrite(ty) => write!(f, "RwLockWriteGuard<{:?}>", ty),
            LockGuardTy::ParkingLotRead(ty) => write!(f, "RwLockReadGuard<{:?}>", ty),
            LockGuardTy::ParkingLotWrite(ty) => write!(f, "RwLockWriteGuard<{:?}>", ty),
            LockGuardTy::SpinRead(ty) => write!(f, "RwLockReadGuard<{:?}>", ty),
            LockGuardTy::SpinWrite(ty) => write!(f, "RwLockWriteGuard<{:?}>", ty),
        }
    }
}

/// The possibility of deadlock
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeadlockPossibility {
    Probably,
    Possibly,
    Unlikely,
    Unknown,
}

impl fmt::Display for DeadlockPossibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeadlockPossibility::Probably => write!(f, "Probably"),
            DeadlockPossibility::Possibly => write!(f, "Possibly"),
            DeadlockPossibility::Unlikely => write!(f, "Unlikely"),
            DeadlockPossibility::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Information about a specific lock guard
#[derive(Clone, Debug)]
pub struct LockGuardInfo {
    pub lockguard_ty: LockGuardTy,
    pub span_str: String,
    /// Lock acquisition locations (creation)
    pub gen_locs: Vec<MirLocation>,
    /// Guard move locations (reassignments)
    pub move_gen_locs: Vec<MirLocation>,
    /// Guard drop locations (release)
    pub kill_locs: Vec<MirLocation>,
    /// Basic blocks guarded by this lock
    pub guarded_blocks: Vec<(BasicBlockIdx, MirLocation)>,
    /// Variable name (if extractable)
    pub var_name: Option<String>,
    /// Source location (file:line:col)
    pub source_loc: Option<String>,
    /// Function name where guard was created
    pub func_name: String,
}

impl LockGuardInfo {
    pub fn new(lockguard_ty: LockGuardTy, span_str: String, func_name: String) -> Self {
        Self {
            lockguard_ty,
            span_str,
            gen_locs: Vec::new(),
            move_gen_locs: Vec::new(),
            kill_locs: Vec::new(),
            guarded_blocks: Vec::new(),
            var_name: None,
            source_loc: None,
            func_name,
        }
    }

    /// Check if this guard is alive at the given location
    pub fn is_alive_at(&self, location: &MirLocation) -> bool {
        // Check if location is between any gen and kill pair
        for gen in &self.gen_locs {
            for kill in &self.kill_locs {
                // Gen must come before or at the location
                // Kill must come after or at the location
                if gen <= location && location <= kill {
                    return true;
                }
            }
        }
        false
    }
}

/// Lock kind enum
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockKind {
    Mutex,
    RwLock,
}

impl fmt::Display for LockKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LockKind::Mutex => write!(f, "Mutex"),
            LockKind::RwLock => write!(f, "RwLock"),
        }
    }
}

/// Lock type information collected during analysis
#[derive(Clone, Debug)]
pub struct LockType {
    /// Type string (e.g., "Mutex<i32>")
    pub type_str: String,
    /// Library (std, parking_lot, spin)
    pub library: String,
    /// Lock kind (Mutex, RwLock)
    pub kind: LockKind,
}

/// Lock type collector for analyzing all types in a crate
pub struct LockTypeCollector {
    /// Map from instance name to lock types found
    pub instance_locks: HashMap<String, Vec<LockType>>,
    /// All lock types found
    pub all_lock_types: Vec<LockType>,
    /// All guard types found
    pub all_guard_types: Vec<String>,
}

impl LockTypeCollector {
    pub fn new() -> Self {
        Self {
            instance_locks: HashMap::new(),
            all_lock_types: Vec::new(),
            all_guard_types: Vec::new(),
        }
    }

    /// Analyze all crate items and collect lock types
    pub fn analyze_crate(&mut self, items: &[stable_mir_wrapper::CrateItem]) {
        debug!("\n=== Lock Type Analysis ===");
        debug!("Analyzing {} crate items", items.len());

        for item in items {
            self.analyze_item(item);
        }

        self.print_summary();
    }

    /// Analyze a single crate item
    fn analyze_item(&mut self, item: &stable_mir_wrapper::CrateItem) {
        // Try to convert to Instance
        if let Ok(instance) = stable_mir_wrapper::Instance::try_from(*item) {
            // Get the instance name
            let instance_name = instance.name();

            // Get the body if available
            if let Some(body) = instance.body() {
                // Visit all types in the body
                self.visit_body_types(&instance_name, &body);
            }
        }
    }

    /// Visit all types in a MIR body
    fn visit_body_types(&mut self, instance_name: &str, body: &stable_mir_wrapper::Body) {
        // Visit all local declarations
        for (local, local_decl) in body.local_decls() {
            let ty = local_decl.ty;

            // Check if this is a lock or guard type
            if self.is_lock_type(&ty) {
                if let Some(lock_type) = self.extract_lock_info(&ty) {
                    debug!("[Instance: {}] Found Lock type: {} ({})",
                        instance_name, lock_type.kind, lock_type.library);

                    // Record lock type for this instance
                    self.instance_locks
                        .entry(instance_name.to_string())
                        .or_insert_with(Vec::new)
                        .push(lock_type.clone());

                    // Record in all lock types
                    self.all_lock_types.push(lock_type);
                }
            }

            // Check if this is a guard type
            if let Some(guard_str) = self.extract_guard_type(&ty) {
                debug!("[Instance: {}] Found Guard type: {}", instance_name, guard_str);

                // Record in all guard types
                self.all_guard_types.push(guard_str);
            }
        }
    }

    /// Check if a Ty is a lock type (Mutex or RwLock)
    fn is_lock_type(&self, ty: &Ty) -> bool {
        let ty_str = format!("{:?}", ty);

        ty_str.contains("Mutex") || ty_str.contains("RwLock")
    }

    /// Extract lock type information from a Ty
    fn extract_lock_info(&self, ty: &Ty) -> Option<LockType> {
        let ty_str = format!("{:?}", ty);

        let library = if ty_str.contains("parking_lot") || ty_str.contains("lock_api") {
            "parking_lot".to_string()
        } else if ty_str.contains("spin") {
            "spin".to_string()
        } else {
            "std".to_string()
        };

        let kind = if ty_str.contains("RwLock") {
            LockKind::RwLock
        } else {
            LockKind::Mutex
        };

        Some(LockType {
            type_str: ty_str,
            library,
            kind,
        })
    }

    /// Extract guard type string from a Ty
    fn extract_guard_type(&self, ty: &Ty) -> Option<String> {
        if let Some(lockguard_ty) = LockGuardTy::from_ty(ty) {
            Some(format!("{}", lockguard_ty))
        } else {
            None
        }
    }

    /// Print summary of collected lock types
    fn print_summary(&self) {
        debug!("\n=== Lock Type Summary ===");

        if self.all_lock_types.is_empty() && self.all_guard_types.is_empty() {
            debug!("No lock or guard types found in this crate");
            return;
        }

        // Print all lock types found
        if !self.all_lock_types.is_empty() {
            debug!("\nLock Types Found ({}):", self.all_lock_types.len());
            for (i, lock_type) in self.all_lock_types.iter().enumerate() {
                debug!("  {}. {} ({} library)",
                    i + 1, lock_type.kind, lock_type.library);
            }
        }

        // Print all guard types found
        if !self.all_guard_types.is_empty() {
            debug!("\nGuard Types Found ({}):", self.all_guard_types.len());
            for (i, guard_type) in self.all_guard_types.iter().enumerate() {
                debug!("  {}. {}", i + 1, guard_type);
            }
        }

        // Print instances with locks
        if !self.instance_locks.is_empty() {
            debug!("\nInstances Using Locks ({}):", self.instance_locks.len());
            for (instance_name, lock_types) in &self.instance_locks {
                debug!("\n  Instance: {}", instance_name);
                for lock_type in lock_types {
                    debug!("    - {} ({} library)", lock_type.kind, lock_type.library);
                }
            }
        }

        debug!("\n=== End Lock Type Summary ===\n");
    }
}

impl Default for LockTypeCollector {
    fn default() -> Self {
        Self::new()
    }
}
