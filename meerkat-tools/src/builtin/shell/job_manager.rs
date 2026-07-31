//! Durable background-shell runner and mechanical projections.
//!
//! `DetachedJobMachine` owns lifecycle, attempts, fencing, cancellation, loss,
//! terminality, and delivery. This module owns only shell process mechanics,
//! bounded output capture, and app-facing projections.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use meerkat_core::completion_feed::{
    CompletionEnrichment, CompletionEnrichmentData, CompletionEnrichmentProvider,
};
use meerkat_core::ops_lifecycle::{
    OperationId, OperationKind, OperationLifecycleSnapshot, OperationResult, OperationSpec,
    OpsLifecycleError, OpsLifecycleRegistry,
};
use meerkat_core::types::SessionId;
use meerkat_core::{BlobId, BlobStore, ExecutionPlacement};
use meerkat_jobs::{
    AttemptClaim, AttemptWriteAuthority, CanonicalArgumentsHash, DetachedJobError,
    DetachedJobService, DetachedJobStore, ExecutionIntentId, InteractionLineageId, JobFailureCode,
    JobHealthCondition, JobNotification, JobPhase, JobProgress, JobResultRef, JobSpec,
    JobSubmissionKey, JobSubscription, JobSubscriptionId, JobTerminalResult, RestartClass,
    RunnerHandleRef, RunnerIdentity, RunnerSpecificationRef, ToolIdentity, WorkerId,
};
use meerkat_runtime::RuntimeOpsLifecycleRegistry;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;
use tracing::{debug, info, instrument, warn};

use super::config::{ShellConfig, ShellError};
use super::monitor_protocol::{
    MonitorAction, MonitorLineOutcome, MonitorOutputProtocol, MonitorProtocolDecoder,
    MonitorProtocolLimits,
};
use super::process_lifecycle::{OwnedProcessGroup, join_output_bounded, join_reader_bounded};
use super::types::{BackgroundJob, JobId, JobStatus, JobSummary, JobSummaryStatus};

const DEFAULT_MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_VISIBLE_ORIGIN_JOBS: usize = 10_000;
const LEASE_SETTLEMENT_MARGIN: Duration = Duration::from_secs(60);
#[cfg(not(test))]
const MONITOR_SETTLEMENT_STORE_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(test)]
const MONITOR_SETTLEMENT_STORE_TIMEOUT: Duration = Duration::from_millis(50);
const ATTEMPT_SETTLEMENT_BASE_CONFLICT_BUDGET: usize = 4;
const ATTEMPT_SETTLEMENT_MAX_BACKOFF: Duration = Duration::from_millis(16);
const PROCESS_CONTAINMENT_MAX_BACKOFF: Duration = Duration::from_secs(1);
const SHELL_RUNNER_MEDIA_TYPE: &str = "application/vnd.meerkat.shell-runner+json";
const SHELL_RESULT_MEDIA_TYPE: &str = "application/vnd.meerkat.shell-result+json";

#[async_trait]
pub trait ShellJobDeliveryProjector: Send + Sync {
    /// Project the terminal outbox entry for exactly this job.
    ///
    /// A successful return means this job's runtime-inbox submission is
    /// durable (or was already durably submitted), never merely that some
    /// bounded global delivery batch completed.
    async fn project_job(&self, job_id: &str) -> Result<(), String>;

    /// Advance runtime delivery only after the completion-feed projection is
    /// durably accepted. Implementations may treat an already-applied delivery
    /// as success.
    async fn acknowledge_applied(&self, _job_id: &str) -> Result<(), String> {
        Ok(())
    }
}

/// Durable resources required before detached shell admission is advertised.
#[derive(Clone)]
pub struct DurableShellJobRuntime {
    realm_id: String,
    origin_session_id: SessionId,
    job_store: Arc<dyn DetachedJobStore>,
    blob_store: Arc<dyn BlobStore>,
    delivery_projector: Arc<dyn ShellJobDeliveryProjector>,
}

impl std::fmt::Debug for DurableShellJobRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DurableShellJobRuntime")
            .field("realm_id", &self.realm_id)
            .field("origin_session_id", &self.origin_session_id)
            .finish_non_exhaustive()
    }
}

impl DurableShellJobRuntime {
    pub fn new(
        realm_id: impl Into<String>,
        origin_session_id: SessionId,
        job_store: Arc<dyn DetachedJobStore>,
        blob_store: Arc<dyn BlobStore>,
        delivery_projector: Arc<dyn ShellJobDeliveryProjector>,
    ) -> Result<Self, ShellError> {
        let realm_id = realm_id.into();
        if realm_id.trim().is_empty()
            || realm_id != realm_id.trim()
            || realm_id.chars().any(char::is_control)
        {
            return Err(shell_io("durable shell realm id is invalid"));
        }
        if !job_store.is_persistent() {
            return Err(shell_io(
                "detached shell requires a persistent detached-job store",
            ));
        }
        if !blob_store.is_persistent() {
            return Err(shell_io(
                "detached shell requires a persistent result/specification blob store",
            ));
        }
        Ok(Self {
            realm_id,
            origin_session_id,
            job_store,
            blob_store,
            delivery_projector,
        })
    }

