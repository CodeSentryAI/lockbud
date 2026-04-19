use std::path::PathBuf;

/// Detector kind selectors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetectorKind {
    Deadlock,
    AtomicityViolation,
    Memory,
    Panic,
    Channel,
}

impl DetectorKind {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "deadlock" => Some(DetectorKind::Deadlock),
            "atomicity_violation" | "atomicity" => Some(DetectorKind::AtomicityViolation),
            "memory" => Some(DetectorKind::Memory),
            "panic" => Some(DetectorKind::Panic),
            "channel" => Some(DetectorKind::Channel),
            _ => None,
        }
    }
}

/// Configuration for lockbud analysis.
pub struct LockbudConfig {
    /// Which detectors to run. Empty means run all.
    pub kinds: Vec<DetectorKind>,
    /// Whitelist of crate names to analyze.
    pub crate_whitelist: Option<Vec<String>>,
    /// Blacklist of crate names to skip.
    pub crate_blacklist: Option<Vec<String>>,
    /// Output path for JSON reports.
    pub report_file: Option<PathBuf>,
}

impl LockbudConfig {
    pub fn new() -> Self {
        Self {
            kinds: Vec::new(),
            crate_whitelist: None,
            crate_blacklist: None,
            report_file: None,
        }
    }

    /// Check if a detector kind is enabled.
    pub fn is_kind_enabled(&self, kind: DetectorKind) -> bool {
        self.kinds.is_empty() || self.kinds.contains(&kind)
    }

    /// Check if a crate should be analyzed based on whitelist/blacklist.
    pub fn is_crate_allowed(&self, crate_name: &str) -> bool {
        if let Some(ref blacklist) = self.crate_blacklist {
            if blacklist.iter().any(|s| s == crate_name) {
                return false;
            }
        }
        if let Some(ref whitelist) = self.crate_whitelist {
            return whitelist.iter().any(|s| s == crate_name);
        }
        // Default: no filtering applied at this level (collectors handle is_local).
        true
    }
}

impl Default for LockbudConfig {
    fn default() -> Self {
        Self::new()
    }
}
