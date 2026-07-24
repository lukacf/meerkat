//! Cross-domain composition for detached jobs.
//!
//! `DetachedJobMachine` owns execution, WorkGraph owns evidence/closure,
//! Schedule owns occurrence delivery, and MeerkatMachine owns explicit
//! per-session wait bindings.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::ops::OperationResult;
use meerkat_core::ops_lifecycle::{
    OperationKind, OperationSource, OperationSpec, OperationStatus, OperationTerminalOutcome,
    OpsLifecycleError, OpsLifecycleRegistry,
};
use meerkat_core::{OperationId, SessionId, ToolCredentialContextRef};
use meerkat_jobs::{
    CanonicalArgumentsHash, DetachedJobError, DetachedJobService, ExecutionIntentId,
    InteractionLineageId, JobReference, JobSpec, JobSubmissionKey, JobTerminalResult,
    OriginMemberId, RestartClass, RunnerIdentity, RunnerSpecificationRef, ToolIdentity,
};
use meerkat_schedule::{
    HostRunnable, HostRunnableError, HostRunnableInvocation, HostRunnableOutcome,
};
use meerkat_workgraph::{
    AddEvidenceRequest, WorkEvidenceRef, WorkGraphError, WorkGraphService, WorkItemRef,
};

use crate::{JobDeliveryApplication, JobDeliveryContent, JobDeliverySink};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum JobCompositionError {
    #[error(transparent)]
    Job(#[from] DetachedJobError),
    #[error(transparent)]
    WorkGraph(#[from] WorkGraphError),
    #[error(transparent)]
    Operations(#[from] OpsLifecycleError),
    #[error("detached job composition rejected input: {0}")]
    InvalidInput(String),
    #[error("detached job composition state is corrupt: {0}")]
    Corrupt(String),
    #[error("failed to encode detached job composition data: {0}")]
    Encode(String),
}

#[derive(Debug, Clone)]
pub struct ScheduledJobTemplate {
    realm_id: String,
    origin_session_id: SessionId,
    tool: ToolIdentity,
    runner: RunnerIdentity,
    restart_class: RestartClass,
    canonical_arguments_hash: CanonicalArgumentsHash,
    origin_member_id: Option<OriginMemberId>,
    runner_specification_ref: Option<RunnerSpecificationRef>,
    credential_context_refs: Vec<ToolCredentialContextRef>,
}

impl ScheduledJobTemplate {
    pub fn new(
        realm_id: impl Into<String>,
        origin_session_id: SessionId,
        tool: ToolIdentity,
        runner: RunnerIdentity,
        restart_class: RestartClass,
        canonical_arguments_hash: CanonicalArgumentsHash,
    ) -> Result<Self, JobCompositionError> {
        let realm_id = realm_id.into();
        validate_scope_component("scheduled job realm", &realm_id)?;
        Ok(Self {
            realm_id,
            origin_session_id,
            tool,
            runner,
            restart_class,
            canonical_arguments_hash,
            origin_member_id: None,
            runner_specification_ref: None,
            credential_context_refs: Vec::new(),
        })
    }

    pub fn with_origin_member_id(mut self, origin_member_id: OriginMemberId) -> Self {
        self.origin_member_id = Some(origin_member_id);
        self
    }

    pub fn with_runner_specification_ref(
        mut self,
        runner_specification_ref: RunnerSpecificationRef,
    ) -> Self {
        self.runner_specification_ref = Some(runner_specification_ref);
        self
    }

    pub fn with_credential_context_refs(
        mut self,
        credential_context_refs: Vec<ToolCredentialContextRef>,
    ) -> Self {
        self.credential_context_refs = credential_context_refs;
        self
    }

    fn spec_for(&self, invocation: &HostRunnableInvocation) -> Result<JobSpec, DetachedJobError> {
        let occurrence_id = invocation.occurrence_id.to_string();
        let mut spec = JobSpec::new(
            self.realm_id.clone(),
            self.origin_session_id.clone(),
            ExecutionIntentId::from_string(format!("schedule_occurrence:{occurrence_id}"))?,
            InteractionLineageId::from_string(format!("schedule_occurrence:{occurrence_id}"))?,
            self.tool.clone(),
            self.runner.clone(),
            self.restart_class,
            self.canonical_arguments_hash.clone(),
            JobSubmissionKey::new(format!("schedule_occurrence:{occurrence_id}"))?,
        );
        spec.origin_member_id = self.origin_member_id.clone();
        spec.runner_specification_ref = self.runner_specification_ref.clone();
        spec.credential_context_refs = self.credential_context_refs.clone();
        Ok(spec)
    }
}

/// Long-running schedule targets commit or re-ensure a job and immediately
/// complete their occurrence invocation.
#[derive(Debug, Clone)]
pub struct ScheduledDurableJobRunnable {
    jobs: DetachedJobService,
    template: ScheduledJobTemplate,
}

impl ScheduledDurableJobRunnable {
    pub fn new(jobs: DetachedJobService, template: ScheduledJobTemplate) -> Self {
        Self { jobs, template }
    }
}

#[async_trait]
impl HostRunnable for ScheduledDurableJobRunnable {
    async fn run(
        &self,
        invocation: HostRunnableInvocation,
    ) -> Result<HostRunnableOutcome, HostRunnableError> {
        let spec =
            self.template
                .spec_for(&invocation)
                .map_err(|error| HostRunnableError::Failed {
                    detail: error.to_string(),
                })?;
        self.jobs
            .submit(spec)
            .await
            .map_err(|error| HostRunnableError::Failed {
                detail: error.to_string(),
            })?;
        Ok(HostRunnableOutcome::completed())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobTerminalEvidenceKind {
    Succeeded,
    Failed,
    Cancelled,
    WorkerLost,
    NeedsAttention,
}

impl JobTerminalEvidenceKind {
    fn from_terminal(result: &JobTerminalResult) -> Self {
        match result {
            JobTerminalResult::Succeeded { .. } => Self::Succeeded,
            JobTerminalResult::Failed { .. } => Self::Failed,
            JobTerminalResult::Cancelled => Self::Cancelled,
            JobTerminalResult::WorkerLost => Self::WorkerLost,
            JobTerminalResult::NeedsAttention { .. } => Self::NeedsAttention,
        }
    }

    fn evidence_kind(self) -> &'static str {
        match self {
            Self::Succeeded => "meerkat.detached_job.terminal.succeeded",
            Self::Failed => "meerkat.detached_job.terminal.failed",
            Self::Cancelled => "meerkat.detached_job.terminal.cancelled",
            Self::WorkerLost => "meerkat.detached_job.terminal.worker_lost",
            Self::NeedsAttention => "meerkat.detached_job.terminal.needs_attention",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobWorkGraphLink {
    job: JobReference,
    work_item: WorkItemRef,
}

impl JobWorkGraphLink {
    pub fn new(job: JobReference, work_item: WorkItemRef) -> Result<Self, JobCompositionError> {
        if job.realm_id() != work_item.realm_id {
            return Err(JobCompositionError::InvalidInput(
                "job and WorkGraph item must belong to the same realm".into(),
            ));
        }
        Ok(Self { job, work_item })
    }

    pub fn job(&self) -> &JobReference {
        &self.job
    }

    pub fn work_item(&self) -> &WorkItemRef {
        &self.work_item
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobTerminalEvidenceProjection {
    WorkGraphDisabled,
    Added(JobTerminalEvidenceKind),
    AlreadyPresent(JobTerminalEvidenceKind),
}

#[derive(Clone)]
pub struct JobTerminalEvidenceProjector {
    jobs: DetachedJobService,
    workgraph: Option<WorkGraphService>,
}

impl std::fmt::Debug for JobTerminalEvidenceProjector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobTerminalEvidenceProjector")
            .field("enabled", &self.workgraph.is_some())
            .finish()
    }
}

impl JobTerminalEvidenceProjector {
    pub fn new(jobs: DetachedJobService, workgraph: WorkGraphService) -> Self {
        Self {
            jobs,
            workgraph: Some(workgraph),
        }
    }

    pub fn disabled(jobs: DetachedJobService) -> Self {
        Self {
            jobs,
            workgraph: None,
        }
    }

    pub async fn project_terminal(
        &self,
        link: &JobWorkGraphLink,
        terminal: &JobTerminalResult,
    ) -> Result<JobTerminalEvidenceProjection, JobCompositionError> {
        let job = self.jobs.get_for_reference(&link.job).await?;
        if job.terminal_result.as_ref() != Some(terminal) {
            return Err(JobCompositionError::Corrupt(format!(
                "terminal evidence for {} disagrees with job authority",
                link.job.job_id()
            )));
        }
        let Some(workgraph) = &self.workgraph else {
            return Ok(JobTerminalEvidenceProjection::WorkGraphDisabled);
        };
        let evidence_kind = JobTerminalEvidenceKind::from_terminal(terminal);
        let evidence = WorkEvidenceRef {
            kind: evidence_kind.evidence_kind().into(),
            id: format!("detached_job:{}:terminal", link.job.job_id()),
            label: Some(format!("Detached job {}", link.job.job_id())),
            summary: Some(
                serde_json::to_string(terminal)
                    .map_err(|error| JobCompositionError::Encode(error.to_string()))?,
            ),
            confirmation_kind: None,
            confirming_owner_key: None,
        };

        loop {
            let item = workgraph
                .get(
                    Some(link.work_item.realm_id.clone()),
                    Some(link.work_item.namespace.clone()),
                    link.work_item.item_id.clone(),
                )
                .await?;
            if let Some(existing) = item
                .evidence_refs
                .iter()
                .find(|existing| existing.id == evidence.id)
            {
                if existing == &evidence {
                    return Ok(JobTerminalEvidenceProjection::AlreadyPresent(evidence_kind));
                }
                return Err(JobCompositionError::Corrupt(format!(
                    "WorkGraph evidence {} conflicts with terminal job evidence",
                    evidence.id
                )));
            }
            let request = AddEvidenceRequest {
                id: item.id,
                realm_id: Some(item.realm_id),
                namespace: Some(item.namespace),
                expected_revision: item.revision,
                evidence: evidence.clone(),
            };
            match workgraph.add_evidence(request).await {
                Ok(_) => return Ok(JobTerminalEvidenceProjection::Added(evidence_kind)),
                Err(WorkGraphError::StaleRevision { .. }) => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobAwaitReceipt {
    pub reference: JobReference,
    pub operation_id: OperationId,
    pub already_terminal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobAwaitActivity {
    pub since_ms: u64,
    pub job_ids: Vec<meerkat_jobs::JobId>,
}

#[derive(Clone)]
pub struct JobAwaitCoordinator {
    realm_id: Arc<str>,
    jobs: DetachedJobService,
    operations: Arc<dyn OpsLifecycleRegistry>,
}

impl std::fmt::Debug for JobAwaitCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobAwaitCoordinator")
            .field("realm_id", &self.realm_id)
            .finish_non_exhaustive()
    }
}

impl JobAwaitCoordinator {
    pub fn new(
        realm_id: impl Into<String>,
        jobs: DetachedJobService,
        operations: Arc<dyn OpsLifecycleRegistry>,
    ) -> Self {
        Self {
            realm_id: Arc::from(realm_id.into()),
            jobs,
            operations,
        }
    }

    pub async fn await_job(
        &self,
        session_id: &SessionId,
        reference: &JobReference,
    ) -> Result<JobAwaitReceipt, JobCompositionError> {
        if reference.realm_id() != self.realm_id.as_ref() {
            return Err(DetachedJobError::NotFound(reference.job_id().clone()).into());
        }
        self.jobs
            .get_authorized_for_session(reference, session_id)
            .await?;
        let operation_id = OperationId::for_detached_job_wait(
            session_id,
            reference.realm_id(),
            reference.job_id().as_str(),
        );
        self.ensure_wait_binding(session_id, reference, &operation_id)?;
        // Close the registration race: terminal delivery may have committed
        // and even been applied after the first authorization read but before
        // MeerkatMachine accepted the wait binding. Reloading does not mutate
        // job authority; it lets the newly durable binding resolve from the
        // latest committed terminal truth.
        let latest = self
            .jobs
            .get_authorized_for_session(reference, session_id)
            .await?;
        if let Some(terminal) = &latest.terminal_result {
            self.apply_terminal_to_operation(&operation_id, terminal)?;
        }
        Ok(JobAwaitReceipt {
            reference: reference.clone(),
            operation_id,
            already_terminal: latest.terminal_result.is_some(),
        })
    }

    pub async fn apply_terminal(
        &self,
        session_id: &SessionId,
        reference: &JobReference,
        terminal: &JobTerminalResult,
    ) -> Result<(), JobCompositionError> {
        if reference.realm_id() != self.realm_id.as_ref() {
            return Err(DetachedJobError::NotFound(reference.job_id().clone()).into());
        }
        let operation_id = OperationId::for_detached_job_wait(
            session_id,
            reference.realm_id(),
            reference.job_id().as_str(),
        );
        let expected_source =
            OperationSource::detached_job(reference.realm_id(), reference.job_id().as_str());
        let Some(operation) = self.operations.snapshot(&operation_id)? else {
            // Terminal delivery and notification subscription are independent
            // from explicit awaiting. Ordinary subscribed delivery must not
            // require the target session to own the job.
            return Ok(());
        };
        validate_wait_operation(&operation, session_id, &expected_source)?;
        let job = self
            .jobs
            .get_authorized_for_session(reference, session_id)
            .await?;
        if job.terminal_result.as_ref() != Some(terminal) {
            return Err(JobCompositionError::Corrupt(format!(
                "terminal delivery for {} disagrees with job authority",
                reference.job_id()
            )));
        }
        self.apply_terminal_to_operation(&operation_id, terminal)?;
        Ok(())
    }

    pub fn activity_for_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<JobAwaitActivity>, JobCompositionError> {
        let mut since_ms = u64::MAX;
        let mut job_ids = Vec::new();
        for operation in self.operations.list_operations()? {
            if operation.kind != OperationKind::DetachedJobWait
                || operation.status != OperationStatus::Running
                || &operation.owner_session_id != session_id
            {
                continue;
            }
            let Some(OperationSource::DetachedJob { realm_id, job_id }) =
                operation.operation_source
            else {
                return Err(JobCompositionError::Corrupt(format!(
                    "running detached job wait {} has no typed job source",
                    operation.id
                )));
            };
            if realm_id != self.realm_id.as_ref() {
                return Err(JobCompositionError::Corrupt(format!(
                    "running detached job wait {} belongs to a different realm",
                    operation.id
                )));
            }
            job_ids.push(meerkat_jobs::JobId::new(job_id)?);
            since_ms = since_ms.min(operation.started_at_ms.unwrap_or(operation.created_at_ms));
        }
        if job_ids.is_empty() {
            return Ok(None);
        }
        job_ids.sort();
        job_ids.dedup();
        Ok(Some(JobAwaitActivity { since_ms, job_ids }))
    }

    fn ensure_wait_binding(
        &self,
        session_id: &SessionId,
        reference: &JobReference,
        operation_id: &OperationId,
    ) -> Result<(), JobCompositionError> {
        let expected_source =
            OperationSource::detached_job(reference.realm_id(), reference.job_id().as_str());
        if let Some(existing) = self.operations.snapshot(operation_id)? {
            validate_wait_operation(&existing, session_id, &expected_source)?;
            if existing.status == OperationStatus::Provisioning {
                self.operations.provisioning_succeeded(operation_id)?;
            }
            return Ok(());
        }
        let spec = OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::DetachedJobWait,
            owner_session_id: session_id.clone(),
            display_name: format!("await detached job {}", reference.job_id()),
            source_label: "await_job".into(),
            operation_source: Some(expected_source.clone()),
            child_session_id: None,
            expect_peer_channel: false,
        };
        match self.operations.register_operation(spec) {
            Ok(()) => self.operations.provisioning_succeeded(operation_id)?,
            Err(OpsLifecycleError::AlreadyRegistered(_)) => {
                let existing = self.operations.snapshot(operation_id)?.ok_or_else(|| {
                    JobCompositionError::Corrupt(format!(
                        "operation {operation_id} reported duplicate registration but is absent"
                    ))
                })?;
                validate_wait_operation(&existing, session_id, &expected_source)?;
                if existing.status == OperationStatus::Provisioning {
                    self.operations.provisioning_succeeded(operation_id)?;
                }
            }
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    fn apply_terminal_to_operation(
        &self,
        operation_id: &OperationId,
        terminal: &JobTerminalResult,
    ) -> Result<(), JobCompositionError> {
        let current = self.operations.snapshot(operation_id)?.ok_or_else(|| {
            JobCompositionError::Corrupt(format!(
                "detached job wait operation {operation_id} disappeared"
            ))
        })?;
        let encoded = serde_json::to_string(terminal)
            .map_err(|error| JobCompositionError::Encode(error.to_string()))?;
        if current.terminal {
            if terminal_outcome_matches_job(current.terminal_outcome.as_ref(), terminal, &encoded) {
                return Ok(());
            }
            return Err(JobCompositionError::Corrupt(format!(
                "wait operation {operation_id} terminal outcome conflicts with job authority"
            )));
        }
        let result = match terminal {
            JobTerminalResult::Succeeded { .. } => self.operations.complete_operation(
                operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: encoded.clone(),
                    is_error: false,
                    duration_ms: 0,
                    tokens_used: 0,
                },
            ),
            JobTerminalResult::Cancelled => self
                .operations
                .cancel_operation(operation_id, Some(encoded.clone())),
            JobTerminalResult::Failed { .. }
            | JobTerminalResult::WorkerLost
            | JobTerminalResult::NeedsAttention { .. } => self
                .operations
                .fail_operation(operation_id, encoded.clone()),
        };
        if let Err(error) = result {
            // Two delivery workers can observe Running before either commits
            // the terminal operation transition. The generated registry
            // serializes them; the loser accepts only the exact terminal truth
            // committed by the winner.
            let latest = self.operations.snapshot(operation_id)?.ok_or_else(|| {
                JobCompositionError::Corrupt(format!(
                    "detached job wait operation {operation_id} disappeared"
                ))
            })?;
            if latest.terminal
                && terminal_outcome_matches_job(
                    latest.terminal_outcome.as_ref(),
                    terminal,
                    &encoded,
                )
            {
                return Ok(());
            }
            return Err(error.into());
        }
        Ok(())
    }
}

fn validate_wait_operation(
    operation: &meerkat_core::ops_lifecycle::OperationLifecycleSnapshot,
    session_id: &SessionId,
    source: &OperationSource,
) -> Result<(), JobCompositionError> {
    if operation.kind != OperationKind::DetachedJobWait
        || &operation.owner_session_id != session_id
        || operation.operation_source.as_ref() != Some(source)
    {
        return Err(JobCompositionError::Corrupt(format!(
            "operation {} conflicts with detached job wait identity",
            operation.id
        )));
    }
    Ok(())
}

fn terminal_outcome_matches_job(
    outcome: Option<&OperationTerminalOutcome>,
    terminal: &JobTerminalResult,
    encoded: &str,
) -> bool {
    match (outcome, terminal) {
        (
            Some(OperationTerminalOutcome::Completed(result)),
            JobTerminalResult::Succeeded { .. },
        ) => !result.is_error && result.content == encoded,
        (Some(OperationTerminalOutcome::Cancelled { reason }), JobTerminalResult::Cancelled) => {
            reason.as_deref() == Some(encoded)
        }
        (
            Some(OperationTerminalOutcome::Failed { error }),
            JobTerminalResult::Failed { .. }
            | JobTerminalResult::WorkerLost
            | JobTerminalResult::NeedsAttention { .. },
        ) => error == encoded,
        _ => false,
    }
}

#[derive(Clone)]
pub struct JobAwaitDeliverySink {
    coordinator: JobAwaitCoordinator,
    downstream: Arc<dyn JobDeliverySink>,
}

impl JobAwaitDeliverySink {
    pub fn new(coordinator: JobAwaitCoordinator, downstream: Arc<dyn JobDeliverySink>) -> Self {
        Self {
            coordinator,
            downstream,
        }
    }
}

#[async_trait]
impl JobDeliverySink for JobAwaitDeliverySink {
    async fn apply(&self, application: JobDeliveryApplication) -> Result<(), String> {
        let (job_id, session_id, terminal) = delivery_terminal(&application);
        if let Some(terminal) = terminal {
            let reference =
                JobReference::new(self.coordinator.realm_id.to_string(), job_id.clone())
                    .map_err(|error| error.to_string())?;
            self.coordinator
                .apply_terminal(session_id, &reference, terminal)
                .await
                .map_err(|error| error.to_string())?;
        }
        self.downstream.apply(application).await
    }
}

fn delivery_terminal(
    application: &JobDeliveryApplication,
) -> (&meerkat_jobs::JobId, &SessionId, Option<&JobTerminalResult>) {
    match application {
        JobDeliveryApplication::Record {
            job_id,
            subscription,
            content,
            ..
        }
        | JobDeliveryApplication::Notification {
            job_id,
            subscription,
            content,
            ..
        }
        | JobDeliveryApplication::Event {
            job_id,
            subscription,
            content,
            ..
        } => (
            job_id,
            subscription.session_id(),
            match content {
                JobDeliveryContent::Terminal(terminal) => Some(terminal),
                JobDeliveryContent::Notification(_) => None,
            },
        ),
    }
}

fn validate_scope_component(label: &str, value: &str) -> Result<(), JobCompositionError> {
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(JobCompositionError::InvalidInput(format!(
            "{label} must be non-empty, canonical, and contain no control characters"
        )));
    }
    Ok(())
}