    fn service(&self) -> DetachedJobService {
        DetachedJobService::new(self.job_store.clone())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ShellRunnerSpecification {
    command: String,
    working_dir: String,
    placement: ExecutionPlacement,
    timeout_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    monitor: Option<MonitorRunnerSpecification>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MonitorRunnerSpecification {
    protocol: MonitorOutputProtocol,
    limits: MonitorProtocolLimits,
    delivery: meerkat_jobs::JobDeliveryKind,
}

#[derive(Debug, Clone)]
pub struct MonitorStartOptions {
    pub protocol: MonitorOutputProtocol,
    pub restart_class: RestartClass,
    pub limits: MonitorProtocolLimits,
    pub delivery: meerkat_jobs::JobDeliveryKind,
}

impl Default for MonitorStartOptions {
    fn default() -> Self {
        Self {
            protocol: MonitorOutputProtocol::FramedJsonl,
            restart_class: RestartClass::NonResumable,
            limits: MonitorProtocolLimits::default(),
            delivery: meerkat_jobs::JobDeliveryKind::Notification,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ShellResultRecord {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    duration_secs: f64,
}

#[derive(Debug, Clone)]
struct JobProjection {
    view: BackgroundJob,
}

#[derive(Debug)]
struct ActiveAttempt {
    cancel: Arc<Notify>,
    _task: JoinHandle<()>,
}

/// Acknowledgement level returned by a shell-job cancellation request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelJobDisposition {
    CancellationRequested,
    Cancelled,
}

/// Mechanical shell runner. It never derives lifecycle transitions.
pub struct JobManager {
    config: ShellConfig,
    resolved_shell_path: Arc<Mutex<Option<PathBuf>>>,
    ops_registry: Arc<dyn OpsLifecycleRegistry>,
    owner_bridge_session_id: SessionId,
    owner_session_bound: bool,
    ops_registry_bound: bool,
    durable: Option<DurableShellJobRuntime>,
    recovery_lock: Mutex<()>,
    projections: Arc<Mutex<HashMap<JobId, JobProjection>>>,
    active_attempts: Arc<Mutex<HashMap<JobId, ActiveAttempt>>>,
    canonical_job_ops: Arc<std::sync::Mutex<HashMap<JobId, OperationId>>>,
}

impl std::fmt::Debug for JobManager {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JobManager")
            .field("config", &self.config)
            .field("durable", &self.durable.is_some())
            .field(
                "exports_canonical_async_ops",
                &self.exports_canonical_async_ops(),
            )
            .finish_non_exhaustive()
    }
}

impl JobManager {
    pub fn new(config: ShellConfig) -> Self {
        Self {
            config,
            resolved_shell_path: Arc::new(Mutex::new(None)),
            ops_registry: Arc::new(RuntimeOpsLifecycleRegistry::new()),
            owner_bridge_session_id: SessionId::new(),
            owner_session_bound: false,
            ops_registry_bound: false,
            durable: None,
            recovery_lock: Mutex::new(()),
            projections: Arc::new(Mutex::new(HashMap::new())),
            active_attempts: Arc::new(Mutex::new(HashMap::new())),
            canonical_job_ops: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn with_owner_bridge_session_id(mut self, session_id: SessionId) -> Self {
        self.owner_bridge_session_id = session_id;
        self.owner_session_bound = true;
        self
    }

    pub(crate) fn with_ops_registry(mut self, registry: Arc<dyn OpsLifecycleRegistry>) -> Self {
        self.ops_registry = registry;
        self.ops_registry_bound = true;
        self
    }

    pub fn bind_canonical_async_ops(
        self,
        owner_bridge_session_id: SessionId,
        ops_registry: Arc<dyn OpsLifecycleRegistry>,
    ) -> Self {
        self.with_owner_bridge_session_id(owner_bridge_session_id)
            .with_ops_registry(ops_registry)
    }

    pub fn with_durable_job_runtime(mut self, durable: DurableShellJobRuntime) -> Self {
        self.owner_bridge_session_id = durable.origin_session_id.clone();
        self.owner_session_bound = true;
        self.durable = Some(durable);
        self
    }

    pub fn exports_canonical_async_ops(&self) -> bool {
        self.owner_session_bound && self.ops_registry_bound && self.durable.is_some()
    }

    fn durable(&self) -> Result<&DurableShellJobRuntime, ShellError> {
        self.durable.as_ref().ok_or_else(|| {
            shell_io(
                "detached shell execution requires a durable realm job/blob runtime; \
                 process-local background execution is not available",
            )
        })
    }

    async fn ensure_recovered(&self) -> Result<(), ShellError> {
        let _guard = self.recovery_lock.lock().await;
        self.reconcile_recovered_jobs().await
    }

    async fn reconcile_recovered_jobs(&self) -> Result<(), ShellError> {
        let Some(durable) = &self.durable else {
            return Ok(());
        };
        let jobs = durable
            .job_store
            .list_for_origin(&durable.realm_id, &durable.origin_session_id, usize::MAX)
            .await
            .map_err(shell_job_error)?;
        let service = durable.service();
        let now = unix_time_ms();
        for job in &jobs {
            if job.machine_state.lifecycle_phase == JobPhase::Running
                && job
                    .machine_state
                    .lease_expires_at_ms
                    .is_some_and(|lease_expires_at_ms| now > lease_expires_at_ms)
            {
                let attempt_id =
                    job.machine_state
                        .current_attempt_id
                        .as_ref()
                        .ok_or_else(|| {
                            shell_io(format!(
                                "running job {} has no committed attempt",
                                job.job_id
                            ))
                        })?;
                let write = AttemptWriteAuthority {
                    attempt_id: meerkat_jobs::AttemptId::new(attempt_id)
                        .map_err(shell_job_error)?,
                    fence: meerkat_jobs::FenceToken::new(job.machine_state.current_fence),
                };
                service
                    .observe_lease_expired(&job.job_id, write, now)
                    .await
                    .map_err(shell_job_error)?;
            }
        }
        let loss_observed = durable
            .job_store
            .list_for_origin(&durable.realm_id, &durable.origin_session_id, usize::MAX)
            .await
            .map_err(shell_job_error)?;
        for job in &loss_observed {
            if job.machine_state.lifecycle_phase != JobPhase::LossObserved {
                continue;
            }
            // A cancellation accepted before the worker lease expired remains
            // the durable lifecycle intent. Let the generated
            // RequestCancelLossObserved arm terminalize it before considering
            // replay, checkpoint resume, or worker-loss classification.
            if job.machine_state.cancel_requested {
                service
                    .request_cancel(&job.job_id)
                    .await
                    .map_err(shell_job_error)?;
                continue;
            }
            match job.spec.restart_class {
                RestartClass::Replayable => {
                    service
                        .schedule_retry(&job.job_id, now)
                        .await
                        .map_err(shell_job_error)?;
                }
                RestartClass::CheckpointResumable if job.machine_state.checkpoint_ref.is_some() => {
                    service
                        .schedule_retry(&job.job_id, now)
                        .await
                        .map_err(shell_job_error)?;
                }
                RestartClass::CheckpointResumable => {
                    service
                        .mark_needs_attention(
                            &job.job_id,
                            now,
                            JobFailureCode::new("monitor_checkpoint_missing")
                                .map_err(shell_job_error)?,
                        )
                        .await
                        .map_err(shell_job_error)?;
                }
                RestartClass::Adoptable => {
                    service
                        .mark_needs_attention(
                            &job.job_id,
                            now,
                            JobFailureCode::new("adoptable_runner_reconciliation_unavailable")
                                .map_err(shell_job_error)?,
                        )
                        .await
                        .map_err(shell_job_error)?;
                }
                RestartClass::NonResumable => {
                    service
                        .classify_worker_loss(&job.job_id, now)
                        .await
                        .map_err(shell_job_error)?;
                }
            }
        }
        let restartable = durable
            .job_store
            .list_for_origin(&durable.realm_id, &durable.origin_session_id, usize::MAX)
            .await
            .map_err(shell_job_error)?;
        for job in restartable {
            if !matches!(
                job.machine_state.lifecycle_phase,
                JobPhase::Queued | JobPhase::RetryScheduled
            ) || !matches!(
                job.spec.restart_class,
                RestartClass::Replayable | RestartClass::CheckpointResumable
            ) || (job.machine_state.lifecycle_phase == JobPhase::RetryScheduled
                && job
                    .machine_state
                    .retry_due_at_ms
                    .is_some_and(|retry_due_at_ms| now < retry_due_at_ms))
            {
                continue;
            }
            let runner_spec = load_runner_spec(durable, &job).await?;
            let Some(monitor) = runner_spec.monitor.clone() else {
                continue;
            };
            self.start_recovered_monitor(durable.clone(), job, runner_spec, monitor)
                .await?;
        }
        let recovered = durable
            .job_store
            .list_for_origin(&durable.realm_id, &durable.origin_session_id, usize::MAX)
            .await
            .map_err(shell_job_error)?;
        for job in recovered {
            let Some(terminal_result) = job.terminal_result.as_ref() else {
                continue;
            };
            let public_job_id = JobId::from_string(job.job_id.as_str());
            durable
                .delivery_projector
                .project_job(public_job_id.as_ref())
                .await
                .map_err(|error| {
                    shell_io(format!(
                        "job delivery recovery failed for {public_job_id}: {error}"
                    ))
                })?;
            let operation_id = self.register_operation(&public_job_id)?;
            if self
                .ops_registry
                .snapshot(&operation_id)
                .map_err(shell_ops_error)?
                .is_some_and(|snapshot| snapshot.terminal)
            {
                durable
                    .delivery_projector
                    .acknowledge_applied(public_job_id.as_ref())
                    .await
                    .map_err(|error| {
                        shell_io(format!(
                            "runtime delivery acknowledgement failed for {public_job_id}: {error}"
                        ))
                    })?;
                continue;
            }
            let status = match hydrate_job(durable, job.clone()).await {
                Ok(hydrated) => hydrated.status,
                Err(error) => terminal_projection_status(terminal_result, error.to_string()),
            };
            project_legacy_operation(&*self.ops_registry, &operation_id, &status)
                .map_err(shell_ops_error)?;
            durable
                .delivery_projector
                .acknowledge_applied(public_job_id.as_ref())
                .await
                .map_err(|error| {
                    shell_io(format!(
                        "runtime delivery acknowledgement failed for {public_job_id}: {error}"
                    ))
                })?;
        }
        Ok(())
    }

    async fn resolved_shell_path(&self) -> Result<PathBuf, ShellError> {
        if let Some(path) = self.resolved_shell_path.lock().await.as_ref() {
            return Ok(path.clone());
        }
        let path = self.config.resolve_shell_path_auto_async().await?;
        *self.resolved_shell_path.lock().await = Some(path.clone());
        Ok(path)
    }

    fn operation_admission_limit(&self) -> Option<usize> {
        match self.config.max_concurrent_processes {
            0 => None,
            limit => Some(limit),
        }
    }

    fn register_operation(&self, job_id: &JobId) -> Result<OperationId, ShellError> {
        let operation_id = operation_id_for_job(job_id);
        if self
            .ops_registry
            .snapshot(&operation_id)
            .map_err(shell_ops_error)?
            .is_some()
        {
            self.canonical_job_ops
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(job_id.clone(), operation_id.clone());
            return Ok(operation_id);
        }
        self.ops_registry
            .register_operation_with_admission_limit(
                OperationSpec {
                    id: operation_id.clone(),
                    kind: OperationKind::BackgroundToolOp,
                    owner_session_id: self.owner_bridge_session_id.clone(),
                    display_name: format!("shell background job {job_id}"),
                    source_label: "durable_shell_job".to_string(),
                    operation_source: None,
                    child_session_id: None,
                    expect_peer_channel: false,
                },
                self.operation_admission_limit(),
            )
            .map_err(shell_ops_error)?;
        self.ops_registry
            .provisioning_succeeded(&operation_id)
            .map_err(shell_ops_error)?;
        self.canonical_job_ops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(job_id.clone(), operation_id.clone());
        Ok(operation_id)
    }

    /// Compatibility entry point for direct callers. Agent dispatch supplies a
    /// stable tool-call key through [`Self::spawn_job_for_call`].
    pub async fn spawn_job(
        &self,
        command: &str,
        working_dir: Option<&Path>,
        timeout_secs: u64,
    ) -> Result<JobId, ShellError> {
        let nonce = meerkat_core::time_compat::new_uuid_v7().to_string();
        self.spawn_job_for_call(command, working_dir, timeout_secs, &nonce)
            .await
    }

    #[instrument(
        skip(self, command, working_dir, tool_call_id),
        fields(timeout_secs, has_working_dir = working_dir.is_some())
    )]
    pub async fn spawn_job_for_call(
        &self,
        command: &str,
        working_dir: Option<&Path>,
        timeout_secs: u64,
        tool_call_id: &str,
    ) -> Result<JobId, ShellError> {
        self.spawn_runner_for_call(
            command,
            working_dir,
            timeout_secs,
            tool_call_id,
            None,
            RestartClass::NonResumable,
        )
        .await
    }

    pub async fn spawn_monitor_for_call(
        &self,
        command: &str,
        working_dir: Option<&Path>,
        timeout_secs: u64,
        tool_call_id: &str,
        options: MonitorStartOptions,
    ) -> Result<JobId, ShellError> {
        if options.restart_class == RestartClass::Adoptable {
            return Err(shell_io(
                "agent-authored script monitors cannot claim adoptable restart semantics",
            ));
        }
        if options.protocol == MonitorOutputProtocol::Lines
            && options.restart_class != RestartClass::NonResumable
        {
            return Err(shell_io(
                "line monitor protocol is non-resumable because it has no caller-stable \
                 notification identity; use framed_jsonl with stable keys for recovery",
            ));
        }
        MonitorProtocolDecoder::new(options.protocol, options.limits)
            .map_err(|error| shell_io(error.to_string()))?;
        self.spawn_runner_for_call(
            command,
            working_dir,
            timeout_secs,
            tool_call_id,
            Some(MonitorRunnerSpecification {
                protocol: options.protocol,
                limits: options.limits,
                delivery: options.delivery,
            }),
            options.restart_class,
        )
        .await
    }

    async fn spawn_runner_for_call(
        &self,
        command: &str,
        working_dir: Option<&Path>,
        timeout_secs: u64,
        tool_call_id: &str,
        monitor: Option<MonitorRunnerSpecification>,
        restart_class: RestartClass,
    ) -> Result<JobId, ShellError> {
        if !self.exports_canonical_async_ops() {
            return Err(shell_io(
                "detached shell requires canonical session, operation, and durable storage binding",
            ));
        }
        self.config.check_allowlist(command)?;
        self.ensure_recovered().await?;
        let durable = self.durable()?.clone();

        let resolved_dir = if let Some(dir) = working_dir {
            self.config.validate_working_dir_async(dir).await?
        } else {
            self.config.default_working_dir_async().await?
        };
        let placement = self
            .config
            .execution_placement_for_working_dir_async(&resolved_dir)
            .await?;
        let shell_path = self.resolved_shell_path().await?;
        let redactions = configured_redactions(&self.config);
        if redactions
            .iter()
            .any(|resolved_value| command.contains(resolved_value))
        {
            return Err(shell_io(
                "detached command contains a resolved environment value; reference the \
                 environment variable instead of persisting credential material",
            ));
        }
        let submitted_at_unix = unix_time_secs();
        let runner_spec = ShellRunnerSpecification {
            command: command.to_string(),
            working_dir: resolved_dir.display().to_string(),
            placement: placement.clone(),
            timeout_secs,
            monitor: monitor.clone(),
        };
        let encoded_spec = serde_json::to_string(&runner_spec).map_err(|error| {
            shell_io(format!("cannot encode shell runner specification: {error}"))
        })?;
        let spec_blob = durable
            .blob_store
            .put_artifact(SHELL_RUNNER_MEDIA_TYPE, &encoded_spec)
            .await
            .map_err(|error| {
                shell_io(format!(
                    "cannot persist shell runner specification: {error}"
                ))
            })?;
        let spec_ref =
            RunnerSpecificationRef::new(spec_blob.blob_id.to_string()).map_err(shell_job_error)?;
        let canonical_arguments_hash =
            CanonicalArgumentsHash::new(spec_blob.blob_id.to_string()).map_err(shell_job_error)?;
        let stable_call = validate_call_identity(tool_call_id)?;
        let runner_label = if monitor.is_some() {
            "monitor"
        } else {
            "shell"
        };
        let execution_intent_id =
            ExecutionIntentId::from_string(format!("{runner_label}-call:{stable_call}"))
                .map_err(shell_job_error)?;
        let interaction_lineage_id = InteractionLineageId::from_string(format!(
            "{runner_label}-session:{}",
            durable.origin_session_id
        ))
        .map_err(shell_job_error)?;
        let submission_key = JobSubmissionKey::new(format!(
            "{runner_label}:{}:{stable_call}",
            durable.origin_session_id
        ))
        .map_err(shell_job_error)?;
        let (tool, runner) = if monitor.is_some() {
            (
                ToolIdentity::new("monitor_start", "v1").map_err(shell_job_error)?,
                RunnerIdentity::new("meerkat.monitor_script", "v1").map_err(shell_job_error)?,
            )
        } else {
            (
                ToolIdentity::new("shell", "v1").map_err(shell_job_error)?,
                RunnerIdentity::new("meerkat.shell", "v1").map_err(shell_job_error)?,
            )
        };
        let spec = JobSpec::new(
            durable.realm_id.clone(),
            durable.origin_session_id.clone(),
            execution_intent_id,
            interaction_lineage_id,
            tool,
            runner,
            restart_class,
            canonical_arguments_hash,
            submission_key,
        )
        .with_runner_specification_ref(spec_ref);
        let service = durable.service();
        let receipt = service.submit(spec).await.map_err(shell_job_error)?;
        let public_job_id = JobId::from_string(receipt.job_id.as_str());
        if receipt.deduplicated {
            let existing = service
                .get(&receipt.job_id)
                .await
                .map_err(shell_job_error)?
                .ok_or_else(|| shell_io("deduplicated shell job disappeared"))?;
            if existing.phase != JobPhase::Queued {
                return Ok(public_job_id);
            }
        }
        if let Some(monitor) = &monitor {
            service
                .subscribe(
                    &receipt.job_id,
                    JobSubscription::new(
                        JobSubscriptionId::new("monitor-origin").map_err(shell_job_error)?,
                        durable.origin_session_id.clone(),
                        monitor.delivery.clone(),
                    ),
                )
                .await
                .map_err(shell_job_error)?;
        }

        let claimed_at_ms = unix_time_ms();
        let lease_expires_at_ms = attempt_lease_expiry_ms(claimed_at_ms, timeout_secs);
        let claim = match service
            .claim_attempt(
                &receipt.job_id,
                AttemptClaim::new(
                    WorkerId::new(format!("{runner_label}-worker:{}", std::process::id()))
                        .map_err(shell_job_error)?,
                    claimed_at_ms,
                    lease_expires_at_ms,
                    RunnerHandleRef::new(format!("inproc-{runner_label}:{}", receipt.job_id))
                        .map_err(shell_job_error)?,
                ),
            )
            .await
        {
            Ok(claim) => claim,
            Err(error) if receipt.deduplicated => {
                let current = service
                    .get(&receipt.job_id)
                    .await
                    .map_err(shell_job_error)?
                    .ok_or_else(|| shell_io("deduplicated shell job disappeared"))?;
                if current.phase != JobPhase::Queued {
                    return Ok(public_job_id);
                }
                return Err(shell_job_error(error));
            }
            Err(error) => return Err(shell_job_error(error)),
        };
        let write = AttemptWriteAuthority::from(&claim);
        let operation_id = match self.register_operation(&public_job_id) {
            Ok(operation_id) => operation_id,
            Err(error) => {
                terminal_fail(
                    &service,
                    &receipt.job_id,
                    write,
                    "shell_operation_admission_failed",
                )
                .await;
                if let Err(delivery_error) = durable
                    .delivery_projector
                    .project_job(public_job_id.as_ref())
                    .await
                {
                    warn!(
                        job_id = %public_job_id,
                        %delivery_error,
                        "shell admission failure delivery remains pending"
                    );
                }
                return Err(error);
            }
        };

        let mut command_builder = Command::new(&shell_path);
        command_builder.arg("-c").arg(command);
        command_builder.current_dir(&resolved_dir);
        command_builder.env("PWD", &resolved_dir);
        command_builder.envs(&self.config.env_vars);
        if monitor.is_some() {
            command_builder.env(
                "MEERKAT_MONITOR_SUBMISSION_KEY",
                format!("{runner_label}:{}:{stable_call}", durable.origin_session_id),
            );
            if let Some(checkpoint) = &claim.resume_checkpoint {
                command_builder.env("MEERKAT_MONITOR_CHECKPOINT", checkpoint.as_str());
            }
        }
        command_builder.stdout(Stdio::piped());
        command_builder.stderr(Stdio::piped());
        command_builder.kill_on_drop(true);
        #[cfg(unix)]
        command_builder.process_group(0);
        let child = match command_builder.spawn() {
            Ok(child) => child,
            Err(error) => {
                terminal_fail(&service, &receipt.job_id, write, "shell_spawn_failed").await;
                match durable
                    .delivery_projector
                    .project_job(public_job_id.as_ref())
                    .await
                {
                    Ok(()) => {
                        if let Err(projection_error) = self
                            .ops_registry
                            .fail_operation(&operation_id, format!("shell spawn failed: {error}"))
                        {
                            warn!(
                                job_id = %public_job_id,
                                %projection_error,
                                "runtime delivery committed but spawn-failure feed projection remains pending"
                            );
                        } else if let Err(ack_error) = durable
                            .delivery_projector
                            .acknowledge_applied(public_job_id.as_ref())
                            .await
                        {
                            warn!(
                                job_id = %public_job_id,
                                %ack_error,
                                "spawn-failure feed committed but runtime delivery acknowledgement remains pending"
                            );
                        }
                    }
                    Err(delivery_error) => {
                        warn!(
                            job_id = %public_job_id,
                            %delivery_error,
                            "shell spawn failure delivery remains pending; refusing early completion-feed projection"
                        );
                    }
                }
                return Err(ShellError::Io(error));
            }
        };
        let process_group = OwnedProcessGroup::new(&child);
        let view = BackgroundJob {
            id: public_job_id.clone(),
            command: command.to_string(),
            working_dir: Some(resolved_dir.display().to_string()),
            placement: Some(placement),
            timeout_secs,
            started_at_unix: submitted_at_unix,
            status: JobStatus::Running {
                started_at_unix: submitted_at_unix,
            },
        };
        self.projections
            .lock()
            .await
            .insert(public_job_id.clone(), JobProjection { view });
        let cancel = Arc::new(Notify::new());
        let attempt = AttemptTask {
            job_id: receipt.job_id,
            public_job_id: public_job_id.clone(),
            write,
            timeout_secs,
            child,
            process_group,
            cancel: cancel.clone(),
            durable,
            projections: self.projections.clone(),
            active_attempts: self.active_attempts.clone(),
            ops_registry: self.ops_registry.clone(),
            operation_id,
            redactions,
            resume_progress_cursor: 0,
        };
        let task = match monitor {
            Some(monitor) => spawn_monitor_attempt_task(attempt, monitor),
            None => spawn_attempt_task(attempt),
        };
        self.active_attempts.lock().await.insert(
            public_job_id.clone(),
            ActiveAttempt {
                cancel,
                _task: task,
            },
        );
        info!(job_id = %public_job_id, runner = runner_label, "durable attempt started");
        Ok(public_job_id)
    }

    async fn start_recovered_monitor(
        &self,
        durable: DurableShellJobRuntime,
        stored: meerkat_jobs::StoredJob,
        runner_spec: ShellRunnerSpecification,
        monitor: MonitorRunnerSpecification,
    ) -> Result<(), ShellError> {
        let public_job_id = JobId::from_string(stored.job_id.as_str());
        if self
            .active_attempts
            .lock()
            .await
            .contains_key(&public_job_id)
        {
            return Ok(());
        }
        let resolved_dir = self
            .config
            .validate_working_dir_async(Path::new(&runner_spec.working_dir))
            .await?;
        let shell_path = self.resolved_shell_path().await?;
        let service = durable.service();
        let claimed_at_ms = unix_time_ms();
        let claim = service
            .claim_attempt(
                &stored.job_id,
                AttemptClaim::new(
                    WorkerId::new(format!("monitor-worker:{}", std::process::id()))
                        .map_err(shell_job_error)?,
                    claimed_at_ms,
                    attempt_lease_expiry_ms(claimed_at_ms, runner_spec.timeout_secs),
                    RunnerHandleRef::new(format!("inproc-monitor:{}", stored.job_id))
                        .map_err(shell_job_error)?,
                ),
            )
            .await
            .map_err(shell_job_error)?;
        let write = AttemptWriteAuthority::from(&claim);
        let operation_id = self.register_operation(&public_job_id)?;
        let mut command_builder = Command::new(&shell_path);
        command_builder.arg("-c").arg(&runner_spec.command);
        command_builder.current_dir(&resolved_dir);
        command_builder.env("PWD", &resolved_dir);
        command_builder.envs(&self.config.env_vars);
        let redactions = configured_redactions(&self.config);
        command_builder.env(
            "MEERKAT_MONITOR_SUBMISSION_KEY",
            stored.spec.submission_key.as_str(),
        );
        if let Some(checkpoint) = &claim.resume_checkpoint {
            command_builder.env("MEERKAT_MONITOR_CHECKPOINT", checkpoint.as_str());
        }
        command_builder.stdout(Stdio::piped());
        command_builder.stderr(Stdio::piped());
        command_builder.kill_on_drop(true);
        #[cfg(unix)]
        command_builder.process_group(0);
        let child = match command_builder.spawn() {
            Ok(child) => child,
            Err(error) => {
                terminal_fail(
                    &service,
                    &stored.job_id,
                    write,
                    "monitor_recovery_spawn_failed",
                )
                .await;
                return Err(ShellError::Io(error));
            }
        };
        let process_group = OwnedProcessGroup::new(&child);
        let started_at_unix = unix_time_secs();
        self.projections.lock().await.insert(
            public_job_id.clone(),
            JobProjection {
                view: BackgroundJob {
                    id: public_job_id.clone(),
                    command: runner_spec.command,
                    working_dir: Some(resolved_dir.display().to_string()),
                    placement: Some(runner_spec.placement),
                    timeout_secs: runner_spec.timeout_secs,
                    started_at_unix,
                    status: JobStatus::Running { started_at_unix },
                },
            },
        );
        let cancel = Arc::new(Notify::new());
        let resume_progress_cursor = stored
            .progress
            .as_ref()
            .map_or(0, |progress| progress.cursor);
        let task = spawn_monitor_attempt_task(
            AttemptTask {
                job_id: stored.job_id,
                public_job_id: public_job_id.clone(),
                write,
                timeout_secs: runner_spec.timeout_secs,
                child,
                process_group,
                cancel: cancel.clone(),
                durable,
                projections: self.projections.clone(),
                active_attempts: self.active_attempts.clone(),
                ops_registry: self.ops_registry.clone(),
                operation_id,
                redactions,
                resume_progress_cursor,
            },
            monitor,
        );
        self.active_attempts.lock().await.insert(
            public_job_id,
            ActiveAttempt {
                cancel,
                _task: task,
            },
        );
        Ok(())
    }

    pub async fn get_status(&self, job_id: &JobId) -> Result<Option<BackgroundJob>, ShellError> {
        self.ensure_recovered().await?;
        if let Some(projection) = self.projections.lock().await.get(job_id).cloned() {
            return Ok(Some(projection.view));
        }
        let durable = match &self.durable {
            Some(durable) => durable,
            None => return Ok(None),
        };
        let domain_id = meerkat_jobs::JobId::new(job_id.to_string()).map_err(shell_job_error)?;
        let Some(stored) = durable
            .job_store
            .get(&domain_id)
            .await
            .map_err(shell_job_error)?
        else {
            return Ok(None);
        };
        Ok(Some(hydrate_job(durable, stored).await?))
    }

    pub async fn list_jobs(&self) -> Result<Vec<JobSummary>, ShellError> {
        self.ensure_recovered().await?;
        let local = self.projections.lock().await.clone();
        let mut summaries = local
            .values()
            .map(|projection| JobSummary {
                id: projection.view.id.clone(),
                command: projection.view.command.clone(),
                status: JobSummaryStatus::from(&projection.view.status),
                started_at_unix: projection.view.started_at_unix,
            })
            .collect::<Vec<_>>();
        let Some(durable) = &self.durable else {
            return Ok(summaries);
        };
        let stored = durable
            .job_store
            .list_for_origin(
                &durable.realm_id,
                &durable.origin_session_id,
                MAX_VISIBLE_ORIGIN_JOBS,
            )
            .await
            .map_err(shell_job_error)?;
        for job in stored {
            let hydrated = hydrate_job(durable, job).await?;
            if local.contains_key(&hydrated.id) {
                continue;
            }
            summaries.push(JobSummary {
                id: hydrated.id,
                command: hydrated.command,
                status: JobSummaryStatus::from(&hydrated.status),
                started_at_unix: hydrated.started_at_unix,
            });
        }
        Ok(summaries)
    }

    pub async fn cancel_job(&self, job_id: &JobId) -> Result<CancelJobDisposition, ShellError> {
        self.ensure_recovered().await?;
        if self.durable.is_none()
            && let Some(projection) = self.projections.lock().await.get_mut(job_id)
        {
            projection.view.status = JobStatus::Cancelled { duration_secs: 0.0 };
            let operation_id = self
                .canonical_operation_for_job(job_id)
                .ok_or_else(|| ShellError::JobNotFound(job_id.to_string()))?;
            self.ops_registry
                .cancel_operation(&operation_id, Some("cancelled synthetic operation".into()))
                .map_err(shell_ops_error)?;
            return Ok(CancelJobDisposition::Cancelled);
        }
        let durable = self.durable()?;
        let domain_id = meerkat_jobs::JobId::new(job_id.to_string()).map_err(shell_job_error)?;
        let snapshot = durable
            .service()
            .get(&domain_id)
            .await
            .map_err(shell_job_error)?
            .ok_or_else(|| ShellError::JobNotFound(job_id.to_string()))?;
        if snapshot.terminal_result.is_some() {
            return Err(ShellError::JobNotRunning);
        }
        let requested = durable
            .service()
            .request_cancel(&domain_id)
            .await
            .map_err(shell_job_error)?;
        if requested.terminal_result == Some(JobTerminalResult::Cancelled) {
            durable
                .delivery_projector
                .project_job(job_id.as_ref())
                .await
                .map_err(|error| shell_io(format!("job delivery failed: {error}")))?;
            let operation_id = self.register_operation(job_id)?;
            project_legacy_operation(
                &*self.ops_registry,
                &operation_id,
                &JobStatus::Cancelled { duration_secs: 0.0 },
            )
            .map_err(shell_ops_error)?;
            durable
                .delivery_projector
                .acknowledge_applied(job_id.as_ref())
                .await
                .map_err(|error| {
                    shell_io(format!(
                        "runtime delivery acknowledgement failed for {job_id}: {error}"
                    ))
                })?;
            return Ok(CancelJobDisposition::Cancelled);
        }
        if requested.terminal_result.is_some() {
            return Err(ShellError::JobNotRunning);
        }
        if let Some(attempt) = self.active_attempts.lock().await.get(job_id) {
            attempt.cancel.notify_one();
        }
        // The active attempt may be owned by another manager instance. Its
        // heartbeat observes the durable cancel_requested fact and performs
        // containment; local Notify is only a latency optimization.
        Ok(CancelJobDisposition::CancellationRequested)
    }

    pub async fn remove_job(&self, job_id: &JobId) -> bool {
        let projection = self.projections.lock().await.remove(job_id).is_some();
        self.canonical_job_ops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(job_id);
        projection
    }

    pub fn canonical_operation_for_job(&self, job_id: &JobId) -> Option<OperationId> {
        self.canonical_job_ops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(job_id)
            .cloned()
    }

    /// Durable inbox delivery is the only agent-visible completion path.
    pub async fn drain_completed(&self) -> Vec<meerkat_core::agent::DetachedOpCompletion> {
        Vec::new()
    }

    pub async fn job_count(&self) -> usize {
        self.list_jobs().await.map_or(0, |jobs| jobs.len())
    }

    pub async fn completed_job_count(&self) -> usize {
        self.list_jobs().await.map_or(0, |jobs| {
            jobs.into_iter()
                .filter(|job| {
                    matches!(
                        job.status,
                        JobSummaryStatus::Completed
                            | JobSummaryStatus::Failed
                            | JobSummaryStatus::Cancelled
                            | JobSummaryStatus::WorkerLost
                            | JobSummaryStatus::NeedsAttention
                    )
                })
                .count()
        })
    }

    pub async fn running_job_count(&self) -> Result<usize, ShellError> {
        Ok(self
            .list_jobs()
            .await?
            .into_iter()
            .filter(|job| job.status == JobSummaryStatus::Running)
            .count())
    }

    pub async fn acquire_sync_slot(&self) -> Result<SyncSlotGuard, ShellError> {
        let operation_id = OperationId::new();
        self.ops_registry
            .register_operation_with_admission_limit(
                OperationSpec {
                    id: operation_id.clone(),
                    kind: OperationKind::BackgroundToolCapacitySlot,
                    owner_session_id: self.owner_bridge_session_id.clone(),
                    display_name: "shell:sync-slot".to_string(),
                    source_label: "shell_sync_slot".to_string(),
                    operation_source: None,
                    child_session_id: None,
                    expect_peer_channel: false,
                },
                self.operation_admission_limit(),
            )
            .map_err(shell_ops_error)?;
        self.ops_registry
            .provisioning_succeeded(&operation_id)
            .map_err(shell_ops_error)?;
        Ok(SyncSlotGuard {
            ops_registry: self.ops_registry.clone(),
            operation_id,
        })
    }

    /// Test/support projection that does not launch a subprocess.
    pub async fn register_synthetic_running_job(
        &self,
        command: &str,
        working_dir: Option<&Path>,
        timeout_secs: u64,
    ) -> Result<JobId, ShellError> {
        if !self.owner_session_bound || !self.ops_registry_bound {
            return Err(shell_io(
                "synthetic shell job requires canonical session binding",
            ));
        }
        let job_id = JobId::new();
        self.register_operation(&job_id)?;
        self.projections.lock().await.insert(
            job_id.clone(),
            JobProjection {
                view: BackgroundJob {
                    id: job_id.clone(),
                    command: command.to_string(),
                    working_dir: working_dir.map(|path| path.display().to_string()),
                    placement: None,
                    timeout_secs,
                    started_at_unix: unix_time_secs(),
                    status: JobStatus::Running {
                        started_at_unix: unix_time_secs(),
                    },
                },
            },
        );
        Ok(job_id)
    }

    pub async fn ops_lifecycle_snapshot(
        &self,
        job_id: &JobId,
    ) -> Result<Option<OperationLifecycleSnapshot>, ShellError> {
        let operation_id = self
            .canonical_operation_for_job(job_id)
            .ok_or_else(|| ShellError::JobNotFound(job_id.to_string()))?;
        self.ops_registry
            .snapshot(&operation_id)
            .map_err(shell_ops_error)
    }
}

struct AttemptTask {
    job_id: meerkat_jobs::JobId,
    public_job_id: JobId,
    write: AttemptWriteAuthority,
    timeout_secs: u64,
    child: tokio::process::Child,
    process_group: OwnedProcessGroup,
    cancel: Arc<Notify>,
    durable: DurableShellJobRuntime,
    projections: Arc<Mutex<HashMap<JobId, JobProjection>>>,
    active_attempts: Arc<Mutex<HashMap<JobId, ActiveAttempt>>>,
    ops_registry: Arc<dyn OpsLifecycleRegistry>,
    operation_id: OperationId,
    /// Attempt-local resolved values that must never enter durable job state.
    redactions: Vec<String>,
    resume_progress_cursor: u64,
}

enum MonitorStreamItem {
    Line(String),
    LineTooLong { actual: usize },
    ReadFailed(String),
}

fn spawn_monitor_attempt_task(
    task: AttemptTask,
    monitor: MonitorRunnerSpecification,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let AttemptTask {
            job_id,
            public_job_id,
            write,
            timeout_secs,
            mut child,
            mut process_group,
            cancel,
            durable,
            projections,
            active_attempts,
            ops_registry,
            operation_id,
            redactions,
            resume_progress_cursor,
        } = task;
        let started = Instant::now();
        let (stdout_tx, mut stdout_rx) = tokio::sync::mpsc::channel(64);
        let stdout_task = child.stdout.take().map(|stdout| {
            tokio::spawn(read_monitor_lines(
                stdout,
                monitor.limits.max_line_bytes,
                stdout_tx,
            ))
        });
        let stderr_task = child.stderr.take().map(|stderr| {
            tokio::spawn(read_stream_with_limit(
                stderr,
                monitor.limits.max_retained_diagnostic_bytes,
            ))
        });
        let mut decoder = match MonitorProtocolDecoder::new(monitor.protocol, monitor.limits) {
            Ok(decoder) => decoder,
            Err(error) => {
                warn!(job_id = %public_job_id, %error, "persisted monitor protocol is invalid");
                active_attempts.lock().await.remove(&public_job_id);
                return;
            }
        };
        enum MonitorWaitOutcome {
            Exited(Option<i32>),
            ExplicitComplete,
            WaitFailed(String),
            ProtocolFailed(String),
            TimedOut,
            Cancelled,
        }
        let service = durable.service();
        let mut last_progress_cursor = resume_progress_cursor;
        let mut last_heartbeat_at_ms = unix_time_ms();
        let mut stdout_open = true;
        let wait_outcome = {
            let child_wait = child.wait();
            tokio::pin!(child_wait);
            let timeout = tokio::time::sleep(Duration::from_secs(timeout_secs));
            tokio::pin!(timeout);
            let mut heartbeat = tokio::time::interval(lease_heartbeat_interval());
            heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            heartbeat.tick().await;
            let mut child_exit = None;
            let mut child_wait_open = true;
            loop {
                if !stdout_open && let Some(outcome) = child_exit.take() {
                    break outcome;
                }
                tokio::select! {
                    () = cancel.notified() => break MonitorWaitOutcome::Cancelled,
                    result = &mut child_wait, if child_wait_open => {
                        child_wait_open = false;
                        child_exit = Some(match result {
                            Ok(status) => MonitorWaitOutcome::Exited(status.code()),
                            Err(error) => MonitorWaitOutcome::WaitFailed(error.to_string()),
                        });
                    }
                    () = &mut timeout => break MonitorWaitOutcome::TimedOut,
                    item = stdout_rx.recv(), if stdout_open => {
                        let Some(item) = item else {
                            stdout_open = false;
                            continue;
                        };
                        match item {
                            MonitorStreamItem::Line(line) => {
                                let line = redact_sensitive(&line, &redactions);
                                match decoder.decode_stdout_line_at(&line, unix_time_ms()) {
                                    Ok(MonitorLineOutcome::Action(MonitorAction::Notify {
                                        key,
                                        title,
                                        message,
                                    })) => {
                                        let notification_id = monitor_notification_id(&job_id, &key);
                                        match JobNotification::new(
                                            notification_id,
                                            key,
                                            title,
                                            message,
                                        )
                                        .map_err(shell_job_error)
                                        {
                                            Ok(notification) => {
                                                // A notification commit may
                                                // wait behind a contended
                                                // persistent store. Keep that
                                                // write independently owned so
                                                // cancel/timeout remains a
                                                // control-plane edge, not a
                                                // best-effort poll between
                                                // application awaits.
                                                let notification_service = service.clone();
                                                let notification_job_id = job_id.clone();
                                                let notification_write = write.clone();
                                                let notification_public_job_id =
                                                    public_job_id.clone();
                                                let notification_commit = tokio::spawn(async move {
                                                    let result = notification_service
                                                        .emit_notification(
                                                            &notification_job_id,
                                                            notification_write,
                                                            unix_time_ms(),
                                                            notification,
                                                        )
                                                        .await;
                                                    if let Err(error) = &result {
                                                        warn!(
                                                            job_id = %notification_public_job_id,
                                                            %error,
                                                            "monitor notification commit failed"
                                                        );
                                                    }
                                                    result
                                                });
                                                let notification_result = tokio::select! {
                                                    biased;
                                                    () = cancel.notified() => {
                                                        break MonitorWaitOutcome::Cancelled;
                                                    }
                                                    () = &mut timeout => {
                                                        break MonitorWaitOutcome::TimedOut;
                                                    }
                                                    result = notification_commit => result,
                                                };
                                                match notification_result {
                                                    Ok(Ok(_)) => {
                                                        // Runtime-inbox
                                                        // projection is owned
                                                        // by the durable
                                                        // delivery driver. It
                                                        // must never delay
                                                        // monitor liveness.
                                                    }
                                                    Err(error) => {
                                                        break MonitorWaitOutcome::WaitFailed(
                                                            format!(
                                                                "monitor notification task failed: {error}"
                                                            ),
                                                        );
                                                    }
                                                    Ok(Err(error)) => {
                                                        break MonitorWaitOutcome::WaitFailed(
                                                            format!(
                                                                "monitor notification commit failed: {error}"
                                                            ),
                                                        );
                                                    }
                                                }
                                            }
                                            Err(error) => {
                                                report_monitor_health(
                                                    &service,
                                                    &job_id,
                                                    &write,
                                                    &mut last_progress_cursor,
                                                    JobHealthCondition::MonitorMalformedOutput,
                                                    format!("monitor_malformed_notification:{error}"),
                                                )
                                                .await;
                                            }
                                        }
                                    }
                                    Ok(MonitorLineOutcome::Action(MonitorAction::Checkpoint {
                                        value,
                                    })) => {
                                        let checkpoint = match meerkat_jobs::CheckpointRef::new(value) {
                                            Ok(checkpoint) => checkpoint,
                                            Err(error) => {
                                                report_monitor_health(
                                                    &service,
                                                    &job_id,
                                                    &write,
                                                    &mut last_progress_cursor,
                                                    JobHealthCondition::MonitorMalformedOutput,
                                                    format!("monitor_malformed_checkpoint:{error}"),
                                                )
                                                .await;
                                                continue;
                                            }
                                        };
                                        if let Err(error) = service
                                            .record_checkpoint(
                                                &job_id,
                                                write.clone(),
                                                checkpoint,
                                                unix_time_ms(),
                                            )
                                            .await
                                        {
                                            break MonitorWaitOutcome::WaitFailed(format!(
                                                "monitor checkpoint commit failed: {error}"
                                            ));
                                        }
                                    }
                                    Ok(MonitorLineOutcome::Action(MonitorAction::Progress {
                                        cursor,
                                        message,
                                    })) => {
                                        if cursor <= last_progress_cursor {
                                            report_monitor_health(
                                                &service,
                                                &job_id,
                                                &write,
                                                &mut last_progress_cursor,
                                                JobHealthCondition::MonitorMalformedOutput,
                                                format!(
                                                    "monitor_nonmonotonic_progress:received={cursor}"
                                                ),
                                            )
                                            .await;
                                            continue;
                                        }
                                        let progress = match JobProgress::new(cursor, message) {
                                            Ok(progress) => progress,
                                            Err(error) => {
                                                report_monitor_health(
                                                    &service,
                                                    &job_id,
                                                    &write,
                                                    &mut last_progress_cursor,
                                                    JobHealthCondition::MonitorMalformedOutput,
                                                    format!("monitor_malformed_progress:{error}"),
                                                )
                                                .await;
                                                continue;
                                            }
                                        };
                                        match service
                                            .report_progress(
                                                &job_id,
                                                write.clone(),
                                                progress,
                                                unix_time_ms(),
                                            )
                                            .await
                                        {
                                            Ok(_) => last_progress_cursor = cursor,
                                            Err(error) => {
                                                break MonitorWaitOutcome::WaitFailed(format!(
                                                    "monitor progress commit failed: {error}"
                                                ));
                                            }
                                        }
                                    }
                                    Ok(MonitorLineOutcome::Action(MonitorAction::Complete)) => {
                                        break MonitorWaitOutcome::ExplicitComplete;
                                    }
                                    Ok(MonitorLineOutcome::Diagnostic) => {}
                                    Ok(MonitorLineOutcome::Suppressed {
                                        reason,
                                        total_suppressed,
                                    }) => {
                                        report_monitor_health(
                                            &service,
                                            &job_id,
                                            &write,
                                            &mut last_progress_cursor,
                                            JobHealthCondition::MonitorNotificationRateLimited {
                                                total_suppressed,
                                            },
                                            format!(
                                                "monitor_notification_suppressed:{reason:?}:total={total_suppressed}"
                                            ),
                                        )
                                        .await;
                                    }
                                    Err(error) => {
                                        report_monitor_health(
                                            &service,
                                            &job_id,
                                            &write,
                                            &mut last_progress_cursor,
                                            JobHealthCondition::MonitorMalformedOutput,
                                            format!("monitor_protocol_error:{error}"),
                                        )
                                        .await;
                                    }
                                }
                            }
                            MonitorStreamItem::LineTooLong { actual } => {
                                report_monitor_health(
                                    &service,
                                    &job_id,
                                    &write,
                                    &mut last_progress_cursor,
                                    JobHealthCondition::MonitorOutputTruncated {
                                        dropped_bytes: u64::try_from(
                                            actual.saturating_sub(
                                                monitor.limits.max_line_bytes
                                            ),
                                        )
                                        .unwrap_or(u64::MAX),
                                    },
                                    format!(
                                        "monitor_line_too_long:actual={actual}:limit={}",
                                        monitor.limits.max_line_bytes
                                    ),
                                )
                                .await;
                            }
                            MonitorStreamItem::ReadFailed(error) => {
                                break MonitorWaitOutcome::ProtocolFailed(format!(
                                    "monitor_stdout_read_failed:{error}"
                                ));
                            }
                        }
                    }
                    _ = heartbeat.tick() => {
                        let heartbeat_at_ms = unix_time_ms();
                        let lease_expires_at_ms =
                            attempt_lease_expiry_ms(heartbeat_at_ms, timeout_secs);
                        let renewal = spawn_attempt_lease_renewal(
                            service.clone(),
                            job_id.clone(),
                            write.clone(),
                            heartbeat_at_ms,
                            lease_expires_at_ms,
                        );
                        let renewal_result = tokio::select! {
                            biased;
                            () = cancel.notified() => {
                                break MonitorWaitOutcome::Cancelled;
                            }
                            () = &mut timeout => {
                                break MonitorWaitOutcome::TimedOut;
                            }
                            result = renewal => result,
                        };
                        match renewal_result {
                            Ok(Ok(snapshot)) if snapshot.cancel_requested => {
                                break MonitorWaitOutcome::Cancelled;
                            }
                            Ok(Ok(_)) => {
                                last_heartbeat_at_ms = heartbeat_at_ms;
                            }
                            Ok(Err(error)) => {
                                break MonitorWaitOutcome::WaitFailed(format!(
                                    "monitor lease renewal failed: {error}"
                                ));
                            }
                            Err(error) => {
                                break MonitorWaitOutcome::WaitFailed(format!(
                                    "monitor lease renewal task failed: {error}"
                                ));
                            }
                        }
                    }
                }
            }
        };
        // Once the monitor has explicitly completed, timed out, or been
        // cancelled, no later stdout frame is admissible. Drop the receiver
        // before containment so a producer cannot remain pipe-blocked behind
        // post-completion output while the process group is asked to exit.
        drop(stdout_rx);
        // Containment is the first await after the control loop. No store or
        // delivery work may delay revoking the process group's ability to
        // execute.
        let mut containment_failures = 0usize;
        loop {
            match process_group.terminate(&mut child).await {
                Ok(()) => break,
                Err(error) => {
                    containment_failures = containment_failures.saturating_add(1);
                    warn!(
                        job_id = %public_job_id,
                        %error,
                        containment_failures,
                        "monitor containment unproven; retaining ownership and retrying"
                    );
                    let heartbeat_at_ms = unix_time_ms();
                    match spawn_attempt_lease_renewal(
                        service.clone(),
                        job_id.clone(),
                        write.clone(),
                        heartbeat_at_ms,
                        attempt_lease_expiry_ms(heartbeat_at_ms, timeout_secs),
                    )
                    .await
                    {
                        Ok(Ok(_)) => last_heartbeat_at_ms = heartbeat_at_ms,
                        Ok(Err(renew_error)) => {
                            warn!(
                                job_id = %public_job_id,
                                %renew_error,
                                "monitor containment retry lease renewal was rejected"
                            );
                        }
                        Err(renew_error) => {
                            warn!(
                                job_id = %public_job_id,
                                %renew_error,
                                "monitor containment retry lease task failed"
                            );
                        }
                    }
                    tokio::time::sleep(process_containment_retry_backoff(containment_failures))
                        .await;
                }
            }
        }
        let mut cancellation_was_requested = matches!(&wait_outcome, MonitorWaitOutcome::Cancelled);
        // Containment is the cancellation acknowledgement boundary. Commit it
        // before bounded reader drains, diagnostic processing, or any other
        // store write can consume the lease's settlement margin.
        let mut cancellation_terminal = if cancellation_was_requested {
            Some(
                settle_attempt_after_containment(
                    &service,
                    &job_id,
                    write.clone(),
                    unix_time_ms(),
                    AttemptSettlement::Cancel,
                )
                .await,
            )
        } else {
            None
        };
        if let Some(task) = stdout_task {
            join_reader_bounded(task, "durable monitor stdout").await;
        }
        let stderr = match stderr_task {
            Some(task) => join_output_bounded(task, "durable monitor stderr").await,
            None => Vec::new(),
        };
        // Cancellation was acknowledged immediately after containment. A
        // redundant renewal here would add a second SQLite CAS between the
        // durable cancel request and terminality; if that best-effort write
        // timed out, returning would strand the job in Running with
        // cancel_requested=true.
        if !cancellation_was_requested {
            let heartbeat_at_ms = unix_time_ms().max(last_heartbeat_at_ms.saturating_add(1));
            if let Err(error) = renew_monitor_settlement_lease_bounded(
                service.clone(),
                job_id.clone(),
                write.clone(),
                heartbeat_at_ms,
                attempt_lease_expiry_ms(heartbeat_at_ms, timeout_secs),
            )
            .await
            {
                match acknowledge_committed_monitor_cancel(
                    &service,
                    &job_id,
                    write.clone(),
                    unix_time_ms(),
                )
                .await
                {
                    Ok(Some(cancelled)) => {
                        cancellation_was_requested = true;
                        cancellation_terminal = Some(Ok(cancelled));
                    }
                    Ok(None) => {
                        warn!(
                            job_id = %public_job_id,
                            %error,
                            "monitor settlement lease renewal failed; converging terminal state against any late renewal"
                        );
                    }
                    Err(cancel_error) => {
                        warn!(
                            job_id = %public_job_id,
                            %error,
                            %cancel_error,
                            "monitor settlement renewal failed and cancellation observation was unavailable; converging terminal state"
                        );
                    }
                }
            }
            if !cancellation_was_requested {
                match acknowledge_committed_monitor_cancel(
                    &service,
                    &job_id,
                    write.clone(),
                    unix_time_ms(),
                )
                .await
                {
                    Ok(Some(cancelled)) => {
                        cancellation_was_requested = true;
                        cancellation_terminal = Some(Ok(cancelled));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        warn!(
                            job_id = %public_job_id,
                            %error,
                            "monitor cancellation observation failed before terminal settlement; settlement will reload authoritative state"
                        );
                    }
                }
            }
        }
        let diagnostics = decoder.retained_diagnostics();
        let protocol_health = decoder.health();
        if !cancellation_was_requested && protocol_health.diagnostic_bytes_dropped > 0 {
            report_monitor_health(
                &service,
                &job_id,
                &write,
                &mut last_progress_cursor,
                JobHealthCondition::MonitorOutputTruncated {
                    dropped_bytes: protocol_health.diagnostic_bytes_dropped,
                },
                format!(
                    "monitor_diagnostics_truncated:dropped_bytes={}",
                    protocol_health.diagnostic_bytes_dropped
                ),
            )
            .await;
        }
        let stderr = redact_sensitive(
            &truncate_output_tail(&stderr, monitor.limits.max_retained_diagnostic_bytes),
            &redactions,
        );
        let duration_secs = started.elapsed().as_secs_f64();
        let completed_at_ms = unix_time_ms();
        let wait_outcome = if cancellation_was_requested {
            MonitorWaitOutcome::Cancelled
        } else {
            wait_outcome
        };
        let (view_status, terminal) = match wait_outcome {
            MonitorWaitOutcome::Exited(Some(0)) | MonitorWaitOutcome::ExplicitComplete => {
                let result = ShellResultRecord {
                    exit_code: Some(0),
                    stdout: diagnostics.clone(),
                    stderr: stderr.clone(),
                    duration_secs,
                };
                match persist_result(&durable, &result).await {
                    Ok(result_ref) => (
                        JobStatus::Completed {
                            exit_code: Some(0),
                            stdout: diagnostics,
                            stderr,
                            duration_secs,
                        },
                        settle_attempt_after_containment(
                            &service,
                            &job_id,
                            write.clone(),
                            completed_at_ms,
                            AttemptSettlement::Complete {
                                result_ref: Some(result_ref),
                            },
                        )
                        .await,
                    ),
                    Err(error) => (
                        JobStatus::Failed {
                            error: error.to_string(),
                            duration_secs,
                        },
                        settle_attempt_after_containment(
                            &service,
                            &job_id,
                            write.clone(),
                            completed_at_ms,
                            AttemptSettlement::Fail {
                                code: "monitor_result_persistence_failed",
                                detail_ref: None,
                            },
                        )
                        .await,
                    ),
                }
            }
            MonitorWaitOutcome::Cancelled => {
                let terminal = cancellation_terminal.unwrap_or_else(|| {
                    Err(DetachedJobError::Store(
                        "monitor cancellation reached settlement without an acknowledgement".into(),
                    ))
                });
                (JobStatus::Cancelled { duration_secs }, terminal)
            }
            MonitorWaitOutcome::TimedOut => (
                JobStatus::Failed {
                    error: "monitor timed out".into(),
                    duration_secs,
                },
                settle_attempt_after_containment(
                    &service,
                    &job_id,
                    write.clone(),
                    completed_at_ms,
                    AttemptSettlement::Fail {
                        code: "monitor_timeout",
                        detail_ref: None,
                    },
                )
                .await,
            ),
            MonitorWaitOutcome::Exited(exit_code) => (
                JobStatus::Failed {
                    error: format!("monitor exited with {exit_code:?}"),
                    duration_secs,
                },
                settle_attempt_after_containment(
                    &service,
                    &job_id,
                    write.clone(),
                    completed_at_ms,
                    AttemptSettlement::Fail {
                        code: "monitor_exit_nonzero",
                        detail_ref: None,
                    },
                )
                .await,
            ),
            MonitorWaitOutcome::WaitFailed(error) | MonitorWaitOutcome::ProtocolFailed(error) => (
                JobStatus::Failed {
                    error,
                    duration_secs,
                },
                settle_attempt_after_containment(
                    &service,
                    &job_id,
                    write.clone(),
                    completed_at_ms,
                    AttemptSettlement::Fail {
                        code: "monitor_runner_failed",
                        detail_ref: None,
                    },
                )
                .await,
            ),
        };
        finalize_attempt_projection(
            &public_job_id,
            view_status,
            terminal,
            &durable,
            &projections,
            &*ops_registry,
            &operation_id,
        )
        .await;
        active_attempts.lock().await.remove(&public_job_id);
    })
}

async fn report_monitor_health(
    service: &DetachedJobService,
    job_id: &meerkat_jobs::JobId,
    write: &AttemptWriteAuthority,
    last_progress_cursor: &mut u64,
    condition: JobHealthCondition,
    detail: String,
) {
    let cursor = last_progress_cursor.saturating_add(1);
    if cursor == *last_progress_cursor {
        return;
    }
    let Ok(progress) = JobProgress::health(cursor, condition, detail) else {
        return;
    };
    match service
        .report_progress(job_id, write.clone(), progress, unix_time_ms())
        .await
    {
        Ok(_) => *last_progress_cursor = cursor,
        Err(error) => {
            warn!(job_id = %job_id, %error, "monitor health projection remains stale");
        }
    }
}

fn monitor_notification_id(job_id: &meerkat_jobs::JobId, key: &str) -> String {
    format!(
        "notification_{}",
        uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_OID,
            format!("{}:{key}", job_id.as_str()).as_bytes(),
        )
    )
}

fn configured_redactions(config: &ShellConfig) -> Vec<String> {
    let mut values = config
        .env_vars
        .values()
        .filter(|value| !value.is_empty())
        .cloned()
        .collect::<Vec<_>>();
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    values.dedup();
    values
}

fn redact_sensitive(input: &str, redactions: &[String]) -> String {
    redactions
        .iter()
        .fold(input.to_string(), |redacted, value| {
            redacted.replace(value, "[REDACTED]")
        })
}

async fn read_monitor_lines<R>(
    mut reader: R,
    max_line_bytes: usize,
    sender: tokio::sync::mpsc::Sender<MonitorStreamItem>,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut chunk = [0_u8; 8 * 1024];
    let mut line = Vec::new();
    let mut actual = 0_usize;
    let mut discarding = false;
    loop {
        let read = match reader.read(&mut chunk).await {
            Ok(read) => read,
            Err(error) => {
                let _ = sender
                    .send(MonitorStreamItem::ReadFailed(error.to_string()))
                    .await;
                return;
            }
        };
        if read == 0 {
            if actual > 0 {
                let item = finish_monitor_line(&mut line, actual, discarding);
                let _ = sender.send(item).await;
            }
            return;
        }
        for byte in &chunk[..read] {
            if *byte == b'\n' {
                let item = finish_monitor_line(&mut line, actual, discarding);
                if sender.send(item).await.is_err() {
                    return;
                }
                actual = 0;
                discarding = false;
                continue;
            }
            actual = actual.saturating_add(1);
            if !discarding {
                if line.len() < max_line_bytes {
                    line.push(*byte);
                } else {
                    discarding = true;
                }
            }
        }
    }
}

fn finish_monitor_line(line: &mut Vec<u8>, actual: usize, discarding: bool) -> MonitorStreamItem {
    if discarding {
        line.clear();
        return MonitorStreamItem::LineTooLong { actual };
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    let decoded = String::from_utf8_lossy(line).into_owned();
    line.clear();
    MonitorStreamItem::Line(decoded)
}

async fn finalize_attempt_projection(
    public_job_id: &JobId,
    view_status: JobStatus,
    terminal: Result<meerkat_jobs::JobSnapshot, DetachedJobError>,
    durable: &DurableShellJobRuntime,
    projections: &Arc<Mutex<HashMap<JobId, JobProjection>>>,
    ops_registry: &dyn OpsLifecycleRegistry,
    operation_id: &OperationId,
) {
    match terminal {
        Ok(snapshot) => {
            let view_status = match reconcile_terminal_view(&snapshot, view_status) {
                Ok(status) => status,
                Err(error) => {
                    warn!(
                        job_id = %public_job_id,
                        %error,
                        "durable terminal snapshot could not be reconciled; refusing projection"
                    );
                    return;
                }
            };
            if let Some(projection) = projections.lock().await.get_mut(public_job_id) {
                projection.view.status = view_status.clone();
            }
            match durable
                .delivery_projector
                .project_job(public_job_id.as_ref())
                .await
            {
                Ok(()) => {
                    if let Err(error) =
                        project_legacy_operation(ops_registry, operation_id, &view_status)
                    {
                        warn!(
                            job_id = %public_job_id,
                            %error,
                            "runtime delivery committed but completion projection remains pending"
                        );
                    } else if let Err(error) = durable
                        .delivery_projector
                        .acknowledge_applied(public_job_id.as_ref())
                        .await
                    {
                        warn!(
                            job_id = %public_job_id,
                            %error,
                            "completion projection committed but runtime delivery acknowledgement remains pending"
                        );
                    }
                }
                Err(error) => {
                    warn!(
                        job_id = %public_job_id,
                        %error,
                        "durable job delivery remains pending; refusing early completion projection"
                    );
                }
            }
            debug!(
                job_id = %public_job_id,
                phase = ?snapshot.phase,
                "durable attempt terminal committed"
            );
        }
        Err(error) => {
            warn!(
                job_id = %public_job_id,
                %error,
                "durable attempt terminal commit failed; refusing volatile terminal projection"
            );
        }
    }
}

fn reconcile_terminal_view(
    snapshot: &meerkat_jobs::JobSnapshot,
    proposed: JobStatus,
) -> Result<JobStatus, DetachedJobError> {
    let duration_secs = match &proposed {
        JobStatus::Completed { duration_secs, .. }
        | JobStatus::Failed { duration_secs, .. }
        | JobStatus::Cancelled { duration_secs } => *duration_secs,
        JobStatus::Queued
        | JobStatus::Running { .. }
        | JobStatus::WorkerLost { .. }
        | JobStatus::NeedsAttention { .. } => 0.0,
    };
    match snapshot.terminal_result.as_ref() {
        Some(JobTerminalResult::Succeeded { .. }) => match proposed {
            completed @ JobStatus::Completed { .. } => Ok(completed),
            _ => Ok(JobStatus::Completed {
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                duration_secs,
            }),
        },
        Some(JobTerminalResult::Failed { code, .. }) => Ok(JobStatus::Failed {
            error: code.to_string(),
            duration_secs,
        }),
        Some(JobTerminalResult::Cancelled) => Ok(JobStatus::Cancelled { duration_secs }),
        Some(JobTerminalResult::WorkerLost) => Ok(JobStatus::WorkerLost {
            error: "non-resumable shell worker was lost".to_string(),
        }),
        Some(JobTerminalResult::NeedsAttention { reason }) => Ok(JobStatus::NeedsAttention {
            error: reason.to_string(),
        }),
        None => Err(DetachedJobError::Store(format!(
            "job {} returned a non-terminal settlement snapshot",
            snapshot.job_id
        ))),
    }
}

fn spawn_attempt_task(task: AttemptTask) -> JoinHandle<()> {
    tokio::spawn(async move {
        let AttemptTask {
            job_id,
            public_job_id,
            write,
            timeout_secs,
            mut child,
            mut process_group,
            cancel,
            durable,
            projections,
            active_attempts,
            ops_registry,
            operation_id,
            redactions,
            resume_progress_cursor: _,
        } = task;
        let started = Instant::now();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdout_task = tokio::spawn(async move {
            match stdout {
                Some(stream) => read_stream_with_limit(stream, DEFAULT_MAX_OUTPUT_BYTES).await,
                None => Ok(Vec::new()),
            }
        });
        let stderr_task = tokio::spawn(async move {
            match stderr {
                Some(stream) => read_stream_with_limit(stream, DEFAULT_MAX_OUTPUT_BYTES).await,
                None => Ok(Vec::new()),
            }
        });
        enum WaitOutcome {
            Completed(Option<i32>),
            WaitFailed(String),
            TimedOut,
            Cancelled,
        }
        let service = durable.service();
        let wait_outcome = {
            let child_wait = child.wait();
            tokio::pin!(child_wait);
            let timeout = tokio::time::sleep(Duration::from_secs(timeout_secs));
            tokio::pin!(timeout);
            let mut heartbeat = tokio::time::interval(lease_heartbeat_interval());
            heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            heartbeat.tick().await;
            loop {
                tokio::select! {
                    () = cancel.notified() => break WaitOutcome::Cancelled,
                    result = &mut child_wait => {
                        break match result {
                            Ok(status) => WaitOutcome::Completed(status.code()),
                            Err(error) => WaitOutcome::WaitFailed(error.to_string()),
                        };
                    }
                    () = &mut timeout => break WaitOutcome::TimedOut,
                    _ = heartbeat.tick() => {
                        let heartbeat_at_ms = unix_time_ms();
                        let lease_expires_at_ms =
                            attempt_lease_expiry_ms(heartbeat_at_ms, timeout_secs);
                        let renewal = spawn_attempt_lease_renewal(
                            service.clone(),
                            job_id.clone(),
                            write.clone(),
                            heartbeat_at_ms,
                            lease_expires_at_ms,
                        );
                        let renewal_result = tokio::select! {
                            biased;
                            () = cancel.notified() => break WaitOutcome::Cancelled,
                            () = &mut timeout => break WaitOutcome::TimedOut,
                            result = renewal => result,
                        };
                        match renewal_result {
                            Ok(Ok(snapshot)) if snapshot.cancel_requested => {
                                break WaitOutcome::Cancelled;
                            }
                            Ok(Ok(_)) => {}
                            Ok(Err(error)) => {
                                break WaitOutcome::WaitFailed(format!(
                                    "shell attempt lease renewal failed: {error}"
                                ));
                            }
                            Err(error) => {
                                break WaitOutcome::WaitFailed(format!(
                                    "shell attempt lease renewal task failed: {error}"
                                ));
                            }
                        }
                    }
                }
            }
        };
        let settled_at_ms = unix_time_ms();
        let mut containment_failures = 0usize;
        loop {
            match process_group.terminate(&mut child).await {
                Ok(()) => break,
                Err(error) => {
                    containment_failures = containment_failures.saturating_add(1);
                    warn!(
                        job_id = %public_job_id,
                        %error,
                        containment_failures,
                        "shell containment unproven; retaining ownership and retrying"
                    );
                    let heartbeat_at_ms = unix_time_ms();
                    match spawn_attempt_lease_renewal(
                        service.clone(),
                        job_id.clone(),
                        write.clone(),
                        heartbeat_at_ms,
                        attempt_lease_expiry_ms(heartbeat_at_ms, timeout_secs),
                    )
                    .await
                    {
                        Ok(Ok(_)) => {}
                        Ok(Err(renew_error)) => {
                            warn!(
                                job_id = %public_job_id,
                                %renew_error,
                                "shell containment retry lease renewal was rejected"
                            );
                        }
                        Err(renew_error) => {
                            warn!(
                                job_id = %public_job_id,
                                %renew_error,
                                "shell containment retry lease task failed"
                            );
                        }
                    }
                    tokio::time::sleep(process_containment_retry_backoff(containment_failures))
                        .await;
                }
            }
        }
        let (stdout, stderr) = tokio::join!(
            join_output_bounded(stdout_task, "durable shell stdout"),
            join_output_bounded(stderr_task, "durable shell stderr")
        );
        let stdout = redact_sensitive(
            &truncate_output_tail(&stdout, DEFAULT_MAX_OUTPUT_BYTES),
            &redactions,
        );
        let stderr = redact_sensitive(
            &truncate_output_tail(&stderr, DEFAULT_MAX_OUTPUT_BYTES),
            &redactions,
        );
        let duration_secs = started.elapsed().as_secs_f64();
        let (view_status, terminal) = match wait_outcome {
            WaitOutcome::Completed(exit_code) => {
                let result = ShellResultRecord {
                    exit_code,
                    stdout: stdout.clone(),
                    stderr: stderr.clone(),
                    duration_secs,
                };
                match persist_result(&durable, &result).await {
                    Ok(result_ref) => (
                        JobStatus::Completed {
                            exit_code,
                            stdout,
                            stderr,
                            duration_secs,
                        },
                        settle_attempt_after_containment(
                            &service,
                            &job_id,
                            write.clone(),
                            settled_at_ms,
                            AttemptSettlement::Complete {
                                result_ref: Some(result_ref),
                            },
                        )
                        .await,
                    ),
                    Err(error) => {
                        warn!(job_id = %public_job_id, %error, "shell result persistence failed");
                        (
                            JobStatus::Failed {
                                error: format!("shell result persistence failed: {error}"),
                                duration_secs,
                            },
                            settle_attempt_after_containment(
                                &service,
                                &job_id,
                                write.clone(),
                                settled_at_ms,
                                AttemptSettlement::Fail {
                                    code: "shell_result_persistence_failed",
                                    detail_ref: None,
                                },
                            )
                            .await,
                        )
                    }
                }
            }
            WaitOutcome::Cancelled => (
                JobStatus::Cancelled { duration_secs },
                settle_attempt_after_containment(
                    &service,
                    &job_id,
                    write.clone(),
                    settled_at_ms,
                    AttemptSettlement::Cancel,
                )
                .await,
            ),
            WaitOutcome::TimedOut => (
                JobStatus::Failed {
                    error: "background job timed out".to_string(),
                    duration_secs,
                },
                settle_attempt_after_containment(
                    &service,
                    &job_id,
                    write.clone(),
                    settled_at_ms,
                    AttemptSettlement::Fail {
                        code: "shell_timeout",
                        detail_ref: None,
                    },
                )
                .await,
            ),
            WaitOutcome::WaitFailed(error) => (
                JobStatus::Failed {
                    error,
                    duration_secs,
                },
                settle_attempt_after_containment(
                    &service,
                    &job_id,
                    write,
                    settled_at_ms,
                    AttemptSettlement::Fail {
                        code: "shell_wait_failed",
                        detail_ref: None,
                    },
                )
                .await,
            ),
        };
        finalize_attempt_projection(
            &public_job_id,
            view_status,
            terminal,
            &durable,
            &projections,
            &*ops_registry,
            &operation_id,
        )
        .await;
        active_attempts.lock().await.remove(&public_job_id);
    })
}

fn project_legacy_operation(
    registry: &dyn OpsLifecycleRegistry,
    operation_id: &OperationId,
    status: &JobStatus,
) -> Result<(), OpsLifecycleError> {
    match status {
        JobStatus::Completed {
            stdout,
            stderr,
            duration_secs,
            ..
        } => registry.complete_operation(
            operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: if stdout.is_empty() {
                    stderr.clone()
                } else {
                    stdout.clone()
                },
                is_error: false,
                duration_ms: (*duration_secs * 1000.0) as u64,
                tokens_used: 0,
            },
        ),
        JobStatus::Failed { error, .. }
        | JobStatus::WorkerLost { error }
        | JobStatus::NeedsAttention { error } => {
            registry.fail_operation(operation_id, error.clone())
        }
        JobStatus::Cancelled { .. } => {
            registry.cancel_operation(operation_id, Some("cancelled by caller".into()))
        }
        JobStatus::Queued | JobStatus::Running { .. } => Ok(()),
    }
}

fn terminal_projection_status(
    terminal_result: &JobTerminalResult,
    hydration_error: String,
) -> JobStatus {
    match terminal_result {
        JobTerminalResult::Succeeded { .. } => JobStatus::Completed {
            exit_code: None,
            stdout: String::new(),
            stderr: format!("durable result unavailable during recovery: {hydration_error}"),
            duration_secs: 0.0,
        },
        JobTerminalResult::Failed { code, .. } => JobStatus::Failed {
            error: format!("{code}: {hydration_error}"),
            duration_secs: 0.0,
        },
        JobTerminalResult::Cancelled => JobStatus::Cancelled { duration_secs: 0.0 },
        JobTerminalResult::WorkerLost => JobStatus::WorkerLost {
            error: "non-resumable shell worker was lost".to_string(),
        },
        JobTerminalResult::NeedsAttention { reason } => JobStatus::NeedsAttention {
            error: reason.to_string(),
        },
    }
}

async fn persist_result(
    durable: &DurableShellJobRuntime,
    result: &ShellResultRecord,
) -> Result<JobResultRef, ShellError> {
    let encoded = serde_json::to_string(result)
        .map_err(|error| shell_io(format!("cannot encode shell result: {error}")))?;
    let blob = durable
        .blob_store
        .put_artifact(SHELL_RESULT_MEDIA_TYPE, &encoded)
        .await
        .map_err(|error| shell_io(format!("cannot persist shell result: {error}")))?;
    JobResultRef::new(blob.blob_id.to_string()).map_err(shell_job_error)
}

async fn hydrate_job(
    durable: &DurableShellJobRuntime,
    stored: meerkat_jobs::StoredJob,
) -> Result<BackgroundJob, ShellError> {
    let spec = load_runner_spec(durable, &stored).await?;
    let started_at_unix = stored.machine_state.heartbeat_at_ms.unwrap_or_default() / 1_000;
    let is_monitor = spec.monitor.is_some();
    let status = match stored.terminal_result {
        None if stored.machine_state.lifecycle_phase == JobPhase::Queued => JobStatus::Queued,
        None => JobStatus::Running { started_at_unix },
        Some(JobTerminalResult::Succeeded {
            result_ref: Some(result_ref),
        }) => {
            let result_payload = durable
                .blob_store
                .get(&BlobId::new(result_ref.as_str()))
                .await
                .map_err(|error| shell_io(format!("cannot read shell result: {error}")))?;
            let result: ShellResultRecord = serde_json::from_str(&result_payload.data)
                .map_err(|error| shell_io(format!("shell result is corrupt: {error}")))?;
            JobStatus::Completed {
                exit_code: result.exit_code,
                stdout: result.stdout,
                stderr: result.stderr,
                duration_secs: result.duration_secs,
            }
        }
        Some(JobTerminalResult::Succeeded { result_ref: None }) if is_monitor => {
            JobStatus::Completed {
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
                duration_secs: 0.0,
            }
        }
        Some(JobTerminalResult::Succeeded { result_ref: None }) => JobStatus::Failed {
            error: "shell job succeeded without a durable result reference".to_string(),
            duration_secs: 0.0,
        },
        Some(JobTerminalResult::Failed { code, .. }) => JobStatus::Failed {
            error: code.to_string(),
            duration_secs: 0.0,
        },
        Some(JobTerminalResult::Cancelled) => JobStatus::Cancelled { duration_secs: 0.0 },
        Some(JobTerminalResult::WorkerLost) => JobStatus::WorkerLost {
            error: "non-resumable shell worker was lost".to_string(),
        },
        Some(JobTerminalResult::NeedsAttention { reason }) => JobStatus::NeedsAttention {
            error: reason.to_string(),
        },
    };
    Ok(BackgroundJob {
        id: JobId::from_string(stored.job_id.as_str()),
        command: spec.command,
        working_dir: Some(spec.working_dir),
        placement: Some(spec.placement),
        timeout_secs: spec.timeout_secs,
        started_at_unix,
        status,
    })
}

async fn load_runner_spec(
    durable: &DurableShellJobRuntime,
    stored: &meerkat_jobs::StoredJob,
) -> Result<ShellRunnerSpecification, ShellError> {
    let specification_ref = stored
        .spec
        .runner_specification_ref
        .as_ref()
        .ok_or_else(|| shell_io(format!("job {} has no runner specification", stored.job_id)))?;
    let payload = durable
        .blob_store
        .get(&BlobId::new(specification_ref.as_str()))
        .await
        .map_err(|error| shell_io(format!("cannot read shell runner specification: {error}")))?;
    let spec: ShellRunnerSpecification = serde_json::from_str(&payload.data)
        .map_err(|error| shell_io(format!("shell runner specification is corrupt: {error}")))?;
    Ok(spec)
}

async fn terminal_fail(
    service: &DetachedJobService,
    job_id: &meerkat_jobs::JobId,
    write: AttemptWriteAuthority,
    code: &str,
) {
    if let Err(error) = fail_shell_attempt(service, job_id, write, unix_time_ms(), code).await {
        warn!(%job_id, %error, "failed to commit shell admission failure");
    }
}

async fn fail_shell_attempt(
    service: &DetachedJobService,
    job_id: &meerkat_jobs::JobId,
    write: AttemptWriteAuthority,
    observed_at_ms: u64,
    code: &str,
) -> Result<meerkat_jobs::JobSnapshot, DetachedJobError> {
    service
        .fail_attempt(
            job_id,
            write,
            observed_at_ms,
            JobFailureCode::new(code)?,
            None,
        )
        .await
}

fn validate_call_identity(value: &str) -> Result<String, ShellError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return Err(shell_io("shell tool-call identity is invalid"));
    }
    Ok(trimmed.to_string())
}

