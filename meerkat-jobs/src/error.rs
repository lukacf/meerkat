use crate::{AttemptId, FenceToken, JobId};

#[derive(Debug, thiserror::Error)]
pub enum DetachedJobError {
    #[error("invalid detached job input: {0}")]
    InvalidInput(String),
    #[error("detached job {0} was not found")]
    NotFound(JobId),
    #[error("detached job submission key already belongs to a different specification")]
    SubmissionConflict,
    #[error("detached job {job_id} revision changed: expected {expected}, actual {actual}")]
    StaleRevision {
        job_id: JobId,
        expected: u64,
        actual: u64,
    },
    #[error("detached job {job_id} rejected stale attempt writer {attempt_id} at fence {fence}")]
    StaleAttempt {
        job_id: JobId,
        attempt_id: AttemptId,
        fence: FenceToken,
    },
    #[error("detached job {job_id} transition was rejected: {detail}")]
    InvalidTransition { job_id: JobId, detail: String },
    #[error("detached job store failed: {0}")]
    Store(String),
}

impl DetachedJobError {
    pub const fn is_stale_attempt(&self) -> bool {
        matches!(self, Self::StaleAttempt { .. })
    }
}
