//! Closure analysis using StableMIR
//!
//! This module analyzes closures to support deadlock detection.
//! For now, provides basic stub for parent-spawn relationship.

use std::collections::HashMap;
use stable_mir_wrapper::{
    Body, Instance, MonoItem,
    TerminatorKind, Rvalue, CastKind, PointerCoercion,
    Ty, TyKind, RigidTy,
    CrateItem, ItemKind,
};

/// Information about a closure and its lock usage
#[derive(Debug)]
pub struct ClosureInfo {
    /// Name of the closure (e.g., `two_closures::{closure#0}`)
    pub name: String,

    /// Variables captured by the closure (e.g., `_1`.0`, `_1`.1`)
    pub captured_vars: Vec<String>,

    /// Lock variables used (e.g., `lock_b1`, `lock_a2`)
    pub lock_vars: Vec<String>,

    /// Lock acquisition order as found in MIR body
    pub lock_order: Vec<String>,
}
