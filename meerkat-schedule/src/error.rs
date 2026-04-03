use crate::{OccurrenceId, ScheduleId, ScheduleStoreKind};

#[derive(Debug, thiserror::Error)]
pub enum ScheduleStoreError {
    #[error("schedule store is unsupported for backend {backend}")]
    UnsupportedBackend { backend: ScheduleStoreKind },
    #[error("schedule not found: {schedule_id}")]
    ScheduleNotFound { schedule_id: ScheduleId },
    #[error("occurrence not found: {occurrence_id}")]
    OccurrenceNotFound { occurrence_id: OccurrenceId },
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("concurrency error: {0}")]
    Concurrency(String),
    #[error("internal error: {0}")]
    Internal(String),
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
    #[error("target probe failed: {0}")]
    ProbeFailed(String),
    #[error("schedule driver stopped")]
    DriverStopped,
    #[error("internal error: {0}")]
    Internal(String),
}
