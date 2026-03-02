//! Quick lock usage detection for crates
//!
//! Provides fast detection of whether a crate uses Mutex/RwLock types
//! without deep MIR analysis.

use std::collections::HashSet;
use log::debug;
use stable_mir_wrapper::Instance;

/// Quick lock usage scanner
pub struct QuickLockScanner {
    /// Lock type patterns to look for
    lock_patterns: Vec<&'static str>,
}

impl QuickLockScanner {
    pub fn new() -> Self {
        Self {
            lock_patterns: vec![
                "Mutex",
                "RwLock",
                "MutexGuard",
                "RwLockReadGuard",
                "RwLockWriteGuard",
            ],
        }
    }

    /// Quickly check if a crate uses any lock types
    /// Returns true if locks are found, false otherwise
    pub fn crate_uses_locks(&self, items: &[stable_mir_wrapper::CrateItem]) -> bool {
        for item in items {
            if self.item_uses_locks(item) {
                debug!("Lock usage found in item");
                return true;
            }
        }
        false
    }

    /// Check if a single item uses locks
    fn item_uses_locks(&self, item: &stable_mir_wrapper::CrateItem) -> bool {
        use stable_mir_wrapper::{Instance, ItemKind};

        // Try to convert to Instance to get more information
        let instance_opt = Instance::try_from(*item).ok();

        // Quick check: does the item def_id contain lock patterns?
        // For now, we'll skip the def-based check since CrateItem doesn't expose it

        // Check item kind
        if let Ok(instance) = Instance::try_from(*item) {
            // For functions, quickly scan local declarations
            if self.instance_has_lock_locals(&instance) {
                return true;
            }
        }

        false
    }

    /// Scan local declarations for lock types (fast path)
    fn instance_has_lock_locals(&self, instance: &Instance) -> bool {
        // Only check local declarations, not full MIR body
        if let Some(body) = instance.body() {
            for (_local, local_decl) in body.local_decls() {
                let ty = local_decl.ty;
                let ty_str = format!("{:?}", ty);

                // Quick pattern match
                for pattern in &self.lock_patterns {
                    if ty_str.contains(pattern) {
                        debug!("  Local has lock type: {} - pattern: {}", ty_str, pattern);
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Get detailed lock type info (only if locks were detected)
    pub fn scan_lock_types(&self, items: &[stable_mir_wrapper::CrateItem]) -> LockUsageInfo {
        use stable_mir_wrapper::Instance;
        use std::collections::HashMap;

        let mut instances_with_locks = Vec::new();
        let mut lock_types_found = HashSet::new();

        for item in items {
            if let Ok(instance) = Instance::try_from(*item) {
                let instance_name = instance.name();

                // Check local declarations
                if let Some(body) = instance.body() {
                    for (_local, local_decl) in body.local_decls() {
                        let ty = local_decl.ty;
                        let ty_str = format!("{:?}", ty);

                        // Identify lock type
                        for pattern in &self.lock_patterns {
                            if ty_str.contains(pattern) {
                                if !instances_with_locks.contains(&instance_name.to_string()) {
                                    instances_with_locks.push(instance_name.to_string());
                                }
                                lock_types_found.insert(ty_str);
                                break;
                            }
                        }
                    }
                }
            }
        }

        let has_locks = !instances_with_locks.is_empty();
        LockUsageInfo {
            instances_with_locks,
            lock_types_found: lock_types_found.into_iter().collect(),
            has_locks,
        }
    }
}

/// Result of quick lock usage scan
#[derive(Clone, Debug)]
pub struct LockUsageInfo {
    /// Functions that use locks
    pub instances_with_locks: Vec<String>,
    /// All lock type strings found
    pub lock_types_found: Vec<String>,
    /// Whether any locks were found
    pub has_locks: bool,
}

impl Default for QuickLockScanner {
    fn default() -> Self {
        Self::new()
    }
}
