//! MIR place representation
//!
//! Places represent locations in memory (variables, fields, array elements, etc.)

use crate::ty::Ty;
use crate::mir::Local;

/// Index of a field in a struct or tuple
pub type FieldIdx = usize;

/// Index of an enum variant
pub type VariantIdx = usize;

/// A place - a location in memory
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Place {
    /// The local variable this place is based on
    pub local: Local,

    /// Projections from the local (field accesses, dereferences, etc.)
    pub projection: Vec<ProjectionElem>,
}

/// A projection element - a single step in a place projection
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProjectionElem {
    /// Dereference a pointer
    Deref,

    /// Access a field
    Field(FieldIdx, Ty),

    /// Index into an array/slice using a local variable
    Index(Local),

    /// Constant index with bounds checking
    ConstantIndex {
        offset: u64,
        min_length: u64,
        from_end: bool,
    },

    /// Subslice (for array slicing)
    Subslice {
        from: u64,
        to: u64,
        from_end: bool,
    },

    /// Downcast to a specific enum variant
    Downcast(VariantIdx),

    /// Opaque cast (for type erasure)
    OpaqueCast(Ty),
}

/// A type constant (used in ConstantIndex projections)
#[derive(Debug, Clone)]
pub struct TyConst {
    pub inner: String, // Opaque representation
}
