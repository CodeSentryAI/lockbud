//! Call graph construction using StableMIR
//!
//! This module implements a cross-crate call graph collector based on Kani's reachability.rs.
//! It uses StableMIR APIs to find all monomorphized items and construct a call graph,
//! including tracking where closures are created for deadlock analysis.

use std::collections::{HashMap, HashSet};
use std::fmt;

use stable_mir_wrapper::{
    Body, Instance, MonoItem, StaticDef,
    TerminatorKind, Rvalue, CastKind, PointerCoercion,
    Ty, TyKind, RigidTy, ClosureKind, FnDef,
    ConstOperand, Operand, Place,
    MirVisitor, Location, BasicBlockIdx,
    CrateItem, ItemKind,
};

/// Reason for introducing an edge in the call graph
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum CollectionReason {
    DirectCall,
    IndirectCall,
    Drop,
    ClosureCreation,
}

impl fmt::Display for CollectionReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CollectionReason::DirectCall => write!(f, "direct"),
            CollectionReason::IndirectCall => write!(f, "indirect"),
            CollectionReason::Drop => write!(f, "drop"),
            CollectionReason::ClosureCreation => write!(f, "closure_creation"),
        }
    }
}

/// A destination of an edge in the call graph with optional location
#[derive(Clone, Debug, Eq, PartialEq)]
struct CollectedItem {
    item: MonoItem,
    reason: CollectionReason,
    location: Option<CallSiteLocation>,  // Location for closure creations and calls
}

/// Detailed location information for a call site
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallSiteLocation {
    pub location: Location,
    pub is_thread_spawn: bool,
}

impl From<Location> for CallSiteLocation {
    fn from(loc: Location) -> Self {
        CallSiteLocation {
            location: loc,
            is_thread_spawn: false,
        }
    }
}

/// Information about a closure creation site
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClosureInfo {
    pub closure_instance: Instance,
    pub creation_location: CallSiteLocation,
    pub upvars: Vec<Place>,  // Captured variables
}

/// Call graph with edges annotated with the reason why they were added
#[derive(Debug, Default)]
pub struct CallGraph {
    /// Nodes of the graph
    nodes: HashSet<Node>,
    /// Edges of the graph (forward)
    edges: HashMap<Node, Vec<CollectedNode>>,
    /// Back edges for reverse traversal
    back_edges: HashMap<Node, Vec<CollectedNode>>,
    /// Map from closure instances to their creation sites
    pub closure_info: HashMap<Instance, Vec<ClosureInfo>>,
}

/// Newtype around MonoItem for use as graph nodes
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Node(pub MonoItem);

/// Newtype around CollectedItem for use as graph edges
#[derive(Clone, Debug, Eq, PartialEq)]
struct CollectedNode(pub CollectedItem);

impl CallGraph {
    /// Create a new empty call graph
    pub fn new() -> Self {
        Self::default()
    }

    /// Analyze all items in the crate and build the call graph
    pub fn analyze(&mut self, items: &[CrateItem]) {
        let mut collector = CallGraphCollector::new();
        collector.collect_crate(items);
        self.nodes = collector.call_graph.nodes;
        self.edges = collector.call_graph.edges;
        self.back_edges = collector.call_graph.back_edges;
    }

    /// Get all nodes in the graph
    pub fn nodes(&self) -> &HashSet<Node> {
        &self.nodes
    }

    /// Get the number of nodes in the graph
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Get the number of edges in the graph
    pub fn edge_count(&self) -> usize {
        self.edges.values().map(|v| v.len()).sum()
    }

