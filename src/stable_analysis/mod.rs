//! Stable analysis module
//!
//! This module provides program analysis implementations that depend only on
//! the `stable_mir_wrapper` library, which wraps `rustc_public` StableMIR.
//!
//! The goal is to provide the same analysis capabilities as the `analysis` module
//! but using the stable StableMIR API instead of the unstable rustc_middle API.

pub mod callgraph;
pub mod closure_analysis;

// Lock detection modules
pub mod lock_types;
pub mod lock_types_quick;
pub mod lock_lifecycle;
pub mod lock_identity;
pub mod lock_detector;

pub use lock_types_quick::{QuickLockScanner, LockUsageInfo};

pub use callgraph::{CallGraph, Node};
pub use closure_analysis::ClosureInfo;

// Lock detection exports
pub use lock_types::{
    LockGuardId, LockGuardTy, LockGuardInfo, DeadlockPossibility,
    TySignature, MirVisitorTypeCollector,
};
pub use lock_lifecycle::LockLifecycleCollector;
pub use lock_identity::{MutexIdentity, MutexIdentityAnalyzer};
pub use lock_detector::{LockDetector, LockReport, LockBugKind};
