//! Type system module
//!
//! Provides stable types representing Rust's type system, matching
//! the rustc_public::ty API one-to-one.

mod ty;

pub use ty::{
    Ty, TyKind, RigidTy,
    Mutability, Movability,
    IntTy, UintTy, FloatTy,
    GenericArgs,
    Region, RegionKind,
    FnSig, PolyFnSig, Abi,
    CoroutineKind,
    BoundExistentialPredicate,
    AliasTy, AliasKind,
};

// Re-export types from crate_def for convenience
pub use crate::crate_def::{
    FnDef, ClosureDef, CoroutineDef, CoroutineClosureDef,
    AdtDef, StructDef, EnumDef, UnionDef,
    TraitDef, TraitImplDef,
    ConstDef,
};
