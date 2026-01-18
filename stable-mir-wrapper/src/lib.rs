//! StableMIR Wrapper - A thin wrapper providing a stable interface to MIR
//!
//! This library provides a stable, owned, index-based representation of Rust MIR
//! by re-exporting types from rustc_public StableMIR.

#![feature(rustc_private)]

extern crate rustc_public;

// Re-export MIR types from rustc_public
pub use rustc_public::mir::{
    Body, BasicBlock, BasicBlockIdx, Local, RETURN_LOCAL,
    LocalDecl, VarDebugInfo,
    Terminator, TerminatorKind,
    Statement, StatementKind,
    Rvalue,
    Operand, ConstOperand,
    Place, ProjectionElem, FieldIdx,
};

// Re-export monomorphization types
pub use rustc_public::mir::mono::{Instance, MonoItem, StaticDef};

// Re-export type system
pub use rustc_public::ty::{Ty, TyKind, RigidTy};

// Re-export crate types
pub use rustc_public::{CrateDef, CrateItem, ItemKind};

// Re-export entry point (macros are at the root)
pub use rustc_public::{run_with_tcx, local_crate, entry_fn, all_local_items};

// Re-export error type
pub use rustc_public::CompilerError;
