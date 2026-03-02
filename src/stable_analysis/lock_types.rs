//! Type definitions for Mutex deadlock detection using stable-mir
//!
//! This module provides the core types for identifying and tracking MutexGuards
//! across the program.

use std::collections::{HashMap, HashSet};
use std::fmt;

use log::debug;
use stable_mir_wrapper::{
    Instance, Local, BasicBlockIdx, Ty, TyKind, RigidTy,
    Body, MirVisitor, Place, Operand, Rvalue, Statement, Terminator,
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

    /// Get formatted summary of collected lock types as a string
    pub fn format_summary(&self) -> String {
        let mut output = String::new();

        if self.all_lock_types.is_empty() && self.all_guard_types.is_empty() {
            output.push_str("No lock or guard types found in this crate\n");
            return output;
        }

        // Print all lock types found
        if !self.all_lock_types.is_empty() {
            output.push_str(&format!("Lock Types Found ({}):\n", self.all_lock_types.len()));
            for (i, lock_type) in self.all_lock_types.iter().enumerate() {
                output.push_str(&format!("  {}. {} ({})\n",
                    i + 1, lock_type.kind, lock_type.library));
            }
            output.push_str("\n");
        }

        // Print all guard types found
        if !self.all_guard_types.is_empty() {
            output.push_str(&format!("Guard Types Found ({}):\n", self.all_guard_types.len()));
            for (i, guard_type) in self.all_guard_types.iter().enumerate() {
                output.push_str(&format!("  {}. {}\n", i + 1, guard_type));
            }
            output.push_str("\n");
        }

        // Print instances with locks
        if !self.instance_locks.is_empty() {
            output.push_str(&format!("Instances Using Locks ({}):\n", self.instance_locks.len()));
            for (instance_name, lock_types) in &self.instance_locks {
                output.push_str(&format!("  Instance: {}\n", instance_name));
                for lock_type in lock_types {
                    output.push_str(&format!("    - {} ({})\n", lock_type.kind, lock_type.library));
                }
            }
            output.push_str("\n");
        }

        output
    }
}

impl Default for LockTypeCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Type signature for efficient type comparison
///
/// Since Ty doesn't implement PartialEq/Hash, we use the Debug string
/// representation as a proxy for type identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TySignature {
    /// String representation of the type
    pub type_str: String,
}

impl TySignature {
    /// Create a signature from a Ty
    pub fn from_ty(ty: &Ty) -> Self {
        Self {
            type_str: format!("{:?}", ty),
        }
    }

    /// Check if this signature matches a lock pattern
    pub fn is_lock_type(&self) -> bool {
        self.type_str.contains("Mutex") || self.type_str.contains("RwLock")
    }

    /// Check if this signature matches a guard pattern
    pub fn is_guard_type(&self) -> bool {
        self.type_str.contains("MutexGuard")
            || self.type_str.contains("RwLockReadGuard")
            || self.type_str.contains("RwLockWriteGuard")
    }
}

/// MirVisitor-based type collector for efficient lock/guard type detection
///
/// This collector uses the MirVisitor trait to traverse all types in a MIR body,
/// collecting type signatures that match lock/guard patterns. This is more
/// efficient than string matching on every local and more complete as it
/// finds types in expressions, not just local declarations.
pub struct MirVisitorTypeCollector<'a> {
    /// The MIR body being analyzed
    body: &'a Body,
    /// Instance name for reporting
    instance_name: String,
    /// Type signatures of lock types found
    lock_signatures: HashSet<TySignature>,
    /// Type signatures of guard types found
    guard_signatures: HashSet<TySignature>,
    /// Map from local to their lock type signatures
    locals_with_locks: HashMap<Local, TySignature>,
    /// Map from local to their guard type signatures
    locals_with_guards: HashMap<Local, TySignature>,
    /// All places (not just locals) that have lock types
    lock_places: Vec<Place>,
    /// All places that have guard types
    guard_places: Vec<Place>,
}

