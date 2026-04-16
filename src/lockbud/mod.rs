//! Lockbud detector ported to analyze ULLBC directly via charon-lib.

pub mod analysis;
pub mod atomic;
pub mod callgraph;
pub mod channel;
pub mod collector;
pub mod condvar;
pub mod detector;
pub mod memory;
pub mod panic;
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
        log::info!("  guard {:?}: ty={:?} gen_locs={} kill_locs={} recv={:?} alias_of={:?}", id, info.lockguard_ty, info.gen_locs.len(), info.kill_locs.len(), info.receiver_place, info.alias_of);
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
    let deadlock_detector = detector::DeadlockDetector::new(crate_data, &callgraph, &lockguards, &analyzer, &condvar_callsites);
    let mut reports = deadlock_detector.detect();

    // 6. Detect atomicity violations.
    let atomic_callsites = atomic::collect_atomic_callsites(crate_data);
    log::info!("Collected {} atomic callsites", atomic_callsites.len());
    reports.extend(atomic::detect_atomicity_violations(crate_data, &atomic_callsites, &callgraph));

    // 7. Detect invalid free.
    let uninit_callsites = memory::collect_uninit_callsites(crate_data);
    log::info!("Collected {} uninit callsites", uninit_callsites.len());
    reports.extend(memory::detect_invalid_free(crate_data, &uninit_callsites));

    // 8. Detect use after free.
    reports.extend(memory::detect_use_after_free(crate_data));

    // 9. Detect panics.
    reports.extend(panic::detect_panics(crate_data));

    // 10. Detect channel deadlocks and orphan senders.
    let channel_callsites = channel::collect_channel_callsites(crate_data);
    log::info!("Collected {} channel callsites", channel_callsites.len());
    let (local_to_endpoint, channels, lifetimes) = channel::build_endpoint_map(crate_data, &channel_callsites);
    reports.extend(channel::detect_channel_deadlocks(crate_data, &channel_callsites));
    reports.extend(channel::detect_orphan_senders(crate_data, &channel_callsites, &local_to_endpoint, &channels, &lifetimes));
    // MissingSend detection requires inter-procedural closure capture analysis; deferred to PR 2.

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
    let atomicity = reports
        .iter()
        .filter(|r| matches!(r, report::Report::AtomicityViolation(_)))
        .count();
    let invalid_free = reports
        .iter()
        .filter(|r| matches!(r, report::Report::InvalidFree(_)))
        .count();
    let use_after_free = reports
        .iter()
        .filter(|r| matches!(r, report::Report::UseAfterFree(_)))
        .count();
    let panic_count = reports
        .iter()
        .filter(|r| matches!(r, report::Report::Panic(_)))
        .count();
    let channel_deadlock = reports
        .iter()
        .filter(|r| matches!(r, report::Report::ChannelDeadlock(_)))
        .count();
    let orphan_sender = reports
        .iter()
        .filter(|r| matches!(r, report::Report::OrphanSender(_)))
        .count();
    let missing_send = reports
        .iter()
        .filter(|r| matches!(r, report::Report::MissingSend(_)))
        .count();

    log::info!(
        "Detection complete: {} doublelock(s), {} conflictlock(s), {} condvar_deadlock(s), {} atomicity(s), {} invalid_free(s), {} use_after_free(s), {} panic(s), {} channel_deadlock(s), {} orphan_sender(s), {} missing_send(s)",
        doublelock,
        conflictlock,
        condvar_deadlock,
        atomicity,
        invalid_free,
        use_after_free,
        panic_count,
        channel_deadlock,
        orphan_sender,
        missing_send
    );
}
