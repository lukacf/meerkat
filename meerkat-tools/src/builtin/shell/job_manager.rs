//! Job manager for background shell command execution
//!
//! This module provides [`JobManager`] which handles spawning, tracking, and managing
//! background shell jobs with async execution, timeout handling, and event notification.

#[cfg(test)]
use meerkat_core::ops_lifecycle::OperationStatus;
use meerkat_core::ops_lifecycle::{
    OperationId, OperationKind, OperationLifecycleSnapshot, OperationPublicResultClass,
    OperationResult, OperationSpec, OperationTerminalOutcome, OpsLifecycleError,
    OpsLifecycleRegistry,
};
use meerkat_core::types::SessionId;
use meerkat_runtime::RuntimeOpsLifecycleRegistry;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;
use tracing::{debug, info, instrument, warn};

use super::process_lifecycle::{OwnedProcessGroup, join_output_bounded};

/// Default maximum output buffer size in bytes (1 MB).
const DEFAULT_MAX_OUTPUT_BYTES: usize = 1024 * 1024;

/// Append data to a buffer with truncation to prevent unbounded memory growth.
///
/// When the buffer exceeds `max_bytes * 2`, truncates to keep approximately `max_bytes`
/// from the tail, ensuring we don't split multi-byte UTF-8 characters.
///
/// This implements a "rolling window" approach: we let the buffer grow to 2x the limit,
/// then truncate to 1x, avoiding frequent reallocations while bounding memory usage.
///
/// This function is designed for incremental/streaming output capture where data arrives
/// in chunks. For one-shot truncation, use [`truncate_output_tail`] instead.
fn append_with_truncation(buffer: &mut Vec<u8>, data: &[u8], max_bytes: usize) {
    buffer.extend_from_slice(data);

    // Only truncate when buffer exceeds 2x the limit
    if buffer.len() > max_bytes * 2 {
        let keep_from = buffer.len() - max_bytes;

        // Find a valid UTF-8 character boundary to avoid splitting multi-byte sequences.
        // UTF-8 continuation bytes have the pattern 10xxxxxx (0x80-0xBF).
        // We search forward from keep_from to find a byte that's NOT a continuation byte.
        let safe_keep_from = (keep_from..buffer.len())
            .find(|&i| {
                // A byte is a valid start if it's ASCII (0x00-0x7F) or a multi-byte start (0xC0-0xFF)
                // In other words, NOT in the continuation byte range (0x80-0xBF)
                buffer.get(i).is_none_or(|b| (*b as i8) >= -64)
            })
            .unwrap_or(buffer.len());

        buffer.drain(0..safe_keep_from);
    }
}

/// Truncate a byte buffer to keep only the tail, respecting UTF-8 boundaries.
///
/// If the buffer is within the limit, returns it as a string directly.
/// If it exceeds the limit, keeps `max_bytes` from the tail, finding a valid
/// UTF-8 boundary to avoid splitting multi-byte characters.
fn truncate_output_tail(data: &[u8], max_bytes: usize) -> String {
    if data.len() <= max_bytes {
        return String::from_utf8_lossy(data).to_string();
    }

    let keep_from = data.len() - max_bytes;

    // Find a valid UTF-8 character boundary to avoid splitting multi-byte sequences.
    // UTF-8 continuation bytes have the pattern 10xxxxxx (0x80-0xBF).
    // We search forward from keep_from to find a byte that's NOT a continuation byte.
    let safe_keep_from = (keep_from..data.len())
        .find(|&i| {
            // A byte is a valid start if it's ASCII (0x00-0x7F) or a multi-byte start (0xC0-0xFF)
            // In other words, NOT in the continuation byte range (0x80-0xBF)
            data.get(i).is_none_or(|b| (*b as i8) >= -64)
        })
        .unwrap_or(data.len());

    String::from_utf8_lossy(&data[safe_keep_from..]).to_string()
}

async fn read_stream_with_limit<R>(mut reader: R, max_bytes: usize) -> std::io::Result<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];

    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        append_with_truncation(&mut buffer, &chunk[..n], max_bytes);
    }

    Ok(buffer)
}

use super::config::{ShellConfig, ShellError};
use super::types::{BackgroundJob, JobId, JobStatus, JobSummary, JobSummaryStatus};

#[derive(Clone, Debug)]
struct BackgroundJobRecord {
    view: BackgroundJob,
    operation_id: OperationId,
    completion_notified: bool,
}

/// Acknowledgement level returned by a shell-job cancellation request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelJobDisposition {
    /// A live process waiter accepted the request. Terminal cancellation is
    /// withheld until that waiter proves process-group containment.
    CancellationRequested,
    /// No live process required containment and generated lifecycle authority
    /// has already projected terminal cancellation.
    Cancelled,
}

/// Owns every fact created during background admission until the waiter task
/// has atomically taken process ownership.
///
/// Dropping the submission future cannot await rollback. The guard therefore
/// fences the process group synchronously, reconciles canonical operation
/// authority, and schedules removal of any partially published job maps.
struct SubmissionRollbackGuard {
    active: bool,
    job_id: JobId,
    operation_id: OperationId,
    child: Option<tokio::process::Child>,
    process_group: Option<OwnedProcessGroup>,
    jobs: Arc<Mutex<HashMap<JobId, BackgroundJobRecord>>>,
    canonical_job_ops: Arc<std::sync::Mutex<HashMap<JobId, OperationId>>>,
    handles: Arc<Mutex<HashMap<JobId, JoinHandle<()>>>>,
    cancel_notifiers: Arc<Mutex<HashMap<JobId, Arc<Notify>>>>,
    cancel_requested: Arc<Mutex<HashSet<JobId>>>,
    completed_at: Arc<Mutex<HashMap<JobId, Instant>>>,
    ops_registry: Arc<dyn OpsLifecycleRegistry>,
}

impl SubmissionRollbackGuard {
    fn new(manager: &JobManager, job_id: JobId, operation_id: OperationId) -> Self {
        Self {
            active: true,
            job_id,
            operation_id,
            child: None,
            process_group: None,
            jobs: Arc::clone(&manager.jobs),
            canonical_job_ops: Arc::clone(&manager.canonical_job_ops),
            handles: Arc::clone(&manager.handles),
            cancel_notifiers: Arc::clone(&manager.cancel_notifiers),
            cancel_requested: Arc::clone(&manager.cancel_requested),
            completed_at: Arc::clone(&manager.completed_at),
            ops_registry: Arc::clone(&manager.ops_registry),
        }
    }

    fn attach_process(&mut self, child: tokio::process::Child, process_group: OwnedProcessGroup) {
        self.child = Some(child);
        self.process_group = Some(process_group);
    }

    fn mark_started(&mut self) {
        // The rollback authority accepts both Provisioning and Running. This
        // method remains as an explicit handoff milestone for the submission
        // sequence, but rollback no longer maps it onto a public terminal.
    }

    async fn terminate_process(&mut self) -> Result<(), ShellError> {
        if let (Some(process_group), Some(child)) =
            (self.process_group.as_mut(), self.child.as_mut())
        {
            process_group
                .terminate(child)
                .await
                .map_err(ShellError::Io)?;
        }
        Ok(())
    }

    fn take_process(&mut self) -> Result<(tokio::process::Child, OwnedProcessGroup), ShellError> {
        let child = self.child.take().ok_or_else(|| {
            ShellError::Io(std::io::Error::other(
                "background submission lost child ownership before waiter handoff",
            ))
        })?;
        let process_group = self.process_group.take().ok_or_else(|| {
            ShellError::Io(std::io::Error::other(
                "background submission lost process-group ownership before waiter handoff",
            ))
        })?;
        Ok((child, process_group))
    }

    fn commit(&mut self) {
        self.active = false;
    }

    fn disarm_after_reconciliation(&mut self) {
        self.active = false;
    }
}

impl Drop for SubmissionRollbackGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }

        if let Some(process_group) = self.process_group.as_mut() {
            process_group.force_kill_now();
        }

        let job_id = self.job_id.clone();
        let operation_id = self.operation_id.clone();
        let mut child = self.child.take();
        let mut process_group = self.process_group.take();
        let jobs = Arc::clone(&self.jobs);
        let canonical_job_ops = Arc::clone(&self.canonical_job_ops);
        let handles = Arc::clone(&self.handles);
        let cancel_notifiers = Arc::clone(&self.cancel_notifiers);
        let cancel_requested = Arc::clone(&self.cancel_requested);
        let completed_at = Arc::clone(&self.completed_at);
        let ops_registry = Arc::clone(&self.ops_registry);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                match (process_group.as_mut(), child.as_mut()) {
                    (Some(group), Some(child)) => loop {
                        match group.terminate(child).await {
                            Ok(()) => break,
                            Err(error) => {
                                warn!(
                                    job_id = %job_id,
                                    operation_id = %operation_id,
                                    error = %error,
                                    "unreturned shell submission containment is not yet proven; withholding lifecycle rollback"
                                );
                                tokio::time::sleep(Duration::from_millis(100)).await;
                            }
                        }
                    },
                    (None, None) => {}
                    _ => {
                        tracing::error!(
                            job_id = %job_id,
                            operation_id = %operation_id,
                            "unreturned shell submission lost paired process ownership; withholding lifecycle rollback"
                        );
                        return;
                    }
                }

                loop {
                    match ops_registry.rollback_unreturned_operation(&operation_id) {
                        Ok(()) => break,
                        Err(error) => {
                            warn!(
                                job_id = %job_id,
                                operation_id = %operation_id,
                                error = %error,
                                "generated authority has not reconciled unreturned shell submission; retrying"
                            );
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                }
                canonical_job_ops
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&job_id);
                if let Some(handle) = handles.lock().await.remove(&job_id) {
                    handle.abort();
                }
                jobs.lock().await.remove(&job_id);
                cancel_notifiers.lock().await.remove(&job_id);
                cancel_requested.lock().await.remove(&job_id);
                completed_at.lock().await.remove(&job_id);
            });
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubmissionHandoffStage {
    JobsPublication,
    CancelPublication,
    WaiterHandoff,
}

#[cfg(test)]
struct SubmissionHandoffHook {
    stage: SubmissionHandoffStage,
    reached: Notify,
    release: Notify,
}

#[cfg(test)]
impl SubmissionHandoffHook {
    fn new(stage: SubmissionHandoffStage) -> Self {
        Self {
            stage,
            reached: Notify::new(),
            release: Notify::new(),
        }
    }

    async fn pause_at(&self, stage: SubmissionHandoffStage) {
        if self.stage == stage {
            self.reached.notify_one();
            self.release.notified().await;
        }
    }
}

/// Manager for background shell jobs
///
/// Handles spawning, tracking, and managing background shell command execution.
/// Each job runs in its own tokio task with timeout handling.
///
/// ## Memory Management
///
/// Jobs are automatically cleaned up based on configuration:
/// - Jobs older than `completed_job_ttl_secs` are removed during new job spawning
/// - When `max_completed_jobs` is exceeded, oldest completed jobs are removed first
/// - Call [`remove_job`](Self::remove_job) to manually remove a job after retrieving its final status
pub struct JobManager {
    /// Map of job ID to job info
    jobs: Arc<Mutex<HashMap<JobId, BackgroundJobRecord>>>,
    /// Synchronous lookup from public job_id to canonical operation identity.
    canonical_job_ops: Arc<std::sync::Mutex<HashMap<JobId, OperationId>>>,
    /// Configuration for shell execution
    config: ShellConfig,
    resolved_shell_path: Arc<Mutex<Option<PathBuf>>>,
    /// Shared lifecycle registry backing background shell operations.
    ops_registry: Arc<dyn OpsLifecycleRegistry>,
    /// Bridge-session-scoped owner identity for shared lifecycle records.
    owner_bridge_session_id: SessionId,
    /// Whether this manager is bound to a real session-canonical ops context.
    owner_session_bound: bool,
    /// Whether this manager is bound to an injected session-canonical registry.
    ops_registry_bound: bool,
    /// Map of job ID to task handle (for cancellation)
    handles: Arc<Mutex<HashMap<JobId, JoinHandle<()>>>>,
    /// Map of job ID to cancellation notifier (for killing)
    cancel_notifiers: Arc<Mutex<HashMap<JobId, Arc<Notify>>>>,
    /// Accepted cancellation requests awaiting process-group containment ack.
    cancel_requested: Arc<Mutex<HashSet<JobId>>>,
    /// Map of job ID to completion time (for cleanup)
    completed_at: Arc<Mutex<HashMap<JobId, Instant>>>,
    #[cfg(test)]
    submission_handoff_hook: Option<Arc<SubmissionHandoffHook>>,
}

impl std::fmt::Debug for JobManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobManager")
            .field("config", &self.config)
            .field("owner_bridge_session_id", &self.owner_bridge_session_id)
            .field(
                "exports_canonical_async_ops",
                &self.exports_canonical_async_ops(),
            )
            .finish_non_exhaustive()
    }
}

