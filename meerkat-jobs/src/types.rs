use std::fmt;

use meerkat_core::{SessionId, ToolCredentialContextRef};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::DetachedJobError;
use crate::machines::detached_job as dsl;

macro_rules! string_id {
    ($name:ident, $label:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, DetachedJobError> {
                let value = value.into();
                let trimmed = value.trim();
                if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
                    return Err(DetachedJobError::InvalidInput(format!(
                        "{} must be non-empty and contain no control characters",
                        $label
                    )));
                }
                Ok(Self(trimmed.to_string()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_id!(JobId, "job id");
string_id!(AttemptId, "attempt id");
string_id!(WorkerId, "worker id");
string_id!(JobSubmissionKey, "job submission key");
string_id!(CanonicalArgumentsHash, "canonical arguments hash");
string_id!(CheckpointRef, "checkpoint reference");
string_id!(RunnerHandleRef, "runner handle reference");
string_id!(JobResultRef, "job result reference");
string_id!(JobFailureCode, "job failure code");
string_id!(OriginMemberId, "origin member id");

impl JobId {
    pub fn generated() -> Self {
        Self(format!("job_{}", Uuid::now_v7()))
    }
}

impl AttemptId {
    pub fn generated() -> Self {
        Self(format!("attempt_{}", Uuid::now_v7()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExecutionIntentId(String);

impl ExecutionIntentId {
    pub fn new() -> Self {
        Self(format!("intent_{}", Uuid::now_v7()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ExecutionIntentId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InteractionLineageId(String);

impl InteractionLineageId {
    pub fn new() -> Self {
        Self(format!("interaction_{}", Uuid::now_v7()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for InteractionLineageId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FenceToken(u64);

impl FenceToken {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for FenceToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolIdentity {
    name: String,
    version: String,
}

impl ToolIdentity {
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
    ) -> Result<Self, DetachedJobError> {
        Ok(Self {
            name: validate_component("tool name", name.into())?,
            version: validate_component("tool version", version.into())?,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> &str {
        &self.version
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerIdentity {
    name: String,
    version: String,
}

impl RunnerIdentity {
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
    ) -> Result<Self, DetachedJobError> {
        Ok(Self {
            name: validate_component("runner name", name.into())?,
            version: validate_component("runner version", version.into())?,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> &str {
        &self.version
    }
}

fn validate_component(label: &str, value: String) -> Result<String, DetachedJobError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return Err(DetachedJobError::InvalidInput(format!(
            "{label} must be non-empty and contain no control characters"
        )));
    }
    Ok(trimmed.to_string())
}

pub type RestartClass = dsl::DetachedJobRestartClass;
pub type JobPhase = dsl::DetachedJobPhase;
pub type JobTerminalKind = dsl::DetachedJobTerminalKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobSpec {
    pub realm_id: String,
    pub origin_session_id: SessionId,
    pub origin_member_id: Option<OriginMemberId>,
    pub execution_intent_id: ExecutionIntentId,
    pub interaction_lineage_id: InteractionLineageId,
    pub tool: ToolIdentity,
    pub runner: RunnerIdentity,
    pub restart_class: RestartClass,
    pub canonical_arguments_hash: CanonicalArgumentsHash,
    pub credential_context_refs: Vec<ToolCredentialContextRef>,
    pub submission_key: JobSubmissionKey,
}

impl JobSpec {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        realm_id: impl Into<String>,
        origin_session_id: SessionId,
        execution_intent_id: ExecutionIntentId,
        interaction_lineage_id: InteractionLineageId,
        tool: ToolIdentity,
        runner: RunnerIdentity,
        restart_class: RestartClass,
        canonical_arguments_hash: CanonicalArgumentsHash,
        submission_key: JobSubmissionKey,
    ) -> Self {
        Self {
            realm_id: realm_id.into(),
            origin_session_id,
            origin_member_id: None,
            execution_intent_id,
            interaction_lineage_id,
            tool,
            runner,
            restart_class,
            canonical_arguments_hash,
            credential_context_refs: Vec::new(),
            submission_key,
        }
    }

    pub fn with_origin_member_id(mut self, origin_member_id: OriginMemberId) -> Self {
        self.origin_member_id = Some(origin_member_id);
        self
    }

    pub fn with_credential_context_refs(
        mut self,
        credential_context_refs: Vec<ToolCredentialContextRef>,
    ) -> Self {
        self.credential_context_refs = credential_context_refs;
        self
    }

    pub(crate) fn equivalent_submission(&self, other: &Self) -> bool {
        self.realm_id == other.realm_id
            && self.origin_session_id == other.origin_session_id
            && self.origin_member_id == other.origin_member_id
            && self.execution_intent_id == other.execution_intent_id
            && self.interaction_lineage_id == other.interaction_lineage_id
            && self.tool == other.tool
            && self.runner == other.runner
            && self.restart_class == other.restart_class
            && self.canonical_arguments_hash == other.canonical_arguments_hash
            && self.credential_context_refs == other.credential_context_refs
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptClaim {
    pub worker_id: WorkerId,
    pub claimed_at_ms: u64,
    pub lease_expires_at_ms: u64,
    pub runner_handle: RunnerHandleRef,
}

impl AttemptClaim {
    pub fn new(
        worker_id: WorkerId,
        claimed_at_ms: u64,
        lease_expires_at_ms: u64,
        runner_handle: RunnerHandleRef,
    ) -> Self {
        Self {
            worker_id,
            claimed_at_ms,
            lease_expires_at_ms,
            runner_handle,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptClaimReceipt {
    pub attempt_id: AttemptId,
    pub attempt_count: u64,
    pub fence: FenceToken,
    pub lease_expires_at_ms: u64,
    pub resume_checkpoint: Option<CheckpointRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptWriteAuthority {
    pub attempt_id: AttemptId,
    pub fence: FenceToken,
}

impl From<&AttemptClaimReceipt> for AttemptWriteAuthority {
    fn from(value: &AttemptClaimReceipt) -> Self {
        Self {
            attempt_id: value.attempt_id.clone(),
            fence: value.fence,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobProgress {
    pub cursor: u64,
    pub detail: String,
}

impl JobProgress {
    pub fn new(cursor: u64, detail: impl Into<String>) -> Result<Self, DetachedJobError> {
        if cursor == 0 {
            return Err(DetachedJobError::InvalidInput(
                "progress cursor must be positive".into(),
            ));
        }
        Ok(Self {
            cursor,
            detail: validate_component("progress detail", detail.into())?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobReceipt {
    pub job_id: JobId,
    pub deduplicated: bool,
    pub restart_class: RestartClass,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobOutboxEntry {
    pub job_id: JobId,
    pub delivery_sequence: u64,
    pub terminal_kind: JobTerminalKind,
    pub terminal_result: JobTerminalResult,
    pub applied: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobTerminalResult {
    Succeeded {
        result_ref: Option<JobResultRef>,
    },
    Failed {
        code: JobFailureCode,
        detail_ref: Option<JobResultRef>,
    },
    Cancelled,
    WorkerLost,
    NeedsAttention {
        reason: JobFailureCode,
    },
}

impl JobTerminalResult {
    pub const fn kind(&self) -> JobTerminalKind {
        match self {
            Self::Succeeded { .. } => JobTerminalKind::Succeeded,
            Self::Failed { .. } => JobTerminalKind::Failed,
            Self::Cancelled => JobTerminalKind::Cancelled,
            Self::WorkerLost => JobTerminalKind::WorkerLost,
            Self::NeedsAttention { .. } => JobTerminalKind::NeedsAttention,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobSnapshot {
    pub job_id: JobId,
    pub revision: u64,
    pub phase: JobPhase,
    pub attempt_count: u64,
    pub current_attempt_id: Option<AttemptId>,
    pub current_fence: FenceToken,
    pub lease_expires_at_ms: Option<u64>,
    pub checkpoint_ref: Option<CheckpointRef>,
    pub runner_handle: Option<RunnerHandleRef>,
    pub progress: Option<JobProgress>,
    pub cancel_requested: bool,
    pub terminal_kind: Option<JobTerminalKind>,
    pub terminal_result: Option<JobTerminalResult>,
    pub outbox: Vec<JobOutboxEntry>,
}