async fn read_stream_with_limit<R>(mut reader: R, max_bytes: usize) -> std::io::Result<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.len() > max_bytes.saturating_mul(2) {
            let keep_from = buffer.len().saturating_sub(max_bytes);
            buffer.drain(..keep_from);
        }
    }
    Ok(buffer)
}

fn operation_id_for_job(job_id: &JobId) -> OperationId {
    OperationId(uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_OID,
        format!("meerkat-durable-shell:{}", job_id.as_ref()).as_bytes(),
    ))
}

fn truncate_output_tail(data: &[u8], max_bytes: usize) -> String {
    let start = data.len().saturating_sub(max_bytes);
    String::from_utf8_lossy(&data[start..]).to_string()
}

fn unix_time_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn attempt_lease_expiry_ms(now_ms: u64, timeout_secs: u64) -> u64 {
    let lease_ms = Duration::from_secs(timeout_secs)
        .saturating_add(LEASE_SETTLEMENT_MARGIN)
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    now_ms.saturating_add(lease_ms)
}

fn lease_heartbeat_interval() -> Duration {
    #[cfg(test)]
    {
        Duration::from_millis(10)
    }
    #[cfg(not(test))]
    {
        Duration::from_secs(10)
    }
}

#[derive(Clone)]
enum AttemptSettlement {
    Cancel,
    Complete {
        result_ref: Option<JobResultRef>,
    },
    Fail {
        code: &'static str,
        detail_ref: Option<JobResultRef>,
    },
}