    /// Get all successors (callees) of a node
    pub fn successors(&self, item: &MonoItem) -> Vec<MonoItem> {
        let node = Node(item.clone());
        self.edges
            .get(&node)
            .map(|edges| {
                edges.iter()
                    .map(|n| n.0.item.clone())
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get all predecessors (callers) of a node
    pub fn predecessors(&self, item: &MonoItem) -> Vec<MonoItem> {
        let node = Node(item.clone());
        self.back_edges
            .get(&node)
            .map(|edges| {
                edges.iter()
                    .map(|n| n.0.item.clone())
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get all closure creation sites for a given creator function
    pub fn closure_creations(&self, creator: &MonoItem) -> Vec<(MonoItem, CallSiteLocation)> {
        let node = Node(creator.clone());
        self.edges
            .get(&node)
            .map(|edges| {
                edges.iter()
                    .filter_map(|n| {
                        if n.0.reason == CollectionReason::ClosureCreation {
                            if let Some(loc) = &n.0.location {
                                Some((n.0.item.clone(), loc.clone()))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Print the graph in DOT format
    pub fn dump_dot(&self) -> String {
        let mut output = String::from("digraph CallGraph {\n");
        for node in &self.nodes {
            output.push_str(&format!("  \"{}\";\n", node));
            if let Some(successors) = self.edges.get(node) {
                for succ in successors {
                    output.push_str(&format!(
                        "  \"{}\" -> \"{}\" [label={}];\n",
                        node, succ, succ.0.reason
                    ));
                }
            }
        }
        output.push('}');
        output
    }

    /// Get closure information for a specific closure instance
    pub fn get_closure_info(&self, closure_instance: &Instance) -> &[ClosureInfo] {
        self.closure_info.get(closure_instance).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Get all closures created in a specific function
    pub fn get_closures_created_in(&self, creator: &MonoItem) -> Vec<&ClosureInfo> {
        let mut closures = Vec::new();
        if let Some(edges) = self.edges.get(&Node(creator.clone())) {
            for edge in edges {
                if edge.0.reason == CollectionReason::ClosureCreation {
                    if let MonoItem::Fn(instance) = &edge.0.item {
                        if let Some(info_list) = self.closure_info.get(instance) {
                            for info in info_list {
                                closures.push(info);
                            }
                        }
                    }
                }
            }
        }
        closures
    }
}

impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            MonoItem::Fn(instance) => write!(f, "{}", instance.name()),
            MonoItem::Static(def) => write!(f, "{:?}", def),
            MonoItem::GlobalAsm(asm) => write!(f, "{:?}", asm),
        }
    }
}

impl fmt::Display for CollectedNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0.item {
            MonoItem::Fn(instance) => write!(f, "{}", instance.name()),
            MonoItem::Static(def) => write!(f, "{:?}", def),
            MonoItem::GlobalAsm(asm) => write!(f, "{:?}", asm),
        }
    }
}

struct CallGraphCollector {
    collected: HashSet<MonoItem>,
    queue: Vec<MonoItem>,
    call_graph: CallGraphInternal,
}

struct CallGraphInternal {
    nodes: HashSet<Node>,
    edges: HashMap<Node, Vec<CollectedNode>>,
    back_edges: HashMap<Node, Vec<CollectedNode>>,
}

impl CallGraphCollector {
    fn new() -> Self {
        CallGraphCollector {
            collected: HashSet::default(),
            queue: Vec::default(),
            call_graph: CallGraphInternal {
                nodes: HashSet::default(),
                edges: HashMap::default(),
                back_edges: HashMap::default(),
            },
        }
    }

    fn collect_crate(&mut self, items: &[CrateItem]) {
        // Collect all function instances as starting points
        for item in items {
            if let ItemKind::Fn = item.kind() {
                if let Ok(instance) = Instance::try_from(*item) {
                    let mono_item = MonoItem::Fn(instance);
                    if !self.collected.contains(&mono_item) {
                        self.queue.push(mono_item);
                    }
                }
            }
        }
        self.reachable_items();
    }

    fn reachable_items(&mut self) {
        while let Some(to_visit) = self.queue.pop() {
            if !self.collected.contains(&to_visit) {
                self.collected.insert(to_visit.clone());
                let next_items = match &to_visit {
                    MonoItem::Fn(instance) => self.visit_fn(*instance),
                    MonoItem::Static(static_def) => self.visit_static(*static_def),
                    MonoItem::GlobalAsm(_) => vec![],
                };
                self.call_graph.add_edges(to_visit.clone(), &next_items);

                self.queue.extend(
                    next_items
                        .into_iter()
                        .filter_map(|CollectedItem { item, .. }| {
                            (!self.collected.contains(&item)).then_some(item)
                        }),
                );
            }
        }
    }

    fn visit_fn(&mut self, instance: Instance) -> Vec<CollectedItem> {
        let body = match instance.body() {
            Some(body) => body,
            None => return vec![],
        };

        let mut collector = FnCollector {
            collected: Vec::new(),
            body: &body,
        };
        collector.visit_body(&body);
        collector.collected
    }

    fn visit_static(&mut self, _def: StaticDef) -> Vec<CollectedItem> {
        // Statics don't have call edges in our simple implementation
        vec![]
    }
}

impl CallGraphInternal {
    fn add_node(&mut self, item: MonoItem) {
        let node = Node(item);
        self.nodes.insert(node.clone());
        self.edges.entry(node.clone()).or_default();
        self.back_edges.entry(node).or_default();
    }

    fn add_edge(&mut self, from: MonoItem, to: MonoItem, reason: CollectionReason, location: Option<CallSiteLocation>) {
        let from_node = Node(from.clone());
        let to_node = Node(to.clone());
        self.add_node(from.clone());
        self.add_node(to.clone());
        self.edges
            .get_mut(&from_node)
            .unwrap()
            .push(CollectedNode(CollectedItem { item: to, reason, location: location.clone() }));
        self.back_edges
            .get_mut(&to_node)
            .unwrap()
            .push(CollectedNode(CollectedItem {
                item: from,
                reason,
                location,
            }));
    }

    fn add_edges(&mut self, from: MonoItem, to: &[CollectedItem]) {
        self.add_node(from.clone());
        for CollectedItem { item, reason, location } in to {
            self.add_edge(from.clone(), item.clone(), *reason, location.clone());
        }
    }
}

struct FnCollector<'a> {
    collected: Vec<CollectedItem>,
    body: &'a Body,
}

impl FnCollector<'_> {
    fn collect_instance(&mut self, instance: Instance, is_direct_call: bool, location: Option<CallSiteLocation>) {
        // Check if instance has a body
        let has_body = instance.body().is_some();

        if !has_body {
            return;
        }

        if !instance.is_foreign_item() {
            let reason = if is_direct_call {
                CollectionReason::DirectCall
            } else {
                CollectionReason::IndirectCall
            };
            self.collected.push(CollectedItem {
                item: MonoItem::Fn(instance),
                reason,
                location,
            });
        }
    }

    /// Check if an instance is thread::spawn
    fn is_thread_spawn(&self, instance: &Instance) -> bool {
        let name = instance.name();
        name.contains("thread::spawn")
    }
}

impl MirVisitor for FnCollector<'_> {
    fn visit_rvalue(&mut self, rvalue: &Rvalue, location: Location) {
        match rvalue {
            Rvalue::Cast(CastKind::PointerCoercion(PointerCoercion::ReifyFnPointer), operand, _) => {
                let fn_ty = operand.ty(self.body.locals()).unwrap();
                if let TyKind::RigidTy(RigidTy::FnDef(fn_def, args)) = fn_ty.kind() {
                    if let Ok(instance) = Instance::resolve_for_fn_ptr(fn_def, &args) {
                        self.collect_instance(instance, false, None);
                    }
                }
            }
            Rvalue::Cast(
                CastKind::PointerCoercion(PointerCoercion::ClosureFnPointer(_)),
                operand,
                _,
            ) => {
                let source_ty = operand.ty(self.body.locals()).unwrap();
                if let TyKind::RigidTy(RigidTy::Closure(def_id, args)) = source_ty.kind() {
                    if let Ok(instance) =
                        Instance::resolve_closure(def_id, &args, ClosureKind::FnOnce)
                    {
                        // Track closure creation with location
                        let call_site_loc = CallSiteLocation {
                            location: location.clone(),
                            is_thread_spawn: false,
                        };
                        self.collect_instance(instance, false, Some(call_site_loc));
                    }
                }
            }
            _ => {}
        }
        self.super_rvalue(rvalue, location);
    }

    fn visit_terminator(&mut self, terminator: &stable_mir_wrapper::Terminator, location: Location) {
        match &terminator.kind {
            TerminatorKind::Call { func, args, .. } => {
                let fn_ty = func.ty(self.body.locals()).unwrap();
                if let TyKind::RigidTy(RigidTy::FnDef(fn_def, args_ty)) = fn_ty.kind() {
                    if let Ok(instance) = Instance::resolve(fn_def, &args_ty) {
                        // Check if this is a thread::spawn call
                        let is_spawn = self.is_thread_spawn(&instance);

                        let call_site_loc = CallSiteLocation {
                            location: location.clone(),
                            is_thread_spawn: is_spawn,
                        };

                        // If this is thread::spawn, try to extract the closure from the arguments
                        if is_spawn && !args.is_empty() {
                            // The first argument to thread::spawn is the closure
                            let closure_arg = &args[0];
                            if let Operand::Move(place) | Operand::Copy(place) = closure_arg {
                                let place_ty = place.ty(self.body.locals()).unwrap();
                                // Check if the argument is a closure type
                                if let TyKind::RigidTy(RigidTy::Closure(closure_def, closure_args)) = place_ty.kind() {
                                    if let Ok(closure_instance) = Instance::resolve_closure(
                                        closure_def,
                                        &closure_args,
                                        ClosureKind::FnOnce
                                    ) {
                                        // Collect the closure as a separate call edge
                                        self.collect_instance(closure_instance, true, Some(call_site_loc.clone()));
                                    }
                                }
                            }
                        }

                        self.collect_instance(instance, true, Some(call_site_loc));
                    }
                }
            }
            TerminatorKind::Drop { place, .. } => {
                let place_ty = place.ty(self.body.locals()).unwrap();
                let instance = Instance::resolve_drop_in_place(place_ty);
                let call_site_loc = CallSiteLocation {
                    location: location.clone(),
                    is_thread_spawn: false,
                };
                self.collect_instance(instance, true, Some(call_site_loc));
            }
            TerminatorKind::InlineAsm { .. } => {
                // Ignore inline assembly
            }
            TerminatorKind::Abort | TerminatorKind::Assert { .. } => {
                // These don't call functions
            }
            TerminatorKind::Goto { .. }
            | TerminatorKind::SwitchInt { .. }
            | TerminatorKind::Resume
            | TerminatorKind::Return
            | TerminatorKind::Unreachable => {}
        }
        self.super_terminator(terminator, location);
    }

    fn visit_ty(&mut self, ty: &Ty, _location: Location) {
        if let TyKind::RigidTy(RigidTy::FnDef(fn_def, args)) = ty.kind() {
            if let Ok(instance) = Instance::resolve(fn_def, &args) {
                self.collect_instance(instance, true, None);
            }
        }
        self.super_ty(ty);
    }
}
