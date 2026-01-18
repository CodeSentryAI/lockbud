# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

LockBud is a static analysis tool for Rust that detects concurrency bugs, memory safety issues, and panic locations. It operates as a rustc driver plugin that analyzes MIR (Mid-level Intermediate Representation) during compilation.

**Required rustc version:** `nightly-2025-10-02` - This version must match exactly when analyzing target projects.

## Common Development Commands

### Building LockBud
```bash
# Build debug version (for development)
cargo build

# Build release version (for analyzing large projects)
cargo build --release

# Install as a cargo subcommand
cargo +nightly-2025-10-02 install --path .
```

### Running Tests
```bash
# Run all tests
cargo test

# Run tests for a specific module
cargo test --package lockbud --lib options::tests

# Run a single test
cargo test test_parse_from_str_blacklist_ok
```

### Testing LockBud on Sample Projects
```bash
# Using detect.sh script (development mode)
./detect.sh toys/inter
./detect.sh toys/conflict-inter

# Using cargo-lockbud directly
cd toys/inter
cargo clean
cargo lockbud -k deadlock

# With crate filtering (blacklist common dependencies)
cargo lockbud -k deadlock -b -l cc,tokio_util,indicatif
```

### Detector Configuration

LockBud accepts flags via environment variable `LOCKBUD_FLAGS`:

- `-k {kind}` or `--detector-kind {kind}`: Select detector type
  - `deadlock` - Double-lock, conflicting lock order, condvar misuse (default)
  - `atomicity_violation` - Atomic operation violations
  - `memory` - Use-after-free, invalid-free
  - `panic` - Panic locations
  - `all` - Run all detectors

- `-b` or `--blacklist-mode`: Treat crate list as blacklist (default is whitelist)

- `-l {crates}` or `--crate-name-list {crates}`: Comma-separated crate names

Example:
```bash
export LOCKBUD_FLAGS="-k deadlock -b -l cc,tokio_util"
```

### Linting
```bash
# Run clippy
cargo clippy

# Format code
cargo fmt
```

## Architecture Overview

### Entry Points

1. **`src/main.rs`** - Standalone `lockbud` binary that implements `rustc_driver::Callbacks`
2. **`src/bin/cargo-lockbud.rs`** - Cargo wrapper subcommand that sets `RUSTC_WRAPPER` environment variable

### Core Analysis Framework

The analysis is organized in layers:

**`src/analysis/`** - Program analysis infrastructure
- `callgraph/` - Inter-procedural call graph construction. Maps monomorphized function instances to nodes, tracks callsites with location data.
- `pointsto/` - Andersen-style pointer analysis with field-sensitive intra-procedural analysis. Provides alias queries with confidence levels (Probably/Possibly/Unlikely). Results are cached for performance.
- `controldep/` - Control dependency analysis for conditional execution paths
- `datadep/` - Data dependency analysis for value flow tracking
- `defuse/` - Definition-use chains
- `postdom/` - Post-dominator tree computation

**`src/interest/`** - Pattern matching system to identify relevant code
- `concurrency/` - Detects lock guards, atomic operations, condition variables
- `memory/` - Detects ownership patterns, casts, raw pointers

This system filters code patterns to reduce analysis overhead - only functions containing "interesting" patterns are fully analyzed.

### Detectors

**`src/detector/`** - Bug detection modules

All detectors follow a common pattern:
1. Collect relevant artifacts across all instances using the interest system
2. Apply analysis using the shared framework (callgraph, points-to, etc.)
3. Generate unified `Report` enum with JSON serialization

- **`lock/`** - Deadlock detection (double-lock, conflicting lock order, condvar misuse)
  - Supports `std::sync::{Mutex, RwLock}`, `parking_lot::{Mutex, RwLock}`, `spin::{Mutex, RwLock}`
  - Uses GenKill algorithm on callgraph to find lock guard pairs
  - Builds lock ordering graphs to detect cycles

