//! Mechanical projection from the job-owned outbox into the runtime-owned
//! durable delivery inbox.

use std::sync::Arc;

use meerkat_jobs::{
    DetachedJobError, DetachedJobService, DetachedJobStore, InteractionLineageId, JobDeliveryKind,
    JobId, JobNotification, JobOutboxEntry, JobOutboxPayload, JobSubscription, JobTerminalResult,
};
use meerkat_runtime::{
    LogicalRuntimeId, RuntimeDeliveryError, RuntimeDeliveryId, RuntimeDeliveryInbox,
    RuntimeDeliveryKind, RuntimeDeliveryRecord, RuntimeDeliverySubmission,
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

/// A pending delivery whose application failed during a drain pass.
///
/// The row is never discarded: it stays pending in the runtime inbox and is
/// retried on the next pass. Because the inbox is an ordered cursor machine,
/// later rows for the same runtime stay queued behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedRuntimeJobDelivery {
    pub delivery_id: RuntimeDeliveryId,
    pub runtime_sequence: u64,
    pub error: String,
}

/// Outcome of one ordered drain pass over a runtime's pending deliveries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeJobDeliveryDrain {
    /// Deliveries applied and acknowledged this pass, in inbox order.
    pub applied: Vec<AppliedRuntimeJobDelivery>,
    /// The first delivery whose application failed, if any. Progress made
    /// before it is retained in `applied`.
    pub blocked: Option<BlockedRuntimeJobDelivery>,
}

impl RuntimeJobDeliveryDrain {
    pub fn is_fully_drained(&self) -> bool {
        self.blocked.is_none()
    }
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

    /// Apply pending deliveries in inbox order, failing SAFE per runtime.
    ///
    /// `Err` means the inbox itself could not be read. Every failure after
    /// that — a corrupt payload, an unsupported kind, a sink rejection, a
    /// failed acknowledgement — is reported as `blocked` in an `Ok` drain
    /// instead of aborting the caller, so one poisoned runtime cannot
    /// head-of-line block every other runtime's delivery drain forever.
    ///
    /// Within one runtime the drain still stops at the blocked row: the
    /// generated delivery authority enforces in-order acknowledgement, so
    /// acknowledging past a failed row would silently discard a durable
    /// delivery that may still be legitimately applied later. For a durable
    /// delivery pipeline, a bounded per-runtime stall (observable, retried,
    /// recoverable once the cause is fixed) is the lesser risk; silent loss
    /// of a session-visible delivery is not recoverable at all.
    pub async fn apply_pending(
        &self,
        runtime_id: &LogicalRuntimeId,
        limit: usize,
    ) -> Result<RuntimeJobDeliveryDrain, JobOutboxProjectionError> {
        let pending = self.runtime_inbox.list_pending(runtime_id, limit).await?;
        let mut drain = RuntimeJobDeliveryDrain {
            applied: Vec::with_capacity(pending.len()),
            blocked: None,
        };
        for record in pending {
            match self.apply_record(runtime_id, &record).await {
                Ok(applications) => drain.applied.push(AppliedRuntimeJobDelivery {
                    delivery_id: record.submission.delivery_id().clone(),
                    runtime_sequence: record.sequence,
                    applications,
                }),
                Err(error) => {
                    drain.blocked = Some(BlockedRuntimeJobDelivery {
                        delivery_id: record.submission.delivery_id().clone(),
                        runtime_sequence: record.sequence,
                        error: error.to_string(),
                    });
                    break;
                }
            }
        }
        Ok(drain)
    }

    async fn apply_record(
        &self,
        runtime_id: &LogicalRuntimeId,
        record: &RuntimeDeliveryRecord,
    ) -> Result<usize, JobOutboxProjectionError> {
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
        Ok(applications)
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

/// A pending outbox entry whose projection failed during a pass.
///
/// The entry is never acknowledged on failure: it stays pending in the job
/// outbox and is retried on the next pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedJobOutboxEntry {
    pub job_id: JobId,
    pub delivery_sequence: u64,
    pub error: String,
}

/// Outcome of one projection pass over the pending job outbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobOutboxProjectionPass {
    /// Entries handed to the runtime inbox and acknowledged, in outbox order.
    pub projected: Vec<ProjectedJobDelivery>,
    /// First failing entry per poisoned job. Later pending entries of the
    /// same job are held back for the pass to preserve per-job delivery
    /// order; entries of other jobs keep projecting.
    pub skipped: Vec<SkippedJobOutboxEntry>,
}