impl<'a> MirVisitorTypeCollector<'a> {
    /// Create a new visitor for the given body
    pub fn new(body: &'a Body, instance_name: String) -> Self {
        Self {
            body,
            instance_name,
            lock_signatures: HashSet::new(),
            guard_signatures: HashSet::new(),
            locals_with_locks: HashMap::new(),
            locals_with_guards: HashMap::new(),
            lock_places: Vec::new(),
            guard_places: Vec::new(),
        }
    }

    /// Analyze the body and collect all lock/guard types
    pub fn analyze(&mut self) {
        debug!("[Instance: {}] Starting MirVisitor type collection", self.instance_name);

        // Visit the entire body - this will call our visitor methods
        self.visit_body(self.body);

        debug!("[Instance: {}] Found {} lock type signatures and {} guard type signatures",
            self.instance_name,
            self.lock_signatures.len(),
            self.guard_signatures.len());
    }

    /// Check if a type signature is interesting (lock or guard)
    fn check_ty_signature(&mut self, ty: &Ty, place: Option<&Place>) {
        let sig = TySignature::from_ty(ty);

        if sig.is_lock_type() {
            self.lock_signatures.insert(sig.clone());

            if let Some(place) = place {
                self.lock_places.push(place.clone());
                let local = place.local;
if true {
                    self.locals_with_locks.insert(local, sig.clone());
                }
            }

            debug!("[Instance: {}] Found lock type: {}", self.instance_name, sig.type_str);
        }

        if sig.is_guard_type() {
            self.guard_signatures.insert(sig.clone());

            if let Some(place) = place {
                self.guard_places.push(place.clone());
                let local = place.local;
if true {
                    self.locals_with_guards.insert(local, sig.clone());
                }
            }

            debug!("[Instance: {}] Found guard type: {}", self.instance_name, sig.type_str);
        }
    }

    /// Get all lock type signatures found
    pub fn lock_signatures(&self) -> &HashSet<TySignature> {
        &self.lock_signatures
    }

    /// Get all guard type signatures found
    pub fn guard_signatures(&self) -> &HashSet<TySignature> {
        &self.guard_signatures
    }

    /// Get locals that have lock types
    pub fn locals_with_locks(&self) -> &HashMap<Local, TySignature> {
        &self.locals_with_locks
    }

    /// Get locals that have guard types
    pub fn locals_with_guards(&self) -> &HashMap<Local, TySignature> {
        &self.locals_with_guards
    }

    /// Check if the body contains any lock or guard types
    pub fn has_lock_types(&self) -> bool {
        !self.lock_signatures.is_empty() || !self.guard_signatures.is_empty()
    }

    /// Format a summary of findings
    pub fn format_summary(&self) -> String {
        let mut output = String::new();

        output.push_str(&format!("Instance: {}\n", self.instance_name));

        if !self.lock_signatures.is_empty() {
            output.push_str(&format!("  Lock types found: {}\n", self.lock_signatures.len()));
            for sig in &self.lock_signatures {
                output.push_str(&format!("    - {}\n", sig.type_str));
            }
        }

        if !self.guard_signatures.is_empty() {
            output.push_str(&format!("  Guard types found: {}\n", self.guard_signatures.len()));
            for sig in &self.guard_signatures {
                output.push_str(&format!("    - {}\n", sig.type_str));
            }
        }

        if !self.locals_with_locks.is_empty() {
            output.push_str(&format!("  Locals with locks: {}\n", self.locals_with_locks.len()));
        }

        if !self.locals_with_guards.is_empty() {
            output.push_str(&format!("  Locals with guards: {}\n", self.locals_with_guards.len()));
        }

        output
    }
}

