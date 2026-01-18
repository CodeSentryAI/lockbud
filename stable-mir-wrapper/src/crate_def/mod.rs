//! Crate definition module
//!
//! Provides stable types for representing crate-level definitions,
//! matching the rustc_public crate_def API one-to-one.

mod item;
mod span;

pub use item::{
    CrateDef,
    CrateItem,
    ItemKind,
    FnDef, ClosureDef, CoroutineDef, CoroutineClosureDef,
    AdtDef, StructDef, EnumDef, UnionDef, ForeignDef,
    TraitDef, TraitImplDef,
    ConstDef, StaticDef,
};

pub use span::{Span, SpanData};
