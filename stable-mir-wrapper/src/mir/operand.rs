//! MIR operand representation
//!
//! Operands are the inputs to rvalues and terminators

use crate::mir::Place;

/// An operand - a value that is used as input
#[derive(Debug, Clone)]
pub enum Operand {
    /// Copy a value (for types that implement Copy)
    Copy(Place),

    /// Move a value (for types that don't implement Copy)
    Move(Place),

    /// A constant value
    Constant(ConstOperand),

    /// Runtime checks
    RuntimeChecks(RuntimeChecks),
}

/// A constant operand
#[derive(Debug, Clone)]
pub struct ConstOperand {
    pub span: crate::crate_def::Span,
    pub user_ty: Option<usize>, // UserTypeAnnotationIndex
    pub const_: MirConst,
}

/// A MIR constant
#[derive(Debug, Clone)]
pub enum MirConst {
    /// A typed constant value
    Typed(Ty, ConstValue),

    /// The allocation of a `static` or `const` item
    /// This contains the DefId of the static/const
    Items(usize), // DefId

    /// A zero-sized value (e.g., `()`, `[]`, empty structs)
    ZeroSized,

    /// An unevaluated constant (will be evaluated during MIR)
    Unevaluated(DefWithTyId, GenericArgs),
}

/// Definition with type
pub type DefWithTyId = usize; // Opaque index

/// Generic arguments
pub type GenericArgs = Vec<Ty>;

use crate::ty::Ty;

/// A constant value
#[derive(Debug, Clone)]
pub enum ConstValue {
    /// A scalar value (integer, float, bool, char)
    Scalar(Scalar),

    /// A slice of bytes
    Slice(Vec<u8>),

    /// An allocation in memory
    Allocation(Allocation),
}

/// A scalar value
#[derive(Debug, Clone)]
pub enum Scalar {
    /// An integer value
    Int(u128),

    /// A float value (as bits)
    Float(u64),

    /// A boolean value
    Bool(bool),

    /// A character value
    Char(char),

    /// A pointer value
    Ptr(u64),
}

/// A memory allocation
#[derive(Debug, Clone)]
pub struct Allocation {
    /// The bytes in the allocation
    pub bytes: Vec<u8>,

    /// Relocations (pointers to other allocations)
    pub relocations: Vec<(usize, usize)>, // (offset, def_id)

    /// Whether the allocation is mutable
    pub mutability: bool,
}

/// Runtime checks (for pointer provenance)
#[derive(Debug, Clone)]
pub struct RuntimeChecks {
    // Placeholder for future pointer provenance features
    _private: (),
}
