//! Mutex instance identification
//!
//! This module analyzes mutex instances to determine when two locks
//! refer to the same Mutex.

use std::collections::HashMap;

use stable_mir_wrapper::{
    Body, Instance, Place, Ty, TyKind, RigidTy, Operand,
    TerminatorKind, StatementKind,
};

use crate::stable_analysis::callgraph::CallGraph;
use crate::stable_analysis::lock_types::LockGuardId;

/// Represents the identity of a Mutex instance
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MutexIdentity {
    /// Local variable: let mutex = Mutex::new(...)
    Local {
        instance: Instance,
        local: stable_mir_wrapper::Local,
    },

    /// Static/global variable
    Static {
        name: String,
    },

    /// Function parameter
    Parameter {
        instance: Instance,
        local: stable_mir_wrapper::Local,
    },

    /// Wrapped in Arc
    Arc {
        inner_place: Place,
    },

    /// Captured by closure
    ClosureUpvar {
        closure_instance: Instance,
        upvar_index: usize,
    },
}

impl std::fmt::Display for MutexIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MutexIdentity::Local { instance, local } => {
                write!(f, "{}::local{}", instance.name(), local)
            }
            MutexIdentity::Static { name } => write!(f, "static({})", name),
            MutexIdentity::Parameter { instance, local } => {
                write!(f, "{}::param{}", instance.name(), local)
            }
            MutexIdentity::Arc { inner_place } => {
                // Format the Place in a more readable way
                write!(f, "arc({})", place_to_readable_string(inner_place))
            }
            MutexIdentity::ClosureUpvar { closure_instance, upvar_index } => {
                write!(f, "{}::upvar{}", closure_instance.name(), upvar_index)
            }
        }
    }
}

/// Convert a Ty to a human-readable string (simplified version for mutex identity)
fn ty_to_readable_string(ty: &stable_mir_wrapper::Ty) -> String {
    let ty_str = format!("{:?}", ty);

    // If this is a Mutex type, extract that info
    if ty_str.contains("Mutex") {
        // Try to extract the protected type
        if ty_str.contains("I32") || ty_str.contains("i32") {
            return "Mutex<i32>".to_string();
        }
        if ty_str.contains("Bool") || ty_str.contains("bool") {
            return "Mutex<bool>".to_string();
        }
        if ty_str.contains("U8") || ty_str.contains("u8") {
            return "Mutex<u8>".to_string();
        }
        if ty_str.contains("USize") || ty_str.contains("usize") {
            return "Mutex<usize>".to_string();
        }
        // For more complex types, try to extract the type name
        if let Some(name_start) = ty_str.find("name: \"") {
            let search_start = name_start + 7;
            if let Some(name_end) = ty_str[search_start..].find('"') {
                let type_name = &ty_str[search_start..search_start + name_end];
                if type_name.contains("Mutex") {
                    return format!("Mutex<static>");
                }
            }
        }
        return "Mutex<static>".to_string();
    }

    // If this is a RwLock type
    if ty_str.contains("RwLock") {
        if ty_str.contains("I32") || ty_str.contains("i32") {
            return "RwLock<i32>".to_string();
        }
        return "RwLock<static>".to_string();
    }

    // Fallback: truncate long types
    if ty_str.len() > 40 {
        "static_mutex".to_string()
    } else {
        ty_str
    }
}

/// Convert a Place to a human-readable string
fn place_to_readable_string(place: &stable_mir_wrapper::Place) -> String {
    // Extract the local and format it nicely
    let local = place.local;
    format!("arc_local{}", local)
}

/// Analyzes mutex identity across the program
pub struct MutexIdentityAnalyzer {
    identities: HashMap<Place, MutexIdentity>,
}

impl MutexIdentityAnalyzer {
    pub fn new() -> Self {
        Self {
            identities: HashMap::new(),
        }
    }

    /// Determine the identity of a mutex from a guard
    pub fn analyze_mutex_from_guard(
        &mut self,
        guard_local: stable_mir_wrapper::Local,
        guard_instance: Instance,
        body: &Body,
        _callgraph: &CallGraph,
    ) -> Option<MutexIdentity> {
        // Find where the guard was created
        // Backtrack from guard_local to find the Mutex::lock() call
        // Extract the mutex place from the call arguments

        for (bb_idx, bb) in body.blocks.iter().enumerate() {
            // Check terminators for lock() calls
            if let TerminatorKind::Call { func, args, destination, .. } = &bb.terminator.kind {
                let dest_local = destination.local;
                if dest_local == guard_local {
                    // Found the lock call that created this guard
                    return self.extract_mutex_identity(args, body, guard_instance);
                }
            }

            // Also check statements for assignments
            for (stmt_idx, stmt) in bb.statements.iter().enumerate() {
                if let StatementKind::Assign(place, _) = &stmt.kind {
                    let local = place.local;
                    if local == guard_local {
                        // Found an assignment to this guard
                        // This could be from a lock call result or a move
                        // For now, assume it's local
                        return Some(MutexIdentity::Local {
                            instance: guard_instance,
                            local: guard_local,
                        });
                    }
                }
            }
        }

        None
    }