async fn acknowledge_committed_monitor_cancel(
    service: &DetachedJobService,
    job_id: &meerkat_jobs::JobId,
    write: AttemptWriteAuthority,
    acknowledged_at_ms: u64,
) -> Result<Option<meerkat_jobs::JobSnapshot>, DetachedJobError> {
    let snapshot = service
        .get(job_id)
        .await?
        .ok_or_else(|| DetachedJobError::NotFound(job_id.clone()))?;
    if snapshot.terminal_result == Some(JobTerminalResult::Cancelled) {
        return Ok(Some(snapshot));
    }
    if !snapshot.cancel_requested {
        return Ok(None);
    }
    settle_attempt_after_containment(
        service,
        job_id,
        write,
        acknowledged_at_ms,
        AttemptSettlement::Cancel,
    )
    .await
    .map(Some)
}

async fn settle_attempt_after_containment(
    service: &DetachedJobService,
    job_id: &meerkat_jobs::JobId,
    write: AttemptWriteAuthority,
    settled_at_ms: u64,
    settlement: AttemptSettlement,
) -> Result<meerkat_jobs::JobSnapshot, DetachedJobError> {
    let initial = service
        .get(job_id)
        .await?
        .ok_or_else(|| DetachedJobError::NotFound(job_id.clone()))?;
    ensure_attempt_snapshot_matches(job_id, &initial, &write)?;
    if initial.terminal_result.is_some() {
        return Ok(initial);
    }
    // After process containment, the only legitimate revision writers are:
    // one already-in-flight attempt mutation, one durable cancellation
    // request, one late heartbeat, and delivery acknowledgements for the
    // finite pending outbox. Budget exactly that closed writer set.
    let pending_delivery_writers = initial.outbox.iter().filter(|entry| !entry.applied).count();
    let conflict_budget =
        pending_delivery_writers.saturating_add(ATTEMPT_SETTLEMENT_BASE_CONFLICT_BUDGET);
    let mut conflicts = 0usize;
    loop {
        let current = service
            .get(job_id)
            .await?
            .ok_or_else(|| DetachedJobError::NotFound(job_id.clone()))?;
        ensure_attempt_snapshot_matches(job_id, &current, &write)?;
        if current.terminal_result.is_some() {
            return Ok(current);
        }
        let outcome = match &settlement {
            AttemptSettlement::Cancel => {
                service
                    .acknowledge_cancel(job_id, write.clone(), settled_at_ms)
                    .await
            }
            AttemptSettlement::Complete { result_ref } => {
                service
                    .complete_attempt(job_id, write.clone(), settled_at_ms, result_ref.clone())
                    .await
            }
            AttemptSettlement::Fail { code, detail_ref } => {
                service
                    .fail_attempt(
                        job_id,
                        write.clone(),
                        settled_at_ms,
                        JobFailureCode::new(*code)?,
                        detail_ref.clone(),
                    )
                    .await
            }
        };
        match outcome {
            Err(DetachedJobError::StaleRevision { .. }) if conflicts < conflict_budget => {
                conflicts = conflicts.saturating_add(1);
                tokio::time::sleep(attempt_settlement_backoff(conflicts)).await;
            }
            outcome => return outcome,
        }
    }
}