impl MirVisitor for MirVisitorTypeCollector<'_> {
    fn visit_body(&mut self, body: &Body) {
        // Visit all basic blocks
        for (bb_idx, bb) in body.blocks.iter().enumerate() {
            // Visit statements
            for stmt in &bb.statements {
                let loc = unsafe { std::mem::zeroed() };
                self.visit_statement(stmt, loc);
            }

            // Visit terminator
            let loc = unsafe { std::mem::zeroed() };
            self.visit_terminator(&bb.terminator, loc);
        }
    }

    fn visit_statement(&mut self, statement: &Statement, _location: stable_mir_wrapper::Location) {
        use stable_mir_wrapper::StatementKind;

        match &statement.kind {
            StatementKind::Assign(place, rvalue) => {
                // Check the type of the destination place
                if let Ok(ty) = place.ty(self.body.locals()) {
                    self.check_ty_signature(&ty, Some(place));
                }

                // Check types in the rvalue
                self.visit_rvalue(rvalue, _location);
            }
            StatementKind::FakeRead(_, place) => {
                if let Ok(ty) = place.ty(self.body.locals()) {
                    self.check_ty_signature(&ty, Some(place));
                }
            }
            StatementKind::SetDiscriminant { place, .. } => {
                if let Ok(ty) = place.ty(self.body.locals()) {
                    self.check_ty_signature(&ty, Some(place));
                }
            }
            _ => {
                self.super_statement(statement, _location);
            }
        }
    }

    fn visit_rvalue(&mut self, rvalue: &Rvalue, _location: stable_mir_wrapper::Location) {
        use stable_mir_wrapper::Rvalue::*;

        match rvalue {
            Use(operand) | Repeat(operand, _) | Cast(_, operand, _) => {
                if let Ok(ty) = operand.ty(self.body.locals()) {
                    self.check_ty_signature(&ty, None);
                }
                self.super_operand(operand, _location);
            }
            Ref(_, _, place) => {
                if let Ok(ty) = place.ty(self.body.locals()) {
                    self.check_ty_signature(&ty, Some(place));
                }
                // Use super_rvalue to avoid PlaceContext complexity
                self.super_rvalue(rvalue, _location);
            }
            BinaryOp(_, op1, op2) | CheckedBinaryOp(_, op1, op2) => {
                if let Ok(ty) = op1.ty(self.body.locals()) {
                    self.check_ty_signature(&ty, None);
                }
                if let Ok(ty) = op2.ty(self.body.locals()) {
                    self.check_ty_signature(&ty, None);
                }
                self.super_operand(op1, _location);
                self.super_operand(op2, _location);
            }
            UnaryOp(_, op1) => {
                if let Ok(ty) = op1.ty(self.body.locals()) {
                    self.check_ty_signature(&ty, None);
                }
                self.super_operand(op1, _location);
            }
            Aggregate(_, operands) => {
                // Check aggregate operands
                for operand in operands {
                    if let Ok(ty) = operand.ty(self.body.locals()) {
                        self.check_ty_signature(&ty, None);
                    }
                    self.super_operand(operand, _location);
                }
            }
            _ => {
                self.super_rvalue(rvalue, _location);
            }
        }
    }

    fn visit_terminator(&mut self, terminator: &Terminator, _location: stable_mir_wrapper::Location) {
        use stable_mir_wrapper::TerminatorKind::*;

        match &terminator.kind {
            Call { func, args, destination, .. } => {
                // Check function type
                if let Ok(ty) = func.ty(self.body.locals()) {
                    self.check_ty_signature(&ty, None);
                }

                // Check destination type
                if let Ok(ty) = destination.ty(self.body.locals()) {
                    self.check_ty_signature(&ty, Some(destination));
                }

                // Check argument types
                for arg in args {
                    if let Ok(ty) = arg.ty(self.body.locals()) {
                        self.check_ty_signature(&ty, None);
                    }
                }
            }
            Drop { place, .. } => {
                if let Ok(ty) = place.ty(self.body.locals()) {
                    self.check_ty_signature(&ty, Some(place));
                }
            }
            Assert { cond, .. } => {
                if let Ok(ty) = cond.ty(self.body.locals()) {
                    self.check_ty_signature(&ty, None);
                }
            }
            _ => {}
        }

        self.super_terminator(terminator, _location);
    }

    fn visit_place(&mut self, place: &Place, _context: rustc_public::mir::visit::PlaceContext, _location: stable_mir_wrapper::Location) {
        if let Ok(ty) = place.ty(self.body.locals()) {
            self.check_ty_signature(&ty, Some(place));
        }
        self.super_place(place, _context, _location);
    }
}
