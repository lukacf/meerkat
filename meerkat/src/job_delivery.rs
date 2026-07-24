//! Mechanical projection from the job-owned outbox into the runtime-owned
//! durable delivery inbox.

use std::sync::Arc;

use meerkat_jobs::{
    DetachedJobError, DetachedJobService, DetachedJobStore, JobId, JobOutboxEntry,
    JobTerminalResult,
};
use meerkat_runtime::{
    LogicalRuntimeId, RuntimeDeliveryError, RuntimeDeliveryId, RuntimeDeliveryInbox,
    RuntimeDeliveryKind, RuntimeDeliverySubmission,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobTerminalDeliveryPayload {
    pub job_id: JobId,
    pub delivery_sequence: u64,
    pub terminal_result: JobTerminalResult,
}

#[derive(Debug, Clone)]
pub struct PreparedJobDelivery {
    pub runtime_id: LogicalRuntimeId,
    pub submission: RuntimeDeliverySubmission,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedJobDelivery {
    pub job_id: JobId,
    pub delivery_sequence: u64,
    pub runtime_sequence: u64,
    pub runtime_deduplicated: bool,
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum JobOutboxProjectionError {
    #[error(transparent)]
    Job(#[from] DetachedJobError),
    #[error(transparent)]
    Runtime(#[from] RuntimeDeliveryError),
    #[error("job outbox projection is corrupt: {0}")]
    Corrupt(String),
    #[error("failed to encode job terminal delivery: {0}")]
    Encode(String),
}

#[derive(Clone)]
pub struct JobOutboxProjector {
    job_store: Arc<dyn DetachedJobStore>,
    job_service: DetachedJobService,
    runtime_inbox: RuntimeDeliveryInbox,
}

impl std::fmt::Debug for JobOutboxProjector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobOutboxProjector").finish_non_exhaustive()
    }
}

impl JobOutboxProjector {
    pub fn new(job_store: Arc<dyn DetachedJobStore>, runtime_inbox: RuntimeDeliveryInbox) -> Self {
        Self {
            job_service: DetachedJobService::new(job_store.clone()),
            job_store,
            runtime_inbox,
        }
    }

    pub async fn prepare(
        &self,
        entry: &JobOutboxEntry,
    ) -> Result<PreparedJobDelivery, JobOutboxProjectionError> {
        let job = self.job_store.get(&entry.job_id).await?.ok_or_else(|| {
            JobOutboxProjectionError::Corrupt(format!(
                "outbox entry points to missing job {}",
                entry.job_id
            ))
        })?;
        let persisted = job
            .outbox
            .iter()
            .find(|candidate| candidate.delivery_sequence == entry.delivery_sequence)
            .ok_or_else(|| {
                JobOutboxProjectionError::Corrupt(format!(
                    "job {} no longer contains delivery {}",
                    entry.job_id, entry.delivery_sequence
                ))
            })?;
        if persisted != entry {
            return Err(JobOutboxProjectionError::Corrupt(format!(
                "job {} delivery {} disagrees with the pending outbox projection",
                entry.job_id, entry.delivery_sequence
            )));
        }
        if entry.applied {
            return Err(JobOutboxProjectionError::Corrupt(format!(
                "job {} delivery {} is already acknowledged",
                entry.job_id, entry.delivery_sequence
            )));
        }

        let payload = serde_json::to_vec(&JobTerminalDeliveryPayload {
            job_id: entry.job_id.clone(),
            delivery_sequence: entry.delivery_sequence,
            terminal_result: entry.terminal_result.clone(),
        })
        .map_err(|error| JobOutboxProjectionError::Encode(error.to_string()))?;
        let delivery_id = RuntimeDeliveryId::new(entry.job_id.as_str())?;
        let submission = RuntimeDeliverySubmission::new(
            delivery_id,
            RuntimeDeliveryKind::JobTerminal,
            entry.job_id.as_str(),
            entry.delivery_sequence,
            job.spec.interaction_lineage_id.as_str(),
            payload,
        )?;
        Ok(PreparedJobDelivery {
            runtime_id: LogicalRuntimeId::for_session(&job.spec.origin_session_id),
            submission,
        })
    }

    pub async fn project_pending(
        &self,
        limit: usize,
    ) -> Result<Vec<ProjectedJobDelivery>, JobOutboxProjectionError> {
        let entries = self.job_store.list_pending_outbox(limit).await?;
        let mut projected = Vec::with_capacity(entries.len());
        for entry in entries {
            let prepared = self.prepare(&entry).await?;
            let runtime = self
                .runtime_inbox
                .submit(&prepared.runtime_id, prepared.submission)
                .await?;
            self.job_service
                .mark_delivery_applied(&entry.job_id, entry.delivery_sequence)
                .await?;
            projected.push(ProjectedJobDelivery {
                job_id: entry.job_id,
                delivery_sequence: entry.delivery_sequence,
                runtime_sequence: runtime.sequence,
                runtime_deduplicated: runtime.deduplicated,
            });
        }
        Ok(projected)
    }
}
