# stable-mir-wrapper

A thin wrapper library around Rust's StableMIR (Stable Mid-level Intermediate Representation) API.

## Overview

This library provides a stable interface to Rust's MIR by re-exporting types from `rustc_public` (the official StableMIR API). It serves as a temporary stable version until the official StableMIR API is stabilized and published to crates.io.

## Design Principles

- **Thin wrapper**: Re-exports `rustc_public` types directly with no duplication
- **Type aliases**: Provides convenient access to commonly used StableMIR types
- **No modification**: Does not modify or reimplement StableMIR types
- **Forward-compatible**: Will be easy to replace with the official StableMIR once published

## What This Library Provides

The library re-exports the following from `rustc_public`:

### MIR Types (`rustc_public::mir`)
- `Body`, `BasicBlock`, `BasicBlockIdx`, `Local`, `RETURN_LOCAL`
- `LocalDecl`, `VarDebugInfo`
- `Terminator`, `TerminatorKind`
- `Statement`, `StatementKind`
- `Rvalue`, `Operand`, `ConstOperand`
- `Place`, `ProjectionElem`, `FieldIdx`

### Monomorphization (`rustc_public::mir::mono`)
- `Instance`, `MonoItem`, `StaticDef`

### Type System (`rustc_public::ty`)
- `Ty`, `TyKind`, `RigidTy`

### Crate Definitions (`rustc_public`)
- `CrateDef`, `CrateItem`, `ItemKind`

### Entry Points (`rustc_public`)
- `run_with_tcx` macro
- `local_crate`, `entry_fn`, `all_local_items`

### Error Types
- `CompilerError`

## Usage Example

```rust
#![feature(rustc_private)]

extern crate rustc_public;

use stable_mir_wrapper::{
    Body, Instance, MonoItem, TerminatorKind, Operand,
    TyKind, RigidTy, CrateItem, ItemKind,
    run_with_tcx, local_crate, all_local_items,
};

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let _ = run_with_tcx!(&args, |tcx| {
        let crate_name = local_crate().name;
        println!("Analyzing crate: {}", crate_name);

        for item in all_local_items() {
            if let ItemKind::Fn = item.kind() {
                if let Ok(instance) = Instance::try_from(item) {
                    println!("Found function: {:?}", instance.name());
                }
            }
        }

        std::ops::ControlFlow::Continue(())
    });
}
```

## Current Status

- ✅ Re-exports all core StableMIR types from `rustc_public`
- ✅ Provides a stable interface for MIR analysis
- ✅ Compiles successfully with `nightly-2025-10-02`

## Differences from Original Plan

The original plan was to create a standalone implementation of StableMIR types. However, this approach was changed to:

1. **Re-export directly** from `rustc_public` instead of reimplementing types
2. **Avoid duplication** of the StableMIR implementation
3. **Stay in sync** with the official StableMIR API as it evolves

This makes the library much simpler and more maintainable.

## References

- [Rust StableMIR](https://github.com/rust-lang/rust/tree/main/compiler/rustc_public)
- [Unstable MIR Syntax](https://github.com/rust-lang/rust/blob/main/compiler/rustc_middle/src/mir/syntax.rs)
- [Conversion Bridge](https://github.com/rust-lang/rust/blob/main/compiler/rustc_public_bridge/src/context/impls.rs)

## License

BSD-3-Clause