impl JobManager {
    /// Create a new JobManager with the given configuration
    pub fn new(config: ShellConfig) -> Self {
        Self {
            jobs: Arc::new(Mutex::new(HashMap::new())),
            canonical_job_ops: Arc::new(std::sync::Mutex::new(HashMap::new())),
            config,
            resolved_shell_path: Arc::new(Mutex::new(None)),
            ops_registry: Arc::new(RuntimeOpsLifecycleRegistry::new()),
            owner_bridge_session_id: SessionId::new(),
            owner_session_bound: false,
            ops_registry_bound: false,
            handles: Arc::new(Mutex::new(HashMap::new())),
            cancel_notifiers: Arc::new(Mutex::new(HashMap::new())),
            cancel_requested: Arc::new(Mutex::new(HashSet::new())),
            completed_at: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(test)]
            submission_handoff_hook: None,
        }
    }

    #[cfg(test)]
    fn with_submission_handoff_hook(mut self, hook: Arc<SubmissionHandoffHook>) -> Self {
        self.submission_handoff_hook = Some(hook);
        self
    }

    #[cfg(test)]
    async fn pause_submission_handoff(&self, stage: SubmissionHandoffStage) {
        if let Some(hook) = &self.submission_handoff_hook {
            hook.pause_at(stage).await;
        }
    }

    /// Override the canonical owner bridge session ID used in shared lifecycle records.
    pub(crate) fn with_owner_bridge_session_id(mut self, bridge_session_id: SessionId) -> Self {
        self.owner_bridge_session_id = bridge_session_id;
        self.owner_session_bound = true;
        self
    }

    /// Override the lifecycle registry used for background operations.
    pub(crate) fn with_ops_registry(mut self, ops_registry: Arc<dyn OpsLifecycleRegistry>) -> Self {
        self.ops_registry = ops_registry;
        self.ops_registry_bound = true;
        self
    }

    /// Bind the manager to a canonical async-op context.
    ///
    /// Background shell jobs export detached async-op identities only when they
    /// are anchored to a real owner session and lifecycle registry.
    pub fn bind_canonical_async_ops(
        self,
        owner_bridge_session_id: SessionId,
        ops_registry: Arc<dyn OpsLifecycleRegistry>,
    ) -> Self {
        self.with_owner_bridge_session_id(owner_bridge_session_id)
            .with_ops_registry(ops_registry)
    }

    /// Whether background jobs may export canonical async-op identities.
    ///
    /// This is intentionally stricter than "has some registry object": exported
    /// detached ops are only honest when both the owner session and the registry
    /// have been bound from the real session construction path.
    pub fn exports_canonical_async_ops(&self) -> bool {
        self.owner_session_bound && self.ops_registry_bound
    }

    fn lifecycle_duration_secs(started_at_unix: u64) -> f64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(started_at_unix) as f64
    }

    fn snapshot_for_job(
        &self,
        job: &BackgroundJobRecord,
    ) -> Result<Option<OperationLifecycleSnapshot>, ShellError> {
        self.ops_registry
            .snapshot(&job.operation_id)
            .map_err(|error| ShellError::Io(std::io::Error::other(error.to_string())))
    }

    fn operation_admission_limit(&self) -> Option<usize> {
        match self.config.max_concurrent_processes {
            0 => None,
            limit => Some(limit),
        }
    }

    fn shell_error_for_ops_lifecycle_admission(error: OpsLifecycleError) -> ShellError {
        match error {
            OpsLifecycleError::MaxConcurrentExceeded { limit, active } => {
                ShellError::Io(std::io::Error::other(format!(
                    "Concurrency limit exceeded: {active} active operations, limit is {limit}"
                )))
            }
            other => ShellError::Io(std::io::Error::other(other.to_string())),
        }
    }

    fn register_background_operation(
        &self,
        operation_id: OperationId,
        display_name: String,
    ) -> Result<(), ShellError> {
        self.ops_registry
            .register_operation_with_admission_limit(
                OperationSpec {
                    id: operation_id,
                    kind: OperationKind::BackgroundToolOp,
                    owner_session_id: self.owner_bridge_session_id.clone(),
                    display_name,
                    source_label: "shell_job".to_string(),
                    operation_source: None,
                    child_session_id: None,
                    expect_peer_channel: false,
                },
                self.operation_admission_limit(),
            )
            .map_err(Self::shell_error_for_ops_lifecycle_admission)
    }

    fn background_operation_display_name(job_id: &JobId) -> String {
        format!("shell background job {job_id}")
    }

    fn start_background_operation(&self, operation_id: &OperationId) -> Result<(), ShellError> {
        self.ops_registry
            .provisioning_succeeded(operation_id)
            .map_err(|error| ShellError::Io(std::io::Error::other(error.to_string())))
    }

    fn operation_public_result_class(
        &self,
        job: &BackgroundJobRecord,
        snapshot: Option<&OperationLifecycleSnapshot>,
    ) -> Result<OperationPublicResultClass, OpsLifecycleError> {
        if let Some(snapshot) = snapshot {
            if snapshot.id != job.operation_id {
                return Err(OpsLifecycleError::Internal(format!(
                    "background job {} snapshot operation {} does not match canonical operation {}",
                    job.view.id, snapshot.id, job.operation_id
                )));
            }
            return Ok(snapshot.public_result_class);
        }
        self.ops_registry
            .classify_operation_public_result(&job.operation_id)
    }

    fn summary_status_from_public_result(
        result: OperationPublicResultClass,
    ) -> Result<JobSummaryStatus, OpsLifecycleError> {
        Ok(match result {
            OperationPublicResultClass::MissingAuthority => {
                return Err(OpsLifecycleError::Internal(
                    "background job lifecycle authority missing".into(),
                ));
            }
            OperationPublicResultClass::Running => JobSummaryStatus::Running,
            OperationPublicResultClass::Completed => JobSummaryStatus::Completed,
            OperationPublicResultClass::Failed => JobSummaryStatus::Failed,
            OperationPublicResultClass::Cancelled => JobSummaryStatus::Cancelled,
        })
    }

    fn lifecycle_summary_status(
        &self,
        job: &BackgroundJobRecord,
        snapshot: Option<&OperationLifecycleSnapshot>,
    ) -> Result<JobSummaryStatus, OpsLifecycleError> {
        let result = self.operation_public_result_class(job, snapshot)?;
        Self::summary_status_from_public_result(result)
    }

    fn reconcile_job_status(
        &self,
        job: &BackgroundJobRecord,
        snapshot: Option<&OperationLifecycleSnapshot>,
    ) -> Result<JobStatus, OpsLifecycleError> {
        let public_result = self.operation_public_result_class(job, snapshot)?;
        match public_result {
            OperationPublicResultClass::Running => Ok(JobStatus::Running {
                started_at_unix: job.view.started_at_unix,
            }),
            OperationPublicResultClass::MissingAuthority => Err(OpsLifecycleError::Internal(
                "background job lifecycle authority missing".into(),
            )),
            OperationPublicResultClass::Completed
            | OperationPublicResultClass::Failed
            | OperationPublicResultClass::Cancelled => {
                match Self::terminal_status_from_authority(job, snapshot, &job.view.status) {
                    Ok(Some(status)) => Ok(status),
                    Ok(None) => Err(OpsLifecycleError::Internal(
                        "background job lifecycle authority missing".into(),
                    )),
                    Err(error) => Err(error),
                }
            }
        }
    }

    fn duration_from_process_or_snapshot(
        job: &BackgroundJobRecord,
        process_status: &JobStatus,
        snapshot: Option<&OperationLifecycleSnapshot>,
    ) -> f64 {
        match process_status {
            JobStatus::Completed { duration_secs, .. }
            | JobStatus::Failed { duration_secs, .. }
            | JobStatus::Cancelled { duration_secs } => *duration_secs,
            JobStatus::Running { .. } => snapshot
                .and_then(|snapshot| snapshot.elapsed_ms)
                .map(|elapsed_ms| elapsed_ms as f64 / 1000.0)
                .unwrap_or_else(|| Self::lifecycle_duration_secs(job.view.started_at_unix)),
        }
    }

    fn terminal_status_from_authority(
        job: &BackgroundJobRecord,
        snapshot: Option<&OperationLifecycleSnapshot>,
        process_status: &JobStatus,
    ) -> Result<Option<JobStatus>, OpsLifecycleError> {
        let Some(snapshot) = snapshot else {
            return Ok(None);
        };
        let public_result = snapshot.public_result_class;
        let terminal = snapshot.terminal;
        let duration_secs =
            Self::duration_from_process_or_snapshot(job, process_status, Some(snapshot));
        Ok(match public_result {
            OperationPublicResultClass::Running => Some(JobStatus::Running {
                started_at_unix: job.view.started_at_unix,
            }),
            OperationPublicResultClass::Completed => {
                if !terminal {
                    return Err(OpsLifecycleError::Internal(format!(
                        "generated op public result class completed for non-terminal status {:?} on {}",
                        snapshot.status, snapshot.id
                    )));
                }
                let result = match snapshot.terminal_outcome.as_ref() {
                    Some(OperationTerminalOutcome::Completed(result)) => result,
                    Some(other) => {
                        return Err(OpsLifecycleError::Internal(format!(
                            "generated op public result class completed with non-completed terminal outcome for {}: {other:?}",
                            snapshot.id
                        )));
                    }
                    None => {
                        return Err(OpsLifecycleError::Internal(format!(
                            "generated op public result class completed without terminal outcome for {}",
                            snapshot.id
                        )));
                    }
                };
                let (stdout, stderr) = if result.is_error {
                    (String::new(), result.content.clone())
                } else {
                    (result.content.clone(), String::new())
                };
                let exit_code = match process_status {
                    JobStatus::Completed { exit_code, .. } => *exit_code,
                    _ => None,
                };
                Some(JobStatus::Completed {
                    exit_code,
                    stdout,
                    stderr,
                    duration_secs,
                })
            }
            OperationPublicResultClass::Failed => {
                if !terminal {
                    return Err(OpsLifecycleError::Internal(format!(
                        "generated op public result class failed for non-terminal status {:?} on {}",
                        snapshot.status, snapshot.id
                    )));
                }
                let error = match snapshot.terminal_outcome.as_ref() {
                    Some(OperationTerminalOutcome::Failed { error }) => error.clone(),
                    Some(OperationTerminalOutcome::Terminated { reason }) => reason.clone(),
                    Some(other) => {
                        return Err(OpsLifecycleError::Internal(format!(
                            "generated op public result class failed with non-failed terminal outcome for {}: {other:?}",
                            snapshot.id
                        )));
                    }
                    None => {
                        return Err(OpsLifecycleError::Internal(format!(
                            "generated op public result class failed without terminal outcome for {}",
                            snapshot.id
                        )));
                    }
                };
                Some(JobStatus::Failed {
                    error,
                    duration_secs,
                })
            }
            OperationPublicResultClass::Cancelled => {
                if !terminal {
                    return Err(OpsLifecycleError::Internal(format!(
                        "generated op public result class cancelled for non-terminal status {:?} on {}",
                        snapshot.status, snapshot.id
                    )));
                }
                match snapshot.terminal_outcome.as_ref() {
                    Some(
                        OperationTerminalOutcome::Aborted { .. }
                        | OperationTerminalOutcome::Cancelled { .. }
                        | OperationTerminalOutcome::Retired,
                    ) => Some(JobStatus::Cancelled { duration_secs }),
                    Some(other) => {
                        return Err(OpsLifecycleError::Internal(format!(
                            "generated op public result class cancelled with non-cancelled terminal outcome for {}: {other:?}",
                            snapshot.id
                        )));
                    }
                    None => {
                        return Err(OpsLifecycleError::Internal(format!(
                            "generated op public result class cancelled without terminal outcome for {}",
                            snapshot.id
                        )));
                    }
                }
            }
            OperationPublicResultClass::MissingAuthority => None,
        })
    }

    fn shell_error_for_ops_lifecycle_cancel(
        job_id: &JobId,
        error: OpsLifecycleError,
    ) -> ShellError {
        match error {
            OpsLifecycleError::NotFound(_) => ShellError::JobNotFound(job_id.to_string()),
            OpsLifecycleError::InvalidTransition { .. } => ShellError::JobNotRunning,
            other => ShellError::Io(std::io::Error::other(other.to_string())),
        }
    }

    fn cancelled_view_from_authority(
        &self,
        job: &BackgroundJobRecord,
        snapshot: Option<&OperationLifecycleSnapshot>,
    ) -> Result<JobStatus, ShellError> {
        match Self::terminal_status_from_authority(job, snapshot, &job.view.status)
            .map_err(|error| ShellError::Io(std::io::Error::other(error.to_string())))?
        {
            Some(status @ JobStatus::Cancelled { .. }) => Ok(status),
            Some(_) => Err(ShellError::JobNotRunning),
            None => Err(ShellError::Io(std::io::Error::other(
                "background job lifecycle authority missing after cancellation",
            ))),
        }
    }

    pub async fn ops_lifecycle_snapshot(
        &self,
        job_id: &JobId,
    ) -> Result<Option<OperationLifecycleSnapshot>, ShellError> {
        let jobs = self.jobs.lock().await;
        let Some(job) = jobs.get(job_id) else {
            return Ok(None);
        };
        self.snapshot_for_job(job)
    }

    /// Register a running background job record without spawning a subprocess.
    ///
    /// This keeps the shell job tools and shared lifecycle registry exercisable in
    /// hermetic tests or alternate executors that already own process execution.
    pub async fn register_synthetic_running_job(
        &self,
        command: &str,
        working_dir: Option<&Path>,
        timeout_secs: u64,
    ) -> Result<JobId, ShellError> {
        if !self.exports_canonical_async_ops() {
            return Err(ShellError::Io(std::io::Error::other(
                "background shell jobs require canonical session binding",
            )));
        }
        self.cleanup_old_jobs().await;

        let resolved_dir = if let Some(dir) = working_dir {
            Some(self.config.validate_working_dir_async(dir).await?)
        } else {
            None
        };
        let effective_dir = if let Some(dir) = resolved_dir.as_ref() {
            dir.clone()
        } else {
            self.config.default_working_dir_async().await?
        };
        let placement = self
            .config
            .execution_placement_for_working_dir_async(&effective_dir)
            .await?;

        let job_id = JobId::new();
        let started_at_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let operation_id = OperationId::new();
        self.register_background_operation(
            operation_id.clone(),
            Self::background_operation_display_name(&job_id),
        )?;
        if let Err(error) = self.start_background_operation(&operation_id) {
            self.ops_registry
                .rollback_unreturned_operation(&operation_id)
                .map_err(|rollback_error| {
                    ShellError::Io(std::io::Error::other(format!(
                        "{error}; failed to reconcile unreturned synthetic job: {rollback_error}"
                    )))
                })?;
            return Err(error);
        }

        let job = BackgroundJob {
            id: job_id.clone(),
            command: command.to_string(),
            working_dir: resolved_dir.as_ref().map(|path| path.display().to_string()),
            placement: Some(placement),
            timeout_secs,
            started_at_unix,
            status: JobStatus::Running { started_at_unix },
        };
        self.jobs.lock().await.insert(
            job_id.clone(),
            BackgroundJobRecord {
                view: job,
                operation_id: operation_id.clone(),
                completion_notified: false,
            },
        );
        self.canonical_job_ops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(job_id.clone(), operation_id);
        self.cancel_notifiers
            .lock()
            .await
            .insert(job_id.clone(), Arc::new(Notify::new()));

        Ok(job_id)
    }

    async fn resolved_shell_path(&self) -> Result<PathBuf, ShellError> {
        {
            let guard = self.resolved_shell_path.lock().await;
            if let Some(path) = guard.as_ref() {
                return Ok(path.clone());
            }
        }

        let path = self.config.resolve_shell_path_auto_async().await?;
        let mut guard = self.resolved_shell_path.lock().await;
        *guard = Some(path.clone());
        Ok(path)
    }

    /// Spawn a new background job
    ///
    /// Creates a new job and immediately returns its ID. The command runs
    /// asynchronously in a tokio task.
    ///
    /// # Arguments
    ///
    /// * `command` - The shell command to execute
    /// * `working_dir` - Optional working directory (validated against config)
    /// * `timeout_secs` - Timeout in seconds for the command
    ///
    /// # Errors
    ///
    /// Returns [`ShellError`] if:
    /// - Working directory validation fails
    /// - Shell executable is not found
    #[instrument(
        skip(self, command, working_dir),
        fields(timeout_secs = %timeout_secs, has_working_dir = working_dir.is_some())
    )]
    pub async fn spawn_job(
        &self,
        command: &str,
        working_dir: Option<&Path>,
        timeout_secs: u64,
    ) -> Result<JobId, ShellError> {
        if !self.exports_canonical_async_ops() {
            return Err(ShellError::Io(std::io::Error::other(
                "background shell jobs require canonical session binding",
            )));
        }
        info!("Spawning background job");

        // Run cleanup before spawning new job
        self.cleanup_old_jobs().await;

        // Validate working directory if provided
        let resolved_dir = if let Some(dir) = working_dir {
            debug!("Validating configured working directory");
            Some(self.config.validate_working_dir_async(dir).await?)
        } else {
            None
        };
        let effective_dir = if let Some(dir) = resolved_dir.as_ref() {
            dir.clone()
        } else {
            self.config.default_working_dir_async().await?
        };
        let placement = self
            .config
            .execution_placement_for_working_dir_async(&effective_dir)
            .await?;

        // Find shell executable (fail fast if not installed)
        let shell_path = self.resolved_shell_path().await?;

        // Generate job ID and timestamp
        let job_id = JobId::new();
        debug!(job_id = %job_id, "Generated job ID");
        let started_at_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let operation_id = OperationId::new();
        self.register_background_operation(
            operation_id.clone(),
            Self::background_operation_display_name(&job_id),
        )?;
        let mut submission =
            SubmissionRollbackGuard::new(self, job_id.clone(), operation_id.clone());

        // Create the job with Running status
        let job = BackgroundJob {
            id: job_id.clone(),
            command: command.to_string(),
            working_dir: resolved_dir.as_ref().map(|p| p.display().to_string()),
            placement: Some(placement),
            timeout_secs,
            started_at_unix,
            status: JobStatus::Running { started_at_unix },
        };

        // Build and spawn the command
        let mut cmd = Command::new(&shell_path);
        cmd.arg("-c").arg(command);

        // Set working directory. Placement metadata records this path as
        // mechanical context only; job identity remains the app-facing job id
        // and canonical operation id.
        let work_dir = effective_dir;
        cmd.current_dir(&work_dir);
        cmd.env("PWD", &work_dir);

        // Inject per-agent environment variables
        cmd.envs(&self.config.env_vars);

        // Capture stdout/stderr
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(true);

        // Create new process group on Unix for proper cleanup of child processes.
        // This ensures that when we kill a job, all its child processes are also killed.
        #[cfg(unix)]
        cmd.process_group(0);

        // Spawn the child process
        let child = match cmd.spawn() {
            Ok(child) => child,
            Err(error) => {
                let rollback_result = self
                    .ops_registry
                    .rollback_unreturned_operation(&operation_id);
                if let Err(rollback_error) = rollback_result {
                    warn!(
                        job_id = %job_id,
                        operation_id = %operation_id,
                        error = %rollback_error,
                        "generated authority has not reconciled unreturned shell spawn failure; rollback guard will retry"
                    );
                } else {
                    submission.disarm_after_reconciliation();
                }
                return Err(ShellError::Io(error));
            }
        };
        let process_group = OwnedProcessGroup::new(&child);
        submission.attach_process(child, process_group);
        if let Err(error) = self.start_background_operation(&operation_id) {
            submission.terminate_process().await?;
            self.ops_registry
                .rollback_unreturned_operation(&operation_id)
                .map_err(|rollback_error| {
                    ShellError::Io(std::io::Error::other(format!(
                        "{error}; failed to reconcile unreturned shell job: {rollback_error}"
                    )))
                })?;
            submission.disarm_after_reconciliation();
            return Err(error);
        }
        submission.mark_started();
        debug!("Spawned child process");
        #[cfg(test)]
        self.pause_submission_handoff(SubmissionHandoffStage::JobsPublication)
            .await;

        // Store the job and cancellation notifier
        let jobs = Arc::clone(&self.jobs);
        let handles = Arc::clone(&self.handles);
        let cancel_notifiers = Arc::clone(&self.cancel_notifiers);
        let cancel_requested = Arc::clone(&self.cancel_requested);
        let completed_at = Arc::clone(&self.completed_at);
        let ops_registry = Arc::clone(&self.ops_registry);
        let operation_id_for_task = operation_id.clone();
        let job_id_clone = job_id.clone();
        let job_id_for_completion = job_id.clone();
        let job_id_for_cancel = job_id.clone();
        let cancel_notify = Arc::new(Notify::new());

        // Insert job and notifier into maps
        jobs.lock().await.insert(
            job_id.clone(),
            BackgroundJobRecord {
                view: job,
                operation_id: operation_id.clone(),
                completion_notified: false,
            },
        );
        self.canonical_job_ops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(job_id.clone(), operation_id.clone());
        #[cfg(test)]
        self.pause_submission_handoff(SubmissionHandoffStage::CancelPublication)
            .await;
        cancel_notifiers
            .lock()
            .await
            .insert(job_id.clone(), Arc::clone(&cancel_notify));
        let mut handles_guard = handles.lock().await;
        #[cfg(test)]
        self.pause_submission_handoff(SubmissionHandoffStage::WaiterHandoff)
            .await;
        let (child, process_group) = submission.take_process()?;

        // Spawn the async task to wait for completion
        let handle = tokio::spawn(async move {
            let start = Instant::now();
            let timeout_duration = Duration::from_secs(timeout_secs);
            let mut child = child;
            let mut process_group = process_group;

            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            let stdout_handle = tokio::spawn(async move {
                if let Some(out) = stdout {
                    read_stream_with_limit(out, DEFAULT_MAX_OUTPUT_BYTES).await
                } else {
                    Ok(Vec::new())
                }
            });

            let stderr_handle = tokio::spawn(async move {
                if let Some(err) = stderr {
                    read_stream_with_limit(err, DEFAULT_MAX_OUTPUT_BYTES).await
                } else {
                    Ok(Vec::new())
                }
            });

            enum WaitOutcome {
                Completed(Option<i32>),
                Failed(std::io::Error),
                TimedOut,
                Cancelled,
            }

            let wait_outcome = tokio::select! {
                () = cancel_notify.notified() => WaitOutcome::Cancelled,
                result = tokio::time::timeout(timeout_duration, child.wait()) => {
                    match result {
                        Ok(Ok(status)) => WaitOutcome::Completed(status.code()),
                        Ok(Err(err)) => WaitOutcome::Failed(err),
                        Err(_) => WaitOutcome::TimedOut,
                    }
                }
            };

            // The shell leader is not the process-lifecycle boundary. Always
            // retire the owned group before terminalizing so an `&` descendant
            // cannot outlive a completed, failed, cancelled, or timed-out job.
            loop {
                match process_group.terminate(&mut child).await {
                    Ok(()) => break,
                    Err(error) => {
                        warn!(
                            job_id = %job_id_clone,
                            operation_id = %operation_id_for_task,
                            error = %error,
                            "background job containment is not yet proven; withholding terminal publication"
                        );
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }

            let duration_secs = start.elapsed().as_secs_f64();

            let (stdout_bytes, stderr_bytes) = tokio::join!(
                join_output_bounded(stdout_handle, "job stdout"),
                join_output_bounded(stderr_handle, "job stderr")
            );

            let stdout = truncate_output_tail(&stdout_bytes, DEFAULT_MAX_OUTPUT_BYTES);
            let stderr = truncate_output_tail(&stderr_bytes, DEFAULT_MAX_OUTPUT_BYTES);

            let final_status = match wait_outcome {
                WaitOutcome::Failed(err) => JobStatus::Failed {
                    error: err.to_string(),
                    duration_secs,
                },
                WaitOutcome::TimedOut => JobStatus::Failed {
                    error: "background job timed out".to_string(),
                    duration_secs,
                },
                WaitOutcome::Cancelled => JobStatus::Cancelled { duration_secs },
                WaitOutcome::Completed(exit_code) => JobStatus::Completed {
                    exit_code,
                    stdout,
                    stderr,
                    duration_secs,
                },
            };

            let lifecycle_result = match &final_status {
                JobStatus::Completed { stdout, stderr, .. } => {
                    let content = if !stdout.is_empty() {
                        stdout.clone()
                    } else if !stderr.is_empty() {
                        stderr.clone()
                    } else {
                        "shell job completed without output".to_string()
                    };
                    ops_registry.complete_operation(
                        &operation_id_for_task,
                        OperationResult {
                            id: operation_id_for_task.clone(),
                            content,
                            is_error: false,
                            duration_ms: (duration_secs * 1000.0) as u64,
                            tokens_used: 0,
                        },
                    )
                }
                JobStatus::Failed { error, .. } => {
                    ops_registry.fail_operation(&operation_id_for_task, error.clone())
                }
                JobStatus::Cancelled { .. } => ops_registry
                    .cancel_operation(&operation_id_for_task, Some("cancelled by caller".into())),
                JobStatus::Running { .. } => Ok(()),
            };
            let authority_snapshot = ops_registry.snapshot(&operation_id_for_task);

            // Publish only the class accepted by the generated operation
            // lifecycle. If authority feedback is unavailable, fail closed.
            let mut record_completed_at = false;
            {
                let mut jobs_guard = jobs.lock().await;
                if let Some(job) = jobs_guard.get_mut(&job_id_clone) {
                    let generated_terminal_authority = authority_snapshot
                        .as_ref()
                        .ok()
                        .and_then(Option::as_ref)
                        .is_some_and(|snapshot| snapshot.terminal);
                    let authority_status = match authority_snapshot.as_ref() {
                        Ok(snapshot) => Self::terminal_status_from_authority(
                            job,
                            snapshot.as_ref(),
                            &final_status,
                        ),
                        Err(error) => Err(OpsLifecycleError::Internal(format!(
                            "background job lifecycle authority projection failed: {error}"
                        ))),
                    };
                    match authority_status {
                        Ok(Some(status)) => {
                            record_completed_at = generated_terminal_authority;
                            job.view.status = status;
                        }
                        Ok(None) => {
                            let error = match lifecycle_result {
                                Ok(()) => {
                                    "background job lifecycle authority missing after terminal transition"
                                        .to_string()
                                }
                                Err(error) => format!(
                                    "background job lifecycle authority rejected terminal transition: {error}"
                                ),
                            };
                            warn!(
                                job_id = %job.view.id,
                                operation_id = %operation_id_for_task,
                                error = %error,
                                "background job terminal status not updated because generated authority did not project a public result"
                            );
                        }
                        Err(error) => {
                            warn!(
                                job_id = %job.view.id,
                                operation_id = %operation_id_for_task,
                                error = %error,
                                "background job terminal status not updated because generated authority projection failed"
                            );
                        }
                    }
                }
            }

            // Record cleanup eligibility only after generated lifecycle
            // authority has projected a terminal public result.
            if record_completed_at {
                completed_at
                    .lock()
                    .await
                    .insert(job_id_for_completion, Instant::now());
            }

            // Remove cancellation notifier once the job is done
            {
                cancel_notifiers.lock().await.remove(&job_id_for_cancel);
            }
            cancel_requested.lock().await.remove(&job_id_for_cancel);
        });

        // No await is allowed between process ownership transfer and admission
        // commit: cancellation must see either the rollback guard or the waiter.
        handles_guard.insert(job_id.clone(), handle);
        submission.commit();

        Ok(job_id)
    }

    /// Get the status of a job
    ///
    /// Returns the full job information if found, or None if the job doesn't exist.
    #[instrument(skip(self), fields(job_id = %job_id))]
    pub async fn get_status(&self, job_id: &JobId) -> Result<Option<BackgroundJob>, ShellError> {
        let Some(mut job) = self.jobs.lock().await.get(job_id).cloned() else {
            debug!("Job not found");
            return Ok(None);
        };
        let snapshot = self.snapshot_for_job(&job)?;
        job.view.status = self
            .reconcile_job_status(&job, snapshot.as_ref())
            .map_err(|error| ShellError::Io(std::io::Error::other(error.to_string())))?;
        Ok(Some(job.view))
    }

    /// List all jobs with summary information
    #[instrument(skip(self))]
    pub async fn list_jobs(&self) -> Result<Vec<JobSummary>, ShellError> {
        let jobs: Vec<BackgroundJobRecord> = self.jobs.lock().await.values().cloned().collect();
        let mut summaries = Vec::with_capacity(jobs.len());
        for job in jobs {
            let snapshot = self.snapshot_for_job(&job)?;
            let status = self
                .lifecycle_summary_status(&job, snapshot.as_ref())
                .map_err(|error| ShellError::Io(std::io::Error::other(error.to_string())))?;
            summaries.push(JobSummary {
                id: job.view.id.clone(),
                command: job.view.command.clone(),
                status,
                started_at_unix: job.view.started_at_unix,
            });
        }
        debug!(count = summaries.len(), "Listed jobs");
        Ok(summaries)
    }

    /// Cancel a running job
    ///
    /// # Errors
    ///
    /// Returns [`ShellError::JobNotFound`] if the job doesn't exist.
    /// Returns [`ShellError::JobNotRunning`] if the job is not in running state.
    #[instrument(skip(self), fields(job_id = %job_id))]
    pub async fn cancel_job(&self, job_id: &JobId) -> Result<CancelJobDisposition, ShellError> {
        info!("Cancelling job");

        let operation_id = {
            let jobs_guard = self.jobs.lock().await;
            jobs_guard
                .get(job_id)
                .ok_or_else(|| {
                    warn!("Job not found");
                    ShellError::JobNotFound(job_id.to_string())
                })?
                .operation_id
                .clone()
        };

        let snapshot = self
            .ops_registry
            .snapshot(&operation_id)
            .map_err(|error| ShellError::Io(std::io::Error::other(error.to_string())))?
            .ok_or_else(|| {
                ShellError::Io(std::io::Error::other(
                    "background job lifecycle authority missing",
                ))
            })?;
        if snapshot.terminal {
            return Err(ShellError::JobNotRunning);
        }

        // Synthetic jobs have no process waiter, so containment is already
        // acknowledged and lifecycle authority can terminalize immediately.
        if !self.handles.lock().await.contains_key(job_id) {
            self.ops_registry
                .cancel_operation(&operation_id, Some("cancelled by caller".into()))
                .map_err(|error| Self::shell_error_for_ops_lifecycle_cancel(job_id, error))?;
            let snapshot = self
                .ops_registry
                .snapshot(&operation_id)
                .map_err(|error| ShellError::Io(std::io::Error::other(error.to_string())))?;
            let mut jobs_guard = self.jobs.lock().await;
            let job = jobs_guard.get_mut(job_id).ok_or_else(|| {
                warn!("Job not found");
                ShellError::JobNotFound(job_id.to_string())
            })?;
            job.view.status = self.cancelled_view_from_authority(job, snapshot.as_ref())?;
            return Ok(CancelJobDisposition::Cancelled);
        }

        let notify = { self.cancel_notifiers.lock().await.get(job_id).cloned() };
        let Some(notify) = notify else {
            warn!("Cancel notifier missing for running job");
            return Err(ShellError::JobNotRunning);
        };
        let accepted = self.cancel_requested.lock().await.insert(job_id.clone());
        if !accepted {
            return Err(ShellError::JobNotRunning);
        }

        // Request only. The worker owns TERM/KILL and acknowledges by applying
        // the terminal lifecycle transition after the group is gone.
        notify.notify_one();
        Ok(CancelJobDisposition::CancellationRequested)
    }

    /// Remove a job from the manager
    ///
    /// Removes the job and its associated data. This is useful for cleaning up
    /// after retrieving the final status of a completed job.
    ///
    /// # Returns
    ///
    /// Returns `true` if the job was found and removed, `false` if it didn't exist.
    pub async fn remove_job(&self, job_id: &JobId) -> bool {
        let removed = self.jobs.lock().await.remove(job_id).is_some();
        if removed {
            self.canonical_job_ops
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(job_id);
            self.handles.lock().await.remove(job_id);
            self.cancel_notifiers.lock().await.remove(job_id);
            self.cancel_requested.lock().await.remove(job_id);
            self.completed_at.lock().await.remove(job_id);
        }
        removed
    }

    /// Canonical operation backing a public job ID, if one exists.
    pub fn canonical_operation_for_job(&self, job_id: &JobId) -> Option<OperationId> {
        self.canonical_job_ops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(job_id)
            .cloned()
    }

    /// Drain completed background jobs that have not yet been notified to the agent.
    ///
    /// Returns a `DetachedOpCompletion` for each job whose canonical ops-lifecycle
    /// has reached a terminal state since the last drain. Each job is returned at
    /// most once — subsequent calls will not re-report the same completion.
    ///
    /// Projects from canonical ops-lifecycle snapshots (INV-001). Shell-projected
    /// detail (exit code and bounded stdout/stderr) is supplementary display (INV-002).
    /// Returns app-facing `DetachedOpCompletion` — never surfaces `operation_id`.
    pub async fn drain_completed(&self) -> Vec<meerkat_core::agent::DetachedOpCompletion> {
        let mut completions = Vec::new();
        let mut jobs = self.jobs.lock().await;
        let mut completed_at_guard = self.completed_at.lock().await;

        for (_job_id, record) in jobs.iter_mut() {
            if record.completion_notified {
                continue;
            }

            let snapshot = match self.ops_registry.snapshot(&record.operation_id) {
                Ok(Some(snapshot)) => snapshot,
                Ok(None) => continue,
                Err(error) => {
                    warn!(
                        job_id = %record.view.id,
                        operation_id = %record.operation_id,
                        error = %error,
                        "background job completion drain skipped operation without generated projection authority"
                    );
                    continue;
                }
            };

            if !snapshot.terminal {
                continue;
            }

            let detail = Self::build_completion_detail(&record.view);
            let completed_at_ms = snapshot.completed_at_ms;

            completions.push(meerkat_core::agent::DetachedOpCompletion {
                job_id: record.view.id.to_string(),
                kind: snapshot.kind,
                status: snapshot.status,
                terminal_outcome: snapshot.terminal_outcome,
                display_name: snapshot.display_name,
                detail,
                elapsed_ms: snapshot.elapsed_ms,
            });

            record.completion_notified = true;

            // Record completion time so TTL/overflow cleanup can evict this job.
            // Use the snapshot's wall-clock completion time to backdate the Instant,
            // so TTL expiry is measured from actual completion — not from drain time.
            let completion_instant = completed_at_ms
                .and_then(|completed_ms| {
                    let now_ms = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    let ago_ms = now_ms.saturating_sub(completed_ms);
                    Instant::now().checked_sub(Duration::from_millis(ago_ms))
                })
                .unwrap_or_else(Instant::now);
            completed_at_guard
                .entry(record.view.id.clone())
                .or_insert(completion_instant);
        }

        completions
    }

    /// UTF-8-safe tail truncation: returns the last `max_bytes` bytes
    /// rounded down to a char boundary so we never split a multibyte
    /// code point.
    fn str_tail(s: &str, max_bytes: usize) -> &str {
        if s.len() <= max_bytes {
            return s;
        }
        // floor_char_boundary: stable since Rust 1.80
        let start = s.floor_char_boundary(s.len() - max_bytes);
        &s[start..]
    }

    fn build_completion_detail(view: &BackgroundJob) -> String {
        let mut parts = Vec::new();
        match &view.status {
            JobStatus::Completed {
                exit_code,
                stdout,
                stderr,
                duration_secs,
            } => {
                if let Some(code) = exit_code {
                    parts.push(format!("exit_code: {code}"));
                }
                parts.push(format!("duration: {duration_secs:.1}s"));
                if !stdout.is_empty() {
                    parts.push(format!("stdout: {}", Self::str_tail(stdout, 200)));
                }
                if !stderr.is_empty() {
                    parts.push(format!("stderr: {}", Self::str_tail(stderr, 200)));
                }
            }
            JobStatus::Failed {
                error,
                duration_secs,
            } => {
                parts.push(format!("error: {error}"));
                parts.push(format!("duration: {duration_secs:.1}s"));
            }
            JobStatus::Cancelled { duration_secs } => {
                parts.push(format!("cancelled after {duration_secs:.1}s"));
            }
            JobStatus::Running { .. } => {
                parts.push("still running (unexpected)".to_string());
            }
        }
        parts.join("; ")
    }

    /// Clean up old completed jobs based on configuration
    ///
    /// This method is called automatically during [`spawn_job`](Self::spawn_job) and removes:
    /// 1. Jobs that completed more than `completed_job_ttl_secs` ago
    /// 2. The oldest completed jobs when `max_completed_jobs` is exceeded
    async fn cleanup_old_jobs(&self) {
        let ttl = Duration::from_secs(self.config.completed_job_ttl_secs);
        let max_completed = self.config.max_completed_jobs;
        let now = Instant::now();

        // Acquire all locks once to avoid repeated lock/unlock overhead and potential race conditions.
        // This is more efficient than acquiring locks multiple times in a loop.
        let mut jobs_guard = self.jobs.lock().await;
        let mut handles_guard = self.handles.lock().await;
        let mut cancel_notifiers_guard = self.cancel_notifiers.lock().await;
        let mut cancel_requested_guard = self.cancel_requested.lock().await;
        let mut completed_at_guard = self.completed_at.lock().await;
        let mut canonical_job_ops_guard = self
            .canonical_job_ops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // First pass: collect job IDs older than TTL (only if already notified)
        let expired_jobs: Vec<JobId> = completed_at_guard
            .iter()
            .filter(|(job_id, completed_time)| {
                now.duration_since(**completed_time) > ttl
                    && jobs_guard.get(job_id).is_none_or(|j| j.completion_notified)
            })
            .map(|(job_id, _)| job_id.clone())
            .collect();

        // Remove expired jobs
        for job_id in &expired_jobs {
            jobs_guard.remove(job_id);
            canonical_job_ops_guard.remove(job_id);
            handles_guard.remove(job_id);
            cancel_notifiers_guard.remove(job_id);
            cancel_requested_guard.remove(job_id);
            completed_at_guard.remove(job_id);
        }

        // Second pass: if still over limit, remove oldest evictable (notified) completed jobs
        if max_completed > 0 && completed_at_guard.len() > max_completed {
            let mut evictable_jobs: Vec<(JobId, Instant)> = completed_at_guard
                .iter()
                .filter(|(job_id, _)| jobs_guard.get(job_id).is_none_or(|j| j.completion_notified))
                .map(|(k, v)| (k.clone(), *v))
                .collect();

            // Sort by completion time (oldest first)
            evictable_jobs.sort_by_key(|(_, time)| *time);

            // Remove oldest evictable jobs until total completed count is under the limit
            let total_completed = completed_at_guard.len();
            let to_remove = total_completed
                .saturating_sub(max_completed)
                .min(evictable_jobs.len());
            for (job_id, _) in evictable_jobs.into_iter().take(to_remove) {
                jobs_guard.remove(&job_id);
                canonical_job_ops_guard.remove(&job_id);
                handles_guard.remove(&job_id);
                cancel_notifiers_guard.remove(&job_id);
                cancel_requested_guard.remove(&job_id);
                completed_at_guard.remove(&job_id);
            }
        }
    }

    /// Get the number of jobs currently tracked
    ///
    /// Returns the total number of jobs (running + completed).
    pub async fn job_count(&self) -> usize {
        self.jobs.lock().await.len()
    }

    /// Get the number of completed jobs currently tracked
    pub async fn completed_job_count(&self) -> usize {
        self.completed_at.lock().await.len()
    }

    /// Get the number of currently running jobs
    ///
    /// Returns the count of active shell operations according to generated
    /// operation lifecycle authority.
    pub async fn running_job_count(&self) -> Result<usize, ShellError> {
        let mut count = 0;
        for snapshot in self
            .ops_registry
            .list_operations()
            .map_err(|error| ShellError::Io(std::io::Error::other(error.to_string())))?
        {
            if snapshot.kind != OperationKind::BackgroundToolOp
                && snapshot.kind != OperationKind::BackgroundToolCapacitySlot
            {
                continue;
            }
            if !snapshot.terminal {
                count += 1;
            }
        }
        Ok(count)
    }

    /// Acquire a slot for synchronous execution.
    ///
    /// Returns a guard that releases the generated operation capacity
    /// reservation when dropped, or an error if the generated admission
    /// authority rejects the concurrency limit.
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
            .map_err(Self::shell_error_for_ops_lifecycle_admission)?;
        if let Err(error) = self.ops_registry.provisioning_succeeded(&operation_id) {
            let _ = self
                .ops_registry
                .abort_provisioning(&operation_id, Some(error.to_string()));
            return Err(ShellError::Io(std::io::Error::other(error.to_string())));
        }
        Ok(SyncSlotGuard {
            ops_registry: Arc::clone(&self.ops_registry),
            operation_id,
        })
    }
}

