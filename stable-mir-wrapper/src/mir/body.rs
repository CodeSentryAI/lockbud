//! MIR body representation
//!
//! Represents a function body in MIR, matching rustc_public::mir::Body

use crate::ty::{Ty, Mutability};
use crate::mir::{Terminator, Statement};

/// Index of a basic block in the MIR body
pub type BasicBlockIdx = usize;

/// Index of a local variable
pub type Local = usize;

/// The return local is always at index 0
pub const RETURN_LOCAL: Local = 0;

/// A basic block - a sequence of statements with a terminator at the end
#[derive(Debug, Clone)]
pub struct BasicBlock {
    pub statements: Vec<Statement>,
    pub terminator: Terminator,
}

/// A MIR body - represents a single function
///
/// This is a stable, owned representation matching rustc_public::mir::Body
#[derive(Debug, Clone)]
pub struct Body {
    /// Basic blocks of the function
    pub blocks: Vec<BasicBlock>,
    /// Local variable declarations (private - accessed via methods)
    pub(super) locals: LocalDecls,
    /// Number of arguments (locals 1..arg_count+1 are arguments)
    pub(super) arg_count: usize,
    /// Debug information for variables
    pub var_debug_info: Vec<VarDebugInfo>,
    /// If present, the local that receives the "spread" argument (for Rust's spread syntax)
    pub(super) spread_arg: Option<Local>,
    /// The span of the function
    pub span: crate::crate_def::Span,
}

impl Body {
    /// Returns the local variable for the return value (always index 0)
    pub fn ret_local(&self) -> Local {
        RETURN_LOCAL
    }

    /// Returns the local variables for function arguments
    /// Arguments are at indices 1..arg_count+1
    pub fn arg_locals(&self) -> &[LocalDecl] {
        &self.locals.raw[1..=self.arg_count]
    }

    /// Returns the inner local variables (temporaries and user variables)
    /// These are at indices arg_count+1..
    pub fn inner_locals(&self) -> &[LocalDecl] {
        &self.locals.raw[self.arg_count + 1..]
    }

    /// Returns the declaration for a specific local variable
    pub fn local_decl(&self, local: Local) -> &LocalDecl {
        &self.locals.raw[local]
    }

    /// Returns all local variable declarations
    pub fn locals(&self) -> &[LocalDecl] {
        &self.locals.raw
    }

    /// Returns the number of local variables
    pub fn local_count(&self) -> usize {
        self.locals.raw.len()
    }

    /// Returns the number of arguments
    pub fn arg_count(&self) -> usize {
        self.arg_count
    }

    /// Returns the spread argument local, if present
    pub fn spread_arg(&self) -> Option<Local> {
        self.spread_arg
    }
}

/// Collection of local variable declarations
#[derive(Debug, Clone)]
pub struct LocalDecls {
    pub(super) raw: Vec<LocalDecl>,
}

/// Declaration of a local variable
#[derive(Debug, Clone)]
pub struct LocalDecl {
    /// The type of the local variable
    pub ty: Ty,
    /// The source span
    pub span: crate::crate_def::Span,
    /// Mutability of the local variable
    pub mutability: Mutability,
}

/// Debug information for a variable
#[derive(Debug, Clone)]
pub struct VarDebugInfo {
    /// The name of the variable
    pub name: String,
    /// The source span
    pub source_info: crate::crate_def::Span,
    /// The local variable this refers to
    pub local: Local,
}
