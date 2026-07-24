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
use super::process_lifecycle::{OwnedProcessGroup, join_output_bounded};
use super::types::{BackgroundJob, JobId, JobStatus, JobSummary, JobSummaryStatus};

const DEFAULT_MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_VISIBLE_ORIGIN_JOBS: usize = 10_000;
const LEASE_SETTLEMENT_MARGIN: Duration = Duration::from_secs(60);
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
            ) {
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
        if let Some(attempt) = self.active_attempts.lock().await.get(job_id) {
            attempt.cancel.notify_one();
            return Ok(CancelJobDisposition::CancellationRequested);
        }
        Err(shell_io(
            "cancel request is durable but no live shell attempt can acknowledge containment",
        ))
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
                                                match service
                                                    .emit_notification(
                                                        &job_id,
                                                        write.clone(),
                                                        unix_time_ms(),
                                                        notification,
                                                    )
                                                    .await
                                                {
                                                    Ok(_) => {
                                                        if let Err(error) = durable
                                                            .delivery_projector
                                                            .project_job(public_job_id.as_ref())
                                                            .await
                                                        {
                                                            warn!(
                                                                job_id = %public_job_id,
                                                                %error,
                                                                "monitor notification remains pending in the durable job outbox"
                                                            );
                                                        }
                                                    }
                                                    Err(error) => {
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
                        if let Err(error) = service
                            .renew_lease(
                                &job_id,
                                write.clone(),
                                heartbeat_at_ms,
                                lease_expires_at_ms,
                            )
                            .await
                        {
                            break MonitorWaitOutcome::WaitFailed(format!(
                                "monitor lease renewal failed: {error}"
                            ));
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
        loop {
            match process_group.terminate(&mut child).await {
                Ok(()) => break,
                Err(error) => {
                    warn!(job_id = %public_job_id, %error, "monitor containment unproven; retrying");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
        if let Some(task) = stdout_task {
            let _ = task.await;
        }
        let stderr = match stderr_task {
            Some(task) => join_output_bounded(task, "durable monitor stderr").await,
            None => Vec::new(),
        };
        let diagnostics = decoder.retained_diagnostics();
        let protocol_health = decoder.health();
        if protocol_health.diagnostic_bytes_dropped > 0 {
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
                        service
                            .complete_attempt(
                                &job_id,
                                write.clone(),
                                completed_at_ms,
                                Some(result_ref),
                            )
                            .await,
                    ),
                    Err(error) => (
                        JobStatus::Failed {
                            error: error.to_string(),
                            duration_secs,
                        },
                        fail_shell_attempt(
                            &service,
                            &job_id,
                            write.clone(),
                            completed_at_ms,
                            "monitor_result_persistence_failed",
                        )
                        .await,
                    ),
                }
            }
            MonitorWaitOutcome::Cancelled => (
                JobStatus::Cancelled { duration_secs },
                service
                    .acknowledge_cancel(&job_id, write.clone(), completed_at_ms)
                    .await,
            ),
            MonitorWaitOutcome::TimedOut => (
                JobStatus::Failed {
                    error: "monitor timed out".into(),
                    duration_secs,
                },
                fail_shell_attempt(
                    &service,
                    &job_id,
                    write.clone(),
                    completed_at_ms,
                    "monitor_timeout",
                )
                .await,
            ),
            MonitorWaitOutcome::Exited(exit_code) => (
                JobStatus::Failed {
                    error: format!("monitor exited with {exit_code:?}"),
                    duration_secs,
                },
                fail_shell_attempt(
                    &service,
                    &job_id,
                    write.clone(),
                    completed_at_ms,
                    "monitor_exit_nonzero",
                )
                .await,
            ),
            MonitorWaitOutcome::WaitFailed(error) | MonitorWaitOutcome::ProtocolFailed(error) => (
                JobStatus::Failed {
                    error,
                    duration_secs,
                },
                fail_shell_attempt(
                    &service,
                    &job_id,
                    write.clone(),
                    completed_at_ms,
                    "monitor_runner_failed",
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
                        if let Err(error) = service
                            .renew_lease(
                                &job_id,
                                write.clone(),
                                heartbeat_at_ms,
                                lease_expires_at_ms,
                            )
                            .await
                        {
                            break WaitOutcome::WaitFailed(format!(
                                "shell attempt lease renewal failed: {error}"
                            ));
                        }
                    }
                }
            }
        };
        loop {
            match process_group.terminate(&mut child).await {
                Ok(()) => break,
                Err(error) => {
                    warn!(job_id = %public_job_id, %error, "shell containment unproven; retrying");
                    let heartbeat_at_ms = unix_time_ms();
                    if let Err(renew_error) = service
                        .renew_lease(
                            &job_id,
                            write.clone(),
                            heartbeat_at_ms,
                            attempt_lease_expiry_ms(heartbeat_at_ms, timeout_secs),
                        )
                        .await
                    {
                        warn!(
                            job_id = %public_job_id,
                            %renew_error,
                            "shell containment continues after lease renewal was rejected"
                        );
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
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
        let completed_at_ms = unix_time_ms();
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
                        service
                            .complete_attempt(
                                &job_id,
                                write.clone(),
                                completed_at_ms,
                                Some(result_ref),
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
                            fail_shell_attempt(
                                &service,
                                &job_id,
                                write.clone(),
                                completed_at_ms,
                                "shell_result_persistence_failed",
                            )
                            .await,
                        )
                    }
                }
            }
            WaitOutcome::Cancelled => (
                JobStatus::Cancelled { duration_secs },
                service
                    .acknowledge_cancel(&job_id, write.clone(), completed_at_ms)
                    .await,
            ),
            WaitOutcome::TimedOut => (
                JobStatus::Failed {
                    error: "background job timed out".to_string(),
                    duration_secs,
                },
                fail_shell_attempt(
                    &service,
                    &job_id,
                    write.clone(),
                    completed_at_ms,
                    "shell_timeout",
                )
                .await,
            ),
            WaitOutcome::WaitFailed(error) => (
                JobStatus::Failed {
                    error,
                    duration_secs,
                },
                fail_shell_attempt(
                    &service,
                    &job_id,
                    write,
                    completed_at_ms,
                    "shell_wait_failed",
                )
                .await,
            ),
        };
        match terminal {
            Ok(snapshot) => {
                if let Some(projection) = projections.lock().await.get_mut(&public_job_id) {
                    projection.view.status = view_status.clone();
                }
                match durable
                    .delivery_projector
                    .project_job(public_job_id.as_ref())
                    .await
                {
                    Ok(()) => {
                        if let Err(error) =
                            project_legacy_operation(&*ops_registry, &operation_id, &view_status)
                        {
                            warn!(
                                job_id = %public_job_id,
                                %error,
                                "runtime delivery committed but completion-feed projection remains pending"
                            );
                        } else if let Err(error) = durable
                            .delivery_projector
                            .acknowledge_applied(public_job_id.as_ref())
                            .await
                        {
                            warn!(
                                job_id = %public_job_id,
                                %error,
                                "completion-feed projection committed but runtime delivery acknowledgement remains pending"
                            );
                        }
                    }
                    Err(error) => {
                        warn!(
                            job_id = %public_job_id,
                            %error,
                            "durable job delivery remains pending; refusing early completion-feed projection"
                        );
                    }
                }
                debug!(
                    job_id = %public_job_id,
                    phase = ?snapshot.phase,
                    "durable shell terminal committed"
                );
            }
            Err(error) => {
                warn!(
                    job_id = %public_job_id,
                    %error,
                    "durable shell terminal commit failed; refusing volatile terminal projection"
                );
            }
        }
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
    use meerkat_jobs::{DetachedJobService, SqliteDetachedJobStore};
    use meerkat_store::FsBlobStore;
    use std::sync::atomic::{AtomicBool, Ordering};
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

    fn durable_fixture(
        temp: &TempDir,
        session_id: SessionId,
    ) -> (
        DurableShellJobRuntime,
        Arc<dyn DetachedJobStore>,
        ShellConfig,
    ) {
        let job_store: Arc<dyn DetachedJobStore> = Arc::new(
            SqliteDetachedJobStore::open(temp.path().join("jobs.db"))
                .expect("open detached job store"),
        );
        let blob_store: Arc<dyn BlobStore> = Arc::new(FsBlobStore::new(temp.path().join("blobs")));
        let runtime = DurableShellJobRuntime::new(
            "test-realm",
            session_id,
            job_store.clone(),
            blob_store,
            Arc::new(NoopDeliveryProjector),
        )
        .expect("durable shell runtime");
        let mut config = ShellConfig::with_project_root(temp.path().to_path_buf());
        config.shell = "sh".to_string();
        config.shell_path = Some(PathBuf::from("/bin/sh"));
        (runtime, job_store, config)
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
        tokio::time::timeout(Duration::from_secs(15), async {
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