- **`memory/`** - Memory safety (use-after-free, invalid-free)
  - Tracks memory allocations and deallocations
  - Integrates with points-to analysis

- **`atomic/`** - Atomicity violations
  - Identifies improper atomic operation ordering
  - Uses control and data dependency analysis

- **`panic/`** - Panic location detection
  - Uses regex matching for Result/Option unwraps
  - Tracks panic API calls

- **`report.rs`** - Unified reporting structure with JSON output

### Key Data Structures

**Call Graph:**
```rust
pub struct CallGraph<'tcx> {
    graph: Graph<CallGraphNode<'tcx>, Vec<CallSiteLocation>, Directed>,
}
```
Nodes are monomorphized function instances (instances are created for each combination of generic type parameters). Edge weights contain location information.

**Points-to Analysis:**
```rust
pub enum ConstraintNode<'tcx> {
    Alloc(PlaceRef<'tcx>),
    Place(PlaceRef<'tcx>),
    Constant(Const<'tcx>),
    ConstantDeref(Const<'tcx>),
}

pub enum ApproximateAliasKind {
    Probably,   // Strong aliasing evidence
    Possibly,   // Potential aliasing
    Unlikely,   // Unlikely aliasing
    Unknown,    // Cannot determine
}
```

**Unified Reporting:**
```rust
pub enum Report {
    DoubleLock(ReportContent<DeadlockDiagnosis>),
    ConflictLock(ReportContent<Vec<DeadlockDiagnosis>>),
    CondvarDeadlock(ReportContent<CondvarDeadlockDiagnosis>),
    AtomicityViolation(ReportContent<AtomicityViolationDiagnosis>),
    InvalidFree(ReportContent<String>),
    UseAfterFree(ReportContent<String>),
}
```

### Analysis Flow

1. **Initialization** (`src/callbacks.rs`):
   - Collect all monomorphized instances from rustc
   - Build call graph for the entire crate
   - Initialize points-to analysis with caching

2. **Per-Detection**:
   - Run interest system to identify relevant functions
   - Collect artifacts (lock guards, allocations, etc.)
   - Apply detector-specific analysis using shared framework
   - Generate JSON reports

3. **Output**:
   - Reports printed to stdout as JSON
   - Each report includes bug kind, confidence level, diagnosis with source spans, and call chains

## Important Notes

### Detector Limitations

**Deadlock Detectors** (most mature):
- Only supports `std::sync`, `parking_lot`, and `spin` lock types
- Call graph is crate-specific (cannot track indirect calls across crate boundaries)
- Points-to analysis makes heuristic assumptions - common false positives from `cc` crate
- Use `-b -l cc` to blacklist known problematic dependencies

**Memory Detectors**:
- May report false positives from standard library and common dependencies
- Recommend using `-l your_project_name` to focus on your own code

**Panic Detector**:
- Very conservative - may report nearly all panic locations
- Less useful due to lack of sophisticated path sensitivity

### Performance Considerations

- The project prioritizes low overhead over precision (no SMT solvers)
- Points-to analysis results are cached per instance
- Interest system reduces analysis scope by filtering irrelevant code
- Use release builds for analyzing large projects

### Known Issues

- Codebase was implemented quickly and has technical debt planned for refactoring (see issue #58)
- Cannot track indirect calls in call graph
- Points-to analysis is imprecise for function calls and assignments

## Development Workflow

When modifying LockBud:
1. Make changes to source code
2. Build with `cargo build`
3. Test on toy examples: `./detect.sh toys/inter`
4. Modify `LOCKBUD_FLAGS` in `detect.sh` to test specific detectors
5. For production use, build release version and update `RUSTC_WRAPPER` path

When adding a new detector:
1. Implement detector module following the common pattern in `src/detector/`
2. Add variant to `Report` enum in `src/detector/report.rs`
3. Wire up in `src/callbacks.rs` analysis loop
4. Add detector kind to `src/options.rs`
5. Create toy examples in `toys/` directory for testing
