//! Fixed commit observations only. No authority, waits, tasks or payload logging.
use super::PrivateOriginalError;
use meerkat::governed_realm::helper_admission::HelperAdmissionRefusal;
use meerkat_workgraph::WorkGraphError;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

#[derive(Clone, Copy)]
#[repr(u8)]
pub(super) enum WorkerStage {
    NotStarted,
    ValidateCall,
    Runtime,
    AdmissionIdentity,
    DispatchFence,
    Spawn,
    Storage,
    ReturnedItem,
    ValidateItem,
    Completed,
}

const STAGES: [&str; 10] = [
    "not_started",
    "validate_call",
    "runtime",
    "admission_identity",
    "dispatch_fence",
    "spawn",
    "storage",
    "returned_item",
    "validate_item",
    "completed",
];
const GRAPH_ERRORS: [&str; 16] = [
    "unobserved",
    "none",
    "NotFound",
    "AttentionNotFound",
    "StaleRevision",
    "Conflict",
    "InvalidTransition",
    "InvalidInput",
    "InvalidTimestampMillis",
    "Store",
    "NamespaceAssignmentRequired",
    "UnsupportedBackend",
    "AttentionTargetRealmMismatch",
    "BackingStoreUnavailable",
    "UnpairedAdmissionIdentity",
    "SchemaMismatch",
];

pub(crate) struct CommitDiagnostic {
    stage: AtomicU8,
    graph_error: AtomicU8,
    fence_refusal: AtomicU8,
    callback_entered: AtomicBool,
    worker_result: AtomicU8,
    joined: AtomicBool,
    cached_join_observed: AtomicBool,
    join_failed: AtomicBool,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct CommitObservation {
    pub(super) worker_stage: &'static str,
    pub(super) graph_error: &'static str,
    pub(super) fence_refusal: &'static str,
    pub(super) callback_entered: bool,
    pub(super) worker_result: &'static str,
    pub(super) joined: bool,
    pub(super) cached_join_observed: bool,
    pub(super) join_failed: bool,
}

impl CommitDiagnostic {
    pub(super) fn new() -> Self {
        Self {
            stage: AtomicU8::new(WorkerStage::NotStarted as u8),
            graph_error: AtomicU8::new(0),
            fence_refusal: AtomicU8::new(0),
            callback_entered: AtomicBool::new(false),
            worker_result: AtomicU8::new(0),
            joined: AtomicBool::new(false),
            cached_join_observed: AtomicBool::new(false),
            join_failed: AtomicBool::new(false),
        }
    }
    // These atomics provide observations, never synchronization or decisions.
    // The existing task join is still the evidence that its worker settled.
    pub(super) fn enter(&self, stage: WorkerStage) {
        self.stage.store(stage as u8, Ordering::Relaxed);
    }
    pub(super) fn graph_result<T>(&self, result: &Result<T, WorkGraphError>) {
        let code = match result {
            Ok(_) => 1,
            Err(error) => match error {
                WorkGraphError::NotFound { .. } => 2,
                WorkGraphError::AttentionNotFound { .. } => 3,
                WorkGraphError::StaleRevision { .. } => 4,
                WorkGraphError::Conflict(_) => 5,
                WorkGraphError::InvalidTransition(_) => 6,
                WorkGraphError::InvalidInput(_) => 7,
                WorkGraphError::InvalidTimestampMillis { .. } => 8,
                WorkGraphError::Store(_) => 9,
                WorkGraphError::NamespaceAssignmentRequired { .. } => 10,
                WorkGraphError::UnsupportedBackend(_) => 11,
                WorkGraphError::AttentionTargetRealmMismatch { .. } => 12,
                WorkGraphError::BackingStoreUnavailable { .. } => 13,
                WorkGraphError::UnpairedAdmissionIdentity { .. } => 14,
                WorkGraphError::SchemaMismatch { .. } => 15,
            },
        };
        self.graph_error.store(code, Ordering::Relaxed);
    }
    pub(super) fn refusal(&self, refusal: Option<HelperAdmissionRefusal>) {
        self.fence_refusal.store(
            match refusal {
                None => 1,
                Some(HelperAdmissionRefusal::Denied) => 2,
                Some(HelperAdmissionRefusal::Unavailable) => 3,
            },
            Ordering::Relaxed,
        );
    }
    pub(super) fn callback_entered(&self) {
        self.callback_entered.store(true, Ordering::Relaxed);
    }
    pub(super) fn returned<T>(&self, result: &Result<T, PrivateOriginalError>) {
        self.worker_result.store(
            match result {
                Ok(_) => 1,
                Err(PrivateOriginalError::Denied) => 2,
                Err(PrivateOriginalError::Conflict) => 3,
                Err(PrivateOriginalError::Unavailable) => 4,
            },
            Ordering::Relaxed,
        );
    }
    pub(super) fn joined(&self) {
        self.joined.store(true, Ordering::Relaxed);
    }
    pub(super) fn cached_join(&self) {
        self.cached_join_observed.store(true, Ordering::Relaxed);
    }
    pub(super) fn join_failed(&self) {
        self.join_failed.store(true, Ordering::Relaxed);
    }
    pub(super) fn observation(&self) -> CommitObservation {
        CommitObservation {
            worker_stage: STAGES
                .get(self.stage.load(Ordering::Relaxed) as usize)
                .copied()
                .unwrap_or("unknown"),
            graph_error: GRAPH_ERRORS
                .get(self.graph_error.load(Ordering::Relaxed) as usize)
                .copied()
                .unwrap_or("unknown"),
            fence_refusal: ["unobserved", "none", "Denied", "Unavailable"]
                .get(self.fence_refusal.load(Ordering::Relaxed) as usize)
                .copied()
                .unwrap_or("unknown"),
            callback_entered: self.callback_entered.load(Ordering::Relaxed),
            worker_result: ["unobserved", "ok", "Denied", "Conflict", "Unavailable"]
                .get(self.worker_result.load(Ordering::Relaxed) as usize)
                .copied()
                .unwrap_or("unknown"),
            joined: self.joined.load(Ordering::Relaxed),
            cached_join_observed: self.cached_join_observed.load(Ordering::Relaxed),
            join_failed: self.join_failed.load(Ordering::Relaxed),
        }
    }
    pub(super) fn report(
        &self,
        operation: &str,
        stage: &'static str,
        origin: &'static str,
        reused: bool,
        error: PrivateOriginalError,
    ) {
        use std::io::Write;
        // Called after retain_io and its nested guards have returned. Invalid
        // operation shapes remain unobserved, with no change to the result.
        let operation = if operation.len() == 70
            && operation.starts_with("setup-")
            && operation.as_bytes()[6..]
                .iter()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
        {
            operation
        } else {
            "unobserved"
        };
        let error = match error {
            PrivateOriginalError::Denied => "Denied",
            PrivateOriginalError::Conflict => "Conflict",
            PrivateOriginalError::Unavailable => "Unavailable",
        };
        let view = self.observation();
        let _ = writeln!(
            std::io::stderr().lock(),
            "AA1 workflow_commit_returned: operation={} stage={} origin={} reused={} error={} worker_stage={} graph_error={} fence_refusal={} callback_entered={} worker_result={} joined={} cached_join_observed={} join_failed={}",
            operation,
            stage,
            origin,
            reused,
            error,
            view.worker_stage,
            view.graph_error,
            view.fence_refusal,
            view.callback_entered,
            view.worker_result,
            view.joined,
            view.cached_join_observed,
            view.join_failed
        );
    }
}
