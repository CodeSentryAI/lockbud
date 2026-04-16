use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "bug_kind", content = "diagnosis")]
pub enum Report {
    DoubleLock(ReportContent<DeadlockDiagnosis>),
    ConflictLock(ReportContent<Vec<DeadlockDiagnosis>>),
    CondvarDeadlock(ReportContent<CondvarDeadlockDiagnosis>),
    AtomicityViolation(ReportContent<AtomicityViolationDiagnosis>),
    InvalidFree(ReportContent<InvalidFreeDiagnosis>),
    UseAfterFree(ReportContent<UseAfterFreeDiagnosis>),
    Panic(ReportContent<PanicDiagnosis>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportContent<T> {
    pub bug_kind: String,
    pub possibility: String,
    pub diagnosis: T,
    pub explanation: String,
}

impl<T> ReportContent<T> {
    pub fn new(bug_kind: String, possibility: String, diagnosis: T, explanation: String) -> Self {
        Self {
            bug_kind,
            possibility,
            diagnosis,
            explanation,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadlockDiagnosis {
    pub first_lock_type: String,
    pub first_lock_span: String,
    pub second_lock_type: String,
    pub second_lock_span: String,
    pub callchains: Vec<Vec<Vec<String>>>,
}

impl DeadlockDiagnosis {
    pub fn new(
        first_lock_type: String,
        first_lock_span: String,
        second_lock_type: String,
        second_lock_span: String,
        callchains: Vec<Vec<Vec<String>>>,
    ) -> Self {
        Self {
            first_lock_type,
            first_lock_span,
            second_lock_type,
            second_lock_span,
            callchains,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaitNotifyLocks {
    pub wait_lock_type: String,
    pub wait_lock_span: String,
    pub notify_lock_type: String,
    pub notify_lock_span: String,
}

impl WaitNotifyLocks {
    pub fn new(
        wait_lock_type: String,
        wait_lock_span: String,
        notify_lock_type: String,
        notify_lock_span: String,
    ) -> Self {
        Self {
            wait_lock_type,
            wait_lock_span,
            notify_lock_type,
            notify_lock_span,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CondvarDeadlockDiagnosis {
    pub condvar_wait_type: String,
    pub condvar_wait_callsite_span: String,
    pub condvar_notify_type: String,
    pub condvar_notify_callsite_span: String,
    pub deadlocks: Vec<WaitNotifyLocks>,
}

impl CondvarDeadlockDiagnosis {
    pub fn new(
        condvar_wait_type: String,
        condvar_wait_callsite_span: String,
        condvar_notify_type: String,
        condvar_notify_callsite_span: String,
        deadlocks: Vec<WaitNotifyLocks>,
    ) -> Self {
        Self {
            condvar_wait_type,
            condvar_wait_callsite_span,
            condvar_notify_type,
            condvar_notify_callsite_span,
            deadlocks,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtomicityViolationDiagnosis {
    pub fn_name: String,
    pub atomic_reader: String,
    pub atomic_writer: String,
    pub dep_kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvalidFreeDiagnosis {
    pub ty: String,
    pub uninit_span: String,
    pub assume_init_span: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UseAfterFreeDiagnosis {
    pub raw_ptr_local: usize,
    pub use_span: String,
    pub drop_span: String,
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanicDiagnosis {
    pub fn_name: String,
    pub panic_api: String,
    pub callsite_span: String,
}
