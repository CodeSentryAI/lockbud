//! MIR rvalue representation
//!
//! Rvalues are expressions that produce a value

use crate::mir::{Place, Operand, Local};
use crate::ty::{Ty, GenericArgs};
use crate::crate_def::{
    StructDef, EnumDef, ClosureDef, CoroutineDef, CoroutineClosureDef,
    AdtDef,
};

pub type VariantIdx = usize;

/// An rvalue - an expression that produces a value
#[derive(Debug, Clone)]
pub enum Rvalue {
    /// Get the address of a place (create a raw pointer)
    AddressOf(RawPtrKind, Place),

    /// Create an aggregate value (struct, tuple, array, enum variant)
    Aggregate(AggregateKind, Vec<Operand>),

    /// Binary operation
    BinaryOp(BinOp, Operand, Operand),

    /// Cast a value to a different type
    Cast(CastKind, Operand, Ty),

    /// Checked binary operation (returns (result, overflowed))
    CheckedBinaryOp(BinOp, Operand, Operand),

    /// Copy a value for dereferencing
    CopyForDeref(Place),

    /// Get the discriminant of an enum
    Discriminant(Place),

    /// Get the length of a slice/array
    Len(Place),

    /// Create a reference
    Ref(Region, BorrowKind, Place),

    /// Repeat a value to create an array
    Repeat(Operand, TyConst),

    /// Create a shallow initialized box
    ShallowInitBox(Operand, Ty),

    /// Get a thread local static
    ThreadLocalRef(ThreadLocalDef),

    /// Unary operation
    UnaryOp(UnOp, Operand),

    /// Use an operand directly
    Use(Operand),
}

/// Kind of raw pointer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawPtrKind {
    Mutable,
    Immutable,
}

/// Aggregate kind for struct/tuple/array/enum construction
#[derive(Debug, Clone)]
pub enum AggregateKind {
    /// A struct or struct variant
    Struct(StructDef, GenericArgs),
    /// A tuple
    Tuple,
    /// An array
    Array(Ty),
    /// An enum variant
    EnumVariant(EnumDef, VariantIdx, GenericArgs),
    /// A closure
    Closure(ClosureDef, GenericArgs),
    /// A coroutine
    Coroutine(CoroutineDef, GenericArgs),
    /// A coroutine closure
    CoroutineClosure(CoroutineClosureDef, GenericArgs),
    /// An ADT (maybe with specialization)
    Adt(AdtDef, VariantIdx, GenericArgs, Option<usize>),
}

/// Binary operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    BitXor,
    BitAnd,
    BitOr,
    Shl,
    Shr,
    Eq,
    Lt,
    Le,
    Ne,
    Ge,
    Gt,
    Offset,
}

/// Cast kind
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastKind {
    PointerExposeAddress,
    PointerFromExposedAddress,
    Pointer(AddressCastKind),
    IntToInt(IntCastKind),
    FloatToInt,
    FloatToFloat,
    IntToFloat,
    FnPtrToPtr,
    PtrToPtr,
    PointerToExposedAddress,
}

/// Address cast kind
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressCastKind {
    PtrToPtr,
    InBounds,
}

/// Integer cast kind
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntCastKind {
    Signed,
    Unsigned,
}

/// Unary operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    /// Raw pointer metadata operation (for fat pointers)
    RawPtrMetadataOp,
}

/// Borrow kind
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorrowKind {
    /// A shared borrow (multiple readers allowed)
    Shared,
    /// A mutable borrow (exclusive access)
    Mut { kind: MutRefKind },
    /// A two-phase mutable borrow
    TwoPhaseMut { kind: MutRefKind },
    /// A frozen borrow (for closures)
    Frozen,
    /// A shallow freeze borrow
    Shallow,
}

/// Mutable reference kind
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutRefKind {
    Default,
    /// Capture at declaration point (for closures)
    CaptureAtDecl,
}

/// A region (lifetime) - simplified for stable MIR
#[derive(Debug, Clone)]
pub enum Region {
    /// An erased region (lifetime info not available)
    Erased,
}

/// Type constant (for array sizes, etc.)
#[derive(Debug, Clone)]
pub struct TyConst {
    pub inner: String, // Opaque representation
}

/// Thread local definition
pub type ThreadLocalDef = usize; // Opaque index
