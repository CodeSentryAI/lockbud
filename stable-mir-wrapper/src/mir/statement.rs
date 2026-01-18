//! MIR statement representation
//!
//! Statements are non-terminating instructions within a basic block

use crate::mir::{Place, Local, Rvalue, Operand};
use crate::ty::Ty;

/// A statement - a non-terminating instruction
#[derive(Debug, Clone)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: crate::crate_def::Span,
}

/// Kinds of statements
#[derive(Debug, Clone)]
pub enum StatementKind {
    /// Assign a value to a place
    Assign(Place, Rvalue),

    /// A fake read of a value
    ///
    /// This is used for various purposes like ensuring a value is used
    FakeRead(FakeReadCause, Place),

    /// Set the discriminant of an enum
    SetDiscriminant {
        place: Place,
        variant_index: usize, // VariantIdx
    },

    /// Mark a local variable as live
    StorageLive(Local),

    /// Mark a local variable as dead
    StorageDead(Local),

    /// Retag a reference for Stacked Borrows
    Retag(RetagKind, Place),

    /// Mention a place (e.g., for drop elaboration)
    PlaceMention(Place),

    /// Ascribe a user type to a place
    AscribeUserType {
        place: Place,
        projections: Vec<UserTypeProjection>,
        variance: Variance,
    },

    /// Coverage instrumentation
    Coverage(Coverage),

    /// A non-diverging intrinsic
    Intrinsic(NonDivergingIntrinsic),

    /// A counter for const evaluation
    ConstEvalCounter,

    /// No-op statement
    Nop,
}

/// Reason for a fake read
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeReadCause {
    /// For a match statement
    ForMatchGuard,
    /// For a match arm's scrutinee
    ForMatchedPlace,
    /// For a let binding
    ForLet,
    /// For a closure capture
    ForClosureCapture,
    /// For an index projection
    ForIndex,
}

/// Retagging kind for Stacked Borrows
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetagKind {
    /// Initial retagging (for function arguments)
    FnEntry,
    /// Retagging for a two-way borrow
    TwoWay,
    /// Retagging for a raw reference
    Raw,
    /// Default retagging
    Default,
}

/// User type projection (for type annotations)
#[derive(Debug, Clone)]
pub struct UserTypeProjection {
    pub base: usize,
    pub projections: Vec<TypeProjection>,
}

/// A single type projection
#[derive(Debug, Clone)]
pub enum TypeProjection {
    Field(usize, Ty),
    FieldProj(String, usize),
    Index(Ty),
    Subtype(Ty),
}

/// Variance for type ascriptions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variance {
    Covariant,
    Invariant,
    Contravariant,
    Bivariant,
}

/// Coverage instrumentation data
#[derive(Debug, Clone)]
pub struct Coverage {
    pub kind: CoverageKind,
    pub code_region: Option<String>,
}

/// Coverage instrumentation kind
#[derive(Debug, Clone)]
pub enum CoverageKind {
    /// A counter expression
    Counter {
        id: usize,
        region: String,
    },
    /// An expression that combines counters
    Expression {
        id: usize,
        lhs: usize,
        op: BinOp,
        rhs: usize,
        region: String,
    },
    /// Unreachable code
    Unreachable,
}

/// Binary operation for coverage expressions
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

/// A non-diverging intrinsic
#[derive(Debug, Clone)]
pub enum NonDivergingIntrinsic {
    /// Assume a condition is true
    Assume(Operand),
    /// A copy of a value for dereferencing
    CopyForDeref(Place),
}
