//! Lockbud detector ported to analyze ULLBC directly via charon-lib.

pub mod analysis;
pub mod callgraph;
pub mod collector;
pub mod condvar;
pub mod detector;
pub mod report;
pub mod types;

use charon_lib::export::CrateData;

/// Entry point to run lockbud-style analysis on a `CrateData`.
pub fn run(crate_data: &CrateData, report_path: Option<&std::path::Path>) {
    log::info!("Running Lockbud-ULLBC analysis...");

    // 1. Build callgraph.
    let callgraph = callgraph::CallGraph::build(crate_data);

    // 2. Collect lockguards.
    let mut collector = collector::LockGuardCollector::new(crate_data);
    collector.collect();
    let lockguards = collector.lockguards;

    log::info!("Collected {} lockguards", lockguards.len());
    for (id, info) in lockguards.iter() {
        log::info!("  guard {:?}: ty={:?} gen_locs={} kill_locs={} recv={:?}", id, info.lockguard_ty, info.gen_locs.len(), info.kill_locs.len(), info.receiver_place);
    }

    // 3. Collect condvar API callsites.
    let condvar_callsites = condvar::collect_condvar_callsites(crate_data);
    log::info!("Collected {} condvar callsites", condvar_callsites.len());

    // 4. Run intra-/inter-procedural analysis to establish relations.
    let mut analyzer = analysis::Analyzer::new(crate_data, &callgraph, &lockguards)
        .with_condvar_callsites(&condvar_callsites);
    analyzer.analyze();

    log::info!(
        "Collected {} lockguard relations",
        analyzer.relations.len()
    );
    for (a, b) in &analyzer.relations {
        log::info!("  relation {:?} -> {:?}", a, b);
    }

    // 5. Detect deadlocks.
    let detector = detector::DeadlockDetector::new(crate_data, &callgraph, &lockguards, &analyzer, &condvar_callsites);
    let reports = detector.detect();

    if !reports.is_empty() {
        let j = serde_json::to_string_pretty(&reports).unwrap();
        let out_path = report_path
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp/lockbud_ullbc_reports.json"));
        if let Err(e) = std::fs::write(&out_path, &j) {
            log::warn!("Failed to write reports to {:?}: {}", out_path, e);
        } else {
            log::warn!("Lockbud reports written to {:?}", out_path);
        }
    }

    let doublelock = reports
        .iter()
        .filter(|r| matches!(r, report::Report::DoubleLock(_)))
        .count();
    let conflictlock = reports
        .iter()
        .filter(|r| matches!(r, report::Report::ConflictLock(_)))
        .count();
    let condvar_deadlock = reports
        .iter()
        .filter(|r| matches!(r, report::Report::CondvarDeadlock(_)))
        .count();

    log::info!(
        "Detection complete: {} doublelock(s), {} conflictlock(s), {} condvar_deadlock(s)",
        doublelock,
        conflictlock,
        condvar_deadlock
    );
}
