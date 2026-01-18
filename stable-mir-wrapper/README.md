# stable-mir-wrapper

A thin wrapper library around Rust's StableMIR (Stable Mid-level Intermediate Representation) API.

## Overview

This library provides a stable, owned, index-based representation of Rust's MIR that matches the `rustc_public` StableMIR API one-to-one. It serves as a temporary stable version until the official StableMIR API is stabilized in the Rust compiler.

## Design Principles

- **Index-based**: All types use opaque indices (usize) instead of references
- **Owned data**: No lifetime parameters, all data is owned
- **One-to-one mapping**: Matches `rustc_public` StableMIR API exactly
- **Monomorphized only**: Only fully instantiated (monomorphic) MIR is exposed

## Structure

The library is organized into three main modules:

### `mir` - MIR Representation
- `body` - Function body, basic blocks, local variables
- `terminator` - Control flow terminators (return, call, switch, etc.)
- `statement` - Non-terminating statements (assign, storage, etc.)
- `rvalue` - Producing expressions (binary ops, casts, aggregates, etc.)
- `operand` - Inputs to rvalues (copy, move, constant)
- `place` - Memory locations (variables, fields, indices)
- `mono` - Monomorphized items (instances, statics)
- `visit` - Visitor trait for MIR traversal

### `ty` - Type System
- Type representations (`Ty`, `TyKind`, `RigidTy`)
- Primitive types (integers, floats, bool, char)
- Compound types (arrays, slices, tuples, ADTs)
- Function types and signatures
- Generic arguments and regions (lifetimes)

### `crate_def` - Crate Definitions
- Function, closure, coroutine definitions
- Struct, enum, union definitions
- Trait and trait impl definitions
- Static and constant definitions
- Source spans and locations

## Usage

This library provides type definitions but does not yet implement the actual conversion from unstable MIR to StableMIR. The types are ready to use as data structures.

```rust
use stable_mir_wrapper::mir::{Body, BasicBlock, TerminatorKind};
use stable_mir_wrapper::ty::{Ty, RigidTy};

// Types can be used to represent MIR data structures
let body = Body {
    blocks: vec![],
    locals: /* ... */,
    arg_count: 0,
    var_debug_info: vec![],
    spread_arg: None,
    span: /* ... */,
};
```

## Current Status

- ✅ All core StableMIR types implemented
- ✅ Library compiles successfully
- ⏳ Conversion from rustc unstable MIR not yet implemented
- ⏳ Methods that call into rustc return placeholder values

## Future Work

1. Implement conversion logic from `rustc_middle::mir` to StableMIR types
2. Implement the `rustc_public_bridge` equivalent conversion
3. Add thread-local context for managing type tables
4. Implement actual data retrieval methods (currently return `None`/`Err`/empty)

## References

- [Rust StableMIR](https://github.com/rust-lang/rust/tree/main/compiler/rustc_public)
- [Unstable MIR Syntax](https://github.com/rust-lang/rust/blob/main/compiler/rustc_middle/src/mir/syntax.rs)
- [Conversion Bridge](https://github.com/rust-lang/rust/blob/main/compiler/rustc_public_bridge/src/context/impls.rs)

## License

BSD-3-Clause
