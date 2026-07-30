use crate::{OccurrenceId, ScheduleId, ScheduleStoreKind, types::DeliveryCompletionFailureReason};

#[derive(Debug, thiserror::Error)]
pub enum ScheduleStoreError {
    #[error("schedule store is unsupported for backend {backend}")]
    UnsupportedBackend { backend: ScheduleStoreKind },
    #[error("schedule store backend {backend} does not provide a durable push wake")]
    DurableWakeUnsupported { backend: ScheduleStoreKind },
    #[error("schedule not found: {schedule_id}")]
    ScheduleNotFound { schedule_id: ScheduleId },
    #[error("occurrence not found: {occurrence_id}")]
    OccurrenceNotFound { occurrence_id: OccurrenceId },
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("io error: {0}")]
    Io(String),
    /// A store mechanism failure for which retrying the exact same
    /// claim-fenced operation is safe. Semantic concurrency (a stale claim or
    /// failed write precondition) is reported separately and must never be
    /// retried as though it were transport contention.
    #[error("transient store error: {0}")]
    Transient(String),
    #[error("concurrency error: {0}")]
    Concurrency(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl ScheduleStoreError {
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Transient(_))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScheduleDomainError {
    #[error(transparent)]
    Store(#[from] ScheduleStoreError),
    #[error("invalid schedule: {0}")]
    InvalidSchedule(String),
    #[error("invalid trigger: {0}")]
    InvalidTrigger(String),
    #[error("invalid cron authoring: {0}")]
    InvalidCron(String),
    #[error("delivery failed for occurrence {occurrence_id}: {reason}")]
    DeliveryFailed {
        occurrence_id: OccurrenceId,
        reason: String,
    },
    #[error("delivery completion failed ({reason:?}): {detail}")]
    DeliveryCompletionFailed {
        reason: DeliveryCompletionFailureReason,
        detail: String,
    },
    /// The target's durable terminal could not be persisted inside the
    /// bounded live repair window. The driver must leave the durable
    /// occurrence nonterminal and stop renewing its lease so another process
    /// can reclaim and redrive the same stable delivery identity.
    #[error("delivery repair deferred to lease reclaim: {detail}")]
    DeliveryRepairDeferred { detail: String },
    #[error("target probe failed: {0}")]
    ProbeFailed(String),
    #[error("schedule driver stopped")]
    DriverStopped,
    #[error("internal error: {0}")]
    Internal(String),
}
