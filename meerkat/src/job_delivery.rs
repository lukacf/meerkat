//! Mechanical projection from the job-owned outbox into the runtime-owned
//! durable delivery inbox.

use std::sync::Arc;

use meerkat_jobs::{
    DetachedJobError, DetachedJobService, DetachedJobStore, InteractionLineageId, JobDeliveryKind,
    JobId, JobNotification, JobOutboxEntry, JobOutboxPayload, JobSubscription, JobTerminalResult,
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
    pub origin_session_id: meerkat_core::SessionId,
    pub interaction_lineage_id: InteractionLineageId,
    pub targets: Vec<JobSubscription>,
    pub terminal_result: JobTerminalResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobNotificationDeliveryPayload {
    pub job_id: JobId,
    pub delivery_sequence: u64,
    pub origin_session_id: meerkat_core::SessionId,
    pub interaction_lineage_id: InteractionLineageId,
    pub targets: Vec<JobSubscription>,
    pub notification: JobNotification,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobDeliveryContent {
    Notification(JobNotification),
    Terminal(JobTerminalResult),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobDeliveryApplication {
    Record {
        job_id: JobId,
        delivery_sequence: u64,
        subscription: JobSubscription,
        content: JobDeliveryContent,
    },
    Notification {
        job_id: JobId,
        delivery_sequence: u64,
        subscription: JobSubscription,
        content: JobDeliveryContent,
    },
    Event {
        job_id: JobId,
        delivery_sequence: u64,
        subscription: JobSubscription,
        interaction_lineage_id: InteractionLineageId,
        handling_mode: meerkat_core::HandlingMode,
        content: JobDeliveryContent,
    },
}

#[async_trait::async_trait]
pub trait JobDeliverySink: Send + Sync {
    /// Apply one stable subscription delivery idempotently.
    ///
    /// `Notification` is a turn-free user-visible append. Only `Event` may
    /// request ordinary runtime work/provider inference.
    async fn apply(&self, application: JobDeliveryApplication) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedRuntimeJobDelivery {
    pub delivery_id: RuntimeDeliveryId,
    pub runtime_sequence: u64,
    pub applications: usize,
}

#[derive(Clone)]
pub struct JobRuntimeDeliveryApplier {
    runtime_inbox: RuntimeDeliveryInbox,
    sink: Arc<dyn JobDeliverySink>,
}

impl std::fmt::Debug for JobRuntimeDeliveryApplier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobRuntimeDeliveryApplier")
            .finish_non_exhaustive()
    }
}

impl JobRuntimeDeliveryApplier {
    pub fn new(runtime_inbox: RuntimeDeliveryInbox, sink: Arc<dyn JobDeliverySink>) -> Self {
        Self {
            runtime_inbox,
            sink,
        }
    }

    pub async fn apply_pending(
        &self,
        runtime_id: &LogicalRuntimeId,
        limit: usize,
    ) -> Result<Vec<AppliedRuntimeJobDelivery>, JobOutboxProjectionError> {
        let pending = self.runtime_inbox.list_pending(runtime_id, limit).await?;
        let mut applied = Vec::with_capacity(pending.len());
        for record in pending {
            let applications = match record.submission.kind() {
                RuntimeDeliveryKind::JobNotification => {
                    let payload: JobNotificationDeliveryPayload =
                        serde_json::from_slice(record.submission.payload()).map_err(|error| {
                            JobOutboxProjectionError::Corrupt(format!(
                                "notification delivery {} payload is invalid: {error}",
                                record.submission.delivery_id()
                            ))
                        })?;
                    apply_subscriptions(
                        &*self.sink,
                        payload.job_id,
                        payload.delivery_sequence,
                        payload.origin_session_id,
                        payload.interaction_lineage_id,
                        payload.targets,
                        JobDeliveryContent::Notification(payload.notification),
                    )
                    .await?
                }
                RuntimeDeliveryKind::JobTerminal => {
                    let payload: JobTerminalDeliveryPayload =
                        serde_json::from_slice(record.submission.payload()).map_err(|error| {
                            JobOutboxProjectionError::Corrupt(format!(
                                "terminal delivery {} payload is invalid: {error}",
                                record.submission.delivery_id()
                            ))
                        })?;
                    apply_subscriptions(
                        &*self.sink,
                        payload.job_id,
                        payload.delivery_sequence,
                        payload.origin_session_id,
                        payload.interaction_lineage_id,
                        payload.targets,
                        JobDeliveryContent::Terminal(payload.terminal_result),
                    )
                    .await?
                }
                _ => {
                    return Err(JobOutboxProjectionError::Corrupt(format!(
                        "delivery {} has an unsupported runtime kind",
                        record.submission.delivery_id()
                    )));
                }
            };
            self.runtime_inbox
                .mark_applied(runtime_id, record.submission.delivery_id(), record.sequence)
                .await?;
            applied.push(AppliedRuntimeJobDelivery {
                delivery_id: record.submission.delivery_id().clone(),
                runtime_sequence: record.sequence,
                applications,
            });
        }
        Ok(applied)
    }
}

async fn apply_subscriptions(
    sink: &dyn JobDeliverySink,
    job_id: JobId,
    delivery_sequence: u64,
    origin_session_id: meerkat_core::SessionId,
    interaction_lineage_id: InteractionLineageId,
    mut targets: Vec<JobSubscription>,
    content: JobDeliveryContent,
) -> Result<usize, JobOutboxProjectionError> {
    if targets.is_empty() {
        targets.push(JobSubscription::new(
            meerkat_jobs::JobSubscriptionId::new("origin")
                .map_err(JobOutboxProjectionError::Job)?,
            origin_session_id,
            JobDeliveryKind::Notification,
        ));
    }
    let applications = targets.len();
    for subscription in targets {
        let delivery = subscription.delivery().clone();
        let application = match delivery {
            JobDeliveryKind::Record => JobDeliveryApplication::Record {
                job_id: job_id.clone(),
                delivery_sequence,
                subscription,
                content: content.clone(),
            },
            JobDeliveryKind::Notification => JobDeliveryApplication::Notification {
                job_id: job_id.clone(),
                delivery_sequence,
                subscription,
                content: content.clone(),
            },
            JobDeliveryKind::Event { handling_mode } => JobDeliveryApplication::Event {
                job_id: job_id.clone(),
                delivery_sequence,
                subscription,
                interaction_lineage_id: interaction_lineage_id.clone(),
                handling_mode,
                content: content.clone(),
            },
        };
        sink.apply(application)
            .await
            .map_err(JobOutboxProjectionError::Apply)?;
    }
    Ok(applications)
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
    #[error("failed to encode job delivery: {0}")]
    Encode(String),
    #[error("failed to apply job delivery: {0}")]
    Apply(String),
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

        let (kind, payload) = match &entry.payload {
            JobOutboxPayload::Terminal(terminal_result) => (
                RuntimeDeliveryKind::JobTerminal,
                serde_json::to_vec(&JobTerminalDeliveryPayload {
                    job_id: entry.job_id.clone(),
                    delivery_sequence: entry.delivery_sequence,
                    origin_session_id: job.spec.origin_session_id.clone(),
                    interaction_lineage_id: job.spec.interaction_lineage_id.clone(),
                    targets: entry.targets.clone(),
                    terminal_result: terminal_result.clone(),
                }),
            ),
            JobOutboxPayload::Notification(notification) => (
                RuntimeDeliveryKind::JobNotification,
                serde_json::to_vec(&JobNotificationDeliveryPayload {
                    job_id: entry.job_id.clone(),
                    delivery_sequence: entry.delivery_sequence,
                    origin_session_id: job.spec.origin_session_id.clone(),
                    interaction_lineage_id: job.spec.interaction_lineage_id.clone(),
                    targets: entry.targets.clone(),
                    notification: notification.clone(),
                }),
            ),
        };
        let payload =
            payload.map_err(|error| JobOutboxProjectionError::Encode(error.to_string()))?;
        let delivery_id = RuntimeDeliveryId::new(entry.runtime_delivery_id())?;
        let submission = RuntimeDeliverySubmission::new(
            delivery_id,
            kind,
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

#[async_trait::async_trait]
impl meerkat_tools::builtin::shell::ShellJobDeliveryProjector for JobOutboxProjector {
    async fn project_job(&self, job_id: &str) -> Result<(), String> {
        let job_id = JobId::new(job_id).map_err(|error| error.to_string())?;
        loop {
            let Some(job) = self
                .job_store
                .get(&job_id)
                .await
                .map_err(|error| error.to_string())?
            else {
                return Err(format!("cannot project missing job {job_id}"));
            };
            let Some(entry) = job.outbox.iter().find(|entry| !entry.applied).cloned() else {
                return Ok(());
            };
            let prepared = self
                .prepare(&entry)
                .await
                .map_err(|error| error.to_string())?;
            self.runtime_inbox
                .submit(&prepared.runtime_id, prepared.submission)
                .await
                .map_err(|error| error.to_string())?;
            if let Err(error) = self
                .job_service
                .mark_delivery_applied(&entry.job_id, entry.delivery_sequence)
                .await
            {
                let applied_by_racer = self
                    .job_store
                    .get(&job_id)
                    .await
                    .map_err(|reload_error| reload_error.to_string())?
                    .is_some_and(|current| {
                        current.outbox.iter().any(|candidate| {
                            candidate.delivery_sequence == entry.delivery_sequence
                                && candidate.applied
                        })
                    });
                if !applied_by_racer {
                    return Err(error.to_string());
                }
            }
        }
    }

    async fn acknowledge_applied(&self, job_id: &str) -> Result<(), String> {
        let job_id = JobId::new(job_id).map_err(|error| error.to_string())?;
        let Some(job) = self
            .job_store
            .get(&job_id)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Err(format!("cannot acknowledge missing job {job_id}"));
        };
        let runtime_id = LogicalRuntimeId::for_session(&job.spec.origin_session_id);
        let pending = self
            .runtime_inbox
            .list_pending(&runtime_id, usize::MAX)
            .await
            .map_err(|error| error.to_string())?;
        let Some(record) = pending
            .into_iter()
            .find(|record| record.submission.delivery_id().as_str() == job_id.as_str())
        else {
            return Ok(());
        };
        self.runtime_inbox
            .mark_applied(
                &runtime_id,
                record.submission.delivery_id(),
                record.sequence,
            )
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}