impl JobOutboxProjectionPass {
    pub fn is_fully_projected(&self) -> bool {
        self.skipped.is_empty()
    }
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
    realm_id: Option<String>,
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
            realm_id: None,
        }
    }

    /// Bind projection authority to one realm.
    ///
    /// Stores are normally realm-scoped already, but this explicit filter
    /// keeps shared/test providers from projecting or acknowledging another
    /// realm's outbox through the current runtime.
    pub fn new_for_realm(
        job_store: Arc<dyn DetachedJobStore>,
        runtime_inbox: RuntimeDeliveryInbox,
        realm_id: impl Into<String>,
    ) -> Self {
        let mut projector = Self::new(job_store, runtime_inbox);
        projector.realm_id = Some(realm_id.into());
        projector
    }

    fn owns_job(&self, job: &meerkat_jobs::StoredJob) -> bool {
        self.realm_id
            .as_deref()
            .is_none_or(|realm_id| job.spec.realm_id == realm_id)
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
        if !self.owns_job(&job) {
            return Err(JobOutboxProjectionError::Corrupt(format!(
                "job {} belongs to realm {}, outside projector realm {}",
                entry.job_id,
                job.spec.realm_id,
                self.realm_id.as_deref().unwrap_or("<unscoped>")
            )));
        }
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

    /// Project pending outbox entries, failing SAFE per job.
    ///
    /// `Err` means the pending outbox itself could not be listed. Any
    /// failure on an individual entry is reported in `skipped` instead of
    /// aborting the pass, so one poisoned outbox row cannot head-of-line
    /// block every other job's delivery projection forever. A failed entry
    /// is never acknowledged — it stays pending and retries next pass — and
    /// the rest of that job's entries are held back for the pass so per-job
    /// delivery order is preserved.
    pub async fn project_pending(
        &self,
        limit: usize,
    ) -> Result<JobOutboxProjectionPass, JobOutboxProjectionError> {
        let entries = self.job_store.list_pending_outbox(limit).await?;
        let mut pass = JobOutboxProjectionPass {
            projected: Vec::with_capacity(entries.len()),
            skipped: Vec::new(),
        };
        let mut poisoned_jobs = std::collections::BTreeSet::new();
        for entry in entries {
            if poisoned_jobs.contains(&entry.job_id) {
                continue;
            }
            match self.project_entry(&entry).await {
                Ok(Some(delivery)) => pass.projected.push(delivery),
                Ok(None) => {}
                Err(error) => {
                    pass.skipped.push(SkippedJobOutboxEntry {
                        job_id: entry.job_id.clone(),
                        delivery_sequence: entry.delivery_sequence,
                        error: error.to_string(),
                    });
                    poisoned_jobs.insert(entry.job_id);
                }
            }
        }
        Ok(pass)
    }

    /// Project one pending entry; `Ok(None)` means the entry belongs to a
    /// realm outside this projector's authority.
    async fn project_entry(
        &self,
        entry: &JobOutboxEntry,
    ) -> Result<Option<ProjectedJobDelivery>, JobOutboxProjectionError> {
        if self.realm_id.is_some() {
            let Some(job) = self.job_store.get(&entry.job_id).await? else {
                return Err(JobOutboxProjectionError::Corrupt(format!(
                    "outbox entry points to missing job {}",
                    entry.job_id
                )));
            };
            if !self.owns_job(&job) {
                return Ok(None);
            }
        }
        let prepared = self.prepare(entry).await?;
        let runtime = self
            .runtime_inbox
            .submit(&prepared.runtime_id, prepared.submission)
            .await?;
        self.job_service
            .mark_delivery_applied(&entry.job_id, entry.delivery_sequence)
            .await?;
        Ok(Some(ProjectedJobDelivery {
            job_id: entry.job_id.clone(),
            delivery_sequence: entry.delivery_sequence,
            runtime_sequence: runtime.sequence,
            runtime_deduplicated: runtime.deduplicated,
        }))
    }
}

#[cfg(not(target_arch = "wasm32"))]
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
        if !self.owns_job(&job) {
            return Err(format!(
                "cannot acknowledge job {job_id} outside projector realm {}",
                self.realm_id.as_deref().unwrap_or("<unscoped>")
            ));
        }
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
