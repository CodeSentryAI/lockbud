//! StableMIR Wrapper - A thin wrapper providing a stable interface to MIR
//!
//! This library provides a stable, owned, index-based representation of Rust MIR
//! that can be used without lifetime constraints. It mirrors the rustc_public
//! StableMIR API one-to-one, providing a stable version until the official
//! StableMIR API is stabilized.
//!
//! # Design Principles
//!
//! - **Index-based**: All types use opaque indices (usize) instead of references
//! - **Owned data**: No lifetime parameters, all data is owned
//! - **One-to-one mapping**: Matches rustc_public StableMIR API exactly
//! - **Monomorphized only**: Only fully instantiated (monomorphic) MIR is exposed

// Re-export the core modules
pub mod mir;
pub mod ty;
pub mod crate_def;

// Re-export commonly used types for convenience
pub use mir::{
    body::{Body, BasicBlock, LocalDecl, VarDebugInfo},
    terminator::{Terminator, TerminatorKind},
    statement::{Statement, StatementKind},
    rvalue::{Rvalue},
    operand::{Operand, ConstOperand},
    place::{Place, ProjectionElem},
    mono::{Instance, MonoItem, StaticDef},
};
pub use ty::{
    Ty, TyKind, RigidTy,
    Mutability, Movability,
    IntTy, UintTy, FloatTy,
    GenericArgs,
};
pub use crate_def::{
    CrateDef, CrateItem,
    FnDef, ClosureDef, CoroutineDef, CoroutineClosureDef,
    AdtDef, StructDef, EnumDef, UnionDef, ForeignDef,
    TraitDef, TraitImplDef,
    ConstDef, StaticDef as CrateStaticDef,
    Span,
};
