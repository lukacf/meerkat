//! Durable detached-job application and host-callback wire contracts.
//!
//! App-facing projections deliberately omit attempt ids, worker ids, fences,
//! runner handles, and credential references. Host mutation contracts carry
//! the exact attempt authority required for fenced writes.

use serde::{Deserialize, Serialize};

use super::WireHandlingMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum JobPhase {
    Unsubmitted,
    Queued,
    Claimed,
    Running,
    WaitingExternal,
    LossObserved,
    RetryScheduled,
    Succeeded,
    Failed,
    Cancelled,
    WorkerLost,
    NeedsAttention,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum JobRestartClass {
    Adoptable,
    CheckpointResumable,
    Replayable,
    NonResumable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobRunner {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum JobIdempotencyScope {
    ToolCall,
    InteractionAndArguments,
    HostSemanticKey,
}

/// Private host/gateway execution declaration for a callback tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum CallbackToolExecution {
    Fast,
    Detached {
        runner: JobRunner,
        restart_class: JobRestartClass,
        idempotency_scope: JobIdempotencyScope,
        submission_timeout_ms: u64,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        credential_scopes: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobProgress {
    pub cursor: u64,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobTerminalResult {
    Succeeded {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_ref: Option<String>,
    },
    Failed {
        code: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail_ref: Option<String>,
    },
    Cancelled,
    WorkerLost,
    NeedsAttention {
        reason: String,
    },
}

/// Safe application-facing job projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobSummary {
    pub job_id: String,
    pub runner: JobRunner,
    pub phase: JobPhase,
    pub restart_class: JobRestartClass,
    pub attempt_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<JobProgress>,
    pub cancel_requested: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_result: Option<JobTerminalResult>,
    pub subscription_count: u64,
    pub delivery_backlog: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobReceipt {
    pub job_id: String,
    pub status: JobPhase,
    pub deduplicated: bool,
    pub restart_class: JobRestartClass,
    pub awaitable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobsGetParams {
    pub job_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobsGetResult {
    pub job: JobSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobsListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobsListResult {
    pub jobs: Vec<JobSummary>,
}

pub type JobsCancelParams = JobsGetParams;
pub type JobsCancelResult = JobsGetResult;

pub type JobsProgressParams = JobsGetParams;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobsProgressResult {
    pub job_id: String,
    pub phase: JobPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<JobProgress>,
}

pub type JobsResultParams = JobsGetParams;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobsResultResult {
    pub job_id: String,
    pub phase: JobPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<JobTerminalResult>,
}

pub type JobsArtifactsParams = JobsGetParams;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobArtifactRef {
    pub reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobsArtifactsResult {
    pub job_id: String,
    pub artifacts: Vec<JobArtifactRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobsRetryParams {
    pub job_id: String,
    pub retry_due_at_ms: u64,
}

pub type JobsRetryResult = JobsGetResult;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobsHealthParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobsHealthResult {
    pub detached_jobs: JobHealthSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MonitorOutputProtocol {
    #[default]
    FramedJsonl,
    Lines,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MonitorsStartParams {
    pub session_id: String,
    /// Caller-stable idempotency identity for this monitor submission.
    pub submission_key: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    pub timeout_secs: u64,
    #[serde(default)]
    pub protocol: MonitorOutputProtocol,
    pub restart_class: JobRestartClass,
    pub delivery: JobDeliveryKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_line_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_notifications_per_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_window_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retained_diagnostic_bytes: Option<u64>,
}

pub type MonitorsStartResult = JobsGetResult;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobDeliveryKind {
    Record,
    Notification,
    Event { handling_mode: WireHandlingMode },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobsSubscribeParams {
    pub job_id: String,
    pub subscription_id: String,
    pub session_id: String,
    pub delivery: JobDeliveryKind,
}

pub type JobsSubscribeResult = JobsGetResult;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobsUnsubscribeParams {
    pub job_id: String,
    pub subscription_id: String,
}

pub type JobsUnsubscribeResult = JobsGetResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum JobHealthStatus {
    Ok,
    Degraded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobHealthSummary {
    pub status: JobHealthStatus,
    pub queued: u64,
    pub running: u64,
    pub awaiting_members: u64,
    pub stale_leases: u64,
    pub needs_attention: u64,
    pub delivery_backlog: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum JobExecutionActivityKind {
    Idle,
    CallingProvider,
    ExecutingFastTool,
    StreamingTool,
    AwaitingDetached,
    Draining,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobExecutionActivity {
    pub kind: JobExecutionActivityKind,
    pub since_ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub job_ids: Vec<String>,
}

/// Detached-job census attached to member status without changing member
/// lifecycle semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DetachedJobsActivitySummary {
    pub active: u64,
    pub needs_attention: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest_active_ms: Option<u64>,
}

/// Host-only write authority. This type must never be embedded in app job
/// summaries or ordinary status responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JobAttemptAuthority {
    pub job_id: String,
    pub attempt_id: String,
    pub fence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CallbackJobStartParams {
    pub authority: JobAttemptAuthority,
    pub runner: JobRunner,
    pub restart_class: JobRestartClass,
    /// Stable non-secret handle already committed with the claim. The host
    /// must echo this handle when accepting the attempt.
    pub runner_handle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_specification_ref: Option<String>,
    /// Non-secret runner arguments loaded from the committed specification
    /// reference for this short control-plane callback.
    #[cfg_attr(feature = "schema", schemars(with = "serde_json::Value"))]
    pub arguments: serde_json::Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credential_scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_checkpoint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CallbackJobStartResult {
    pub accepted: bool,
    pub runner_handle: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CallbackJobCancelParams {
    pub authority: JobAttemptAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CallbackJobCancelResult {
    pub accepted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CallbackJobReconcileParams {
    pub attempts: Vec<CallbackJobReconcileAttempt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CallbackJobReconcileAttempt {
    pub authority: JobAttemptAuthority,
    pub runner: JobRunner,
    pub restart_class: JobRestartClass,
    pub runner_handle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_ref: Option<String>,
    pub lease_expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CallbackJobReconcileResult {
    pub live_attempts: Vec<JobAttemptAuthority>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobkitJobHeartbeatParams {
    pub authority: JobAttemptAuthority,
    pub heartbeat_at_ms: u64,
    pub lease_expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobkitJobProgressParams {
    pub authority: JobAttemptAuthority,
    pub cursor: u64,
    pub detail: String,
    pub observed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobkitJobCheckpointParams {
    pub authority: JobAttemptAuthority,
    pub checkpoint_ref: String,
    pub observed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobkitJobCompleteParams {
    pub authority: JobAttemptAuthority,
    pub completed_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobkitJobFailParams {
    pub authority: JobAttemptAuthority,
    pub failed_at_ms: u64,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobkitJobCancelAckParams {
    pub authority: JobAttemptAuthority,
    pub acknowledged_at_ms: u64,
}

pub type MobkitJobMutationResult = JobsGetResult;
