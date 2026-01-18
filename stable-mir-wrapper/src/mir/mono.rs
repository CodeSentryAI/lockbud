//! Monomorphization - fully instantiated (non-generic) MIR items
//!
//! This module provides types for representing monomorphized items,
//! which are MIR bodies with all generics fully instantiated.

use crate::mir::Body;
use crate::ty::Ty;

/// A monomorphized instance - a function with all generics instantiated
///
/// This is a key type in StableMIR, representing a specific instantiation
/// of a potentially generic function.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Instance {
    /// The DefId of the function being instantiated
    pub def: usize, // FnDef

    /// The generic arguments for this instantiation
    pub args: Vec<Ty>, // GenericArgs
}

impl Instance {
    /// Create a new instance
    pub fn new(def: usize, args: Vec<Ty>) -> Self {
        Self { def, args }
    }

    /// Get the MIR body for this instance, if available
    pub fn body(&self) -> Option<Body> {
        // This would be implemented by calling into rustc
        // For now, return None
        None
    }

    /// Get the name of this instance
    pub fn name(&self) -> String {
        // This would be implemented by calling into rustc
        String::new()
    }

    /// Try to convert a CrateItem to an Instance
    pub fn try_from(item: crate::crate_def::CrateItem) -> Result<Self, ()> {
        // This would be implemented by calling into rustc
        Err(())
    }

    /// Resolve a function definition with generic arguments to an instance
    pub fn resolve(def: usize, args: &[Ty]) -> Result<Self, ()> {
        // This would be implemented by calling into rustc
        Err(())
    }
}

/// A static variable definition
///
/// Represents a `static` or `const` item
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StaticDef {
    /// The DefId of this static
    pub def: usize,
}

impl StaticDef {
    /// Try to convert a CrateItem to a StaticDef
    pub fn try_from(item: crate::crate_def::CrateItem) -> Result<Self, ()> {
        // This would be implemented by calling into rustc
        Err(())
    }
}

/// A monomorphized item - either an Instance or a StaticDef
///
/// This represents items that will be codegen'd
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MonoItem {
    /// A function instance
    Instance(Instance),

    /// A static variable
    Static(StaticDef),
}

impl From<Instance> for MonoItem {
    fn from(instance: Instance) -> Self {
        MonoItem::Instance(instance)
    }
}

impl From<StaticDef> for MonoItem {
    fn from(static_def: StaticDef) -> Self {
        MonoItem::Static(static_def)
    }
}
