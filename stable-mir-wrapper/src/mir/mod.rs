//! MIR (Mid-level Intermediate Representation) module
//!
//! This module provides stable types representing Rust's MIR, matching the
//! rustc_public::mir API one-to-one.

pub mod body;
pub mod terminator;
pub mod statement;
pub mod rvalue;
pub mod operand;
pub mod place;
pub mod mono;
pub mod visit;

// Re-export core types
pub use body::{Body, BasicBlock, BasicBlockIdx, Local, LocalDecls, RETURN_LOCAL, LocalDecl, VarDebugInfo};
pub use terminator::{Terminator, TerminatorKind};
pub use statement::{Statement, StatementKind};
pub use rvalue::Rvalue;
pub use operand::{Operand, ConstOperand};
pub use place::{Place, ProjectionElem, FieldIdx};
pub use mono::{Instance, MonoItem, StaticDef};

// Re-export helper types
pub use terminator::{SwitchTargets, UnwindAction, AssertMessage};
pub use statement::{FakeReadCause, RetagKind, Coverage, NonDivergingIntrinsic};
pub use rvalue::{BinOp, UnOp, CastKind, AggregateKind, RawPtrKind, BorrowKind};
pub use place::TyConst;