fn ensure_attempt_snapshot_matches(
    job_id: &meerkat_jobs::JobId,
    snapshot: &meerkat_jobs::JobSnapshot,
    write: &AttemptWriteAuthority,
) -> Result<(), DetachedJobError> {
    if snapshot.current_attempt_id.as_ref() != Some(&write.attempt_id)
        || snapshot.current_fence != write.fence
    {
        return Err(DetachedJobError::StaleAttempt {
            job_id: job_id.clone(),
            attempt_id: write.attempt_id.clone(),
            fence: write.fence,
        });
    }
    Ok(())
}

fn attempt_settlement_backoff(conflicts: usize) -> Duration {
    let shift = conflicts.saturating_sub(1).min(4);
    let millis = 1u64 << shift;
    Duration::from_millis(millis).min(ATTEMPT_SETTLEMENT_MAX_BACKOFF)
}

fn process_containment_retry_backoff(failures: usize) -> Duration {
    let shift = failures.saturating_sub(1).min(4);
    let millis = 100u64.saturating_mul(1u64 << shift);
    Duration::from_millis(millis).min(PROCESS_CONTAINMENT_MAX_BACKOFF)
}

async fn renew_monitor_settlement_lease_bounded(
    service: DetachedJobService,
    job_id: meerkat_jobs::JobId,
    write: AttemptWriteAuthority,
    heartbeat_at_ms: u64,
    lease_expires_at_ms: u64,
) -> Result<(), String> {
    let renewal =
        spawn_attempt_lease_renewal(service, job_id, write, heartbeat_at_ms, lease_expires_at_ms);
    match tokio::time::timeout(MONITOR_SETTLEMENT_STORE_TIMEOUT, renewal).await {
        Ok(Ok(Ok(_))) => Ok(()),
        Ok(Ok(Err(error))) => Err(error.to_string()),
        Ok(Err(error)) => Err(format!("settlement lease task failed: {error}")),
        Err(_) => Err(format!(
            "settlement lease renewal exceeded {} ms",
            MONITOR_SETTLEMENT_STORE_TIMEOUT.as_millis()
        )),
    }
}

fn spawn_attempt_lease_renewal(
    service: DetachedJobService,
    job_id: meerkat_jobs::JobId,
    write: AttemptWriteAuthority,
    heartbeat_at_ms: u64,
    lease_expires_at_ms: u64,
) -> JoinHandle<Result<meerkat_jobs::JobSnapshot, DetachedJobError>> {
    // SQLite-backed DetachedJobStore methods currently perform their
    // rusqlite work synchronously inside the async trait future. Every monitor
    // lease write therefore runs on the blocking pool: a 60-second SQLite busy
    // wait must not pin the monitor control task between cancel/timeout polls.
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        runtime.block_on(service.renew_lease(&job_id, write, heartbeat_at_ms, lease_expires_at_ms))
    })
}

fn shell_io(message: impl Into<String>) -> ShellError {
    ShellError::Io(std::io::Error::other(message.into()))
}

fn shell_job_error(error: DetachedJobError) -> ShellError {
    shell_io(error.to_string())
}

fn shell_ops_error(error: OpsLifecycleError) -> ShellError {
    shell_io(error.to_string())
}

/// Foreground capacity guard. The generated operation registry owns admission.
pub struct SyncSlotGuard {
    ops_registry: Arc<dyn OpsLifecycleRegistry>,
    operation_id: OperationId,
}

impl std::fmt::Debug for SyncSlotGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SyncSlotGuard")
            .field("operation_id", &self.operation_id)
            .finish_non_exhaustive()
    }
}

impl Drop for SyncSlotGuard {
    fn drop(&mut self) {
        if let Err(error) = self.ops_registry.mark_retired(&self.operation_id) {
            warn!(operation_id = %self.operation_id, %error, "shell sync slot release failed");
        }
    }
}

