# Stable Analysis Module

This module provides program analysis implementations that depend only on the `stable_mir_wrapper` library, which wraps `rustc_public` StableMIR.

## Overview

The `stable_analysis` module mirrors the functionality of the `analysis` module but uses the stable StableMIR API instead of the unstable rustc_middle API. This makes the analysis:

- **Stable**: Not tied to internal rustc types that change frequently
- **Maintainable**: Uses the published StableMIR interface
- **Testable**: Easier to test as it doesn't require full rustc compilation

## Current Implementation

### Call Graph (`callgraph/`)

The call graph implementation provides:

**Core Types:**
- `CallGraph`: Main structure holding the graph data
- `CallGraphNode`: Represents a function (with or without MIR body)
- `CallSiteLocation`: Tracks where calls occur
- `InstanceId`: Node identifier in the graph

**Key Methods:**
- `analyze(items)`: Build the call graph from local items
- `instance_to_index(instance)`: Find node by Instance
- `index_to_instance(idx)`: Get Instance by index
- `callsites(source, target)`: Get call locations
- `callers(target)`: Find all callers of an instance
- `callees(source)`: Find all callees of an instance
- `all_simple_paths(source, target)`: Find all call paths
- `has_path(source, target)`: Check if path exists

## Design Differences from `analysis` Module

1. **No `TyCtxt` dependency**: Uses `CrateItem` instead of needing type context
2. **Simplified Instance handling**: Uses `stable_mir_wrapper::Instance` directly
3. **Location tracking**: Uses basic block indices instead of full `Location` type
4. **No promoted handling**: Simplified for initial implementation

## Usage Example

```rust
use stable_mir_wrapper::{CrateItem, all_local_items};
use stable_analysis::CallGraph;

// Collect all local items
let items: Vec<CrateItem> = all_local_items().into_iter().collect();

// Build the call graph
let mut callgraph = CallGraph::new();
callgraph.analyze(&items);

// Query the graph
for (idx, node) in callgraph.nodes() {
    let callers = callgraph.callers(idx);
    let callees = callgraph.callees(idx);
    println!("{}: {} callers, {} callees",
        node.instance().name(),
        callers.len(),
        callees.len()
    );
}
```

## Future Work

- [ ] Add proper callee resolution from `FnDef` types
- [ ] Add closure tracking
- [ ] Implement indirect call analysis
- [ ] Add more detailed location information
- [ ] Implement points-to analysis
- [ ] Implement control/data dependency analysis

## Testing

Run the demo binary:
```bash
cargo build --bin stable-demo
./target/debug/stable-demo --crate-name static_ref
```

Or use the detect.sh script pattern.
