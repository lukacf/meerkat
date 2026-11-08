//! Failure-only observations retained by this exact admission call.
//! No authority, retry, timer, task or additional custody is created here.

use std::sync::{
    OnceLock,
    atomic::{AtomicU8, Ordering},
};

#[cfg_attr(not(all(feature = "native-host", unix)), allow(dead_code))]
#[derive(Clone, Copy)]
#[repr(u8)]
pub(crate) enum AdmissionStage {
    NotStarted,
    HostAdmission,
    StoreAdmission,
    HostReadback,
    StoreRecovery,
    Advance,
    AdvanceHost,
    AdvanceLoad,
    AdvanceOriginal,
    WaiterJoin,
    RetainInterest,
    PrepareEffects,
    DepartAfterFailure,
    CommitTransfer,
    Launch,
    ContextDelivery,
    PrepareWork,
    WorkStep,
    ObserveHelper,
    PrepareHelper,
    RecoverHelper,
    HostAfterPublication,
    LoadAfterPublication,
    LoadPublication,
    SubmitHelper,
}

const LABELS: [&str; 25] = [
    "not_started",
    "leaf.host_admit",
    "leaf.admit_or_recover",
    "leaf.host_readback",
    "leaf.admit_or_recover_readback",
    "leaf.advance",
    "advance.host",
    "advance.load",
    "advance.workflow_original",
    "advance.workflow_waiter_join",
    "advance.workflow_retain_interest",
    "advance.workflow_prepare_effects",
    "advance.workflow_depart_after_failure",
    "advance.workflow_commit",
    "advance.workflow_launch",
    "advance.workflow_context_delivery",
    "advance.prepare_work",
    "advance.work_step",
    "advance.observe_helper",
    "advance.prepare_helper",
    "advance.recover_helper",
    "advance.host_after_publication",
    "advance.load_after_publication",
    "advance.load_publication",
    "advance.submit_helper",
];

pub(crate) struct AdmissionProgress {
    stage: AtomicU8,
    first_refusal: AtomicU8,
    cleanup: AtomicU8,
    operation: OnceLock<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AdmissionObservation<'a> {
    pub(crate) operation: Option<&'a str>,
    pub(crate) first_refusal: Option<&'static str>,
    pub(crate) last_stage: &'static str,
    pub(crate) cleanup: &'static str,
}

impl AdmissionProgress {
    pub(crate) fn new() -> Self {
        Self {
            stage: AtomicU8::new(AdmissionStage::NotStarted as u8),
            first_refusal: AtomicU8::new(u8::MAX),
            cleanup: AtomicU8::new(0),
            operation: OnceLock::new(),
        }
    }

    // Only the owning admission future writes this observation. Relaxed
    // ordering carries no synchronization or admission meaning. In particular,
    // the marker does not log while an inner future may hold an owner lock.
    pub(crate) fn enter(&self, stage: AdmissionStage) {
        self.stage.store(stage as u8, Ordering::Relaxed);
    }

    // SetupStore already constructs this correlation as setup-<sha256>. Do not
    // log an arbitrary ID, request, native association or error payload. Missing
    // or unexpected shapes remain unobserved, without changing admission.
    pub(crate) fn bind_operation(&self, operation: &str) {
        if operation.len() == 70
            && operation.starts_with("setup-")
            && operation.as_bytes()[6..]
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        {
            let _ = self.operation.set(operation.to_owned());
        }
    }

    pub(crate) fn refused(&self) {
        let stage = self.stage.load(Ordering::Relaxed);
        let _ = self.first_refusal.compare_exchange(
            u8::MAX,
            stage,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }

    #[cfg(all(feature = "native-host", unix))]
    pub(crate) fn cleanup_started(&self) {
        self.cleanup.store(1, Ordering::Relaxed);
    }

    #[cfg(all(feature = "native-host", unix))]
    pub(crate) fn cleanup_completed(&self) {
        self.cleanup.store(2, Ordering::Relaxed);
    }

    #[cfg(all(feature = "native-host", unix))]
    pub(crate) fn cleanup_failed(&self) {
        self.cleanup.store(3, Ordering::Relaxed);
    }

    pub(crate) fn observation(&self) -> AdmissionObservation<'_> {
        let stage = usize::from(self.stage.load(Ordering::Relaxed));
        let first = self.first_refusal.load(Ordering::Relaxed);
        AdmissionObservation {
            operation: self.operation.get().map(String::as_str),
            first_refusal: (first != u8::MAX)
                .then(|| LABELS.get(usize::from(first)).copied().unwrap_or("unknown")),
            last_stage: LABELS.get(stage).copied().unwrap_or("unknown"),
            cleanup: match self.cleanup.load(Ordering::Relaxed) {
                0 => "not_entered",
                1 => "entered_unresolved",
                2 => "completed",
                3 => "failed",
                _ => "unknown",
            },
        }
    }

    pub(crate) fn report_returned(&self, error: &meerkat_core::ToolError) {
        use std::io::Write;
        // Called only after the nested admission future returns and its local
        // guards unwind. The stage is a boundary observation, not an inner cause
        // or the identity of a competing gate holder.
        let observed = self.observation();
        let variant = match error {
            meerkat_core::ToolError::Unavailable { .. } => "ToolError::Unavailable",
            meerkat_core::ToolError::AccessDenied { .. } => "ToolError::AccessDenied",
            meerkat_core::ToolError::InvalidArguments { .. } => "ToolError::InvalidArguments",
            _ => "ToolError::Other",
        };
        let _ = writeln!(
            std::io::stderr().lock(),
            "AA1 admission_returned: operation={} first_refusal={} last_stage={} cleanup={} mapped_error={}",
            observed.operation.unwrap_or("unobserved"),
            observed.first_refusal.unwrap_or("unobserved"),
            observed.last_stage,
            observed.cleanup,
            variant
        );
    }

    pub(crate) fn report_timeout(&self) {
        // Report only after the existing outer timeout returns. This names the
        // last entered await, not the inner task or the cause of its wait.
        let stage = usize::from(self.stage.load(Ordering::Relaxed));
        crate::aa1_failure(
            "leaf.timeout_wait",
            LABELS.get(stage).copied().unwrap_or("unknown"),
        );
    }
}