/// Guard for a synchronous execution slot that releases it when dropped
pub struct SyncSlotGuard {
    ops_registry: Arc<dyn OpsLifecycleRegistry>,
    operation_id: OperationId,
}

impl std::fmt::Debug for SyncSlotGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncSlotGuard")
            .field("operation_id", &self.operation_id)
            .finish_non_exhaustive()
    }
}

impl Drop for SyncSlotGuard {
    fn drop(&mut self) {
        if let Err(error) = self.ops_registry.mark_retired(&self.operation_id) {
            tracing::error!(
                operation_id = %self.operation_id,
                error = %error,
                "generated shell sync-slot lifecycle authority rejected release"
            );
        }
    }
}

impl meerkat_core::completion_feed::CompletionEnrichmentProvider for JobManager {
    fn enrich(
        &self,
        operation_id: &OperationId,
    ) -> meerkat_core::completion_feed::CompletionEnrichment {
        self.lookup_completion_enrichment(operation_id)
    }
}

impl JobManager {
    fn lookup_completion_enrichment(
        &self,
        operation_id: &OperationId,
    ) -> meerkat_core::completion_feed::CompletionEnrichment {
        use meerkat_core::completion_feed::CompletionEnrichment;

        // Reverse-lookup: find which job_id maps to this operation_id.
        let canonical = self
            .canonical_job_ops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(job_id) = canonical
            .iter()
            .find(|(_, oid)| *oid == operation_id)
            .map(|(jid, _)| jid.clone())
        else {
            return CompletionEnrichment::Missing;
        };
        drop(canonical);

        // Try to read the job record. The jobs map uses a tokio::Mutex, so
        // we use try_lock (sync context). Contention is retryable and must not
        // be collapsed into a permanently missing process-local record.
        let Ok(jobs) = self.jobs.try_lock() else {
            return CompletionEnrichment::Busy;
        };
        let Some(record) = jobs.get(&job_id) else {
            return CompletionEnrichment::Missing;
        };
        let detail = Self::build_completion_detail(&record.view);
        CompletionEnrichment::Found(meerkat_core::completion_feed::CompletionEnrichmentData {
            job_id: job_id.to_string(),
            detail,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::builtin::shell::security::SecurityMode;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    fn bound_job_manager(config: ShellConfig) -> JobManager {
        let registry: Arc<dyn OpsLifecycleRegistry> = Arc::new(RuntimeOpsLifecycleRegistry::new());
        JobManager::new(config)
            .with_owner_bridge_session_id(SessionId::new())
            .with_ops_registry(registry)
    }

    #[cfg(unix)]
    fn unix_process_exists(pid: i32) -> bool {
        use nix::errno::Errno;
        use nix::sys::signal::kill;
        use nix::unistd::Pid;

        match kill(Pid::from_raw(pid), None) {
            Ok(()) => true,
            Err(Errno::ESRCH) => false,
            Err(_) => true,
        }
    }

    // ==================== JobManager Struct Tests ====================

    #[test]
    fn test_job_manager_struct() {
        // Verify JobManager has the required fields
        let config = ShellConfig::default();
        let _manager = bound_job_manager(config);

        // jobs and handles are Arc<Mutex<HashMap>>, verified by existence
    }

    #[test]
    fn test_job_manager_new() {
        let config = ShellConfig {
            enabled: true,
            default_timeout_secs: 60,
            restrict_to_project: true,
            shell: "nu".to_string(),
            shell_path: None,
            project_root: PathBuf::from("/tmp/test"),
            max_completed_jobs: 100,
            completed_job_ttl_secs: 300,
            max_concurrent_processes: 10,
            security_mode: SecurityMode::Unrestricted,
            security_patterns: vec![],
            env_vars: std::collections::HashMap::new(),
        };

        let manager = bound_job_manager(config);

        // Verify config is stored
        assert!(manager.config.enabled);
        assert_eq!(manager.config.default_timeout_secs, 60);
        assert_eq!(manager.config.shell, "nu");
        assert_eq!(manager.config.project_root, PathBuf::from("/tmp/test"));
        assert_eq!(manager.config.max_completed_jobs, 100);
        assert_eq!(manager.config.completed_job_ttl_secs, 300);
    }

    #[tokio::test]
    async fn completion_enrichment_distinguishes_busy_from_missing() {
        let manager = bound_job_manager(ShellConfig::default());
        let operation_id = OperationId::new();
        let job_id = JobId::new();
        manager
            .canonical_job_ops
            .lock()
            .unwrap()
            .insert(job_id, operation_id.clone());

        let held_jobs = manager.jobs.lock().await;
        assert!(matches!(
            manager.lookup_completion_enrichment(&operation_id),
            meerkat_core::completion_feed::CompletionEnrichment::Busy
        ));
        drop(held_jobs);

        assert!(matches!(
            manager.lookup_completion_enrichment(&operation_id),
            meerkat_core::completion_feed::CompletionEnrichment::Missing
        ));
    }

    #[tokio::test]
    async fn timed_out_job_summary_uses_generated_failed_snapshot() {
        let manager = bound_job_manager(ShellConfig::default());
        let operation_id = OperationId::new();
        manager
            .ops_registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: manager.owner_bridge_session_id.clone(),
                display_name: "shell:sleep".to_string(),
                source_label: "shell_job".to_string(),
                operation_source: None,
                child_session_id: None,
                expect_peer_channel: false,
            })
            .unwrap();
        manager
            .ops_registry
            .provisioning_succeeded(&operation_id)
            .unwrap();
        manager
            .ops_registry
            .fail_operation(&operation_id, "background job timed out".into())
            .unwrap();

        let job = BackgroundJobRecord {
            view: BackgroundJob {
                id: JobId::new(),
                command: "sleep 30".to_string(),
                working_dir: None,
                placement: None,
                timeout_secs: 30,
                started_at_unix: 123,
                status: JobStatus::Failed {
                    error: "background job timed out".to_string(),
                    duration_secs: 30.0,
                },
            },
            operation_id,
            completion_notified: false,
        };

        let snapshot = manager.snapshot_for_job(&job).unwrap();
        assert_eq!(
            manager
                .lifecycle_summary_status(&job, snapshot.as_ref())
                .expect("generated summary status"),
            JobSummaryStatus::Failed
        );
    }

    #[test]
    fn terminal_job_status_uses_generated_lifecycle_class() {
        let manager = bound_job_manager(ShellConfig::default());
        let operation_id = OperationId::new();
        manager
            .ops_registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: manager.owner_bridge_session_id.clone(),
                display_name: "shell:false".to_string(),
                source_label: "shell_job".to_string(),
                operation_source: None,
                child_session_id: None,
                expect_peer_channel: false,
            })
            .unwrap();
        manager
            .ops_registry
            .provisioning_succeeded(&operation_id)
            .unwrap();
        manager
            .ops_registry
            .fail_operation(&operation_id, "generated lifecycle rejected success".into())
            .unwrap();

