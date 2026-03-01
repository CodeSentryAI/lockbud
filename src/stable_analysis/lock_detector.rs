//! Main Mutex deadlock detector
//!
//! This module orchestrates the deadlock detection by collecting lock guards,
//! building a lock ordering graph, and detecting cycles.

use std::collections::{HashMap, HashSet};
use std::collections::hash_map::Entry;

use stable_mir_wrapper::{CrateItem, Instance};

use crate::stable_analysis::callgraph::CallGraph;
use crate::stable_analysis::lock_types::{
    LockGuardId, LockGuardTy, LockGuardInfo, DeadlockPossibility, MirLocation,
};
use crate::stable_analysis::lock_lifecycle::LockLifecycleCollector;
use crate::stable_analysis::lock_identity::{MutexIdentity, MutexIdentityAnalyzer};
use stable_mir_wrapper::Ty;

/// Compare two Ty objects for equality
/// Ty doesn't implement PartialEq, so we compare their debug representations
fn ty_equal(a: &Ty, b: &Ty) -> bool {
    format!("{:?}", a) == format!("{:?}", b)
}

/// Lock report detailing a potential deadlock
#[derive(Clone, Debug)]
pub struct LockReport {
    pub kind: LockBugKind,
    pub possibility: DeadlockPossibility,
    /// First lock description
    pub first_lock: LockDesc,
    /// Second lock description
    pub second_lock: LockDesc,
    /// Call chain showing how the deadlock occurs
    pub callchain: Vec<String>,
}

/// Description of a lock in a deadlock report
#[derive(Clone, Debug)]
pub struct LockDesc {
    /// Lock type (e.g., "MutexGuard<i32>")
    pub lock_type: String,
    /// Function where lock was acquired
    pub function: String,
    /// Variable name (if available)
    pub var_name: Option<String>,
    /// Source location (file:line:col)
    pub source_loc: Option<String>,
    /// MIR location for debugging
    pub mir_loc: String,
}

/// Type of lock bug detected
#[derive(Clone, Copy, Debug)]
pub enum LockBugKind {
    DoubleLock,
    ConflictLock,
}

/// Lock ordering graph
struct LockGraph {
    /// Nodes: (instance, mutex_id_string, guard_local)
    nodes: HashSet<(Instance, String, stable_mir_wrapper::Local)>,
    /// Edges: (held_mutex, acquired_mutex) -> locations
    edges: HashMap<(String, String), Vec<MirLocation>>,
}

impl LockGraph {
    fn new() -> Self {
        Self {
            nodes: HashSet::new(),
            edges: HashMap::new(),
        }
    }

    fn add_edge(&mut self, from: String, to: String, loc: MirLocation) {
        self.edges
            .entry((from, to))
            .or_insert_with(Vec::new)
            .push(loc);
    }

    /// Find cycles in the graph
    fn find_cycles(&self) -> Vec<Vec<String>> {
        let mut cycles = Vec::new();

        // Build node set
        let mut node_set = HashSet::new();
        for (from, to) in self.edges.keys() {
            node_set.insert(from.clone());
            node_set.insert(to.clone());
        }

        // Simple cycle detection using DFS
        let nodes: Vec<String> = node_set.into_iter().collect();
        let mut visited = HashSet::new();
        let mut rec_stack = Vec::new();

        for node in &nodes {
            if !visited.contains(node) {
                self.dfs_cycle(node, &nodes, &mut visited, &mut rec_stack, &mut cycles);
            }
        }

        cycles
    }

    fn dfs_cycle(
        &self,
        node: &str,
        all_nodes: &[String],
        visited: &mut HashSet<String>,
        rec_stack: &mut Vec<String>,
        cycles: &mut Vec<Vec<String>>,
    ) {
        visited.insert(node.to_string());
        rec_stack.push(node.to_string());

        // Find all successors of this node
        let successors: Vec<String> = all_nodes
            .iter()
            .filter(|n| {
                self.edges.contains_key(&(node.to_string(), n.to_string()))
                    || self.edges.contains_key(&(n.to_string(), node.to_string()))
            })
            .cloned()
            .collect();

        for succ in &successors {
            if !visited.contains(succ) {
                self.dfs_cycle(succ, all_nodes, visited, rec_stack, cycles);
            } else if rec_stack.contains(succ) {
                // Found a cycle
                let cycle_start = rec_stack.iter().position(|x| x == succ).unwrap();
                let cycle: Vec<String> = rec_stack[cycle_start..].to_vec();
                cycles.push(cycle);
            }
        }

        rec_stack.pop();
    }
}