impl CompletionEnrichmentProvider for JobManager {
    fn enrich(&self, operation_id: &OperationId) -> CompletionEnrichment {
        let canonical = self
            .canonical_job_ops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(job_id) = canonical
            .iter()
            .find_map(|(job_id, candidate)| (candidate == operation_id).then(|| job_id.clone()))
        else {
            return CompletionEnrichment::Missing;
        };
        drop(canonical);
        let Ok(projections) = self.projections.try_lock() else {
            return CompletionEnrichment::Busy;
        };
        let Some(projection) = projections.get(&job_id) else {
            return CompletionEnrichment::Missing;
        };
        CompletionEnrichment::Found(CompletionEnrichmentData {
            job_id: job_id.to_string(),
            detail: format!("{:?}", projection.view.status),
        })
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod durable_tests {
    use super::*;
    use meerkat_jobs::{
        DetachedJobService, InsertJobOutcome, JobOutboxEntry, SqliteDetachedJobStore, StoredJob,
    };
    use meerkat_store::FsBlobStore;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tempfile::TempDir;

    #[derive(Debug)]
    struct NoopDeliveryProjector;

    #[async_trait]
    impl ShellJobDeliveryProjector for NoopDeliveryProjector {
        async fn project_job(&self, _job_id: &str) -> Result<(), String> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct GateDeliveryProjector {
        available: AtomicBool,
    }

    #[async_trait]
    impl ShellJobDeliveryProjector for GateDeliveryProjector {
        async fn project_job(&self, _job_id: &str) -> Result<(), String> {
            self.available
                .load(Ordering::SeqCst)
                .then_some(())
                .ok_or_else(|| "runtime inbox unavailable".to_string())
        }
    }

    #[derive(Debug, Default)]
    struct RecordingDeliveryProjector {
        project_calls: AtomicUsize,
        acknowledge_calls: AtomicUsize,
    }

    #[async_trait]
    impl ShellJobDeliveryProjector for RecordingDeliveryProjector {
        async fn project_job(&self, _job_id: &str) -> Result<(), String> {
            self.project_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn acknowledge_applied(&self, _job_id: &str) -> Result<(), String> {
            self.acknowledge_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[derive(Debug, Default)]
    struct BlockingDeliveryProjector {
        calls: AtomicUsize,
        entered: Notify,
        release: Notify,
    }

    #[async_trait]
    impl ShellJobDeliveryProjector for BlockingDeliveryProjector {
        async fn project_job(&self, _job_id: &str) -> Result<(), String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            self.release.notified().await;
            Ok(())
        }
    }

    #[derive(Debug)]
    struct PausingHeartbeatStore {
        inner: Arc<SqliteDetachedJobStore>,
        heartbeat_pause_armed: AtomicBool,
        heartbeat_paused_once: AtomicBool,
        reject_cancel_settlement_renewal: AtomicBool,
        cancel_ack_stale_once: AtomicBool,
        cancel_ack_attempts: AtomicUsize,
        terminal_ack_cas_wins_remaining: AtomicUsize,
        terminal_ack_cas_wins: AtomicUsize,
        non_cancel_terminal_pause_armed: AtomicBool,
        non_cancel_terminal_paused_once: AtomicBool,
        cancel_request_pause_armed: AtomicBool,
        cancel_request_paused_once: AtomicBool,
        heartbeat_entered: Notify,
        heartbeat_release: Notify,
        non_cancel_terminal_entered: Notify,
        non_cancel_terminal_release: Notify,
        cancel_request_entered: Notify,
        cancel_request_release: Notify,
    }

    impl PausingHeartbeatStore {
        fn open(path: PathBuf) -> Self {
            Self {
                inner: Arc::new(
                    SqliteDetachedJobStore::open(path).expect("open detached job store"),
                ),
                heartbeat_pause_armed: AtomicBool::new(false),
                heartbeat_paused_once: AtomicBool::new(false),
                reject_cancel_settlement_renewal: AtomicBool::new(false),
                cancel_ack_stale_once: AtomicBool::new(false),
                cancel_ack_attempts: AtomicUsize::new(0),
                terminal_ack_cas_wins_remaining: AtomicUsize::new(0),
                terminal_ack_cas_wins: AtomicUsize::new(0),
                non_cancel_terminal_pause_armed: AtomicBool::new(false),
                non_cancel_terminal_paused_once: AtomicBool::new(false),
                cancel_request_pause_armed: AtomicBool::new(false),
                cancel_request_paused_once: AtomicBool::new(false),
                heartbeat_entered: Notify::new(),
                heartbeat_release: Notify::new(),
                non_cancel_terminal_entered: Notify::new(),
                non_cancel_terminal_release: Notify::new(),
                cancel_request_entered: Notify::new(),
                cancel_request_release: Notify::new(),
            }
        }

        fn arm_heartbeat_pause(&self) {
            self.heartbeat_pause_armed.store(true, Ordering::SeqCst);
        }

        fn reject_cancel_settlement_renewal(&self) {
            self.reject_cancel_settlement_renewal
                .store(true, Ordering::SeqCst);
        }

        fn inject_one_cancel_ack_stale_revision(&self) {
            self.cancel_ack_stale_once.store(true, Ordering::SeqCst);
        }

        fn arm_non_cancel_terminal_pause(&self) {
            self.non_cancel_terminal_pause_armed
                .store(true, Ordering::SeqCst);
        }

        fn inject_delivery_ack_cas_wins(&self, count: usize) {
            self.terminal_ack_cas_wins_remaining
                .store(count, Ordering::SeqCst);
        }

        fn arm_cancel_request_pause(&self) {
            self.cancel_request_pause_armed
                .store(true, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl DetachedJobStore for PausingHeartbeatStore {
        async fn insert_deduplicated(
            &self,
            job: StoredJob,
        ) -> Result<InsertJobOutcome, DetachedJobError> {
            self.inner.insert_deduplicated(job).await
        }

        async fn get(
            &self,
            job_id: &meerkat_jobs::JobId,
        ) -> Result<Option<StoredJob>, DetachedJobError> {
            self.inner.get(job_id).await
        }

        async fn compare_and_swap(
            &self,
            expected_revision: u64,
            replacement: StoredJob,
        ) -> Result<StoredJob, DetachedJobError> {
            let current = self.inner.get(&replacement.job_id).await?;
            let is_cancel_request = current.as_ref().is_some_and(|current| {
                !current.machine_state.cancel_requested
                    && replacement.machine_state.cancel_requested
                    && replacement.terminal_result.is_none()
            });
            if is_cancel_request
                && self.cancel_request_pause_armed.load(Ordering::SeqCst)
                && self
                    .cancel_request_paused_once
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                self.cancel_request_entered.notify_one();
                self.cancel_request_release.notified().await;
            }
            if replacement.terminal_result.is_some()
                && self
                    .terminal_ack_cas_wins_remaining
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                        remaining.checked_sub(1)
                    })
                    .is_ok()
            {
                let current = current
                    .as_ref()
                    .ok_or_else(|| DetachedJobError::NotFound(replacement.job_id.clone()))?;
                let delivery_sequence = current
                    .outbox
                    .iter()
                    .find(|entry| !entry.applied)
                    .map(|entry| entry.delivery_sequence)
                    .ok_or_else(|| {
                        DetachedJobError::Store(
                            "injected delivery acknowledgement has no pending outbox entry".into(),
                        )
                    })?;
                let applied = DetachedJobService::new(self.inner.clone())
                    .mark_delivery_applied(&replacement.job_id, delivery_sequence)
                    .await?;
                self.terminal_ack_cas_wins.fetch_add(1, Ordering::SeqCst);
                return Err(DetachedJobError::StaleRevision {
                    job_id: replacement.job_id,
                    expected: expected_revision,
                    actual: applied.revision,
                });
            }
            if replacement.terminal_result == Some(JobTerminalResult::Cancelled) {
                self.cancel_ack_attempts.fetch_add(1, Ordering::SeqCst);
                if self.cancel_ack_stale_once.swap(false, Ordering::SeqCst) {
                    return Err(DetachedJobError::StaleRevision {
                        job_id: replacement.job_id,
                        expected: expected_revision,
                        actual: expected_revision.saturating_add(1),
                    });
                }
            }
            if replacement.terminal_result.is_some()
                && replacement.terminal_result != Some(JobTerminalResult::Cancelled)
                && self.non_cancel_terminal_pause_armed.load(Ordering::SeqCst)
                && self
                    .non_cancel_terminal_paused_once
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                self.non_cancel_terminal_entered.notify_one();
                self.non_cancel_terminal_release.notified().await;
            }
            let is_heartbeat = current.as_ref().is_some_and(|current| {
                current.machine_state.heartbeat_at_ms != replacement.machine_state.heartbeat_at_ms
                    && current.machine_state.cancel_requested
                        == replacement.machine_state.cancel_requested
                    && current.outbox == replacement.outbox
                    && current.terminal_result == replacement.terminal_result
            });
            if is_heartbeat
                && current
                    .as_ref()
                    .is_some_and(|current| current.machine_state.cancel_requested)
                && self.reject_cancel_settlement_renewal.load(Ordering::SeqCst)
            {
                return Err(DetachedJobError::Store(
                    "injected cancellation settlement renewal failure".into(),
                ));
            }
            if is_heartbeat
                && self.heartbeat_pause_armed.load(Ordering::SeqCst)
                && self
                    .heartbeat_paused_once
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                self.heartbeat_entered.notify_one();
                self.heartbeat_release.notified().await;
            }
            self.inner
                .compare_and_swap(expected_revision, replacement)
                .await
        }

        async fn list_pending_outbox(
            &self,
            limit: usize,
        ) -> Result<Vec<JobOutboxEntry>, DetachedJobError> {
            self.inner.list_pending_outbox(limit).await
        }

        async fn list_for_origin(
            &self,
            realm_id: &str,
            origin_session_id: &SessionId,
            limit: usize,
        ) -> Result<Vec<StoredJob>, DetachedJobError> {
            self.inner
                .list_for_origin(realm_id, origin_session_id, limit)
                .await
        }

        async fn list_all(&self, limit: usize) -> Result<Vec<StoredJob>, DetachedJobError> {
            self.inner.list_all(limit).await
        }

        fn is_persistent(&self) -> bool {
            true
        }
    }

    fn durable_fixture_with_store_and_projector(
        temp: &TempDir,
        session_id: SessionId,
        job_store: Arc<dyn DetachedJobStore>,
        delivery_projector: Arc<dyn ShellJobDeliveryProjector>,
    ) -> (
        DurableShellJobRuntime,
        Arc<dyn DetachedJobStore>,
        ShellConfig,
    ) {
        let blob_store: Arc<dyn BlobStore> = Arc::new(FsBlobStore::new(temp.path().join("blobs")));
        let runtime = DurableShellJobRuntime::new(
            "test-realm",
            session_id,
            job_store.clone(),
            blob_store,
            delivery_projector,
        )
        .expect("durable shell runtime");
        let mut config = ShellConfig::with_project_root(temp.path().to_path_buf());
        config.shell = "sh".to_string();
        config.shell_path = Some(PathBuf::from("/bin/sh"));
        (runtime, job_store, config)
    }

    fn durable_fixture_with_projector(
        temp: &TempDir,
        session_id: SessionId,
        delivery_projector: Arc<dyn ShellJobDeliveryProjector>,
    ) -> (
        DurableShellJobRuntime,
        Arc<dyn DetachedJobStore>,
        ShellConfig,
    ) {
        let job_store: Arc<dyn DetachedJobStore> = Arc::new(
            SqliteDetachedJobStore::open(temp.path().join("jobs.db"))
                .expect("open detached job store"),
        );
        durable_fixture_with_store_and_projector(temp, session_id, job_store, delivery_projector)
    }

    fn durable_fixture(
        temp: &TempDir,
        session_id: SessionId,
    ) -> (
        DurableShellJobRuntime,
        Arc<dyn DetachedJobStore>,
        ShellConfig,
    ) {
        durable_fixture_with_projector(temp, session_id, Arc::new(NoopDeliveryProjector))
    }

    fn test_job_spec(session_id: SessionId) -> JobSpec {
        JobSpec::new(
            "test-realm",
            session_id,
            ExecutionIntentId::from_string("recovery-intent").expect("intent"),
            InteractionLineageId::from_string("recovery-lineage").expect("lineage"),
            ToolIdentity::new("shell", "v1").expect("tool"),
            RunnerIdentity::new("meerkat.shell", "v1").expect("runner"),
            RestartClass::NonResumable,
            CanonicalArgumentsHash::new("sha256:test").expect("hash"),
            JobSubmissionKey::new("recovery-submission").expect("submission"),
        )
        .with_runner_specification_ref(
            RunnerSpecificationRef::new("sha256:runner").expect("runner ref"),
        )
    }

    async fn test_monitor_job_spec(
        runtime: &DurableShellJobRuntime,
        session_id: SessionId,
        submission_key: &str,
    ) -> JobSpec {
        let runner_spec = ShellRunnerSpecification {
            command: "true".to_string(),
            working_dir: ".".to_string(),
            placement: ExecutionPlacement::new(
                None::<String>,
                None::<PathBuf>,
                std::iter::empty::<PathBuf>(),
                None::<String>,
            )
            .expect("placement"),
            timeout_secs: 5,
            monitor: Some(MonitorRunnerSpecification {
                protocol: MonitorOutputProtocol::FramedJsonl,
                limits: MonitorProtocolLimits::default(),
                delivery: meerkat_jobs::JobDeliveryKind::Record,
            }),
        };
        let encoded = serde_json::to_string(&runner_spec).expect("encode runner specification");
        let blob = runtime
            .blob_store
            .put_artifact(SHELL_RUNNER_MEDIA_TYPE, &encoded)
            .await
            .expect("persist runner specification");
        JobSpec::new(
            "test-realm",
            session_id,
            ExecutionIntentId::from_string(format!("intent:{submission_key}")).expect("intent"),
            InteractionLineageId::from_string(format!("lineage:{submission_key}"))
                .expect("lineage"),
            ToolIdentity::new("monitor_start", "v1").expect("tool"),
            RunnerIdentity::new("meerkat.monitor_script", "v1").expect("runner"),
            RestartClass::CheckpointResumable,
            CanonicalArgumentsHash::new(blob.blob_id.to_string()).expect("hash"),
            JobSubmissionKey::new(submission_key).expect("submission key"),
        )
        .with_runner_specification_ref(
            RunnerSpecificationRef::new(blob.blob_id.to_string()).expect("runner ref"),
        )
    }

    #[tokio::test]
    async fn reopen_classifies_loss_without_advancing_attempt_or_fence() {
        let temp = TempDir::new().expect("tempdir");
        let session_id = SessionId::new();
        let (runtime, job_store, config) = durable_fixture(&temp, session_id.clone());
        let service = DetachedJobService::new(job_store);
        let receipt = service
            .submit(test_job_spec(session_id))
            .await
            .expect("submit");
        let claimed = service
            .claim_attempt(
                &receipt.job_id,
                AttemptClaim::new(
                    WorkerId::new("worker-before-crash").expect("worker"),
                    10,
                    10_000,
                    RunnerHandleRef::new("pid-before-crash").expect("handle"),
                ),
            )
            .await
            .expect("claim");

        let manager = JobManager::new(config).with_durable_job_runtime(runtime);
        manager.ensure_recovered().await.expect("recover");

        let reopened = service
            .get(&receipt.job_id)
            .await
            .expect("read")
            .expect("job");
        assert_eq!(reopened.phase, JobPhase::WorkerLost);
        assert_eq!(reopened.attempt_count, 1);
        assert_eq!(
            reopened.current_attempt_id.as_ref(),
            Some(&claimed.attempt_id)
        );
        assert_eq!(reopened.current_fence, claimed.fence);
        assert_eq!(
            reopened.runner_handle.as_ref().map(RunnerHandleRef::as_str),
            Some("pid-before-crash")
        );
    }

    #[tokio::test]
    async fn early_reopen_does_not_permanently_suppress_later_loss_recovery() {
        let temp = TempDir::new().expect("tempdir");
        let session_id = SessionId::new();
        let (runtime, job_store, config) = durable_fixture(&temp, session_id.clone());
        let service = DetachedJobService::new(job_store);
        let receipt = service
            .submit(test_job_spec(session_id))
            .await
            .expect("submit");
        let claimed_at_ms = unix_time_ms();
        let lease_expires_at_ms = claimed_at_ms + 5_000;
        let claimed = service
            .claim_attempt(
                &receipt.job_id,
                AttemptClaim::new(
                    WorkerId::new("worker-before-early-reopen").expect("worker"),
                    claimed_at_ms,
                    lease_expires_at_ms,
                    RunnerHandleRef::new("pid-before-early-reopen").expect("handle"),
                ),
            )
            .await
            .expect("claim");
        let manager = JobManager::new(config).with_durable_job_runtime(runtime);

        manager.ensure_recovered().await.expect("early reopen");
        assert_eq!(
            service
                .get(&receipt.job_id)
                .await
                .expect("read")
                .expect("job")
                .phase,
            JobPhase::Running
        );
        tokio::time::sleep(Duration::from_millis(
            lease_expires_at_ms
                .saturating_sub(unix_time_ms())
                .saturating_add(25),
        ))
        .await;
        manager
            .ensure_recovered()
            .await
            .expect("later reconciliation");

        let recovered = service
            .get(&receipt.job_id)
            .await
            .expect("read")
            .expect("job");
        assert_eq!(recovered.phase, JobPhase::WorkerLost);
        assert_eq!(recovered.attempt_count, 1);
        assert_eq!(
            recovered.current_attempt_id.as_ref(),
            Some(&claimed.attempt_id)
        );
        assert_eq!(recovered.current_fence, claimed.fence);
    }

    #[tokio::test]
    async fn recovery_terminalizes_persisted_cancel_before_checkpoint_resume() {
        let temp = TempDir::new().expect("tempdir");
        let session_id = SessionId::new();
        let (runtime, job_store, config) = durable_fixture(&temp, session_id.clone());
        let service = DetachedJobService::new(job_store);
        let receipt = service
            .submit(
                test_monitor_job_spec(&runtime, session_id, "cancel-before-recovery-resume").await,
            )
            .await
            .expect("submit");
        let claimed = service
            .claim_attempt(
                &receipt.job_id,
                AttemptClaim::new(
                    WorkerId::new("worker-before-cancelled-recovery").expect("worker"),
                    1,
                    2,
                    RunnerHandleRef::new("monitor-before-cancelled-recovery").expect("handle"),
                ),
            )
            .await
            .expect("claim");
        service
            .record_checkpoint(
                &receipt.job_id,
                (&claimed).into(),
                meerkat_jobs::CheckpointRef::new("checkpoint:cancelled").expect("checkpoint"),
                2,
            )
            .await
            .expect("checkpoint");
        let requested = service
            .request_cancel(&receipt.job_id)
            .await
            .expect("persist cancellation before crash");
        assert_eq!(requested.phase, JobPhase::Running);
        assert!(requested.cancel_requested);

        let manager = JobManager::new(config).with_durable_job_runtime(runtime);
        manager
            .ensure_recovered()
            .await
            .expect("cancel intent wins recovery");

        let recovered = service
            .get(&receipt.job_id)
            .await
            .expect("read")
            .expect("job");
        assert_eq!(recovered.phase, JobPhase::Cancelled);
        assert_eq!(
            recovered.terminal_result,
            Some(JobTerminalResult::Cancelled)
        );
        assert_eq!(recovered.attempt_count, 1);
        assert_eq!(
            recovered.current_attempt_id.as_ref(),
            Some(&claimed.attempt_id)
        );
        assert_eq!(recovered.current_fence, claimed.fence);
    }

    #[tokio::test]
    async fn reopen_leaves_future_retry_committed_without_claiming_it() {
        let temp = TempDir::new().expect("tempdir");
        let session_id = SessionId::new();
        let (runtime, job_store, config) = durable_fixture(&temp, session_id.clone());
        let service = DetachedJobService::new(job_store.clone());
        let receipt = service
            .submit(test_monitor_job_spec(&runtime, session_id, "future-monitor-retry").await)
            .await
            .expect("submit");
        let first = service
            .claim_attempt(
                &receipt.job_id,
                AttemptClaim::new(
                    WorkerId::new("worker-before-future-retry").expect("worker"),
                    1,
                    2,
                    RunnerHandleRef::new("monitor-before-future-retry").expect("handle"),
                ),
            )
            .await
            .expect("claim");
        service
            .record_checkpoint(
                &receipt.job_id,
                (&first).into(),
                meerkat_jobs::CheckpointRef::new("checkpoint:v1").expect("checkpoint"),
                2,
            )
            .await
            .expect("checkpoint");
        service
            .observe_lease_expired(&receipt.job_id, (&first).into(), 3)
            .await
            .expect("observe loss");
        let retry_due_at_ms = unix_time_ms().saturating_add(60_000);
        service
            .schedule_retry(&receipt.job_id, retry_due_at_ms)
            .await
            .expect("schedule future retry");

        let manager = JobManager::new(config).with_durable_job_runtime(runtime);
        manager
            .ensure_recovered()
            .await
            .expect("future retry is valid dormant recovery state");

        let reopened = service
            .get(&receipt.job_id)
            .await
            .expect("read")
            .expect("job");
        assert_eq!(reopened.phase, JobPhase::RetryScheduled);
        assert_eq!(reopened.attempt_count, 1);
        assert_eq!(reopened.current_fence, first.fence);
        let stored = job_store
            .get(&receipt.job_id)
            .await
            .expect("read stored job")
            .expect("stored job");
        assert_eq!(stored.machine_state.retry_due_at_ms, Some(retry_due_at_ms));
    }

    #[tokio::test]
    async fn completed_shell_result_survives_manager_reconstruction() {
        let temp = TempDir::new().expect("tempdir");
        let session_id = SessionId::new();
        let (runtime, job_store, config) = durable_fixture(&temp, session_id.clone());
        let registry: Arc<dyn OpsLifecycleRegistry> = Arc::new(RuntimeOpsLifecycleRegistry::new());
        let manager = JobManager::new(config.clone())
            .bind_canonical_async_ops(session_id.clone(), registry)
            .with_durable_job_runtime(runtime.clone());
        let job_id = manager
            .spawn_job_for_call("printf durable-shell", None, 5, "tool-call-1")
            .await
            .expect("spawn");
        let replayed_job_id = manager
            .spawn_job_for_call("printf durable-shell", None, 5, "tool-call-1")
            .await
            .expect("replayed spawn");
        assert_eq!(replayed_job_id, job_id);

        let completed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let status = manager
                    .get_status(&job_id)
                    .await
                    .expect("status")
                    .expect("job");
                if matches!(status.status, JobStatus::Completed { .. }) {
                    break status;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("completion");
        assert!(matches!(
            completed.status,
            JobStatus::Completed { ref stdout, .. } if stdout == "durable-shell"
        ));

        drop(manager);
        let reopened = JobManager::new(config)
            .bind_canonical_async_ops(session_id, Arc::new(RuntimeOpsLifecycleRegistry::new()))
            .with_durable_job_runtime(runtime);
        let restored = reopened
            .get_status(&job_id)
            .await
            .expect("reopened status")
            .expect("reopened job");
        assert!(matches!(
            restored.status,
            JobStatus::Completed { ref stdout, .. } if stdout == "durable-shell"
        ));
        assert_eq!(
            job_store
                .list_for_origin("test-realm", &reopened.owner_bridge_session_id, 10)
                .await
                .expect("list")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn framed_monitor_notifies_checkpoints_and_continues_until_explicit_completion() {
        let temp = TempDir::new().expect("tempdir");
        let session_id = SessionId::new();
        let (runtime, job_store, config) = durable_fixture(&temp, session_id.clone());
        let service = DetachedJobService::new(job_store);
        let manager = JobManager::new(config)
            .bind_canonical_async_ops(session_id, Arc::new(RuntimeOpsLifecycleRegistry::new()))
            .with_durable_job_runtime(runtime);
        let command = concat!(
            "printf '%s\\n' ",
            "'{\"type\":\"notify\",\"key\":\"release:v1\",\"message\":\"v1\"}' ",
            "'{\"type\":\"checkpoint\",\"value\":\"etag:v1\"}'; ",
            "sleep 0.05; ",
            "printf '%s\\n' ",
            "'{\"type\":\"notify\",\"key\":\"release:v2\",\"message\":\"v2\"}' ",
            "'{\"type\":\"complete\"}'"
        );
        let public_job_id = manager
            .spawn_monitor_for_call(
                command,
                None,
                5,
                "monitor-call-1",
                MonitorStartOptions {
                    restart_class: RestartClass::CheckpointResumable,
                    ..MonitorStartOptions::default()
                },
            )
            .await
            .expect("spawn monitor");
        let job_id = meerkat_jobs::JobId::new(public_job_id.to_string()).expect("domain job id");

        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let snapshot = service.get(&job_id).await.expect("read").expect("job");
                if !snapshot.outbox.is_empty() && snapshot.checkpoint_ref.is_some() {
                    assert!(
                        snapshot.terminal_result.is_none(),
                        "a notification frame alone must not terminalize the monitor"
                    );
                    assert_eq!(
                        snapshot.checkpoint_ref.as_ref().map(|value| value.as_str()),
                        Some("etag:v1")
                    );
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("first notification");

        let completed = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let snapshot = service.get(&job_id).await.expect("read").expect("job");
                if snapshot.terminal_result.is_some() {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("monitor completion");
        assert_eq!(completed.attempt_count, 1);
        assert_eq!(completed.current_fence.get(), 1);
        assert_eq!(completed.outbox.len(), 3);
        assert!(matches!(
            &completed.outbox[0].payload,
            meerkat_jobs::JobOutboxPayload::Notification(notification)
                if notification.idempotency_key() == "release:v1"
        ));
        assert!(matches!(
            &completed.outbox[1].payload,
            meerkat_jobs::JobOutboxPayload::Notification(notification)
                if notification.idempotency_key() == "release:v2"
        ));
        assert!(matches!(
            &completed.outbox[2].payload,
            meerkat_jobs::JobOutboxPayload::Terminal(JobTerminalResult::Succeeded { .. })
        ));
    }

    #[tokio::test]
    async fn resolved_monitor_credentials_are_redacted_without_entering_durable_spec_or_output() {
        let temp = TempDir::new().expect("tempdir");
        let session_id = SessionId::new();
        let (runtime, job_store, mut config) = durable_fixture(&temp, session_id.clone());
        let secret = "credential-redaction-canary-7f4c";
        config
            .env_vars
            .insert("MONITOR_TEST_TOKEN".into(), secret.into());
        let service = DetachedJobService::new(job_store.clone());
        let blob_store = runtime.blob_store.clone();
        let manager = JobManager::new(config)
            .bind_canonical_async_ops(session_id, Arc::new(RuntimeOpsLifecycleRegistry::new()))
            .with_durable_job_runtime(runtime);
        let command = concat!(
            "printf '%s\\n' ",
            "\"{\\\"type\\\":\\\"notify\\\",\\\"key\\\":\\\"key:$MONITOR_TEST_TOKEN\\\",",
            "\\\"message\\\":\\\"body:$MONITOR_TEST_TOKEN\\\"}\" ",
            "\"diagnostic:$MONITOR_TEST_TOKEN\" ",
            "'{\"type\":\"complete\"}'; ",
            "printf 'stderr:%s\\n' \"$MONITOR_TEST_TOKEN\" >&2"
        );
        let public_job_id = manager
            .spawn_monitor_for_call(
                command,
                None,
                5,
                "monitor-redaction-call",
                MonitorStartOptions::default(),
            )
            .await
            .expect("spawn monitor");
        let job_id = meerkat_jobs::JobId::new(public_job_id.to_string()).expect("domain job id");
        let completion = tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let snapshot = service.get(&job_id).await.expect("read").expect("job");
                if snapshot.terminal_result.is_some() {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        let completed = match completion {
            Ok(snapshot) => snapshot,
            Err(_) => {
                let snapshot = service
                    .get(&job_id)
                    .await
                    .expect("read timed-out monitor")
                    .expect("timed-out monitor job");
                let active = manager
                    .active_attempts
                    .lock()
                    .await
                    .contains_key(&public_job_id);
                let stored = job_store
                    .get(&job_id)
                    .await
                    .expect("read timed-out stored monitor")
                    .expect("timed-out stored monitor job");
                panic!(
                    "monitor completion timed out; active={active}; snapshot={snapshot:?}; stored={stored:?}"
                );
            }
        };

        let stored = job_store
            .get(&job_id)
            .await
            .expect("stored job")
            .expect("job");
        assert!(
            stored
                .progress
                .as_ref()
                .is_none_or(|progress| !progress.detail.contains(secret))
        );
        let notification = stored
            .outbox
            .iter()
            .find_map(|entry| match &entry.payload {
                meerkat_jobs::JobOutboxPayload::Notification(notification) => Some(notification),
                meerkat_jobs::JobOutboxPayload::Terminal(_) => None,
            })
            .expect("notification");
        assert!(!notification.idempotency_key().contains(secret));
        assert!(!notification.title().contains(secret));
        assert!(!notification.body().contains(secret));
        assert!(notification.body().contains("[REDACTED]"));

        let runner_blob_id = BlobId::new(
            stored
                .spec
                .runner_specification_ref
                .as_ref()
                .expect("runner spec")
                .as_str(),
        );
        let runner_blob = blob_store.get(&runner_blob_id).await.expect("runner blob");
        assert!(!runner_blob.data.contains(secret));

        let result_ref = match completed.terminal_result.expect("terminal result") {
            JobTerminalResult::Succeeded {
                result_ref: Some(result_ref),
            } => result_ref,
            other => panic!("unexpected terminal result: {other:?}"),
        };
        let result_blob = blob_store
            .get(&BlobId::new(result_ref.as_str()))
            .await
            .expect("result blob");
        assert!(!result_blob.data.contains(secret));
        assert!(result_blob.data.contains("[REDACTED]"));
    }

    #[tokio::test]
    async fn resolved_monitor_credential_literal_is_rejected_before_job_or_spec_persistence() {
        let temp = TempDir::new().expect("tempdir");
        let session_id = SessionId::new();
        let (runtime, job_store, mut config) = durable_fixture(&temp, session_id.clone());
        let secret = "credential-persistence-canary-94d1";
        config
            .env_vars
            .insert("MONITOR_TEST_TOKEN".into(), secret.into());
        let manager = JobManager::new(config)
            .bind_canonical_async_ops(
                session_id.clone(),
                Arc::new(RuntimeOpsLifecycleRegistry::new()),
            )
            .with_durable_job_runtime(runtime);
        let error = manager
            .spawn_monitor_for_call(
                &format!("printf '%s\\n' '{secret}'"),
                None,
                5,
                "monitor-secret-literal",
                MonitorStartOptions::default(),
            )
            .await
            .expect_err("resolved literal must fail closed");
        assert!(error.to_string().contains("resolved environment value"));
        assert!(
            job_store
                .list_for_origin("test-realm", &session_id, 10)
                .await
                .expect("list")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn explicit_completion_cannot_deadlock_behind_post_complete_stdout_flood() {
        let temp = TempDir::new().expect("tempdir");
        let session_id = SessionId::new();
        let (runtime, job_store, config) = durable_fixture(&temp, session_id.clone());
        let service = DetachedJobService::new(job_store);
        let manager = JobManager::new(config)
            .bind_canonical_async_ops(session_id, Arc::new(RuntimeOpsLifecycleRegistry::new()))
            .with_durable_job_runtime(runtime);
        let public_job_id = manager
            .spawn_monitor_for_call(
                "printf '%s\\n' '{\"type\":\"complete\"}'; \
                 i=0; while test \"$i\" -lt 200; do printf 'after-complete-%s\\n' \"$i\"; \
                 i=$((i + 1)); done",
                None,
                5,
                "monitor-complete-flood",
                MonitorStartOptions::default(),
            )
            .await
            .expect("spawn monitor");
        let job_id = meerkat_jobs::JobId::new(public_job_id.to_string()).expect("domain job id");
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let snapshot = service.get(&job_id).await.expect("read").expect("job");
                if snapshot.terminal_result.is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("explicit completion must not deadlock");
    }

    #[tokio::test]
    async fn line_protocol_refuses_restart_classes_that_cannot_preserve_line_identity() {
        let temp = TempDir::new().expect("tempdir");
        let session_id = SessionId::new();
        let (runtime, job_store, config) = durable_fixture(&temp, session_id.clone());
        let manager = JobManager::new(config)
            .bind_canonical_async_ops(
                session_id.clone(),
                Arc::new(RuntimeOpsLifecycleRegistry::new()),
            )
            .with_durable_job_runtime(runtime);
        let error = manager
            .spawn_monitor_for_call(
                "printf 'notification\\n'",
                None,
                5,
                "line-restart",
                MonitorStartOptions {
                    protocol: MonitorOutputProtocol::Lines,
                    restart_class: RestartClass::Replayable,
                    ..MonitorStartOptions::default()
                },
            )
            .await
            .expect_err("line restart must fail closed");
        assert!(error.to_string().contains("non-resumable"));
        assert!(
            job_store
                .list_for_origin("test-realm", &session_id, 10)
                .await
                .expect("list")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn checkpoint_monitor_recovery_claims_once_after_loss_and_resumes_from_committed_state() {
        let temp = TempDir::new().expect("tempdir");
        let session_id = SessionId::new();
        let (runtime, job_store, config) = durable_fixture(&temp, session_id.clone());
        let service = DetachedJobService::new(job_store);
        let resolved_dir = config
            .default_working_dir_async()
            .await
            .expect("working dir");
        let placement = config
            .execution_placement_for_working_dir_async(&resolved_dir)
            .await
            .expect("placement");
        let command = concat!(
            "test \"$MEERKAT_MONITOR_CHECKPOINT\" = 'baseline:v1' || exit 9; ",
            "printf '%s\\n' ",
            "'{\"type\":\"progress\",\"cursor\":10,\"message\":\"resumed\"}' ",
            "'{\"type\":\"notify\",\"key\":\"observation:v2\",\"message\":\"v2\"}' ",
            "'{\"type\":\"checkpoint\",\"value\":\"baseline:v2\"}' ",
            "'{\"type\":\"notify\",\"key\":\"observation:v3\",\"message\":\"v3\"}' ",
            "'{\"type\":\"checkpoint\",\"value\":\"baseline:v3\"}' ",
            "'{\"type\":\"complete\"}'"
        );
        let runner_spec = ShellRunnerSpecification {
            command: command.to_string(),
            working_dir: resolved_dir.display().to_string(),
            placement,
            timeout_secs: 5,
            monitor: Some(MonitorRunnerSpecification {
                protocol: MonitorOutputProtocol::FramedJsonl,
                limits: MonitorProtocolLimits::default(),
                delivery: meerkat_jobs::JobDeliveryKind::Record,
            }),
        };
        let encoded = serde_json::to_string(&runner_spec).expect("encode");
        let blob = runtime
            .blob_store
            .put_artifact(SHELL_RUNNER_MEDIA_TYPE, &encoded)
            .await
            .expect("persist runner spec");
        let receipt = service
            .submit(
                JobSpec::new(
                    "test-realm",
                    session_id.clone(),
                    ExecutionIntentId::from_string("monitor-recovery-intent").expect("intent"),
                    InteractionLineageId::from_string("monitor-recovery-lineage").expect("lineage"),
                    ToolIdentity::new("monitor_start", "v1").expect("tool"),
                    RunnerIdentity::new("meerkat.monitor_script", "v1").expect("runner"),
                    RestartClass::CheckpointResumable,
                    CanonicalArgumentsHash::new(blob.blob_id.to_string()).expect("hash"),
                    JobSubmissionKey::new("monitor-recovery-submission").expect("key"),
                )
                .with_runner_specification_ref(
                    RunnerSpecificationRef::new(blob.blob_id.to_string()).expect("ref"),
                ),
            )
            .await
            .expect("submit");
        let first = service
            .claim_attempt(
                &receipt.job_id,
                AttemptClaim::new(
                    WorkerId::new("monitor-before-crash").expect("worker"),
                    1,
                    10,
                    RunnerHandleRef::new("inproc-monitor:lost").expect("handle"),
                ),
            )
            .await
            .expect("claim");
        service
            .record_checkpoint(
                &receipt.job_id,
                (&first).into(),
                meerkat_jobs::CheckpointRef::new("baseline:v1").expect("checkpoint"),
                2,
            )
            .await
            .expect("baseline");
        service
            .report_progress(
                &receipt.job_id,
                (&first).into(),
                JobProgress::new(9, "before restart").expect("progress"),
                3,
            )
            .await
            .expect("baseline progress");

        let manager = JobManager::new(config)
            .bind_canonical_async_ops(session_id, Arc::new(RuntimeOpsLifecycleRegistry::new()))
            .with_durable_job_runtime(runtime);
        manager.ensure_recovered().await.expect("recover and claim");
        let completed = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let snapshot = service
                    .get(&receipt.job_id)
                    .await
                    .expect("read")
                    .expect("job");
                if snapshot.terminal_result.is_some() {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("recovered monitor completion");
        assert_eq!(completed.attempt_count, 2);
        assert_eq!(completed.current_fence.get(), 2);
        assert_ne!(
            completed.current_attempt_id.as_ref(),
            Some(&first.attempt_id)
        );
        assert_eq!(
            completed
                .checkpoint_ref
                .as_ref()
                .map(|value| value.as_str()),
            Some("baseline:v3")
        );
        assert_eq!(
            completed.progress.as_ref().map(|progress| progress.cursor),
            Some(10)
        );
        let keys = completed
            .outbox
            .iter()
            .filter_map(|entry| match &entry.payload {
                meerkat_jobs::JobOutboxPayload::Notification(notification) => {
                    Some(notification.idempotency_key())
                }
                meerkat_jobs::JobOutboxPayload::Terminal(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["observation:v2", "observation:v3"]);
    }

    #[tokio::test]
    async fn settlement_converges_after_one_and_two_delivery_acknowledgement_cas_wins() {
        for acknowledgement_wins in 1..=2 {
            let temp = TempDir::new().expect("tempdir");
            let session_id = SessionId::new();
            let pausing_store = Arc::new(PausingHeartbeatStore::open(temp.path().join("jobs.db")));
            let (runtime, job_store, _config) = durable_fixture_with_store_and_projector(
                &temp,
                session_id.clone(),
                pausing_store.clone(),
                Arc::new(NoopDeliveryProjector),
            );
            let service = DetachedJobService::new(job_store);
            let receipt = service
                .submit(
                    test_monitor_job_spec(
                        &runtime,
                        session_id,
                        &format!("delivery-ack-race-{acknowledgement_wins}"),
                    )
                    .await,
                )
                .await
                .expect("submit");
            let claim = service
                .claim_attempt(
                    &receipt.job_id,
                    AttemptClaim::new(
                        WorkerId::new(format!("delivery-ack-worker-{acknowledgement_wins}"))
                            .expect("worker"),
                        1,
                        100,
                        RunnerHandleRef::new(format!(
                            "inproc-monitor:delivery-ack-{acknowledgement_wins}"
                        ))
                        .expect("handle"),
                    ),
                )
                .await
                .expect("claim");
            let write = AttemptWriteAuthority::from(&claim);
            for index in 0..acknowledgement_wins {
                let key = format!("delivery-ack:{acknowledgement_wins}:{index}");
                service
                    .emit_notification(
                        &receipt.job_id,
                        write.clone(),
                        2 + u64::try_from(index).expect("small index"),
                        JobNotification::new(
                            monitor_notification_id(&receipt.job_id, &key),
                            key,
                            "delivery acknowledgement race",
                            "pending",
                        )
                        .expect("notification"),
                    )
                    .await
                    .expect("seed pending delivery");
            }
            pausing_store.inject_delivery_ack_cas_wins(acknowledgement_wins);

            let settled = settle_attempt_after_containment(
                &service,
                &receipt.job_id,
                write,
                50,
                AttemptSettlement::Complete { result_ref: None },
            )
            .await
            .expect("settlement must converge after finite delivery acknowledgements");

            assert_eq!(
                settled.terminal_result,
                Some(JobTerminalResult::Succeeded { result_ref: None })
            );
            assert_eq!(
                pausing_store.terminal_ack_cas_wins.load(Ordering::SeqCst),
                acknowledgement_wins
            );
            assert_eq!(
                settled
                    .outbox
                    .iter()
                    .filter(|entry| matches!(
                        entry.payload,
                        meerkat_jobs::JobOutboxPayload::Notification(_)
                    ))
                    .filter(|entry| entry.applied)
                    .count(),
                acknowledgement_wins
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn settlement_converges_after_timed_out_renewal_commits_late() {
        let temp = TempDir::new().expect("tempdir");
        let session_id = SessionId::new();
        let pausing_store = Arc::new(PausingHeartbeatStore::open(temp.path().join("jobs.db")));
        let (runtime, job_store, _config) = durable_fixture_with_store_and_projector(
            &temp,
            session_id.clone(),
            pausing_store.clone(),
            Arc::new(NoopDeliveryProjector),
        );
        let service = DetachedJobService::new(job_store);
        let receipt = service
            .submit(test_monitor_job_spec(&runtime, session_id, "late-renewal").await)
            .await
            .expect("submit");
        let claim = service
            .claim_attempt(
                &receipt.job_id,
                AttemptClaim::new(
                    WorkerId::new("late-renewal-worker").expect("worker"),
                    1,
                    100,
                    RunnerHandleRef::new("inproc-monitor:late-renewal").expect("handle"),
                ),
            )
            .await
            .expect("claim");
        let write = AttemptWriteAuthority::from(&claim);

        pausing_store.arm_heartbeat_pause();
        let renewal = tokio::spawn(renew_monitor_settlement_lease_bounded(
            service.clone(),
            receipt.job_id.clone(),
            write.clone(),
            20,
            200,
        ));
        tokio::time::timeout(
            Duration::from_secs(5),
            pausing_store.heartbeat_entered.notified(),
        )
        .await
        .expect("renewal should enter the deterministic store gate");
        let renewal_error = tokio::time::timeout(Duration::from_secs(5), renewal)
            .await
            .expect("bounded renewal should return")
            .expect("renewal task")
            .expect_err("blocked store write must exceed the settlement timeout");
        assert!(renewal_error.contains("exceeded"));

        pausing_store.arm_non_cancel_terminal_pause();
        let settle_service = service.clone();
        let settle_job_id = receipt.job_id.clone();
        let settlement = tokio::spawn(async move {
            settle_attempt_after_containment(
                &settle_service,
                &settle_job_id,
                write,
                30,
                AttemptSettlement::Fail {
                    code: "shell_timeout",
                    detail_ref: None,
                },
            )
            .await
        });
        tokio::time::timeout(
            Duration::from_secs(5),
            pausing_store.non_cancel_terminal_entered.notified(),
        )
        .await
        .expect("terminal write should enter the deterministic store gate");

        pausing_store.heartbeat_release.notify_one();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let stored = pausing_store
                    .inner
                    .get(&receipt.job_id)
                    .await
                    .expect("read")
                    .expect("job");
                if stored.machine_state.heartbeat_at_ms == Some(20) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("timed-out renewal must be allowed to commit late");
        pausing_store.non_cancel_terminal_release.notify_one();

        let settled = tokio::time::timeout(Duration::from_secs(5), settlement)
            .await
            .expect("settlement should converge")
            .expect("settlement task")
            .expect("late renewal must only cause a retry");
        assert_eq!(settled.phase, JobPhase::Failed);
        assert!(matches!(
            settled.terminal_result,
            Some(JobTerminalResult::Failed { ref code, .. })
                if code.as_str() == "shell_timeout"
        ));
        assert_eq!(
            settled
                .outbox
                .iter()
                .filter(|entry| matches!(
                    entry.payload,
                    meerkat_jobs::JobOutboxPayload::Terminal(_)
                ))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn committed_cancel_dominates_complete_and_fail_in_every_public_projection() {
        for proposed_terminal in ["complete", "fail"] {
            let temp = TempDir::new().expect("tempdir");
            let session_id = SessionId::new();
            let projector = Arc::new(RecordingDeliveryProjector::default());
            let (runtime, job_store, config) =
                durable_fixture_with_projector(&temp, session_id.clone(), projector.clone());
            let service = DetachedJobService::new(job_store);
            let receipt = service
                .submit(
                    test_monitor_job_spec(
                        &runtime,
                        session_id.clone(),
                        &format!("cancel-vs-{proposed_terminal}"),
                    )
                    .await,
                )
                .await
                .expect("submit");
            let claim = service
                .claim_attempt(
                    &receipt.job_id,
                    AttemptClaim::new(
                        WorkerId::new(format!("cancel-vs-{proposed_terminal}-worker"))
                            .expect("worker"),
                        1,
                        100,
                        RunnerHandleRef::new(format!(
                            "inproc-monitor:cancel-vs-{proposed_terminal}"
                        ))
                        .expect("handle"),
                    ),
                )
                .await
                .expect("claim");
            service
                .request_cancel(&receipt.job_id)
                .await
                .expect("commit cancellation");
            let settlement = match proposed_terminal {
                "complete" => AttemptSettlement::Complete { result_ref: None },
                "fail" => AttemptSettlement::Fail {
                    code: "shell_wait_failed",
                    detail_ref: None,
                },
                _ => unreachable!(),
            };
            let terminal = settle_attempt_after_containment(
                &service,
                &receipt.job_id,
                AttemptWriteAuthority::from(&claim),
                50,
                settlement,
            )
            .await
            .expect("committed cancellation must settle as cancelled");
            assert_eq!(terminal.terminal_result, Some(JobTerminalResult::Cancelled));
            assert!(terminal.outbox.iter().any(|entry| matches!(
                entry.payload,
                meerkat_jobs::JobOutboxPayload::Terminal(JobTerminalResult::Cancelled)
            )));

            let registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
            let manager = JobManager::new(config)
                .bind_canonical_async_ops(session_id, registry.clone())
                .with_durable_job_runtime(runtime.clone());
            let public_job_id = JobId::from_string(receipt.job_id.as_str());
            let operation_id = manager
                .register_operation(&public_job_id)
                .expect("register operation");
            manager.projections.lock().await.insert(
                public_job_id.clone(),
                JobProjection {
                    view: BackgroundJob {
                        id: public_job_id.clone(),
                        command: proposed_terminal.to_string(),
                        working_dir: None,
                        placement: None,
                        timeout_secs: 5,
                        started_at_unix: unix_time_secs(),
                        status: JobStatus::Running {
                            started_at_unix: unix_time_secs(),
                        },
                    },
                },
            );
            let proposed_view = match proposed_terminal {
                "complete" => JobStatus::Completed {
                    exit_code: Some(0),
                    stdout: "optimistic success".to_string(),
                    stderr: String::new(),
                    duration_secs: 1.0,
                },
                "fail" => JobStatus::Failed {
                    error: "optimistic failure".to_string(),
                    duration_secs: 1.0,
                },
                _ => unreachable!(),
            };
            finalize_attempt_projection(
                &public_job_id,
                proposed_view,
                Ok(terminal),
                &runtime,
                &manager.projections,
                &*registry,
                &operation_id,
            )
            .await;

            let projected = manager
                .projections
                .lock()
                .await
                .get(&public_job_id)
                .expect("projection")
                .view
                .status
                .clone();
            assert!(matches!(projected, JobStatus::Cancelled { .. }));
            let operation = registry
                .snapshot(&operation_id)
                .expect("operation snapshot")
                .expect("operation");
            assert_eq!(
                operation.status,
                meerkat_core::ops_lifecycle::OperationStatus::Cancelled
            );
            assert_eq!(
                operation.public_result_class,
                meerkat_core::ops_lifecycle::OperationPublicResultClass::Cancelled
            );
            assert!(matches!(
                operation.terminal_outcome,
                Some(meerkat_core::ops_lifecycle::OperationTerminalOutcome::Cancelled { .. })
            ));
            assert_eq!(projector.project_calls.load(Ordering::SeqCst), 1);
            assert_eq!(projector.acknowledge_calls.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn committed_cancel_wins_after_non_cancel_settlement_has_started() {
        let temp = TempDir::new().expect("tempdir");
        let session_id = SessionId::new();
        let pausing_store = Arc::new(PausingHeartbeatStore::open(temp.path().join("jobs.db")));
        let (runtime, job_store, _config) = durable_fixture_with_store_and_projector(
            &temp,
            session_id.clone(),
            pausing_store.clone(),
            Arc::new(NoopDeliveryProjector),
        );
        let service = DetachedJobService::new(job_store);
        let receipt = service
            .submit(
                test_monitor_job_spec(&runtime, session_id, "late-cancel-settlement-race").await,
            )
            .await
            .expect("submit");
        let claim = service
            .claim_attempt(
                &receipt.job_id,
                AttemptClaim::new(
                    WorkerId::new("late-cancel-worker").expect("worker"),
                    1,
                    100,
                    RunnerHandleRef::new("inproc-monitor:late-cancel").expect("handle"),
                ),
            )
            .await
            .expect("claim");
        let write = AttemptWriteAuthority::from(&claim);

        pausing_store.arm_non_cancel_terminal_pause();
        let settle_service = service.clone();
        let settle_job_id = receipt.job_id.clone();
        let settle_write = write.clone();
        let settlement = tokio::spawn(async move {
            settle_attempt_after_containment(
                &settle_service,
                &settle_job_id,
                settle_write,
                2,
                AttemptSettlement::Complete { result_ref: None },
            )
            .await
        });
        tokio::time::timeout(
            Duration::from_secs(5),
            pausing_store.non_cancel_terminal_entered.notified(),
        )
        .await
        .expect("non-cancel settlement should enter the deterministic CAS gate");

        let requested = service
            .request_cancel(&receipt.job_id)
            .await
            .expect("commit cancellation during settlement");
        assert_eq!(requested.phase, JobPhase::Running);
        assert!(requested.cancel_requested);
        pausing_store.non_cancel_terminal_release.notify_one();

        let cancelled = tokio::time::timeout(Duration::from_secs(5), settlement)
            .await
            .expect("bounded settlement should finish")
            .expect("settlement task")
            .expect("committed cancellation should win");
        assert_eq!(cancelled.phase, JobPhase::Cancelled);
        assert_eq!(
            cancelled.terminal_result,
            Some(JobTerminalResult::Cancelled)
        );
        assert_eq!(cancelled.current_attempt_id, Some(claim.attempt_id));
        assert_eq!(cancelled.current_fence, claim.fence);
        assert_eq!(
            pausing_store.cancel_ack_attempts.load(Ordering::SeqCst),
            1,
            "one stale non-cancel CAS must reload directly into cancellation acknowledgement"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sqlite_recovered_monitor_cancel_preempts_racing_notification_projection() {
        let temp = TempDir::new().expect("tempdir");
        let session_id = SessionId::new();
        let projector = Arc::new(BlockingDeliveryProjector::default());
        let pausing_store = Arc::new(PausingHeartbeatStore::open(temp.path().join("jobs.db")));
        let (runtime, job_store, config) = durable_fixture_with_store_and_projector(
            &temp,
            session_id.clone(),
            pausing_store.clone(),
            projector.clone(),
        );
        let service = DetachedJobService::new(job_store);
        let resolved_dir = config
            .default_working_dir_async()
            .await
            .expect("working dir");
        let placement = config
            .execution_placement_for_working_dir_async(&resolved_dir)
            .await
            .expect("placement");
        let command = concat!(
            "test \"$MEERKAT_MONITOR_CHECKPOINT\" = 'cancel-baseline:v1' || exit 9; ",
            "printf '%s\\n' ",
            "'{\"type\":\"progress\",\"cursor\":10,\"message\":\"resumed for cancellation\"}' ",
            "'{\"type\":\"notify\",\"key\":\"cancel:resumed\",\"message\":\"ready\"}'; ",
            "while true; do sleep 1; done"
        );
        let runner_spec = ShellRunnerSpecification {
            command: command.to_string(),
            working_dir: resolved_dir.display().to_string(),
            placement,
            timeout_secs: 30,
            monitor: Some(MonitorRunnerSpecification {
                protocol: MonitorOutputProtocol::FramedJsonl,
                limits: MonitorProtocolLimits::default(),
                delivery: meerkat_jobs::JobDeliveryKind::Record,
            }),
        };
        let encoded = serde_json::to_string(&runner_spec).expect("encode");
        let blob = runtime
            .blob_store
            .put_artifact(SHELL_RUNNER_MEDIA_TYPE, &encoded)
            .await
            .expect("persist runner spec");
        let receipt = service
            .submit(
                JobSpec::new(
                    "test-realm",
                    session_id.clone(),
                    ExecutionIntentId::from_string("monitor-cancel-recovery-intent")
                        .expect("intent"),
                    InteractionLineageId::from_string("monitor-cancel-recovery-lineage")
                        .expect("lineage"),
                    ToolIdentity::new("monitor_start", "v1").expect("tool"),
                    RunnerIdentity::new("meerkat.monitor_script", "v1").expect("runner"),
                    RestartClass::CheckpointResumable,
                    CanonicalArgumentsHash::new(blob.blob_id.to_string()).expect("hash"),
                    JobSubmissionKey::new("monitor-cancel-recovery-submission").expect("key"),
                )
                .with_runner_specification_ref(
                    RunnerSpecificationRef::new(blob.blob_id.to_string()).expect("ref"),
                ),
            )
            .await
            .expect("submit");
        let first = service
            .claim_attempt(
                &receipt.job_id,
                AttemptClaim::new(
                    WorkerId::new("monitor-before-cancel-recovery").expect("worker"),
                    1,
                    10,
                    RunnerHandleRef::new("inproc-monitor:cancel-lost").expect("handle"),
                ),
            )
            .await
            .expect("claim");
        service
            .record_checkpoint(
                &receipt.job_id,
                (&first).into(),
                meerkat_jobs::CheckpointRef::new("cancel-baseline:v1").expect("checkpoint"),
                2,
            )
            .await
            .expect("baseline");

        let manager = JobManager::new(config)
            .bind_canonical_async_ops(session_id, Arc::new(RuntimeOpsLifecycleRegistry::new()))
            .with_durable_job_runtime(runtime);
        manager.ensure_recovered().await.expect("recover and claim");
        let resumed = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let snapshot = service
                    .get(&receipt.job_id)
                    .await
                    .expect("read")
                    .expect("job");
                if snapshot.phase == JobPhase::Running
                    && snapshot.attempt_count == 2
                    && snapshot
                        .progress
                        .as_ref()
                        .is_some_and(|progress| progress.cursor == 10)
                    && snapshot.outbox.iter().any(|entry| {
                        matches!(
                            &entry.payload,
                            meerkat_jobs::JobOutboxPayload::Notification(notification)
                                if notification.idempotency_key() == "cancel:resumed"
                        )
                    })
                {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("recovered monitor must resume");
        assert_ne!(resumed.current_attempt_id.as_ref(), Some(&first.attempt_id));
        let notification_sequence = resumed
            .outbox
            .iter()
            .find_map(|entry| {
                matches!(
                    &entry.payload,
                    meerkat_jobs::JobOutboxPayload::Notification(notification)
                        if notification.idempotency_key() == "cancel:resumed"
                )
                .then_some(entry.delivery_sequence)
            })
            .expect("resumed notification delivery");

        // Model the production delivery driver's race: it can acknowledge the
        // durable job outbox while a monitor-owned inline projector is still
        // blocked. The monitor must not call the projector for notifications
        // at all; otherwise the applied row below would make the public
        // snapshot look ready while cancellation remained trapped in that
        // blocking call.
        service
            .mark_delivery_applied(&receipt.job_id, notification_sequence)
            .await
            .expect("racing delivery driver applies notification");
        assert_eq!(
            projector.calls.load(Ordering::SeqCst),
            0,
            "notification projection must not execute on the monitor liveness task"
        );

        // Deterministically park the monitor in the exact production failure
        // seam: a SQLite-backed heartbeat CAS whose async trait future is
        // blocked while the cancellation writer remains able to commit on a
        // separate connection. Cancel/timeout must preempt this heartbeat
        // owner; merely moving notification projection out of the loop is
        // insufficient.
        pausing_store.arm_heartbeat_pause();
        tokio::time::timeout(
            Duration::from_secs(5),
            pausing_store.heartbeat_entered.notified(),
        )
        .await
        .expect("monitor heartbeat should enter the deterministic SQLite gate");
        pausing_store.reject_cancel_settlement_renewal();
        pausing_store.inject_one_cancel_ack_stale_revision();

        let public_job_id = JobId::from_string(receipt.job_id.as_str());
        assert_eq!(
            manager
                .cancel_job(&public_job_id)
                .await
                .expect("request cancellation"),
            CancelJobDisposition::CancellationRequested
        );
        let cancelled = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let snapshot = service
                    .get(&receipt.job_id)
                    .await
                    .expect("read")
                    .expect("job");
                if snapshot.phase == JobPhase::Cancelled {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("recovered monitor cancellation must terminalize");
        assert_eq!(cancelled.attempt_count, 2);
        assert_eq!(
            cancelled.terminal_result,
            Some(JobTerminalResult::Cancelled)
        );
        assert!(cancelled.outbox.iter().any(|entry| {
            matches!(
                &entry.payload,
                meerkat_jobs::JobOutboxPayload::Terminal(JobTerminalResult::Cancelled)
            )
        }));
        assert_eq!(
            pausing_store.cancel_ack_attempts.load(Ordering::SeqCst),
            2,
            "one preempted monitor mutation permits exactly one cancellation authority reload"
        );

        // Terminal delivery remains deliberately sequenced after the durable
        // cancellation commit. Release that projection so the attempt task can
        // finish and prove the blocking test double did not merely get leaked.
        tokio::time::timeout(Duration::from_secs(5), projector.entered.notified())
            .await
            .expect("terminal projector should run after cancellation commit");
        projector.release.notify_one();
        pausing_store.heartbeat_release.notify_one();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if !manager
                    .active_attempts
                    .lock()
                    .await
                    .contains_key(&public_job_id)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("attempt task should retire after terminal projection release");
        assert_eq!(
            projector.calls.load(Ordering::SeqCst),
            1,
            "only terminal projection belongs on the attempt settlement path"
        );
    }

    #[tokio::test]
    async fn running_shell_attempt_renews_its_committed_lease() {
        let temp = TempDir::new().expect("tempdir");
        let session_id = SessionId::new();
        let (runtime, job_store, config) = durable_fixture(&temp, session_id.clone());
        let service = DetachedJobService::new(job_store);
        let manager = JobManager::new(config)
            .bind_canonical_async_ops(session_id, Arc::new(RuntimeOpsLifecycleRegistry::new()))
            .with_durable_job_runtime(runtime);
        let public_job_id = manager
            .spawn_job_for_call("sleep 0.05", None, 5, "lease-heartbeat")
            .await
            .expect("spawn");
        let job_id =
            meerkat_jobs::JobId::new(public_job_id.to_string()).expect("domain job identity");

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let snapshot = service.get(&job_id).await.expect("read").expect("job");
                if snapshot.terminal_result.is_some() {
                    assert!(
                        snapshot.revision >= 3,
                        "submit, claim, lease renewal, and terminal commit must all be durable"
                    );
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("completion");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ordinary_shell_observes_cancellation_requested_by_another_manager() {
        let temp = TempDir::new().expect("tempdir");
        let session_id = SessionId::new();
        let projector = Arc::new(RecordingDeliveryProjector::default());
        let pausing_store = Arc::new(PausingHeartbeatStore::open(temp.path().join("jobs.db")));
        let (runtime, job_store, config) = durable_fixture_with_store_and_projector(
            &temp,
            session_id.clone(),
            pausing_store.clone(),
            projector.clone(),
        );
        let service = DetachedJobService::new(job_store);
        let owner_registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        let owner = JobManager::new(config.clone())
            .bind_canonical_async_ops(session_id.clone(), owner_registry.clone())
            .with_durable_job_runtime(runtime.clone());
        let remote = Arc::new(
            JobManager::new(config)
                .bind_canonical_async_ops(session_id, Arc::new(RuntimeOpsLifecycleRegistry::new()))
                .with_durable_job_runtime(runtime),
        );

        let public_job_id = owner
            .spawn_job_for_call("while :; do sleep 1; done", None, 5, "cross-manager-cancel")
            .await
            .expect("spawn");
        let job_id =
            meerkat_jobs::JobId::new(public_job_id.to_string()).expect("domain job identity");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if service
                    .get(&job_id)
                    .await
                    .expect("read")
                    .is_some_and(|snapshot| snapshot.phase == JobPhase::Running)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("attempt should be running");

        let baseline_heartbeat = pausing_store
            .inner
            .get(&job_id)
            .await
            .expect("read baseline")
            .expect("job")
            .machine_state
            .heartbeat_at_ms;
        pausing_store.arm_cancel_request_pause();
        let cancel_remote = Arc::clone(&remote);
        let cancel_job_id = public_job_id.clone();
        let cancellation =
            tokio::spawn(async move { cancel_remote.cancel_job(&cancel_job_id).await });
        tokio::time::timeout(
            Duration::from_secs(5),
            pausing_store.cancel_request_entered.notified(),
        )
        .await
        .expect("cancel request should enter the deterministic CAS gate");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let heartbeat = pausing_store
                    .inner
                    .get(&job_id)
                    .await
                    .expect("read racing heartbeat")
                    .expect("job")
                    .machine_state
                    .heartbeat_at_ms;
                if heartbeat != baseline_heartbeat {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("owner heartbeat must win the first cancellation CAS");
        pausing_store.cancel_request_release.notify_one();
        assert_eq!(
            cancellation
                .await
                .expect("cancellation task")
                .expect("remote cancellation request must converge"),
            CancelJobDisposition::CancellationRequested
        );
        let cancelled = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let snapshot = service.get(&job_id).await.expect("read").expect("job");
                if snapshot.phase == JobPhase::Cancelled {
                    break snapshot;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("owner heartbeat must observe durable cancellation");
        assert_eq!(
            cancelled.terminal_result,
            Some(JobTerminalResult::Cancelled)
        );
        assert!(cancelled.outbox.iter().any(|entry| matches!(
            entry.payload,
            meerkat_jobs::JobOutboxPayload::Terminal(JobTerminalResult::Cancelled)
        )));

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if !owner
                    .active_attempts
                    .lock()
                    .await
                    .contains_key(&public_job_id)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("owning attempt should finish projection");
        let view = owner
            .projections
            .lock()
            .await
            .get(&public_job_id)
            .expect("projection")
            .view
            .clone();
        assert!(matches!(view.status, JobStatus::Cancelled { .. }));
        let operation = owner_registry
            .snapshot(&operation_id_for_job(&public_job_id))
            .expect("operation snapshot")
            .expect("operation");
        assert_eq!(
            operation.status,
            meerkat_core::ops_lifecycle::OperationStatus::Cancelled
        );
        assert_eq!(
            operation.public_result_class,
            meerkat_core::ops_lifecycle::OperationPublicResultClass::Cancelled
        );
        assert!(matches!(
            operation.terminal_outcome,
            Some(meerkat_core::ops_lifecycle::OperationTerminalOutcome::Cancelled { .. })
        ));
        assert_eq!(projector.project_calls.load(Ordering::SeqCst), 1);
        assert_eq!(projector.acknowledge_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn replay_after_submit_before_claim_claims_the_committed_job_once() {
        let temp = TempDir::new().expect("tempdir");
        let session_id = SessionId::new();
        let (runtime, job_store, config) = durable_fixture(&temp, session_id.clone());
        let resolved_dir = config
            .default_working_dir_async()
            .await
            .expect("working dir");
        let placement = config
            .execution_placement_for_working_dir_async(&resolved_dir)
            .await
            .expect("placement");
        let runner_spec = ShellRunnerSpecification {
            command: "printf recovered-submit".to_string(),
            working_dir: resolved_dir.display().to_string(),
            placement,
            timeout_secs: 5,
            monitor: None,
        };
        let encoded = serde_json::to_string(&runner_spec).expect("encode");
        let blob = runtime
            .blob_store
            .put_artifact(SHELL_RUNNER_MEDIA_TYPE, &encoded)
            .await
            .expect("persist runner spec");
        let tool_call_id = "tool-call-before-receipt";
        let spec = JobSpec::new(
            "test-realm",
            session_id.clone(),
            ExecutionIntentId::from_string(format!("shell-call:{tool_call_id}")).expect("intent"),
            InteractionLineageId::from_string(format!("shell-session:{session_id}"))
                .expect("lineage"),
            ToolIdentity::new("shell", "v1").expect("tool"),
            RunnerIdentity::new("meerkat.shell", "v1").expect("runner"),
            RestartClass::NonResumable,
            CanonicalArgumentsHash::new(blob.blob_id.to_string()).expect("hash"),
            JobSubmissionKey::new(format!("shell:{session_id}:{tool_call_id}"))
                .expect("submission key"),
        )
        .with_runner_specification_ref(
            RunnerSpecificationRef::new(blob.blob_id.to_string()).expect("runner ref"),
        );
        let service = DetachedJobService::new(job_store.clone());
        let committed = service.submit(spec).await.expect("commit before crash");
        assert_eq!(
            service
                .get(&committed.job_id)
                .await
                .expect("read")
                .expect("job")
                .phase,
            JobPhase::Queued
        );

        let manager = JobManager::new(config)
            .bind_canonical_async_ops(
                session_id.clone(),
                Arc::new(RuntimeOpsLifecycleRegistry::new()),
            )
            .with_durable_job_runtime(runtime);
        let replayed = manager
            .spawn_job_for_call("printf recovered-submit", None, 5, tool_call_id)
            .await
            .expect("replay");
        assert_eq!(replayed.to_string(), committed.job_id.as_str());

        let snapshot = service
            .get(&committed.job_id)
            .await
            .expect("read")
            .expect("job");
        assert_ne!(snapshot.phase, JobPhase::Queued);
        assert_eq!(snapshot.attempt_count, 1);
        assert_eq!(
            job_store
                .list_for_origin("test-realm", &session_id, 10)
                .await
                .expect("list")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn completion_feed_waits_for_durable_delivery_and_recovers_afterward() {
        let temp = TempDir::new().expect("tempdir");
        let session_id = SessionId::new();
        let job_store: Arc<dyn DetachedJobStore> = Arc::new(
            SqliteDetachedJobStore::open(temp.path().join("jobs.db"))
                .expect("open detached job store"),
        );
        let blob_store: Arc<dyn BlobStore> = Arc::new(FsBlobStore::new(temp.path().join("blobs")));
        let projector = Arc::new(GateDeliveryProjector {
            available: AtomicBool::new(true),
        });
        let runtime = DurableShellJobRuntime::new(
            "test-realm",
            session_id.clone(),
            job_store.clone(),
            blob_store,
            projector.clone(),
        )
        .expect("runtime");
        let mut config = ShellConfig::with_project_root(temp.path().to_path_buf());
        config.shell = "sh".to_string();
        config.shell_path = Some(PathBuf::from("/bin/sh"));
        let registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        let manager = JobManager::new(config.clone())
            .bind_canonical_async_ops(session_id.clone(), registry.clone())
            .with_durable_job_runtime(runtime.clone());
        let job_id = manager
            .spawn_job_for_call(
                "sleep 0.2; printf delivery-gated",
                None,
                5,
                "delivery-gated-call",
            )
            .await
            .expect("spawn");
        projector.available.store(false, Ordering::SeqCst);
        let domain_job_id = meerkat_jobs::JobId::new(job_id.to_string()).expect("domain job id");
        let service = DetachedJobService::new(job_store);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if service
                    .get(&domain_job_id)
                    .await
                    .expect("read")
                    .is_some_and(|snapshot| snapshot.terminal_result.is_some())
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("terminal commit");
        let operation_id = operation_id_for_job(&job_id);
        assert!(
            !registry
                .snapshot(&operation_id)
                .expect("snapshot")
                .expect("operation")
                .terminal,
            "completion feed must not publish before durable runtime delivery"
        );

        projector.available.store(true, Ordering::SeqCst);
        let reopened = JobManager::new(config)
            .bind_canonical_async_ops(session_id, registry.clone())
            .with_durable_job_runtime(runtime);
        reopened
            .get_status(&job_id)
            .await
            .expect("reconcile")
            .expect("job");
        assert!(
            registry
                .snapshot(&operation_id)
                .expect("snapshot")
                .expect("operation")
                .terminal,
            "reopen must publish the already-committed terminal delivery"
        );
    }
}