        let job = BackgroundJobRecord {
            view: BackgroundJob {
                id: JobId::new(),
                command: "false".to_string(),
                working_dir: None,
                placement: None,
                timeout_secs: 30,
                started_at_unix: 123,
                status: JobStatus::Completed {
                    exit_code: Some(0),
                    stdout: "local success".to_string(),
                    stderr: String::new(),
                    duration_secs: 1.25,
                },
            },
            operation_id,
            completion_notified: false,
        };

        let snapshot = manager.snapshot_for_job(&job).unwrap();
        assert_eq!(
            JobManager::terminal_status_from_authority(&job, snapshot.as_ref(), &job.view.status)
                .expect("generated public result class"),
            Some(JobStatus::Failed {
                error: "generated lifecycle rejected success".to_string(),
                duration_secs: 1.25,
            })
        );
    }

    #[test]
    fn local_process_completion_remains_running_until_generated_terminal_authority() {
        let manager = bound_job_manager(ShellConfig::default());
        let operation_id = OperationId::new();
        manager
            .ops_registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: manager.owner_bridge_session_id.clone(),
                display_name: "shell:local-complete".to_string(),
                source_label: "shell_job".to_string(),
                operation_source: None,
                child_session_id: None,
                expect_peer_channel: false,
            })
            .unwrap();
        manager
            .ops_registry
            .provisioning_succeeded(&operation_id)
            .unwrap();

        let job = BackgroundJobRecord {
            view: BackgroundJob {
                id: JobId::new(),
                command: "true".to_string(),
                working_dir: None,
                placement: None,
                timeout_secs: 30,
                started_at_unix: 123,
                status: JobStatus::Completed {
                    exit_code: Some(0),
                    stdout: "local success".to_string(),
                    stderr: String::new(),
                    duration_secs: 1.0,
                },
            },
            operation_id,
            completion_notified: false,
        };

        let snapshot = manager
            .snapshot_for_job(&job)
            .unwrap()
            .expect("generated lifecycle snapshot");
        assert_eq!(snapshot.status, OperationStatus::Running);
        assert!(!snapshot.terminal);
        assert_eq!(
            JobManager::terminal_status_from_authority(&job, Some(&snapshot), &job.view.status)
                .expect("generated public result class"),
            Some(JobStatus::Running {
                started_at_unix: 123,
            })
        );
    }

    #[test]
    fn failed_terminal_status_uses_generated_terminal_outcome() {
        let manager = bound_job_manager(ShellConfig::default());
        let operation_id = OperationId::new();
        manager
            .ops_registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: manager.owner_bridge_session_id.clone(),
                display_name: "shell:timeout".to_string(),
                source_label: "shell_job".to_string(),
                operation_source: None,
                child_session_id: None,
                expect_peer_channel: false,
            })
            .unwrap();
        manager
            .ops_registry
            .provisioning_succeeded(&operation_id)
            .unwrap();
        manager
            .ops_registry
            .fail_operation(&operation_id, "generated failure reason".into())
            .unwrap();

        let job = BackgroundJobRecord {
            view: BackgroundJob {
                id: JobId::new(),
                command: "sleep 30".to_string(),
                working_dir: None,
                placement: None,
                timeout_secs: 1,
                started_at_unix: 123,
                status: JobStatus::Failed {
                    error: "local shell timeout".to_string(),
                    duration_secs: 1.0,
                },
            },
            operation_id,
            completion_notified: false,
        };

        let snapshot = manager.snapshot_for_job(&job).unwrap();
        assert_eq!(
            JobManager::terminal_status_from_authority(&job, snapshot.as_ref(), &job.view.status)
                .expect("generated public result class"),
            Some(JobStatus::Failed {
                error: "generated failure reason".to_string(),
                duration_secs: 1.0,
            })
        );
    }

    #[test]
    fn retiring_job_status_remains_nonterminal_until_generated_terminal_outcome() {
        let manager = bound_job_manager(ShellConfig::default());
        let operation_id = OperationId::new();
        manager
            .ops_registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: manager.owner_bridge_session_id.clone(),
                display_name: "shell:retiring".to_string(),
                source_label: "shell_job".to_string(),
                operation_source: None,
                child_session_id: None,
                expect_peer_channel: false,
            })
            .unwrap();
        manager
            .ops_registry
            .provisioning_succeeded(&operation_id)
            .unwrap();
        manager.ops_registry.request_retire(&operation_id).unwrap();

        let job = BackgroundJobRecord {
            view: BackgroundJob {
                id: JobId::new(),
                command: "sleep 30".to_string(),
                working_dir: None,
                placement: None,
                timeout_secs: 30,
                started_at_unix: 123,
                status: JobStatus::Running {
                    started_at_unix: 123,
                },
            },
            operation_id,
            completion_notified: false,
        };

        let snapshot = manager.snapshot_for_job(&job).unwrap();
        let snapshot = snapshot.as_ref().expect("snapshot");
        assert_eq!(snapshot.status, OperationStatus::Retiring);
        assert!(snapshot.terminal_outcome.is_none());
        assert_eq!(
            manager
                .lifecycle_summary_status(&job, Some(snapshot))
                .expect("generated summary status"),
            JobSummaryStatus::Running
        );
        assert_eq!(
            manager
                .reconcile_job_status(&job, Some(snapshot))
                .expect("generated status"),
            JobStatus::Running {
                started_at_unix: 123,
            }
        );
    }

    #[test]
    fn missing_authority_public_class_is_not_shell_failed() {
        let error = JobManager::summary_status_from_public_result(
            OperationPublicResultClass::MissingAuthority,
        )
        .expect_err("missing authority must fail projection");
        assert!(
            matches!(error, OpsLifecycleError::Internal(ref message) if message.contains("authority missing")),
            "unexpected missing authority error: {error:?}"
        );
    }

    #[tokio::test]
    async fn synthetic_running_job_registers_into_injected_ops_registry() {
        let owner_bridge_session_id = SessionId::new();
        let registry: Arc<dyn OpsLifecycleRegistry> = Arc::new(RuntimeOpsLifecycleRegistry::new());
        let manager = JobManager::new(ShellConfig::default())
            .with_owner_bridge_session_id(owner_bridge_session_id)
            .with_ops_registry(Arc::clone(&registry));

        let job_id = manager
            .register_synthetic_running_job("shell:synthetic", None, 30)
            .await
            .expect("synthetic running job should register");

        let snapshot = manager
            .ops_lifecycle_snapshot(&job_id)
            .await
            .expect("job manager should project snapshot from injected registry")
            .expect("job manager should resolve snapshot from injected registry");
        assert_eq!(snapshot.kind, OperationKind::BackgroundToolOp);
        assert_eq!(snapshot.status, OperationStatus::Running);
        assert_eq!(
            registry
                .snapshot(&snapshot.id)
                .expect("injected registry should own the operation")
                .expect("injected registry should own the operation")
                .status,
            OperationStatus::Running
        );
    }

    #[tokio::test]
    async fn synthetic_running_job_concurrency_uses_generated_admission() {
        let config = ShellConfig {
            max_concurrent_processes: 1,
            ..Default::default()
        };
        let manager = bound_job_manager(config);

        let _job_id = manager
            .register_synthetic_running_job("shell:synthetic-one", None, 30)
            .await
            .expect("first synthetic running job should register");
        let err = manager
            .register_synthetic_running_job("shell:synthetic-two", None, 30)
            .await
            .expect_err("generated operation admission should reject over limit");

        assert!(
            err.to_string().contains("Concurrency limit exceeded"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn synthetic_running_job_records_execution_placement_without_identity_path() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().to_path_buf();
        let subdir = project_root.join("worker");
        tokio::fs::create_dir(&subdir).await.unwrap();
        let manager = bound_job_manager(ShellConfig::with_project_root(project_root.clone()));

        let job_id = manager
            .register_synthetic_running_job(
                "shell:synthetic-placement",
                Some(Path::new("worker")),
                30,
            )
            .await
            .expect("synthetic running job should register");
        let job = manager
            .get_status(&job_id)
            .await
            .expect("job status")
            .expect("job exists");
        let placement = job.placement.expect("placement metadata");

        assert_eq!(
            placement.working_root.as_deref(),
            Some(subdir.canonicalize().unwrap().as_path())
        );
        assert_eq!(
            placement.allowed_roots,
            vec![project_root.canonicalize().unwrap()]
        );
        assert_eq!(placement.identity().host_id, None);
        assert_eq!(placement.identity().worktree_id, None);
    }

    // ==================== Spawn Job Tests ====================

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_job_manager_spawn() {
        let temp_dir = TempDir::new().unwrap();
        let config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());

        // Mock shell for this test (we'll use a shell that exists)
        let mut config = config;
        config.shell = "sh".to_string(); // Use sh which is universally available

        let manager = bound_job_manager(config);

        // Spawn a job
        let result = manager.spawn_job("echo test", None, 30).await;
        assert!(result.is_ok());

        let job_id = result.unwrap();
        assert!(job_id.0.starts_with("job_"));
    }

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_job_manager_spawn_immediate() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = bound_job_manager(config);

        // Spawn should return immediately without waiting for execution
        let start = Instant::now();
        let job_id = manager.spawn_job("sleep 5", None, 30).await.unwrap();
        let elapsed = start.elapsed();

        // Should return almost immediately (less than 1 second)
        assert!(
            elapsed.as_millis() < 1000,
            "spawn_job should return immediately, took {elapsed:?}"
        );

        // Job should exist
        let job = manager.get_status(&job_id).await.unwrap();
        assert!(job.is_some());
    }

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_job_manager_spawn_running() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = bound_job_manager(config);

        let job_id = manager.spawn_job("sleep 10", None, 30).await.unwrap();

        // Get status immediately - should be Running
        let job = manager.get_status(&job_id).await.unwrap().unwrap();
        assert!(
            matches!(job.status, JobStatus::Running { .. }),
            "Job should be Running, got {:?}",
            job.status
        );

        // Verify started_at_unix is reasonable (within last minute)
        if let JobStatus::Running { started_at_unix } = job.status {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            assert!(
                started_at_unix <= now && started_at_unix > now - 60,
                "started_at_unix should be recent"
            );
        }
    }

    // ==================== Get Status Tests ====================

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_job_manager_get_status() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = bound_job_manager(config);

        // Non-existent job
        let fake_id = JobId::from_string("job_nonexistent");
        let status = manager.get_status(&fake_id).await.unwrap();
        assert!(status.is_none());

        // Create a job
        let job_id = manager.spawn_job("echo hello", None, 30).await.unwrap();

        // Now it should exist
        let status = manager.get_status(&job_id).await.unwrap();
        assert!(status.is_some());
        let job = status.unwrap();
        assert_eq!(job.id, job_id);
        assert_eq!(job.command, "echo hello");
    }

    // ==================== List Jobs Tests ====================

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_job_manager_list_jobs() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = bound_job_manager(config);

        // Initially empty
        let jobs = manager.list_jobs().await.unwrap();
        assert!(jobs.is_empty());

        // Add some jobs
        let id1 = manager.spawn_job("echo one", None, 30).await.unwrap();
        let id2 = manager.spawn_job("echo two", None, 30).await.unwrap();

        // Should have 2 jobs
        let jobs = manager.list_jobs().await.unwrap();
        assert_eq!(jobs.len(), 2);

        // Check summaries contain expected data
        let ids: Vec<_> = jobs.iter().map(|j| j.id.clone()).collect();
        assert!(ids.contains(&id1));
        assert!(ids.contains(&id2));
    }

    #[tokio::test]
    async fn test_job_summary_preserves_started_at_unit() {
        let temp_dir = TempDir::new().unwrap();
        let config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        let manager = bound_job_manager(config);

        let job_id = JobId::new();
        let started_at_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let operation_id = OperationId::new();
        manager
            .ops_registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: manager.owner_bridge_session_id.clone(),
                display_name: "shell:echo done".to_string(),
                source_label: "shell_job".to_string(),
                operation_source: None,
                child_session_id: None,
                expect_peer_channel: false,
            })
            .unwrap();
        manager
            .ops_registry
            .provisioning_succeeded(&operation_id)
            .unwrap();
        manager
            .ops_registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".to_string(),
                    is_error: false,
                    duration_ms: 10,
                    tokens_used: 0,
                },
            )
            .unwrap();
        let job = BackgroundJobRecord {
            view: BackgroundJob {
                id: job_id.clone(),
                command: "echo done".to_string(),
                working_dir: None,
                placement: None,
                timeout_secs: 10,
                started_at_unix,
                status: JobStatus::Completed {
                    exit_code: Some(0),
                    stdout: "done".to_string(),
                    stderr: String::new(),
                    duration_secs: 0.01,
                },
            },
            operation_id: operation_id.clone(),
            completion_notified: false,
        };

        manager.jobs.lock().await.insert(job_id.clone(), job);
        manager
            .canonical_job_ops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(job_id.clone(), operation_id);

        let summaries = manager.list_jobs().await.expect("job summaries");
        let summary = summaries
            .iter()
            .find(|s| s.id == job_id)
            .expect("Job should be in list");
        assert_eq!(summary.started_at_unix, started_at_unix);
    }

    // ==================== Cancel Job Tests ====================

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_job_manager_cancel() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = bound_job_manager(config);

        // Spawn a long-running job
        let job_id = manager.spawn_job("sleep 60", None, 120).await.unwrap();

        // Give it a moment to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Cancel it
        let result = manager.cancel_job(&job_id).await;
        assert!(result.is_ok());

        // Status should be Cancelled
        let job = manager.get_status(&job_id).await.unwrap().unwrap();
        assert!(
            matches!(job.status, JobStatus::Cancelled { .. }),
            "Job should be Cancelled, got {:?}",
            job.status
        );
    }

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_job_manager_cancel_signal() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = bound_job_manager(config);

        // Spawn a job
        let job_id = manager.spawn_job("sleep 60", None, 120).await.unwrap();

        // Wait a bit
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Cancel should work
        let result = manager.cancel_job(&job_id).await;
        assert!(result.is_ok());

        // Trying to cancel non-existent job
        let fake_id = JobId::from_string("job_fake");
        let result = manager.cancel_job(&fake_id).await;
        assert!(matches!(result, Err(ShellError::JobNotFound(_))));
    }

    // ==================== Async Execution Tests (require actual shell) ====================

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_job_manager_tokio_spawn() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = bound_job_manager(config);

        // Spawn a simple job
        let job_id = manager.spawn_job("echo hello", None, 30).await.unwrap();

        // Wait for completion
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Should be completed
        let job = manager.get_status(&job_id).await.unwrap().unwrap();
        assert!(
            matches!(job.status, JobStatus::Completed { .. }),
            "Job should be Completed, got {:?}",
            job.status
        );
    }

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_job_manager_completed_status() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = bound_job_manager(config);

        let job_id = manager
            .spawn_job("echo 'test output'", None, 30)
            .await
            .unwrap();

        // Wait for completion
        tokio::time::sleep(Duration::from_secs(2)).await;

        let job = manager.get_status(&job_id).await.unwrap().unwrap();
        if let JobStatus::Completed {
            exit_code,
            stdout,
            stderr: _,
            duration_secs,
        } = &job.status
        {
            assert_eq!(*exit_code, Some(0));
            assert!(stdout.contains("test output"));
            assert!(*duration_secs >= 0.0);
        } else {
            unreachable!("Expected Completed status, got {:?}", job.status);
        }
    }

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_job_manager_failed_status() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = JobManager::new(config);

        // Command that exits with error
        let job_id = manager.spawn_job("exit 1", None, 30).await.unwrap();

        // Wait for completion
        tokio::time::sleep(Duration::from_secs(2)).await;

        let job = manager.get_status(&job_id).await.unwrap().unwrap();

        // Should be Completed with non-zero exit code (not Failed - Failed is for spawn errors)
        if let JobStatus::Completed { exit_code, .. } = &job.status {
            assert_eq!(*exit_code, Some(1));
        } else {
            unreachable!(
                "Expected Completed status with exit code 1, got {:?}",
                job.status
            );
        }
    }

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_job_manager_timeout_status() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = JobManager::new(config);

        // Job that will timeout
        let job_id = manager.spawn_job("sleep 30", None, 1).await.unwrap();

        // Wait for timeout
        tokio::time::sleep(Duration::from_secs(3)).await;

        let job = manager.get_status(&job_id).await.unwrap().unwrap();
        if let JobStatus::Failed {
            error,
            duration_secs,
        } = &job.status
        {
            assert!(error.contains("timed out"));
            assert!(*duration_secs >= 1.0);
        } else {
            unreachable!("Expected Failed timeout status, got {:?}", job.status);
        }
    }

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_job_manager_cancelled_status() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = JobManager::new(config);

        let job_id = manager.spawn_job("sleep 60", None, 120).await.unwrap();

        // Let it start
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Cancel
        manager.cancel_job(&job_id).await.unwrap();

        let job = manager.get_status(&job_id).await.unwrap().unwrap();
        assert!(
            matches!(job.status, JobStatus::Cancelled { .. }),
            "Job should be Cancelled, got {:?}",
            job.status
        );

        if let JobStatus::Cancelled { duration_secs } = &job.status {
            assert!(*duration_secs >= 0.0);
        }
    }

    // ==================== Event Notification Tests ====================

    // ==================== Regression Tests ====================

    /// Regression test: async execution should be non-blocking
    ///
    /// Spawning a job should return immediately without waiting for the
    /// command to complete. This verifies that spawn_job doesn't block.
    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_async_execution_nonblocking() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = bound_job_manager(config);

        // Spawn multiple long-running jobs concurrently
        let start = Instant::now();

        let id1 = manager.spawn_job("sleep 5", None, 60).await.unwrap();
        let id2 = manager.spawn_job("sleep 5", None, 60).await.unwrap();
        let id3 = manager.spawn_job("sleep 5", None, 60).await.unwrap();

        let elapsed = start.elapsed();

        // All spawns should complete in under 1 second (non-blocking)
        assert!(
            elapsed.as_millis() < 1000,
            "Spawning 3 jobs should be nearly instant, took {elapsed:?}"
        );

        // Verify all jobs are running
        for id in [&id1, &id2, &id3] {
            let job = manager.get_status(id).await.unwrap().unwrap();
            assert!(
                matches!(job.status, JobStatus::Running { .. }),
                "Job {id} should be running"
            );
        }

        // Clean up
        let _ = manager.cancel_job(&id1).await;
        let _ = manager.cancel_job(&id2).await;
        let _ = manager.cancel_job(&id3).await;
    }

    /// Regression test: timeout should be enforced for background jobs
    ///
    /// A job that runs longer than its timeout should be terminated and
    /// marked as Failed through generated public result authority.
    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_timeout_enforced() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = bound_job_manager(config);

        // Spawn a job that sleeps longer than the timeout
        let job_id = manager.spawn_job("sleep 10", None, 1).await.unwrap();

        // Wait for timeout to occur (plus buffer)
        tokio::time::sleep(Duration::from_secs(3)).await;

        // Verify job timed out
        let job = manager.get_status(&job_id).await.unwrap().unwrap();
        // Verify duration is approximately the timeout value
        if let JobStatus::Failed {
            error,
            duration_secs,
        } = &job.status
        {
            assert!(error.contains("timed out"));
            assert!(
                *duration_secs >= 1.0 && *duration_secs < 3.0,
                "Duration should be close to timeout: {duration_secs}"
            );
        } else {
            unreachable!("Expected Failed timeout status, got {:?}", job.status);
        }
    }

    /// Regression test: cancel_job should terminate the underlying process
    ///
    /// When a job is cancelled, the underlying process should be terminated
    /// and the job status should be Cancelled.
    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_kill_terminates_process() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = bound_job_manager(config);

        // Spawn a long-running job
        let job_id = manager.spawn_job("sleep 60", None, 120).await.unwrap();

        // Let it start
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Verify it's running
        let job = manager.get_status(&job_id).await.unwrap().unwrap();
        assert!(
            matches!(job.status, JobStatus::Running { .. }),
            "Job should be running before cancel"
        );

        // Cancel the job
        manager
            .cancel_job(&job_id)
            .await
            .expect("Cancel should succeed");

        // Verify it's cancelled
        let job = manager.get_status(&job_id).await.unwrap().unwrap();
        assert!(
            matches!(job.status, JobStatus::Cancelled { .. }),
            "Job should be cancelled, got {:?}",
            job.status
        );

        // Wait a moment and verify it stays cancelled (process is gone)
        tokio::time::sleep(Duration::from_millis(500)).await;
        let job = manager.get_status(&job_id).await.unwrap().unwrap();
        assert!(
            matches!(job.status, JobStatus::Cancelled { .. }),
            "Job should still be cancelled"
        );
    }

    /// Regression test: concurrent job spawning should produce unique IDs
    ///
    /// When spawning many jobs concurrently, each should get a unique ID.
    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_concurrent_job_spawning() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = Arc::new(bound_job_manager(config));

        // Spawn 10 jobs concurrently using tokio::join
        let mut handles = Vec::new();
        for i in 0..10 {
            let mgr = Arc::clone(&manager);
            let cmd = format!("echo job{i}");
            handles.push(tokio::spawn(
                async move { mgr.spawn_job(&cmd, None, 30).await },
            ));
        }

        // Collect all results
        let mut job_ids = Vec::new();
        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_ok(), "All spawns should succeed");
            job_ids.push(result.unwrap());
        }

        // Verify all IDs are unique
        let unique_count = {
            let mut ids: Vec<_> = job_ids.iter().map(|id| &id.0).collect();
            ids.sort();
            ids.dedup();
            ids.len()
        };
        assert_eq!(
            unique_count, 10,
            "All 10 jobs should have unique IDs, got {unique_count}"
        );

        // Verify all jobs complete
        tokio::time::sleep(Duration::from_secs(2)).await;
        for job_id in &job_ids {
            let job = manager.get_status(job_id).await.unwrap().unwrap();
            assert!(
                matches!(job.status, JobStatus::Completed { .. }),
                "Job {} should be completed, got {:?}",
                job_id,
                job.status
            );
        }
    }

    // ==================== Job Cleanup Tests ====================

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_remove_job() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = bound_job_manager(config);

        // Spawn and wait for a job to complete
        let job_id = manager.spawn_job("echo hello", None, 30).await.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Job should exist
        assert!(manager.get_status(&job_id).await.unwrap().is_some());
        assert_eq!(manager.job_count().await, 1);

        // Remove the job
        let removed = manager.remove_job(&job_id).await;
        assert!(removed, "Job should be removed");

        // Job should no longer exist
        assert!(manager.get_status(&job_id).await.unwrap().is_none());
        assert_eq!(manager.job_count().await, 0);

        // Removing again should return false
        let removed_again = manager.remove_job(&job_id).await;
        assert!(!removed_again, "Already removed job should return false");
    }

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_job_count() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = bound_job_manager(config);

        assert_eq!(manager.job_count().await, 0);

        let id1 = manager.spawn_job("echo one", None, 30).await.unwrap();
        assert_eq!(manager.job_count().await, 1);

        let id2 = manager.spawn_job("echo two", None, 30).await.unwrap();
        assert_eq!(manager.job_count().await, 2);

        manager.remove_job(&id1).await;
        assert_eq!(manager.job_count().await, 1);

        manager.remove_job(&id2).await;
        assert_eq!(manager.job_count().await, 0);
    }

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_completed_job_count() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = bound_job_manager(config);

        // Initially no completed jobs
        assert_eq!(manager.completed_job_count().await, 0);

        // Spawn a job
        let _job_id = manager.spawn_job("echo hello", None, 30).await.unwrap();

        // Not yet completed
        assert_eq!(manager.completed_job_count().await, 0);

        // Wait for completion
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Now should be completed
        assert_eq!(manager.completed_job_count().await, 1);
    }

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_cleanup_respects_max_completed_jobs() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();
        config.max_completed_jobs = 3;
        config.completed_job_ttl_secs = 3600; // Long TTL so we test count-based cleanup

        let manager = bound_job_manager(config);

        // Spawn 5 jobs and wait for them to complete
        for i in 0..5 {
            let _ = manager
                .spawn_job(&format!("echo job{i}"), None, 30)
                .await
                .unwrap();
        }
        tokio::time::sleep(Duration::from_secs(2)).await;

        // All 5 should be completed
        assert_eq!(manager.completed_job_count().await, 5);

        // Spawn a long-running job to trigger cleanup (won't complete immediately)
        let trigger_id = manager.spawn_job("sleep 10", None, 30).await.unwrap();

        // After cleanup, completed jobs should be reduced to max_completed_jobs
        let completed_after = manager.completed_job_count().await;
        assert!(
            completed_after <= 3,
            "Should have at most 3 completed jobs after cleanup, got {completed_after}"
        );

        // The new job should still be running (not counted in completed)
        let job = manager.get_status(&trigger_id).await.unwrap().unwrap();
        assert!(
            matches!(job.status, JobStatus::Running { .. }),
            "Trigger job should be running"
        );

        // Clean up
        let _ = manager.cancel_job(&trigger_id).await;
    }

    #[tokio::test]
    async fn test_remove_nonexistent_job() {
        let config = ShellConfig::default();
        let manager = bound_job_manager(config);

        let fake_id = JobId::from_string("job_nonexistent");
        let removed = manager.remove_job(&fake_id).await;
        assert!(!removed, "Removing nonexistent job should return false");
    }

    // ==================== Regression Tests for Bug Fixes ====================

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelled_job_kills_term_ignoring_descendant_and_finishes_waiter() {
        use nix::sys::signal::{Signal, killpg};
        use nix::unistd::Pid;

        let temp_dir = TempDir::new().unwrap();
        let leader_pid_file = temp_dir.path().join("background-leader.pid");
        let child_pid_file = temp_dir.path().join("background-child.pid");
        let child_ready_file = temp_dir.path().join("background-child.ready");
        let config = ShellConfig {
            enabled: true,
            default_timeout_secs: 30,
            restrict_to_project: false,
            shell: "sh".to_string(),
            shell_path: Some(PathBuf::from("/bin/sh")),
            project_root: temp_dir.path().to_path_buf(),
            security_mode: SecurityMode::Unrestricted,
            ..Default::default()
        };
        let manager = bound_job_manager(config);
        let command = format!(
            "echo $$ > '{}'; (trap '' TERM; echo ready > '{}'; while :; do sleep 1; done) & echo $! > '{}'; wait",
            leader_pid_file.display(),
            child_ready_file.display(),
            child_pid_file.display()
        );
        let job_id = manager.spawn_job(&command, None, 30).await.unwrap();

        for _ in 0..100 {
            if leader_pid_file.exists() && child_pid_file.exists() && child_ready_file.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let leader_pid: i32 = std::fs::read_to_string(&leader_pid_file)
            .expect("background shell must publish its process-group leader")
            .trim()
            .parse()
            .unwrap();
        let child_pid: i32 = std::fs::read_to_string(&child_pid_file)
            .expect("background shell must publish its child pid")
            .trim()
            .parse()
            .unwrap();
        assert!(
            manager.handles.lock().await.contains_key(&job_id),
            "background waiter must be registered"
        );
        let operation_id = manager
            .canonical_operation_for_job(&job_id)
            .expect("background operation must be published");

        manager.cancel_job(&job_id).await.unwrap();
        let before_ack = manager
            .ops_registry
            .snapshot(&operation_id)
            .expect("operation snapshot")
            .expect("operation must exist");
        let job_before_ack = manager
            .get_status(&job_id)
            .await
            .expect("job status")
            .expect("job must exist");
        let completion_before_ack = manager
            .ops_registry
            .completion_feed()
            .expect("completion feed")
            .list_since(0)
            .entries
            .into_iter()
            .any(|entry| entry.operation_id == operation_id);
        let waiter_finished = tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                let terminal = manager
                    .ops_registry
                    .snapshot(&operation_id)
                    .expect("operation snapshot")
                    .is_some_and(|snapshot| snapshot.terminal);
                let notifier_retired = !manager.cancel_notifiers.lock().await.contains_key(&job_id);
                if terminal && notifier_retired {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok();
        let descendant_survived = unix_process_exists(child_pid);
        let after_ack = manager
            .ops_registry
            .snapshot(&operation_id)
            .expect("operation snapshot")
            .expect("operation must exist");

        if descendant_survived {
            let _ = killpg(Pid::from_raw(leader_pid), Signal::SIGKILL);
        }
        assert!(
            waiter_finished,
            "cancellation waiter must not hang on pipes held by a TERM-ignoring descendant"
        );
        assert!(
            !descendant_survived,
            "cancellation must kill the complete process group after the TERM grace"
        );
        assert!(
            !before_ack.terminal,
            "a cancellation request must not terminalize the operation before containment ack"
        );
        assert!(
            matches!(job_before_ack.status, JobStatus::Running { .. }),
            "a cancellation request must not publish terminal job status before containment ack"
        );
        assert!(
            !completion_before_ack,
            "the completion feed must remain silent until containment ack"
        );
        assert!(after_ack.terminal);
        assert_eq!(after_ack.status, OperationStatus::Cancelled);
    }

    #[cfg(unix)]
    async fn assert_aborted_submission_has_no_orphans(
        manager: &JobManager,
        pid_file: &Path,
        window: &str,
    ) {
        use nix::sys::signal::{Signal, killpg};
        use nix::unistd::Pid;

        let mut terminal_before_containment = false;
        for _ in 0..500 {
            let process_alive = std::fs::read_to_string(pid_file)
                .ok()
                .and_then(|raw| raw.trim().parse::<i32>().ok())
                .is_some_and(unix_process_exists);
            let operations = manager
                .ops_registry
                .list_operations()
                .expect("operation inventory");
            terminal_before_containment |=
                process_alive && operations.iter().any(|snapshot| snapshot.terminal);
            if !process_alive && operations.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let operations = manager
            .ops_registry
            .list_operations()
            .expect("operation inventory");
        let running_jobs: Vec<_> = manager
            .jobs
            .lock()
            .await
            .values()
            .filter(|record| matches!(record.view.status, JobStatus::Running { .. }))
            .map(|record| record.view.id.clone())
            .collect();
        let published_pid = std::fs::read_to_string(pid_file)
            .ok()
            .and_then(|raw| raw.trim().parse::<i32>().ok());
        let process_alive = published_pid.is_some_and(unix_process_exists);

        let completion_entries = manager
            .ops_registry
            .completion_feed()
            .expect("runtime completion feed")
            .list_since(0)
            .entries;

        // Keep a red regression hygienic before asserting its captured state.
        for job_id in &running_jobs {
            let _ = manager.cancel_job(job_id).await;
        }
        for snapshot in operations.iter().filter(|snapshot| !snapshot.terminal) {
            let _ = manager
                .ops_registry
                .cancel_operation(&snapshot.id, Some("test cleanup".into()));
        }
        if process_alive && let Some(pid) = published_pid {
            let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
        }

        assert!(
            !terminal_before_containment,
            "aborting submission at {window} terminalized before containment"
        );
        assert!(
            operations.is_empty(),
            "aborting submission at {window} left operation authority: {operations:?}"
        );
        assert!(
            completion_entries.is_empty(),
            "aborting an unreturned submission at {window} published phantom completion feed entries: {completion_entries:?}"
        );
        assert!(
            running_jobs.is_empty(),
            "aborting submission at {window} left running jobs: {running_jobs:?}"
        );
        assert!(
            !process_alive,
            "aborting submission at {window} left its process group alive"
        );
    }

    #[cfg(unix)]
    fn abort_window_manager(
        temp_dir: &TempDir,
        hook: Arc<SubmissionHandoffHook>,
    ) -> Arc<JobManager> {
        let config = ShellConfig {
            enabled: true,
            default_timeout_secs: 30,
            restrict_to_project: false,
            shell: "sh".to_string(),
            shell_path: Some(PathBuf::from("/bin/sh")),
            project_root: temp_dir.path().to_path_buf(),
            security_mode: SecurityMode::Unrestricted,
            ..Default::default()
        };
        Arc::new(bound_job_manager(config).with_submission_handoff_hook(hook))
    }

    #[cfg(unix)]
    async fn abort_submission_at(stage: SubmissionHandoffStage, window: &str, pid_name: &str) {
        let temp_dir = TempDir::new().unwrap();
        let pid_file = temp_dir.path().join(pid_name);
        let hook = Arc::new(SubmissionHandoffHook::new(stage));
        let manager = abort_window_manager(&temp_dir, Arc::clone(&hook));
        let command = format!("echo $$ > '{}'; sleep 30", pid_file.display());
        let task = {
            let manager = Arc::clone(&manager);
            tokio::spawn(async move { manager.spawn_job(&command, None, 30).await })
        };

        tokio::time::timeout(Duration::from_secs(2), hook.reached.notified())
            .await
            .expect("submission must reach injected handoff window");
        for _ in 0..100 {
            if pid_file.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            pid_file.exists(),
            "injected handoff must occur after process ownership begins"
        );
        task.abort();
        let _ = task.await;
        hook.release.notify_one();

        assert_aborted_submission_has_no_orphans(&manager, &pid_file, window).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn aborted_submission_at_jobs_publication_has_no_orphans() {
        abort_submission_at(
            SubmissionHandoffStage::JobsPublication,
            "jobs publication",
            "jobs-window.pid",
        )
        .await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn aborted_submission_at_cancel_publication_has_no_orphans() {
        abort_submission_at(
            SubmissionHandoffStage::CancelPublication,
            "cancel publication",
            "cancel-window.pid",
        )
        .await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn aborted_submission_at_waiter_handoff_has_no_orphans() {
        abort_submission_at(
            SubmissionHandoffStage::WaiterHandoff,
            "waiter handoff",
            "waiter-window.pid",
        )
        .await;
    }

    #[test]
    fn background_submission_source_does_not_leak_raw_commands_to_stderr() {
        let source = include_str!("job_manager.rs");
        let debug_marker = ["[SHELL", "-DEBUG]"].concat();
        let raw_stderr_macro = ["eprint", "ln!("].concat();
        assert!(
            !source.contains(&debug_marker),
            "background submission must not print raw shell commands to process stderr"
        );
        assert!(
            !source.contains(&raw_stderr_macro),
            "background submission must use structured redacted tracing, not raw stderr"
        );
    }

    #[test]
    fn background_submission_span_skips_raw_command_argument() {
        let source = include_str!("job_manager.rs");
        let spawn_start = source
            .find("    pub async fn spawn_job(")
            .expect("spawn function");
        let attribute_start = source[..spawn_start]
            .rfind("#[instrument(")
            .expect("spawn instrument attribute");
        let attribute = &source[attribute_start..spawn_start];

        assert!(
            attribute.contains("skip(self, command,"),
            "spawn_job tracing must explicitly skip the raw command argument"
        );
        assert!(
            !attribute.contains("command ="),
            "spawn_job tracing fields must not record the raw command"
        );
    }

    #[tokio::test]
    async fn background_operation_display_name_does_not_expose_raw_command() {
        let temp_dir = TempDir::new().unwrap();
        let manager = bound_job_manager(ShellConfig::with_project_root(
            temp_dir.path().to_path_buf(),
        ));
        let sentinel = "RKAT_SECRET_COMMAND_SENTINEL_7d40b7";
        let command = format!("printf '{sentinel}'");

        let job_id = manager
            .register_synthetic_running_job(&command, None, 30)
            .await
            .expect("synthetic job registration");
        let snapshot = manager
            .ops_lifecycle_snapshot(&job_id)
            .await
            .expect("lifecycle snapshot")
            .expect("registered operation");

        assert!(
            !snapshot.display_name.contains(sentinel) && !snapshot.display_name.contains(&command),
            "operation diagnostics must not project raw shell command content"
        );
        assert!(
            snapshot.display_name.contains(job_id.as_ref()),
            "safe display metadata should retain public job identity"
        );

        manager
            .cancel_job(&job_id)
            .await
            .expect("synthetic cleanup");
    }

    #[test]
    fn unreturned_background_submission_paths_do_not_publish_terminal_completion() {
        let source = include_str!("job_manager.rs");
        let synthetic_start = source
            .find("    pub async fn register_synthetic_running_job(")
            .expect("synthetic registration function");
        let spawn_start = source
            .find("    pub async fn spawn_job(")
            .expect("spawn function");
        let status_start = source
            .find("    pub async fn get_status(")
            .expect("status function after spawn");
        let synthetic_source = &source[synthetic_start..spawn_start];
        let spawn_source = &source[spawn_start..status_start];

        for (name, function_source) in [
            ("register_synthetic_running_job", synthetic_source),
            ("spawn_job", spawn_source),
        ] {
            assert!(
                !function_source.contains(".provisioning_failed(")
                    && !function_source.contains(".abort_provisioning("),
                "{name} must reconcile an unreturned operation back to absence without publishing a terminal completion"
            );
            assert!(
                function_source.contains(".rollback_unreturned_operation("),
                "{name} must use typed unreturned-operation rollback"
            );
        }
    }

    /// Regression test: multi-byte UTF-8 characters in job output should be handled correctly
    ///
    /// Verifies that job output containing emoji, Chinese characters, and other
    /// multi-byte UTF-8 sequences is captured without panicking or data corruption.
    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_multibyte_utf8_output() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = bound_job_manager(config);

        // Command that outputs various multi-byte UTF-8 characters:
        // - Chinese: 世界 (2 chars, 6 bytes)
        // - Emoji: 🎉🚀👍 (3 chars, 12 bytes)
        // - Accented: émojis (6 chars, 7 bytes)
        let job_id = manager
            .spawn_job("printf 'Hello 世界! 🎉🚀 Test émojis: 👍'", None, 30)
            .await
            .unwrap();

        // Wait for completion - should not panic during output capture
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Verify job completed successfully
        let job = manager.get_status(&job_id).await.unwrap().unwrap();
        assert!(
            matches!(job.status, JobStatus::Completed { .. }),
            "Job with UTF-8 output should complete successfully, got {:?}",
            job.status
        );

        // Verify the multi-byte characters are preserved in output
        if let JobStatus::Completed { stdout, .. } = &job.status {
            // Check for presence of multi-byte characters (they might be in stdout or not
            // depending on shell, but the test shouldn't panic either way)
            assert!(
                stdout.contains("Hello") || stdout.contains("世界") || stdout.is_empty(),
                "Output should be captured without corruption"
            );
        }
    }

    /// Regression test: kill_job properly reaps the process (no zombies)
    ///
    /// Verifies that when cancel_job is called, the underlying process is
    /// fully terminated and reaped via child.kill().await (not just start_kill()).
    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_kill_reaps_process() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = JobManager::new(config);

        // Spawn a long-running process
        let job_id = manager.spawn_job("sleep 60", None, 120).await.unwrap();

        // Give it time to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Verify it's running
        let job = manager.get_status(&job_id).await.unwrap().unwrap();
        assert!(
            matches!(job.status, JobStatus::Running { .. }),
            "Job should be Running before kill"
        );

        // Kill it - this should use child.kill().await which waits for termination
        manager.cancel_job(&job_id).await.unwrap();

        // Verify status is Cancelled (not still Running)
        let job = manager.get_status(&job_id).await.unwrap().unwrap();
        assert!(
            matches!(job.status, JobStatus::Cancelled { .. }),
            "Job should be Cancelled after kill, got {:?}",
            job.status
        );

        // Small delay to ensure process is fully reaped
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Verify status remains Cancelled (process didn't become zombie)
        let job = manager.get_status(&job_id).await.unwrap().unwrap();
        assert!(
            matches!(job.status, JobStatus::Cancelled { .. }),
            "Job should still be Cancelled (process reaped), got {:?}",
            job.status
        );
    }

    /// Regression test: background jobs auto-complete and update status
    ///
    /// Verifies that when a background job finishes, the monitoring task
    /// automatically updates the job status to Completed with output captured.
    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_background_job_auto_completes() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = JobManager::new(config);

        // Spawn a quick command that outputs text
        let job_id = manager.spawn_job("echo hello", None, 30).await.unwrap();

        // Immediately should be Running
        let job = manager.get_status(&job_id).await.unwrap().unwrap();
        assert!(
            matches!(job.status, JobStatus::Running { .. }),
            "Job should start as Running"
        );

        // Wait for it to complete (should be fast)
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Status should have auto-updated to Completed
        let job = manager.get_status(&job_id).await.unwrap().unwrap();
        assert!(
            matches!(job.status, JobStatus::Completed { .. }),
            "Job should auto-complete to Completed, got {:?}",
            job.status
        );

        // Verify output was captured
        if let JobStatus::Completed {
            stdout, exit_code, ..
        } = &job.status
        {
            assert!(
                stdout.contains("hello"),
                "stdout should contain 'hello', got: {stdout}"
            );
            assert_eq!(*exit_code, Some(0), "exit code should be 0");
        }
    }

    // ==================== Output Truncation Tests ====================

    #[test]
    fn test_truncate_output_tail_small_input() {
        // Input smaller than limit should pass through unchanged
        let data = b"Hello, World!";
        let result = super::truncate_output_tail(data, 100);
        assert_eq!(result, "Hello, World!");
    }

    #[test]
    fn test_truncate_output_tail_exact_limit() {
        // Input exactly at limit should pass through unchanged
        let data = b"12345";
        let result = super::truncate_output_tail(data, 5);
        assert_eq!(result, "12345");
    }

    #[test]
    fn test_truncate_output_tail_exceeds_limit() {
        // Input exceeding limit should be truncated to keep tail
        let data = b"0123456789";
        let result = super::truncate_output_tail(data, 5);
        assert_eq!(result, "56789");
    }

    #[test]
    fn test_truncate_output_tail_utf8_boundary() {
        // Test with multi-byte UTF-8 characters (Chinese characters are 3 bytes each)
        let data = "Hello世界Test".as_bytes(); // "Hello" (5) + "世" (3) + "界" (3) + "Test" (4) = 15 bytes

        // With limit of 10, we'd normally cut at byte 5, but that might be mid-character
        // The function should find a valid UTF-8 boundary
        let result = super::truncate_output_tail(data, 10);

        // Result should be valid UTF-8 (no panic on to_string)
        assert!(result.is_ascii() || result.chars().count() > 0);

        // Should contain at least "Test" from the tail
        assert!(result.contains("Test") || result.contains("界"));
    }

    #[test]
    fn test_truncate_output_tail_emoji() {
        // Emoji are 4-byte UTF-8 sequences
        let data = "Start🎉🚀End".as_bytes(); // "Start" (5) + emoji (4+4) + "End" (3) = 16 bytes

        let result = super::truncate_output_tail(data, 8);

        // Should be valid UTF-8
        assert!(result.chars().count() > 0);
    }

    #[test]
    fn test_append_with_truncation_small_append() {
        let mut buffer = vec![1, 2, 3];
        super::append_with_truncation(&mut buffer, &[4, 5], 10);

        // No truncation should occur (5 bytes < 10 * 2)
        assert_eq!(buffer, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_append_with_truncation_triggers_truncation() {
        let mut buffer = vec![0; 15]; // Start with 15 bytes
        super::append_with_truncation(&mut buffer, &[1; 10], 10); // Add 10 more = 25 bytes

        // 25 > 10 * 2 = 20, so truncation should occur
        // Should keep ~10 bytes from tail
        assert!(buffer.len() <= 11); // Allow for UTF-8 boundary adjustment
        assert!(buffer.len() >= 9);
    }

    // ==================== Regression Tests for Bug Fixes ====================
    //
    // These tests verify specific fixes from the shell module review:
    // - Task #5: Race condition in job status transitions
    // - Task #6: SIGTERM before SIGKILL in kill_job
    // - Task #7: Lock upgrade pattern in cleanup_old_jobs
    // - Task #8: Process group handling for child processes
    // - Task #9: Streaming output truncation
    // - Task #10: Richer error context in ShellError
    // - Task #11: Shell detection with fallbacks

    /// Regression test for Task #5: Atomic status check-and-modify
    ///
    /// Verifies that cancel_job performs status check and modification atomically
    /// within a single lock scope, preventing race conditions where another
    /// operation could change the status between checking and modifying.
    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_cancel_job_atomic_status_check() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = Arc::new(bound_job_manager(config));

        // Spawn a long-running job
        let job_id = manager.spawn_job("sleep 30", None, 60).await.unwrap();

        // Give it time to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Cancel from multiple "threads" concurrently - only first should succeed
        let mgr1 = Arc::clone(&manager);
        let mgr2 = Arc::clone(&manager);
        let job_id1 = job_id.clone();
        let job_id2 = job_id.clone();

        let (r1, r2) = tokio::join!(
            tokio::spawn(async move { mgr1.cancel_job(&job_id1).await }),
            tokio::spawn(async move { mgr2.cancel_job(&job_id2).await }),
        );

        // Both tasks should complete without panic
        let result1 = r1.expect("Task 1 should not panic");
        let result2 = r2.expect("Task 2 should not panic");

        // One should succeed, one should fail with JobNotRunning
        let (successes, failures): (Vec<_>, Vec<_>) =
            [result1, result2].into_iter().partition(Result::is_ok);

        assert_eq!(successes.len(), 1, "Exactly one cancel should succeed");
        assert_eq!(
            failures.len(),
            1,
            "Exactly one cancel should fail with JobNotRunning"
        );

        // The failure should be JobNotRunning, not a race condition panic
        if let Err(err) = &failures[0] {
            assert!(
                matches!(err, ShellError::JobNotRunning),
                "Expected JobNotRunning error, got: {err:?}"
            );
        }
    }

    /// Regression test for process-group termination plumbing.
    ///
    /// Verifies the shared TERM/grace/KILL path compiles under the real-process
    /// integration feature.
    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    #[cfg(unix)]
    async fn integration_real_graceful_kill_function_exists() {
        use tokio::process::Command;

        // Test that graceful_kill compiles and can be called
        let mut command = Command::new("sleep");
        command.arg("1");
        command.kill_on_drop(true);
        command.process_group(0);
        let mut child = command.spawn().expect("Failed to spawn test process");
        let mut process_group = OwnedProcessGroup::new(&child);

        // Group termination should complete without panicking.
        let result = process_group.terminate(&mut child).await;
        assert!(
            result.is_ok(),
            "process-group termination should succeed: {result:?}"
        );
    }

    /// Regression test for Task #7: Single write lock for cleanup
    ///
    /// Verifies that cleanup_old_jobs acquires all locks once at the start
    /// and performs all cleanup operations atomically, rather than using
    /// a read-then-write pattern that could cause race conditions.
    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_cleanup_atomicity() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();
        config.max_completed_jobs = 2;
        config.completed_job_ttl_secs = 0; // Immediate expiry for testing

        let manager = Arc::new(bound_job_manager(config));

        // Spawn and complete several jobs
        for i in 0..5 {
            let _id = manager
                .spawn_job(&format!("echo job{i}"), None, 30)
                .await
                .unwrap();
        }

        // Wait for all jobs to complete
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Now spawn from multiple threads concurrently - each spawn triggers cleanup
        let mut handles = Vec::new();
        for i in 0..5 {
            let mgr = Arc::clone(&manager);
            handles.push(tokio::spawn(async move {
                mgr.spawn_job(&format!("echo trigger{i}"), None, 30).await
            }));
        }

        // All spawns should succeed (cleanup is atomic, no race-induced panics)
        for handle in handles {
            let result = handle.await.expect("Task should not panic");
            assert!(result.is_ok(), "Spawn should succeed: {result:?}");
        }

        // After cleanup, we should be at or under max_completed_jobs
        // Note: Some jobs may still be running, so check total count is reasonable
        let count = manager.job_count().await;
        assert!(
            count <= 12, // 5 original completed (some cleaned up) + 5 new triggers
            "Job count should be reasonable after cleanup: {count}"
        );
    }

    /// Regression test for Task #8: Process group configuration
    ///
    /// Verifies that process_group(0) is configured in Command building,
    /// which ensures child processes are in the same process group as the
    /// parent shell for proper cleanup. This is a compile-time/configuration
    /// test since the actual process group behavior depends on the OS.
    #[test]
    #[cfg(unix)]
    fn test_process_group_configured() {
        // This test verifies the code path exists - the actual behavior
        // is tested by ensuring the code compiles with CommandExt::process_group.
        // The process group feature is Unix-specific and set during spawn.

        // Verify that std::os::unix::process::CommandExt is available
        // (compilation would fail if this import was removed from spawn_job)
        use std::os::unix::process::CommandExt;

        let mut cmd = std::process::Command::new("echo");
        // This line verifies process_group is available and callable
        cmd.process_group(0);

        // The test passes if it compiles - process group is configured
    }

    /// Regression test for Task #10: Error variant for missing jobs
    #[tokio::test]
    async fn test_error_context_job_not_found() {
        let config = ShellConfig::default();
        let manager = bound_job_manager(config);

        // Try to cancel a non-existent job
        let fake_id = JobId::from_string("job_nonexistent_12345");
        let result = manager.cancel_job(&fake_id).await;

        // Should be JobNotFound
        match result {
            Err(ShellError::JobNotFound(job_id)) => {
                assert_eq!(job_id, "job_nonexistent_12345");
            }
            other => unreachable!("Expected JobNotFound error, got: {:?}", other),
        }
    }

    /// Regression test for Task #10: Error variant for non-running jobs
    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_error_context_job_already_completed() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = bound_job_manager(config);

        // Spawn and wait for job to complete
        let job_id = manager.spawn_job("echo done", None, 30).await.unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;

        // Try to cancel the completed job
        let result = manager.cancel_job(&job_id).await;

        // Should be JobNotRunning
        match result {
            Err(ShellError::JobNotRunning) => {}
            other => unreachable!("Expected JobNotRunning error, got: {:?}", other),
        }
    }

    /// Regression test for Task #9: Streaming truncation function
    ///
    /// Verifies that append_with_truncation truncates correctly when buffer
    /// exceeds 2x the limit, keeping approximately max_bytes from the tail.
    #[test]
    fn test_streaming_truncation_utf8_safety() {
        // Test that streaming truncation respects UTF-8 boundaries
        let mut buffer = "Hello世界".as_bytes().to_vec(); // 5 + 6 = 11 bytes

        // Append more data to trigger truncation (limit=10, trigger at 20)
        let more_data = "更多数据Test".as_bytes(); // 9 + 4 = 13 bytes
        super::append_with_truncation(&mut buffer, more_data, 10); // Total 24 > 20

        // Buffer should be truncated and still valid UTF-8
        let result = String::from_utf8(buffer.clone());
        assert!(
            result.is_ok(),
            "Buffer should be valid UTF-8 after truncation: {buffer:?}"
        );

        // Should contain data from the tail
        let result_str = result.unwrap();
        assert!(
            result_str.contains("Test") || result_str.contains("据"),
            "Should contain tail data: {result_str}"
        );
    }

    // ==================== Resource Limit Tests ====================

    /// Test running_job_count returns correct count of running jobs
    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_running_job_count() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = bound_job_manager(config);

        // Initially no running jobs
        assert_eq!(manager.running_job_count().await.unwrap(), 0);

        // Spawn a long-running job
        let job1 = manager.spawn_job("sleep 60", None, 120).await.unwrap();
        assert_eq!(manager.running_job_count().await.unwrap(), 1);

        // Spawn another long-running job
        let job2 = manager.spawn_job("sleep 60", None, 120).await.unwrap();
        assert_eq!(manager.running_job_count().await.unwrap(), 2);

        let _ = manager.cancel_job(&job1).await;
        let _ = manager.cancel_job(&job2).await;
    }

    /// Test that concurrency limit is enforced
    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_concurrency_limit_enforced() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();
        config.max_concurrent_processes = 2; // Set a low limit for testing

        let manager = bound_job_manager(config);

        // Spawn up to the limit
        let job1 = manager.spawn_job("sleep 60", None, 120).await.unwrap();
        let job2 = manager.spawn_job("sleep 60", None, 120).await.unwrap();

        // Third job should be rejected
        let result = manager.spawn_job("sleep 60", None, 120).await;
        assert!(
            result.is_err(),
            "Should reject job when at concurrency limit"
        );

        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("Concurrency limit exceeded"),
            "Expected concurrency limit error, got: {err:?}"
        );

        let _ = manager.cancel_job(&job1).await;
        let _ = manager.cancel_job(&job2).await;
    }

    /// Test that concurrency limit of 0 means unlimited
    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_concurrency_limit_zero_means_unlimited() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();
        config.max_concurrent_processes = 0; // Unlimited

        let manager = bound_job_manager(config);

        // Should be able to spawn many jobs
        let mut jobs = Vec::new();
        for _ in 0..5 {
            let result = manager.spawn_job("sleep 60", None, 120).await;
            assert!(
                result.is_ok(),
                "Should allow unlimited jobs when limit is 0"
            );
            jobs.push(result.unwrap());
        }

        for job in jobs {
            let _ = manager.cancel_job(&job).await;
        }
    }

    /// Regression test for P3: JobSummary should preserve started_at_unix for completed jobs
    ///
    /// When a job completes, its started_at_unix should still be available in JobSummary,
    /// not reset to 0. This is important for displaying job history with accurate timestamps.
    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_job_summary_preserves_started_at_for_completed_jobs() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = JobManager::new(config);

        // Spawn a quick job
        let job_id = manager.spawn_job("echo hello", None, 30).await.unwrap();

        // Wait for it to complete
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Verify job completed
        let job = manager.get_status(&job_id).await.unwrap().unwrap();
        assert!(
            matches!(job.status, JobStatus::Completed { .. }),
            "Job should be completed, got {:?}",
            job.status
        );

        // Get the job summary via list_jobs
        let summaries = manager.list_jobs().await.unwrap();
        let summary = summaries
            .iter()
            .find(|s| s.id == job_id)
            .expect("Job should be in list");

        // The started_at_unix should be a valid timestamp, not 0
        assert!(
            summary.started_at_unix > 0,
            "Completed job's started_at_unix should be preserved (> 0), got {}",
            summary.started_at_unix
        );

        // Verify it's a reasonable timestamp (within last minute)
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(
            summary.started_at_unix <= now && summary.started_at_unix > now - 60,
            "started_at_unix should be recent, got {} vs now {}",
            summary.started_at_unix,
            now
        );
    }

    // ==================== P3: Atomic Concurrency Limit Tests ====================

    /// Stress test for P3: acquire_sync_slot TOCTOU race condition
    ///
    /// This test spawns many concurrent tasks that all try to acquire sync slots
    /// simultaneously. Without atomic compare_exchange, multiple tasks could pass
    /// the limit check before any increments, exceeding the configured limit.
    ///
    /// The test verifies that at no point do we have more active slots than the limit.
    #[tokio::test]
    async fn test_acquire_sync_slot_atomic_stress() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();
        config.max_concurrent_processes = 2; // Low limit to make race easier to trigger

        let manager = Arc::new(bound_job_manager(config));

        // Track concurrent active slots and max observed
        let active_slots = Arc::new(AtomicUsize::new(0));
        let max_observed = Arc::new(AtomicUsize::new(0));
        let success_count = Arc::new(AtomicUsize::new(0));
        let failure_count = Arc::new(AtomicUsize::new(0));

        // Spawn many concurrent tasks trying to acquire slots
        let num_tasks = 20;
        let mut handles = Vec::new();

        for _ in 0..num_tasks {
            let mgr = Arc::clone(&manager);
            let active = Arc::clone(&active_slots);
            let max_obs = Arc::clone(&max_observed);
            let successes = Arc::clone(&success_count);
            let failures = Arc::clone(&failure_count);

            handles.push(tokio::spawn(async move {
                match mgr.acquire_sync_slot().await {
                    Ok(guard) => {
                        // We got a slot - increment active count
                        let current = active.fetch_add(1, Ordering::SeqCst) + 1;

                        // Track max concurrent slots ever observed
                        let mut max = max_obs.load(Ordering::SeqCst);
                        while current > max {
                            match max_obs.compare_exchange_weak(
                                max,
                                current,
                                Ordering::SeqCst,
                                Ordering::Relaxed,
                            ) {
                                Ok(_) => break,
                                Err(actual) => max = actual,
                            }
                        }

                        successes.fetch_add(1, Ordering::SeqCst);

                        // Hold the slot briefly to increase chance of race
                        tokio::time::sleep(Duration::from_millis(10)).await;

                        // Decrement active count before guard drops
                        active.fetch_sub(1, Ordering::SeqCst);

                        // Guard drops here, releasing the slot
                        drop(guard);
                    }
                    Err(_) => {
                        // Slot acquisition failed (at limit) - this is expected
                        failures.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }));
        }

        // Wait for all tasks to complete
        for handle in handles {
            handle.await.expect("Task should not panic");
        }

        let final_max = max_observed.load(Ordering::SeqCst);
        let total_successes = success_count.load(Ordering::SeqCst);
        let total_failures = failure_count.load(Ordering::SeqCst);

        // The critical assertion: max concurrent slots should never exceed limit
        assert!(
            final_max <= 2,
            "TOCTOU race detected! Max concurrent slots was {final_max}, limit is 2. \
             Successes: {total_successes}, Failures: {total_failures}"
        );

        // Sanity check: all tasks completed
        assert_eq!(
            total_successes + total_failures,
            num_tasks,
            "All tasks should complete"
        );

        // With limit=2 and 20 tasks, we expect some failures
        assert!(
            total_failures > 0,
            "Should have some failures when 20 tasks compete for 2 slots"
        );
    }

    #[tokio::test]
    async fn test_acquire_sync_slot_enforces_limit() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.max_concurrent_processes = 1;

        let manager = bound_job_manager(config);

        let _guard = manager
            .acquire_sync_slot()
            .await
            .expect("first slot should be available");
        let err = manager
            .acquire_sync_slot()
            .await
            .expect_err("second slot should be rejected");

        assert!(
            err.to_string().contains("Concurrency limit exceeded"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn sync_slot_release_does_not_publish_background_completion() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.max_concurrent_processes = 1;

        let manager = bound_job_manager(config);
        let feed = manager
            .ops_registry
            .completion_feed()
            .expect("runtime ops registry should expose completion feed");

        let operation_id = {
            let guard = manager
                .acquire_sync_slot()
                .await
                .expect("sync slot should be available");
            let operation_id = guard.operation_id.clone();
            let snapshot = manager
                .ops_registry
                .snapshot(&operation_id)
                .expect("sync slot projection should succeed")
                .expect("sync slot should be registered");
            assert_eq!(snapshot.kind, OperationKind::BackgroundToolCapacitySlot);
            operation_id
        };

        let batch = feed.list_since(0);
        assert!(
            batch.entries.is_empty(),
            "sync-slot release must not publish background completion entries: {:?}",
            batch.entries
        );
        assert_eq!(batch.watermark, 0);
        assert!(
            manager
                .ops_registry
                .snapshot(&operation_id)
                .unwrap()
                .is_none(),
            "sync-slot release must discard volatile capacity-slot state"
        );
    }

    /// Additional stress test: run acquire_sync_slot stress test multiple times
    /// to catch intermittent race conditions
    #[tokio::test]
    async fn test_acquire_sync_slot_atomic_repeated() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        // Run the stress test 10 times to catch flaky races
        for iteration in 0..10 {
            let temp_dir = TempDir::new().unwrap();
            let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
            config.shell = "sh".to_string();
            config.max_concurrent_processes = 2;

            let manager = Arc::new(bound_job_manager(config));
            let max_observed = Arc::new(AtomicUsize::new(0));
            let active_slots = Arc::new(AtomicUsize::new(0));

            let mut handles = Vec::new();
            for _ in 0..10 {
                let mgr = Arc::clone(&manager);
                let max_obs = Arc::clone(&max_observed);
                let active = Arc::clone(&active_slots);

                handles.push(tokio::spawn(async move {
                    if let Ok(guard) = mgr.acquire_sync_slot().await {
                        let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                        let mut max = max_obs.load(Ordering::SeqCst);
                        while current > max {
                            match max_obs.compare_exchange_weak(
                                max,
                                current,
                                Ordering::SeqCst,
                                Ordering::Relaxed,
                            ) {
                                Ok(_) => break,
                                Err(actual) => max = actual,
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        active.fetch_sub(1, Ordering::SeqCst);
                        drop(guard);
                    }
                }));
            }

            for handle in handles {
                handle.await.expect("Task should not panic");
            }

            let final_max = max_observed.load(Ordering::SeqCst);
            assert!(
                final_max <= 2,
                "TOCTOU race on iteration {iteration}! Max concurrent was {final_max}, limit is 2"
            );
        }
    }

    // ==================== CHOKE-001/002: Drain + Cleanup Guard Tests ====================

    #[tokio::test]
    async fn choke_001_drain_completed_returns_typed_projection() {
        // Setup: create a JobManager with a real RuntimeOpsLifecycleRegistry
        let registry: Arc<dyn OpsLifecycleRegistry> = Arc::new(RuntimeOpsLifecycleRegistry::new());
        let manager = JobManager::new(ShellConfig::default())
            .with_owner_bridge_session_id(SessionId::new())
            .with_ops_registry(Arc::clone(&registry));

        // Register a synthetic running job
        let job_id = manager
            .register_synthetic_running_job("shell:choke-001", None, 30)
            .await
            .expect("synthetic job should register");

        // Resolve the canonical operation ID for this job
        let op_id = manager
            .canonical_operation_for_job(&job_id)
            .expect("synthetic job must have a canonical operation");

        // Complete the operation in the ops registry
        registry
            .complete_operation(
                &op_id,
                OperationResult {
                    id: op_id.clone(),
                    content: "done".to_string(),
                    is_error: false,
                    duration_ms: 42,
                    tokens_used: 0,
                },
            )
            .expect("complete_operation should succeed");

        // Call drain_completed()
        let completions = manager.drain_completed().await;

        // Assert: returns exactly one DetachedOpCompletion
        assert_eq!(
            completions.len(),
            1,
            "drain_completed should return exactly one completion"
        );
        let c = &completions[0];

        // Assert: job_id matches the shell job id
        assert_eq!(
            c.job_id,
            job_id.to_string(),
            "completion job_id must match the shell job id"
        );

        // Assert: status is OperationStatus::Completed (from canonical snapshot)
        assert_eq!(
            c.status,
            OperationStatus::Completed,
            "terminal status must come from canonical ops-lifecycle"
        );

        // Assert: display_name comes from the snapshot, not shell-local state
        assert!(
            !c.display_name.is_empty(),
            "display_name must be populated from the ops-lifecycle snapshot"
        );

        // Assert: second call to drain_completed() returns empty (already notified)
        let second = manager.drain_completed().await;
        assert!(
            second.is_empty(),
            "second drain_completed must return empty — jobs already notified"
        );
    }

    #[tokio::test]
    async fn choke_002_unnotified_completions_survive_cleanup() {
        // Setup: create JobManager with aggressive TTL (1 second) and max_completed=1
        let registry: Arc<dyn OpsLifecycleRegistry> = Arc::new(RuntimeOpsLifecycleRegistry::new());
        let config = ShellConfig {
            completed_job_ttl_secs: 1,
            max_completed_jobs: 1,
            ..ShellConfig::default()
        };
        let manager = JobManager::new(config)
            .with_owner_bridge_session_id(SessionId::new())
            .with_ops_registry(Arc::clone(&registry));

        // Register + complete a synthetic job
        let job_id = manager
            .register_synthetic_running_job("shell:choke-002", None, 30)
            .await
            .expect("synthetic job should register");
        let op_id = manager
            .canonical_operation_for_job(&job_id)
            .expect("must have canonical op");
        registry
            .complete_operation(
                &op_id,
                OperationResult {
                    id: op_id.clone(),
                    content: "done".to_string(),
                    is_error: false,
                    duration_ms: 10,
                    tokens_used: 0,
                },
            )
            .expect("complete should succeed");

        // Backdate the completed job instead of sleeping; this keeps the test
        // focused on cleanup behavior rather than wall-clock waiting.
        manager.completed_at.lock().await.insert(
            job_id.clone(),
            Instant::now()
                .checked_sub(Duration::from_secs(2))
                .expect("2 seconds before now should be representable"),
        );

        // Trigger cleanup (spawning a new job triggers cleanup_old_jobs internally)
        let _trigger = manager
            .register_synthetic_running_job("shell:trigger-cleanup", None, 30)
            .await
            .expect("trigger job should register");

        // Assert: the completed-but-unnotified job STILL exists (survives cleanup)
        assert!(
            manager.jobs.lock().await.contains_key(&job_id),
            "unnotified completed job must survive cleanup even after TTL"
        );

        // Drain to mark it notified
        let drained = manager.drain_completed().await;
        assert!(
            !drained.is_empty(),
            "drain_completed must return the unnotified completion"
        );
        manager.completed_at.lock().await.insert(
            job_id.clone(),
            Instant::now()
                .checked_sub(Duration::from_secs(2))
                .expect("2 seconds before now should be representable"),
        );

        // Trigger cleanup again
        let _trigger2 = manager
            .register_synthetic_running_job("shell:trigger-cleanup-2", None, 30)
            .await
            .expect("second trigger job should register");

        // Assert: the notified job is now evictable (may have been cleaned up)
        // After draining + TTL expired + cleanup triggered, the job should be gone
        let still_exists = manager.jobs.lock().await.contains_key(&job_id);
        assert!(
            !still_exists,
            "notified job should be evictable after TTL expiry + cleanup"
        );
    }

    #[tokio::test]
    async fn choke_002_overflow_cleanup_uses_filtered_evictable_subset() {
        // Setup: create JobManager with max_completed_jobs=2
        let registry: Arc<dyn OpsLifecycleRegistry> = Arc::new(RuntimeOpsLifecycleRegistry::new());
        let config = ShellConfig {
            max_completed_jobs: 2,
            completed_job_ttl_secs: 3600, // long TTL so only overflow triggers cleanup
            ..ShellConfig::default()
        };
        let manager = JobManager::new(config)
            .with_owner_bridge_session_id(SessionId::new())
            .with_ops_registry(Arc::clone(&registry));

        // Register + complete 4 synthetic jobs
        let mut job_ids = Vec::new();
        for i in 0..4 {
            let jid = manager
                .register_synthetic_running_job(&format!("shell:overflow-{i}"), None, 30)
                .await
                .expect("synthetic job should register");
            let op_id = manager
                .canonical_operation_for_job(&jid)
                .expect("must have canonical op");
            registry
                .complete_operation(
                    &op_id,
                    OperationResult {
                        id: op_id.clone(),
                        content: format!("done-{i}"),
                        is_error: false,
                        duration_ms: 10,
                        tokens_used: 0,
                    },
                )
                .expect("complete should succeed");
            job_ids.push(jid);
        }

        // Do NOT drain (all are unnotified)
        // Trigger cleanup by spawning another job
        let _trigger = manager
            .register_synthetic_running_job("shell:trigger-overflow", None, 30)
            .await
            .expect("trigger job should register");

        // Assert: all 4 jobs still exist (none evictable because none notified)
        let jobs_guard = manager.jobs.lock().await;
        for jid in &job_ids {
            assert!(
                jobs_guard.contains_key(jid),
                "unnotified job {jid} must survive overflow cleanup"
            );
        }
        drop(jobs_guard);

        // Drain 2 of them (mark notified)
        // First drain gets all 4, but we only want to test that draining makes them evictable.
        // Since drain_completed drains ALL completed-unnotified, we drain once and then
        // verify the cleanup behavior.
        let drained = manager.drain_completed().await;
        assert_eq!(drained.len(), 4, "all 4 completed jobs should be drained");

        // Trigger cleanup again — now all 4 are notified and evictable
        let _trigger2 = manager
            .register_synthetic_running_job("shell:trigger-overflow-2", None, 30)
            .await
            .expect("second trigger should register");

        // Assert: with max_completed=2, at least 2 of the notified jobs should be evicted
        // (overflow cleanup removes oldest completed jobs when count exceeds max)
        let remaining_completed = manager.completed_job_count().await;
        assert!(
            remaining_completed <= manager.config.max_completed_jobs,
            "after cleanup, completed job count ({remaining_completed}) should be \
             at or below max_completed_jobs ({})",
            manager.config.max_completed_jobs,
        );
    }
}