/// Main deadlock detector
pub struct LockDetector {
    pub reports: Vec<LockReport>,
    lock_graph: LockGraph,
}

impl LockDetector {
    /// Check if two guards are on the same mutex, using both identity and type information
    fn guards_on_same_mutex(
        &self,
        id1: &LockGuardId,
        info1: &LockGuardInfo,
        id2: &LockGuardId,
        info2: &LockGuardInfo,
        identities: &HashMap<LockGuardId, MutexIdentity>,
        callgraph: &CallGraph,
    ) -> bool {
        use log::debug;

        let same_mutex = match (identities.get(id1), identities.get(id2)) {
            (Some(id1), Some(id2)) => {
                let analyzer = MutexIdentityAnalyzer::new();
                analyzer.same_mutex(id1, id2, callgraph)
            }
            _ => false,
        };

        if same_mutex {
            debug!("    [guards_on_same_mutex] Same mutex by identity: {} vs {}", id1, id2);
            return true;
        }

        // If identities are different, check if they're Local in same instance with same type
        match (identities.get(id1), identities.get(id2)) {
            (Some(MutexIdentity::Local { instance: i1, local: l1 }),
             Some(MutexIdentity::Local { instance: i2, local: l2 })) => {
                if i1 == i2 && l1 != l2 {
                    // Same instance, different locals - use type info to guess
                    let same_lock_type = std::mem::discriminant(&info1.lockguard_ty)
                        .eq(&std::mem::discriminant(&info2.lockguard_ty));

                    // For RwLock, also check if we have incompatible lock types (read vs write)
                    let is_incompatible_rwlock = matches!(
                        (&info1.lockguard_ty, &info2.lockguard_ty),
                        (
                            LockGuardTy::StdRwLockRead(_),
                            LockGuardTy::StdRwLockWrite(_)
                        ) | (
                            LockGuardTy::StdRwLockWrite(_),
                            LockGuardTy::StdRwLockRead(_)
                        ) | (
                            LockGuardTy::ParkingLotRead(_),
                            LockGuardTy::ParkingLotWrite(_)
                        ) | (
                            LockGuardTy::ParkingLotWrite(_),
                            LockGuardTy::ParkingLotRead(_)
                        ) | (
                            LockGuardTy::SpinRead(_),
                            LockGuardTy::SpinWrite(_)
                        ) | (
                            LockGuardTy::SpinWrite(_),
                            LockGuardTy::SpinRead(_)
                        )
                    );

                    if same_lock_type || is_incompatible_rwlock {
                        // Check if they protect the same data type
                        let same_protected = match (&info1.lockguard_ty, &info2.lockguard_ty) {
                            (crate::stable_analysis::lock_types::LockGuardTy::StdMutex(a),
                             crate::stable_analysis::lock_types::LockGuardTy::StdMutex(b)) => ty_equal(a, b),
                            (crate::stable_analysis::lock_types::LockGuardTy::ParkingLotMutex(a),
                             crate::stable_analysis::lock_types::LockGuardTy::ParkingLotMutex(b)) => ty_equal(a, b),
                            (crate::stable_analysis::lock_types::LockGuardTy::SpinMutex(a),
                             crate::stable_analysis::lock_types::LockGuardTy::SpinMutex(b)) => ty_equal(a, b),
                            (crate::stable_analysis::lock_types::LockGuardTy::StdRwLockRead(a),
                             crate::stable_analysis::lock_types::LockGuardTy::StdRwLockRead(b)) => ty_equal(a, b),
                            (crate::stable_analysis::lock_types::LockGuardTy::StdRwLockWrite(a),
                             crate::stable_analysis::lock_types::LockGuardTy::StdRwLockWrite(b)) => ty_equal(a, b),
                            // Incompatible RwLock types - still check data type
                            (crate::stable_analysis::lock_types::LockGuardTy::StdRwLockRead(a),
                             crate::stable_analysis::lock_types::LockGuardTy::StdRwLockWrite(b)) => ty_equal(a, b),
                            (crate::stable_analysis::lock_types::LockGuardTy::StdRwLockWrite(a),
                             crate::stable_analysis::lock_types::LockGuardTy::StdRwLockRead(b)) => ty_equal(a, b),
                            (crate::stable_analysis::lock_types::LockGuardTy::ParkingLotRead(a),
                             crate::stable_analysis::lock_types::LockGuardTy::ParkingLotRead(b)) => ty_equal(a, b),
                            (crate::stable_analysis::lock_types::LockGuardTy::ParkingLotWrite(a),
                             crate::stable_analysis::lock_types::LockGuardTy::ParkingLotWrite(b)) => ty_equal(a, b),
                            (crate::stable_analysis::lock_types::LockGuardTy::ParkingLotRead(a),
                             crate::stable_analysis::lock_types::LockGuardTy::ParkingLotWrite(b)) => ty_equal(a, b),
                            (crate::stable_analysis::lock_types::LockGuardTy::ParkingLotWrite(a),
                             crate::stable_analysis::lock_types::LockGuardTy::ParkingLotRead(b)) => ty_equal(a, b),
                            (crate::stable_analysis::lock_types::LockGuardTy::SpinRead(a),
                             crate::stable_analysis::lock_types::LockGuardTy::SpinRead(b)) => ty_equal(a, b),
                            (crate::stable_analysis::lock_types::LockGuardTy::SpinWrite(a),
                             crate::stable_analysis::lock_types::LockGuardTy::SpinWrite(b)) => ty_equal(a, b),
                            (crate::stable_analysis::lock_types::LockGuardTy::SpinRead(a),
                             crate::stable_analysis::lock_types::LockGuardTy::SpinWrite(b)) => ty_equal(a, b),
                            (crate::stable_analysis::lock_types::LockGuardTy::SpinWrite(a),
                             crate::stable_analysis::lock_types::LockGuardTy::SpinRead(b)) => ty_equal(a, b),
                            _ => false,
                        };

                        debug!("    [guards_on_same_mutex] Same instance, different locals: same_lock_type={}, is_incompatible_rwlock={}, same_protected={}", same_lock_type, is_incompatible_rwlock, same_protected);
                        same_protected
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            _ => {
                debug!("    [guards_on_same_mutex] Not both Local mutexes: {:?} {:?}", identities.get(id1), identities.get(id2));
                false
            }
        }
    }
    pub fn new() -> Self {
        Self {
            reports: Vec::new(),
            lock_graph: LockGraph::new(),
        }
    }

    /// Main entry point: analyze all functions in the crate
    pub fn detect(
        &mut self,
        items: &[CrateItem],
        callgraph: &CallGraph,
    ) -> Vec<LockReport> {
        // Phase 1: Collect all lock guards across all instances
        let mut all_lockguards = HashMap::new();
        let mut instances_with_locks: Vec<Instance> = Vec::new();

        for item in items {
            if let Ok(instance) = Instance::try_from(*item) {
                if let Some(body) = instance.body() {
                    let mut collector = LockLifecycleCollector::new(instance, &body);
                    collector.analyze();

                    if !collector.lockguards.is_empty() {
                        for (id, info) in collector.lockguards.iter() {
                            all_lockguards.insert(*id, info.clone());
                        }
                        instances_with_locks.push(instance);
                    }
                }
            }
        }

        // Phase 2: Analyze mutex identities
        let mut identity_analyzer = MutexIdentityAnalyzer::new();
        let mut mutex_identities = HashMap::new();

        for (guard_id, _guard_info) in &all_lockguards {
            // Re-fetch the body when needed
            if let Some(body) = guard_id.instance.body() {
                if let Some(identity) = identity_analyzer.analyze_mutex_from_guard(
                    guard_id.local,
                    guard_id.instance,
                    &body,
                    callgraph,
                ) {
                    mutex_identities.insert(*guard_id, identity);
                }
            }
        }

        // Phase 3: Build lock ordering graph
        self.build_lock_graph(
            &all_lockguards,
            &mutex_identities,
            &instances_with_locks,
            callgraph,
        );

        // Phase 4: Detect deadlocks
        self.detect_deadlocks(&all_lockguards, &mutex_identities, callgraph);

        self.reports.clone()
    }

    fn build_lock_graph(
        &mut self,
        lockguards: &HashMap<LockGuardId, LockGuardInfo>,
        identities: &HashMap<LockGuardId, MutexIdentity>,
        instances: &[Instance],
        _callgraph: &CallGraph,
    ) {
        // For each instance, analyze lock acquisition order
        for instance in instances {
            // Find all lock acquisitions in this instance
            let mut lock_acquisitions = Vec::new();

            for (guard_id, info) in lockguards {
                if guard_id.instance != *instance {
                    continue;
                }

                for gen_loc in &info.gen_locs {
                    lock_acquisitions.push((*guard_id, gen_loc.clone()));
                }
            }

            // Sort by location to get temporal order
            lock_acquisitions.sort_by_key(|(_, loc)| *loc);

            // Build edges: each lock held when next lock acquired
            for (i, (guard1, loc1)) in lock_acquisitions.iter().enumerate() {
                for (guard2, loc2) in lock_acquisitions.iter().skip(i + 1) {
                    // Check if guard1 is still alive when guard2 is acquired
                    if self.is_guard_alive_at(guard1, loc2, lockguards) {
                        // Get guard info for both guards
                        if let (Some(info1), Some(info2)) = (lockguards.get(guard1), lockguards.get(guard2)) {
                            // Skip adding edges when guards are on the same mutex
                            // This prevents self-loops which are detected as ConflictLocks
                            // (they should be detected as DoubleLocks instead)
                            if self.guards_on_same_mutex(guard1, info1, guard2, info2, identities, _callgraph) {
                                continue;
                            }

                            if let (Some(id1), Some(id2)) = (
                                identities.get(guard1),
                                identities.get(guard2),
                            ) {
                                let key1 = self.mutex_id_to_string(id1);
                                let key2 = self.mutex_id_to_string(id2);

                                self.lock_graph.add_edge(
                                    key1.clone(),
                                    key2.clone(),
                                    loc2.clone(),
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    fn is_guard_alive_at(
        &self,
        guard_id: &LockGuardId,
        location: &MirLocation,
        lockguards: &HashMap<LockGuardId, LockGuardInfo>,
    ) -> bool {
        if let Some(info) = lockguards.get(guard_id) {
            info.is_alive_at(location)
        } else {
            false
        }
    }

    fn mutex_id_to_string(&self, id: &MutexIdentity) -> String {
        format!("{}", id)
    }

    fn detect_deadlocks(
        &mut self,
        lockguards: &HashMap<LockGuardId, LockGuardInfo>,
        identities: &HashMap<LockGuardId, MutexIdentity>,
        callgraph: &CallGraph,
    ) {
        // Detect double lock: same mutex locked twice
        self.detect_double_lock(lockguards, identities, callgraph);

        // Detect conflicting lock: cycles in lock graph
        self.detect_conflicting_lock();
    }

    fn detect_double_lock(
        &mut self,
        lockguards: &HashMap<LockGuardId, LockGuardInfo>,
        identities: &HashMap<LockGuardId, MutexIdentity>,
        callgraph: &CallGraph,
    ) {
        use log::debug;

        // Group guards by instance for analysis
        let mut guards_by_instance: std::collections::HashMap<Instance, Vec<&LockGuardId>> = std::collections::HashMap::new();

        for guard_id in lockguards.keys() {
            guards_by_instance.entry(guard_id.instance).or_default().push(guard_id);
        }

        // For each instance, check for double locks
        for (instance, guards) in guards_by_instance {
            debug!("  [detect_double_lock] Checking instance: {}", instance.name());
            debug!("  [detect_double_lock] Found {} guards", guards.len());

            // Check pairs of guards within the same instance
            for i in 0..guards.len() {
                for j in (i + 1)..guards.len() {
                    let id1 = guards[i];
                    let id2 = guards[j];

                    let info1 = &lockguards[id1];
                    let info2 = &lockguards[id2];

                    debug!("  [detect_double_lock] Checking pair: {} vs {}", id1, id2);
                    debug!("  [detect_double_lock] Identities: {:?} {:?}", identities.get(id1), identities.get(id2));

                    // Check if they are on the SAME mutex instance
                    // This is critical - two guards on different mutexes should not be a double lock
                    let same_mutex = match (identities.get(id1), identities.get(id2)) {
                        (Some(_id1), Some(_id2)) => {
                            // Use the helper function that considers both identity and type info
                            self.guards_on_same_mutex(id1, info1, id2, info2, identities, callgraph)
                        }
                        (None, None) => {
                            // Both identities unknown - conservatively assume same mutex for local guards
                            debug!("  [detect_double_lock] Both identities unknown, assuming same mutex");
                            true
                        }
                        _ => {
                            debug!("  [detect_double_lock] One or both identities unknown, treating as different mutex");
                            false
                        }
                    };

                    if !same_mutex {
                        debug!("  [detect_double_lock] Different mutex instances, skipping");
                        continue;  // Different mutex instances - not a double lock
                    }

                    // For RwLock, check if we have incompatible lock types (read vs write)
                    // This is also a deadlock situation
                    let is_incompatible_rwlock = matches!(
                        (&info1.lockguard_ty, &info2.lockguard_ty),
                        (
                            LockGuardTy::StdRwLockRead(_),
                            LockGuardTy::StdRwLockWrite(_)
                        ) | (
                            LockGuardTy::StdRwLockWrite(_),
                            LockGuardTy::StdRwLockRead(_)
                        ) | (
                            LockGuardTy::ParkingLotRead(_),
                            LockGuardTy::ParkingLotWrite(_)
                        ) | (
                            LockGuardTy::ParkingLotWrite(_),
                            LockGuardTy::ParkingLotRead(_)
                        ) | (
                            LockGuardTy::SpinRead(_),
                            LockGuardTy::SpinWrite(_)
                        ) | (
                            LockGuardTy::SpinWrite(_),
                            LockGuardTy::SpinRead(_)
                        )
                    );

                    // Check if they have the same lock type (or incompatible for RwLock)
                    let same_lock_type = std::mem::discriminant(&info1.lockguard_ty)
                        .eq(&std::mem::discriminant(&info2.lockguard_ty));

                    if !same_lock_type && !is_incompatible_rwlock {
                        debug!("  [detect_double_lock] Different lock types (not incompatible RwLock): {:?} vs {:?}", info1.lockguard_ty, info2.lockguard_ty);
                        continue;  // Different lock types (not incompatible RwLock)
                    }

                    // Check if they protect the same data type
                    let same_data = match (&info1.lockguard_ty, &info2.lockguard_ty) {
                        (crate::stable_analysis::lock_types::LockGuardTy::StdMutex(a),
                         crate::stable_analysis::lock_types::LockGuardTy::StdMutex(b)) => ty_equal(a, b),
                        (crate::stable_analysis::lock_types::LockGuardTy::ParkingLotMutex(a),
                         crate::stable_analysis::lock_types::LockGuardTy::ParkingLotMutex(b)) => ty_equal(a, b),
                        (crate::stable_analysis::lock_types::LockGuardTy::SpinMutex(a),
                         crate::stable_analysis::lock_types::LockGuardTy::SpinMutex(b)) => ty_equal(a, b),
                        (crate::stable_analysis::lock_types::LockGuardTy::StdRwLockRead(a),
                         crate::stable_analysis::lock_types::LockGuardTy::StdRwLockRead(b)) => ty_equal(a, b),
                        (crate::stable_analysis::lock_types::LockGuardTy::StdRwLockWrite(a),
                         crate::stable_analysis::lock_types::LockGuardTy::StdRwLockWrite(b)) => ty_equal(a, b),
                        // Incompatible RwLock types - still need to check data type
                        (crate::stable_analysis::lock_types::LockGuardTy::StdRwLockRead(a),
                         crate::stable_analysis::lock_types::LockGuardTy::StdRwLockWrite(b)) => ty_equal(a, b),
                        (crate::stable_analysis::lock_types::LockGuardTy::StdRwLockWrite(a),
                         crate::stable_analysis::lock_types::LockGuardTy::StdRwLockRead(b)) => ty_equal(a, b),
                        (crate::stable_analysis::lock_types::LockGuardTy::ParkingLotRead(a),
                         crate::stable_analysis::lock_types::LockGuardTy::ParkingLotRead(b)) => ty_equal(a, b),
                        (crate::stable_analysis::lock_types::LockGuardTy::ParkingLotWrite(a),
                         crate::stable_analysis::lock_types::LockGuardTy::ParkingLotWrite(b)) => ty_equal(a, b),
                        // Incompatible parking_lot RwLock types
                        (crate::stable_analysis::lock_types::LockGuardTy::ParkingLotRead(a),
                         crate::stable_analysis::lock_types::LockGuardTy::ParkingLotWrite(b)) => ty_equal(a, b),
                        (crate::stable_analysis::lock_types::LockGuardTy::ParkingLotWrite(a),
                         crate::stable_analysis::lock_types::LockGuardTy::ParkingLotRead(b)) => ty_equal(a, b),
                        (crate::stable_analysis::lock_types::LockGuardTy::SpinRead(a),
                         crate::stable_analysis::lock_types::LockGuardTy::SpinRead(b)) => ty_equal(a, b),
                        (crate::stable_analysis::lock_types::LockGuardTy::SpinWrite(a),
                         crate::stable_analysis::lock_types::LockGuardTy::SpinWrite(b)) => ty_equal(a, b),
                        // Incompatible spin RwLock types
                        (crate::stable_analysis::lock_types::LockGuardTy::SpinRead(a),
                         crate::stable_analysis::lock_types::LockGuardTy::SpinWrite(b)) => ty_equal(a, b),
                        (crate::stable_analysis::lock_types::LockGuardTy::SpinWrite(a),
                         crate::stable_analysis::lock_types::LockGuardTy::SpinRead(b)) => ty_equal(a, b),
                        _ => false,
                    };

                    debug!("  [detect_double_lock] same_data check: {}", same_data);

                    if !same_data {
                        debug!("  [detect_double_lock] Different data types, skipping");
                        continue;  // Different data types
                    }

                    // Check if locks overlap
                    let overlaps = self.locks_overlap(info1, info2);
                    debug!("  [detect_double_lock] locks_overlap check: {}", overlaps);

                    if overlaps {
                        let possibility = info1.lockguard_ty.deadlock_with(&info2.lockguard_ty);

                        let first_lock = self.create_lock_desc(info1, id1);
                        let second_lock = self.create_lock_desc(info2, id2);

                        // Create callchain showing the deadlock
                        let callchain = vec![
                            format!("1. Lock {} in {}", first_lock.lock_type, first_lock.function),
                            format!("2. While holding, attempt to lock {} in {}",
                                    second_lock.lock_type, second_lock.function),
                            format!("3. Deadlock! {} and {} are in conflict",
                                    first_lock.lock_type, second_lock.lock_type),
                        ];

                        self.reports.push(LockReport {
                            kind: LockBugKind::DoubleLock,
                            possibility,
                            first_lock,
                            second_lock,
                            callchain,
                        });
                    }
                }
            }
        }
    }

    fn locks_overlap(
        &self,
        info1: &LockGuardInfo,
        info2: &LockGuardInfo,
    ) -> bool {
        // Check if any gen/kill pair from info1 overlaps with info2
        for gen1 in &info1.gen_locs {
            for kill1 in &info1.kill_locs {
                for gen2 in &info2.gen_locs {
                    // info1 alive at [gen1, kill1]
                    // info2 acquired at gen2
                    // Overlap if gen1 <= gen2 <= kill1
                    if gen1.block <= gen2.block && gen2.block <= kill1.block {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Create a LockDesc from LockGuardInfo
    fn create_lock_desc(&self, info: &LockGuardInfo, guard_id: &LockGuardId) -> LockDesc {
        LockDesc {
            lock_type: format!("{}", info.lockguard_ty),
            function: info.func_name.clone(),
            var_name: info.var_name.clone(),
            source_loc: info.source_loc.clone(),
            mir_loc: format!("local{}", guard_id.local),
        }
    }

    fn detect_conflicting_lock(&mut self) {
        // Find cycles in the lock graph
        let cycles = self.lock_graph.find_cycles();

        for cycle in cycles {
            if cycle.len() >= 2 {
                // Generate report for this cycle
                let possibility = DeadlockPossibility::Possibly;

                // Get first and last lock in cycle
                let first = &cycle[0];
                let last = &cycle[cycle.len() - 1];

                // For conflicting locks, create generic descriptions
                let first_lock = LockDesc {
                    lock_type: "Unknown".to_string(),
                    function: first.clone(),
                    var_name: None,
                    source_loc: None,
                    mir_loc: first.clone(),
                };

                let second_lock = LockDesc {
                    lock_type: "Unknown".to_string(),
                    function: last.clone(),
                    var_name: None,
                    source_loc: None,
                    mir_loc: last.clone(),
                };

                // Create callchain showing the cycle
                let callchain = vec![
                    format!("Lock cycle detected:"),
                    format!("  Thread 1: {} -> {}", first, cycle.get(1).unwrap_or(&first)),
                    format!("  Thread 2: {} -> {}", last, cycle.get(cycle.len()-2).unwrap_or(&last)),
                    format!("  These form a cycle causing potential deadlock"),
                ];

                self.reports.push(LockReport {
                    kind: LockBugKind::ConflictLock,
                    possibility,
                    first_lock,
                    second_lock,
                    callchain,
                });
            }
        }
    }
}

impl Default for LockDetector {
    fn default() -> Self {
        Self::new()
    }
}