    fn extract_mutex_identity(
        &mut self,
        args: &[Operand],
        body: &Body,
        instance: Instance,
    ) -> Option<MutexIdentity> {
        use log::debug;

        // First argument is &self or &mut self (the mutex reference)
        if let Some(operand) = args.first() {
            let (mutex_place, mutex_ty) = match operand {
                Operand::Move(place) => (place, place.ty(body.locals()).ok()),
                Operand::Copy(place) => (place, place.ty(body.locals()).ok()),
                Operand::Constant(_) => return None,
            };

            if let Some(ty) = mutex_ty {
                // The mutex_ty is the type of the argument (should be &Mutex or &mut Mutex)
                // We need to dereference to get the actual Mutex type for type checks
                let local = mutex_place.local;

                debug!("    [extract_mutex_identity] instance: {}, local: {}, ty: {:?}",
                    instance.name(), local, ty);

                // Check if wrapped in Arc (check the inner type for Arc pattern)
                // Need to dereference reference types first
                let is_arc = match ty.kind() {
                    TyKind::RigidTy(RigidTy::Ref(_, inner, _)) => self.is_arc_mutex(&inner),
                    TyKind::RigidTy(RigidTy::RawPtr(inner, _)) => self.is_arc_mutex(&inner),
                    _ => self.is_arc_mutex(&ty),
                };

                if is_arc {
                    debug!("    [extract_mutex_identity] Identified as Arc-wrapped mutex");
                    return Some(MutexIdentity::Arc {
                        inner_place: mutex_place.clone(),
                    });
                }

                // Check if local is a parameter (for passed-in mutexes)
                if self.is_parameter(local, body) {
                    debug!("    [extract_mutex_identity] Identified as Parameter");
                    return Some(MutexIdentity::Parameter {
                        instance,
                        local,
                    });
                }

                // Check if local is a captured upvar (in closure)
                if self.is_closure_upvar(local, body, instance) {
                    debug!("    [extract_mutex_identity] Identified as ClosureUpvar");
                    return Some(MutexIdentity::ClosureUpvar {
                        closure_instance: instance,
                        upvar_index: local,
                    });
                }

                // Default to Local - this is the most common case for local mutex variables
                // We don't use is_static_reference here because it incorrectly classifies
                // all mutex references as static (since lock() takes &self)
                // For true static mutexes, the place would need to point to a static item
                debug!("    [extract_mutex_identity] Identified as Local");
                return Some(MutexIdentity::Local {
                    instance,
                    local,
                });
            }
        }

        None
    }

    fn is_parameter(
        &self,
        local: stable_mir_wrapper::Local,
        body: &Body,
    ) -> bool {
        // In MIR, arguments are locals 1..N (local 0 is return)
        // Check local decl for argument info
        if let Some(local_decl) = body.local_decls().nth(local) {
            // Check if local has a source info that indicates it's a parameter
            // For now, use a more conservative heuristic
            // Parameters typically have lower indices and don't have mutability
            // Local variables created in the function typically have higher indices
            local > 0 && local <= 3  // More conservative - only first few locals might be args
        } else {
            false
        }
    }

    fn is_closure_upvar(
        &self,
        _local: stable_mir_wrapper::Local,
        _body: &Body,
        instance: Instance,
    ) -> bool {
        // Check if this instance is a closure
        let name = instance.name();
        name.contains("closure") || name.contains("Closure")
    }

    fn is_arc_mutex(&self, ty: &Ty) -> bool {
        match ty.kind() {
            TyKind::RigidTy(RigidTy::Adt(adt_def, _)) => {
                let path = format!("{:?}", adt_def);
                path.contains("sync::Arc") || path.contains("Arc<")
            }
            _ => false,
        }
    }

    fn is_static_reference(&self, ty: &Ty) -> bool {
        match ty.kind() {
            TyKind::RigidTy(RigidTy::Ref(_, _, _)) => true,
            TyKind::RigidTy(RigidTy::RawPtr(..)) => true,
            _ => false,
        }
    }

    /// Check if two mutex identities refer to the same mutex
    pub fn same_mutex(
        &self,
        id1: &MutexIdentity,
        id2: &MutexIdentity,
        _callgraph: &CallGraph,
    ) -> bool {
        match (id1, id2) {
            // Same local in same instance
            (
                MutexIdentity::Local { instance: i1, local: l1 },
                MutexIdentity::Local { instance: i2, local: l2 },
            ) => {
                // Same instance AND same local = same mutex
                if i1 == i2 && l1 == l2 {
                    return true;
                }

                // Same instance but different locals
                // This could be:
                // 1. Different mutexes (most common)
                // 2. Same mutex with different intermediate locals (MIR creates temporaries)
                // For case 2, we need to be conservative and return false
                // The double lock detection will handle this case by checking overlaps
                false
            }

            // Same static
            (
                MutexIdentity::Static { name: n1 },
                MutexIdentity::Static { name: n2 },
            ) => n1 == n2,

            // Same parameter in same instance
            (
                MutexIdentity::Parameter { instance: i1, local: l1 },
                MutexIdentity::Parameter { instance: i2, local: l2 },
            ) => i1 == i2 && l1 == l2,

            // Arc wrappers - conservative approximation
            (MutexIdentity::Arc { inner_place: p1 }, MutexIdentity::Arc { inner_place: p2 }) => {
                // For now, use string comparison
                format!("{:?}", p1) == format!("{:?}", p2)
            }

            // Closure upvar - trace back to original mutex
            (
                MutexIdentity::ClosureUpvar {
                    closure_instance: c1,
                    upvar_index: u1,
                },
                MutexIdentity::ClosureUpvar {
                    closure_instance: c2,
                    upvar_index: u2,
                },
            ) => {
                if c1 != c2 {
                    return false;
                }
                u1 == u2
            }

            // Cross-type checks (e.g., local vs Arc wrapping that local)
            // For now, return false (conservative)
            _ => false,
        }
    }
}
