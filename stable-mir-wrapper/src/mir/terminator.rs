//! MIR terminator representation
//!
//! Terminators are the last statement in a basic block and describe control flow

use crate::mir::{BasicBlockIdx, Operand, Place};
use crate::ty::Ty;

/// A terminator - the last statement in a basic block
#[derive(Debug, Clone)]
pub struct Terminator {
    pub kind: TerminatorKind,
    pub span: crate::crate_def::Span,
}

/// Kinds of terminators
///
/// These represent different ways control flow can leave a basic block
#[derive(Debug, Clone)]
pub enum TerminatorKind {
    /// Jump to another basic block
    Goto { target: BasicBlockIdx },

    /// Switch on a value (e.g., match statement)
    SwitchInt {
        discr: Operand,
        targets: SwitchTargets,
    },

    /// Resume from unwinding (continue unwinding)
    Resume,

    /// Abort the process
    Abort,

    /// Return from the function
    Return,

    /// Unreachable code
    Unreachable,

    /// Drop a value
    Drop {
        place: Place,
        target: BasicBlockIdx,
        unwind: UnwindAction,
    },

    /// Function call
    Call {
        /// The function to call (typically an Operand::Constant with FnDef type)
        func: Operand,
        /// Arguments to pass
        args: Vec<Operand>,
        /// Where to store the return value
        destination: Place,
        /// Basic block to continue to after call (None if call diverges)
        target: Option<BasicBlockIdx>,
        /// What to do on unwinding
        unwind: UnwindAction,
    },

    /// Assert a condition is true
    Assert {
        cond: Operand,
        expected: bool,
        /// The message to emit if the assertion fails
        msg: AssertMessage,
        target: BasicBlockIdx,
        unwind: UnwindAction,
    },

    /// Inline assembly
    InlineAsm {
        template: String,
        operands: Vec<InlineAsmOperand>,
        options: String,
        line_spans: String,
        destination: Option<BasicBlockIdx>,
        unwind: UnwindAction,
    },
}

/// Targets for a SwitchInt terminator
#[derive(Debug, Clone)]
pub struct SwitchTargets {
    /// Mapping from values to target basic blocks
    /// Stored as (value, target) pairs
    pub branches: Vec<(u128, BasicBlockIdx)>,
    /// The "otherwise" target (default case)
    pub otherwise: BasicBlockIdx,
}

/// Action to take when unwinding
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnwindAction {
    /// Continue unwinding
    Continue,
    /// Unwind to the given basic block
    Cleanup(BasicBlockIdx),
    /// Terminate the process (cannot unwind)
    Terminate,
}

/// Message for an Assert terminator
#[derive(Debug, Clone)]
pub enum AssertMessage {
    /// A bounds check failed
    BoundsCheck {
        len: Operand,
        index: Operand,
    },
    /// An overflow check failed
    Overflow(OpKind, Operand, Operand),
    /// Division by zero
    DivisionByZero(Operand),
    /// Remainder by zero
    RemainderByZero(Operand),
    /// Generic assertion
    ///
    /// The string contains the assertion message
    Message(String),
}

/// Binary operation kind for overflow assertions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

/// An inline assembly operand
#[derive(Debug, Clone)]
pub struct InlineAsmOperand {
    /// Whether this is an input or output
    pub in_out: bool,
    /// The register or constraint
    pub reg: String,
    /// The value (for inputs)
    pub value: Option<Operand>,
}
