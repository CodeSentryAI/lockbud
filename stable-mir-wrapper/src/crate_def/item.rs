//! Crate item definitions

use crate::ty::{Ty, GenericArgs};

/// Definition ID - opaque identifier for any definition
pub type DefId = usize;

/// A crate-level definition
///
/// This is a generic handle to any definition in a crate
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CrateDef {
    pub def_id: DefId,
}

impl CrateDef {
    /// Get the name of this definition
    pub fn name(&self) -> String {
        // This would be implemented by calling into rustc
        String::new()
    }

    /// Get the crate containing this definition
    pub fn krate(&self) -> Crate {
        Crate { index: 0 }
    }
}

/// An item in a crate
///
/// This represents any item that can be named and has a DefId
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CrateItem {
    pub def_id: DefId,
}

impl CrateItem {
    /// Get the kind of this item
    pub fn kind(&self) -> ItemKind {
        // This would be implemented by calling into rustc
        ItemKind::Fn
    }

    /// Get the name of this item
    pub fn name(&self) -> String {
        // This would be implemented by calling into rustc
        String::new()
    }

    /// Get the body of this item, if it has one
    pub fn body(&self) -> Option<crate::mir::Body> {
        // This would be implemented by calling into rustc
        None
    }
}

/// The kind of an item
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    /// A function
    Fn,

    /// A static variable
    Static,

    /// A constant
    Const,

    /// A constructor (tuple struct/enum variant)
    Ctor(CtorKind),

    /// Anything else (not exposed in StableMIR)
    Other,
}

/// Constructor kind
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtorKind {
    /// A struct constructor
    Struct,

    /// An enum variant constructor
    EnumVariant,
}

/// Function definition
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FnDef {
    pub def_id: DefId,
}

impl FnDef {
    /// Get the name of this function
    pub fn name(&self) -> String {
        // This would be implemented by calling into rustc
        String::new()
    }

    /// Get the generic arguments for this function
    pub fn generic_args(&self) -> GenericArgs {
        // This would be implemented by calling into rustc
        Vec::new()
    }
}

/// Closure definition
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClosureDef {
    pub def_id: DefId,
}

/// Coroutine (generator) definition
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CoroutineDef {
    pub def_id: DefId,
}

/// Coroutine closure definition (from coroutine-closure feature)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CoroutineClosureDef {
    pub def_id: DefId,
}

/// ADT (Algebraic Data Type) definition
///
/// Represents a struct, enum, or union
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AdtDef {
    pub def_id: DefId,
}

impl AdtDef {
    /// Get the name of this ADT
    pub fn name(&self) -> String {
        // This would be implemented by calling into rustc
        String::new()
    }

    /// Get the kind of ADT
    pub fn kind(&self) -> AdtKind {
        // This would be implemented by calling into rustc
        AdtKind::Struct
    }

    /// Get the generic arguments for this ADT
    pub fn generic_args(&self) -> GenericArgs {
        // This would be implemented by calling into rustc
        Vec::new()
    }
}

/// Struct definition (specific ADT type)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StructDef {
    pub def_id: DefId,
}

/// Enum definition (specific ADT type)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnumDef {
    pub def_id: DefId,
}

/// Union definition (specific ADT type)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UnionDef {
    pub def_id: DefId,
}

/// ADT kind
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdtKind {
    Struct,
    Enum,
    Union,
}

/// Foreign type definition (from FFI)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ForeignDef {
    pub def_id: DefId,
}

/// Trait definition
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TraitDef {
    pub def_id: DefId,
}

impl TraitDef {
    /// Get the name of this trait
    pub fn name(&self) -> String {
        // This would be implemented by calling into rustc
        String::new()
    }
}

/// Trait implementation definition
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TraitImplDef {
    pub def_id: DefId,
}

/// Constant definition
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConstDef {
    pub def_id: DefId,
}

/// Static variable definition
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StaticDef {
    pub def_id: DefId,
}

/// A crate
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Crate {
    pub index: usize,
}

impl Crate {
    /// Get the name of this crate
    pub fn name(&self) -> String {
        // This would be implemented by calling into rustc
        String::new()
    }

    /// Get all function definitions in this crate
    pub fn fn_defs(&self) -> Vec<FnDef> {
        // This would be implemented by calling into rustc
        Vec::new()
    }

    /// Get all static variables in this crate
    pub fn statics(&self) -> Vec<StaticDef> {
        // This would be implemented by calling into rustc
        Vec::new()
    }
}
