//! MIR visitor - traversal and analysis of MIR
//!
//! Provides a visitor pattern for traversing and analyzing MIR

use crate::mir::{Body, BasicBlock, Terminator, Statement, Place, Local, Operand, Rvalue};

/// A visitor for MIR
///
/// This trait allows traversing and analyzing MIR in a structured way
pub trait MirVisitor {
    /// Called at the start of visiting a body
    fn visit_body(&mut self, _body: &Body) {
        // Default: do nothing
    }

    /// Called at the end of visiting a body
    fn visit_body_post(&mut self, _body: &Body) {
        // Default: do nothing
    }

    /// Called for each basic block
    fn visit_basic_block(&mut self, _bb: &BasicBlock) {
        // Default: do nothing
    }

    /// Called for each statement
    fn visit_statement(&mut self, _stmt: &Statement) {
        // Default: do nothing
    }

    /// Called for each terminator
    fn visit_terminator(&mut self, _term: &Terminator) {
        // Default: do nothing
    }

    /// Called for each place
    fn visit_place(&mut self, _place: &Place, _context: PlaceContext) {
        // Default: do nothing
    }

    /// Called for each local
    fn visit_local(&mut self, _local: Local, _context: PlaceContext) {
        // Default: do nothing
    }

    /// Called for each operand
    fn visit_operand(&mut self, _operand: &Operand, _location: Location) {
        // Default: do nothing
    }

    /// Called for each rvalue
    fn visit_rvalue(&mut self, _rvalue: &Rvalue, _location: Location) {
        // Default: do nothing
    }
}

/// The context in which a place is used
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceContext {
    /// A non-mutating use (read)
    NonMutatingUse(NonMutatingUseContext),

    /// A mutating use
    MutatingUse(MutatingUseContext),

    /// A save (for later restoration)
    Saving,

    /// A debug use
    DebugUse,
}

/// Non-mutating use context
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonMutatingUseContext {
    /// Inspect a place
    Inspect,

    /// Copy from a place
    Copy,

    /// Move from a place
    Move,

    /// Borrow a place
    Borrow,

    /// Address of a place
    AddressOf,

    /// Compare a place
    Compare,

    /// Check if a place is live
    ///
    /// For drop elaboration
    Drop,

    /// Projection for debugging
    Projection,

    /// Match guard
    Guard,
}

/// Mutating use context
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutatingUseContext {
    /// Store to a place
    Store,

    /// Set discriminant
    SetDiscriminant,

    /// Store to a projection
    Projection,

    /// Call
    Call,

    /// Terminate
    Terminate,

    /// Yield
    Yield,

    /// Assemble user type
    AssembleUserType,

    /// Fake read
    FakeRead,

    /// Retag
    Retag,
}

/// A location in the MIR (basic block and statement index)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Location {
    /// The basic block
    pub block: usize,

    /// The statement index within the block
    pub statement_index: usize,
}

impl Location {
    /// Create a new location
    pub const fn new(block: usize, statement_index: usize) -> Self {
        Self { block, statement_index }
    }

    /// The start of a basic block (before any statements)
    pub const fn start(block: usize) -> Self {
        Self { block, statement_index: 0 }
    }
}
