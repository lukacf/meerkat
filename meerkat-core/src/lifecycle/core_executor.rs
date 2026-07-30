//! CoreExecutor trait — the interface core exposes to the runtime layer.
//!
//! The runtime layer implements this trait (as `AgentCoreExecutor`) to bridge
//! RunPrimitive into Agent session mutations. The trait lives in core so both
//! layers can reference it without circular dependencies.

use super::RunId;
use super::run_primitive::RunPrimitive;
use super::run_receipt::RunBoundaryReceiptDraft;
use crate::error::AgentError;
use crate::lifecycle::run_primitive::TurnRequestContext;
use crate::service::SessionError;
use crate::turn_execution_authority::{TurnTerminalCauseKind, TurnTerminalOutcome};
use crate::types::RunResult;
use crate::{TurnErrorMetadata, event::AgentEvent, interaction::InteractionId};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Closed classifier for failures observed while applying a run primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CoreApplyFailureCauseKind {
    PrimitiveRejected,
    RuntimeContextApply,
    RuntimeTurn,
    HookDenied,
    HookRuntimeFailure,
    ExecutorStopped,
    ExecutorControlFailed,
    ExecutorInternal,
    Unknown,
}

impl CoreApplyFailureCauseKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PrimitiveRejected => "PrimitiveRejected",
            Self::RuntimeContextApply => "RuntimeContextApply",
            Self::RuntimeTurn => "RuntimeTurn",
            Self::HookDenied => "HookDenied",
            Self::HookRuntimeFailure => "HookRuntimeFailure",
            Self::ExecutorStopped => "ExecutorStopped",
            Self::ExecutorControlFailed => "ExecutorControlFailed",
            Self::ExecutorInternal => "ExecutorInternal",
            Self::Unknown => "Unknown",
        }
    }

    pub fn from_wire_str(value: &str) -> Option<Self> {
        match value {
            "PrimitiveRejected" => Some(Self::PrimitiveRejected),
            "RuntimeContextApply" => Some(Self::RuntimeContextApply),
            "RuntimeTurn" => Some(Self::RuntimeTurn),
            "HookDenied" => Some(Self::HookDenied),
            "HookRuntimeFailure" => Some(Self::HookRuntimeFailure),
            "ExecutorStopped" => Some(Self::ExecutorStopped),
            "ExecutorControlFailed" => Some(Self::ExecutorControlFailed),
            "ExecutorInternal" => Some(Self::ExecutorInternal),
            "Unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// Typed apply-failure cause plus its human-readable display projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreApplyFailureCause {
    pub kind: CoreApplyFailureCauseKind,
    pub message: String,
}

impl CoreApplyFailureCause {
    pub fn new(kind: CoreApplyFailureCauseKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn primitive_rejected(message: impl Into<String>) -> Self {
        Self::new(CoreApplyFailureCauseKind::PrimitiveRejected, message)
    }

    pub fn runtime_context_apply(message: impl Into<String>) -> Self {
        Self::new(CoreApplyFailureCauseKind::RuntimeContextApply, message)
    }

    pub fn runtime_turn(message: impl Into<String>) -> Self {
        Self::new(CoreApplyFailureCauseKind::RuntimeTurn, message)
    }

    pub fn hook_denied(message: impl Into<String>) -> Self {
        Self::new(CoreApplyFailureCauseKind::HookDenied, message)
    }

    pub fn hook_runtime_failure(message: impl Into<String>) -> Self {
        Self::new(CoreApplyFailureCauseKind::HookRuntimeFailure, message)
    }

    pub fn executor_stopped() -> Self {
        Self::new(
            CoreApplyFailureCauseKind::ExecutorStopped,
            "executor is stopped",
        )
    }

    pub fn executor_control_failed(message: impl Into<String>) -> Self {
        Self::new(CoreApplyFailureCauseKind::ExecutorControlFailed, message)
    }

    pub fn executor_internal(message: impl Into<String>) -> Self {
        Self::new(CoreApplyFailureCauseKind::ExecutorInternal, message)
    }

    pub fn unknown(message: impl Into<String>) -> Self {
        Self::new(CoreApplyFailureCauseKind::Unknown, message)
    }

    pub fn from_agent_error(error: &AgentError) -> Self {
        match error {
            AgentError::HookDenied { .. } => Self::hook_denied(error.to_string()),
            AgentError::HookTimeout { .. }
            | AgentError::HookExecutionFailed { .. }
            | AgentError::HookConfigInvalid { .. } => Self::hook_runtime_failure(error.to_string()),
            _ => Self::runtime_turn(error.to_string()),
        }
    }

    pub fn from_session_error(error: &SessionError) -> Self {
        match error {
            SessionError::Agent(agent_error) => Self::from_agent_error(agent_error),
            _ => Self::runtime_turn(error.to_string()),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for CoreApplyFailureCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Closed classifier for failures observed while applying control commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CoreControlFailureCauseKind {
    RuntimeControl,
    ExecutorInternal,
    Unknown,
}

/// Typed control-failure cause plus its human-readable display projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreControlFailureCause {
    pub kind: CoreControlFailureCauseKind,
    pub message: String,
}

/// Machine-independent reason an executor can no longer own its live session.
///
/// This is a handoff request, not an ordinary apply failure: the runtime loop
/// must close the staged run, publish the exact executor, and let the
/// machine-owned unregister saga perform external cleanup. Executors must not
/// call unregister (or discard their session) from inside `apply`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CoreExecutorTeardownReason {
    ArchivedSession,
    SessionUnavailable,
    DurableProjectionAuthorityUnknown,
}

impl CoreExecutorTeardownReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ArchivedSession => "ArchivedSession",
            Self::SessionUnavailable => "SessionUnavailable",
            Self::DurableProjectionAuthorityUnknown => "DurableProjectionAuthorityUnknown",
        }
    }

    pub fn from_wire_str(value: &str) -> Option<Self> {
        match value {
            "ArchivedSession" => Some(Self::ArchivedSession),
            "SessionUnavailable" => Some(Self::SessionUnavailable),
            "DurableProjectionAuthorityUnknown" => Some(Self::DurableProjectionAuthorityUnknown),
            _ => None,
        }
    }
}

impl CoreControlFailureCause {
    pub fn new(kind: CoreControlFailureCauseKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn runtime_control(message: impl Into<String>) -> Self {
        Self::new(CoreControlFailureCauseKind::RuntimeControl, message)
    }

    pub fn executor_internal(message: impl Into<String>) -> Self {
        Self::new(CoreControlFailureCauseKind::ExecutorInternal, message)
    }

    pub fn unknown(message: impl Into<String>) -> Self {
        Self::new(CoreControlFailureCauseKind::Unknown, message)
    }
}

impl std::fmt::Display for CoreControlFailureCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Errors from CoreExecutor operations.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum CoreExecutorError {
    /// The primitive could not be applied (conversation mutation failed).
    #[error("Apply failed: {cause}")]
    ApplyFailed { cause: CoreApplyFailureCause },

    /// The core executor observed a machine-owned terminal turn failure while
    /// applying a runtime turn. The runtime loop must preserve this typed
    /// terminal cause instead of reclassifying it as a runtime apply failure.
    #[error("Terminal failure: {outcome:?} ({cause_kind:?}): {message}")]
    TerminalFailure {
        outcome: TurnTerminalOutcome,
        cause_kind: TurnTerminalCauseKind,
        message: String,
    },

    /// The executor's owned session reached a terminal/unavailable condition
    /// that requires canonical teardown after the runtime loop hands off the
    /// exact executor. This variant must never enter failed-batch backlog
    /// retry, and must never be realized by unregistering inside `apply`.
    #[error("Executor requires teardown ({reason:?}): {message}")]
    TeardownRequired {
        reason: CoreExecutorTeardownReason,
        message: String,
    },

    /// The control command could not be executed.
    #[error("Control failed: {cause}")]
    ControlFailed { cause: CoreControlFailureCause },

    /// The executor is in a terminal state and cannot accept more work.
    #[error("Executor is stopped")]
    Stopped,

    /// The applied turn reached the canonical cancellation terminal.
    #[error("Run was cancelled")]
    Cancelled,

    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),
}

impl CoreExecutorError {
    pub fn apply_failed(cause: CoreApplyFailureCause) -> Self {
        Self::ApplyFailed { cause }
    }

    pub fn apply_failed_primitive_rejected(message: impl Into<String>) -> Self {
        Self::apply_failed(CoreApplyFailureCause::primitive_rejected(message))
    }

    pub fn apply_failed_runtime_context(message: impl Into<String>) -> Self {
        Self::apply_failed(CoreApplyFailureCause::runtime_context_apply(message))
    }

    pub fn apply_failed_runtime_turn(message: impl Into<String>) -> Self {
        Self::apply_failed(CoreApplyFailureCause::runtime_turn(message))
    }

    pub fn terminal_failure(
        outcome: TurnTerminalOutcome,
        cause_kind: TurnTerminalCauseKind,
        message: impl Into<String>,
    ) -> Self {
        Self::TerminalFailure {
            outcome,
            cause_kind,
            message: message.into(),
        }
    }

    pub fn teardown_required(
        reason: CoreExecutorTeardownReason,
        message: impl Into<String>,
    ) -> Self {
        Self::TeardownRequired {
            reason,
            message: message.into(),
        }
    }

    pub fn archived_session_requires_teardown(message: impl Into<String>) -> Self {
        Self::teardown_required(CoreExecutorTeardownReason::ArchivedSession, message)
    }

    pub fn session_unavailable_requires_teardown(message: impl Into<String>) -> Self {
        Self::teardown_required(CoreExecutorTeardownReason::SessionUnavailable, message)
    }

    pub fn durable_projection_authority_unknown_requires_teardown(
        message: impl Into<String>,
    ) -> Self {
        Self::teardown_required(
            CoreExecutorTeardownReason::DurableProjectionAuthorityUnknown,
            message,
        )
    }

    pub fn apply_failed_from_session_error(error: SessionError) -> Self {
        if error.requests_runtime_executor_stop() {
            return Self::Stopped;
        }
        match error {
            SessionError::Agent(AgentError::Cancelled) => Self::Cancelled,
            SessionError::Agent(AgentError::StickyModelFallbackAuthorityUnknown { message }) => {
                Self::session_unavailable_requires_teardown(message)
            }
            SessionError::Agent(AgentError::SessionDurableProjectionAuthorityUnknown {
                message,
            }) => Self::durable_projection_authority_unknown_requires_teardown(message),
            SessionError::Agent(AgentError::TerminalFailure {
                outcome,
                cause_kind,
                message,
            }) if cause_kind.is_specific_failure_cause() => {
                Self::terminal_failure(outcome, cause_kind, message)
            }
            SessionError::Agent(AgentError::TerminalFailure { cause_kind, .. }) => Self::Internal(
                format!("runtime turn returned unknown machine terminal cause: {cause_kind:?}"),
            ),
            error => Self::apply_failed(CoreApplyFailureCause::from_session_error(&error)),
        }
    }

    pub fn apply_failed_unknown(message: impl Into<String>) -> Self {
        Self::apply_failed(CoreApplyFailureCause::unknown(message))
    }

    pub fn cancelled() -> Self {
        Self::Cancelled
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    pub fn requires_runtime_teardown(&self) -> bool {
        matches!(self, Self::TeardownRequired { .. })
    }

    pub fn control_failed(cause: CoreControlFailureCause) -> Self {
        Self::ControlFailed { cause }
    }

    pub fn control_failed_runtime(message: impl Into<String>) -> Self {
        Self::control_failed(CoreControlFailureCause::runtime_control(message))
    }

    pub fn apply_failure_cause(&self) -> CoreApplyFailureCause {
        match self {
            Self::ApplyFailed { cause } => cause.clone(),
            Self::TerminalFailure { cause_kind, .. } => {
                CoreApplyFailureCause::executor_internal(format!(
                    "typed machine terminal failure escaped runtime-loop handling: {cause_kind:?}"
                ))
            }
            Self::TeardownRequired { reason, message } => CoreApplyFailureCause::new(
                CoreApplyFailureCauseKind::ExecutorStopped,
                format!("executor requested {} teardown: {message}", reason.as_str()),
            ),
            Self::ControlFailed { cause } => {
                CoreApplyFailureCause::executor_control_failed(cause.message.clone())
            }
            Self::Stopped => CoreApplyFailureCause::executor_stopped(),
            Self::Cancelled => CoreApplyFailureCause::runtime_turn("cancelled"),
            Self::Internal(message) => CoreApplyFailureCause::executor_internal(message.clone()),
        }
    }
}

/// Successful result of applying a run primitive.
#[derive(Debug, Clone)]
pub enum CoreApplyTerminal {
    /// The run completed and produced a result.
    RunResult(Box<RunResult>),
    /// A resume-pending request reached the session with no pending boundary.
    NoPendingBoundary,
    /// The exact admitted runtime turn reached a generated hard-failure
    /// terminal after mutating the session. The runtime must atomically commit
    /// the accompanying receipt/session snapshot with failed-run lifecycle;
    /// this is a completed application, not an executor-mechanism error.
    MachineTerminalFailure { error: TurnErrorMetadata },
    /// The run committed a continuation boundary and is waiting for external
    /// tool results before it can continue.
    CallbackPending {
        tool_use_id: String,
        tool_name: String,
        args: Value,
    },
    /// The run committed one assistant batch containing multiple external
    /// callback calls. All results must be supplied as one exact set.
    CallbackBatchPending {
        pending_tool_calls: Vec<crate::error::PendingCallbackToolCall>,
    },
}

/// Failure to materialize the whole-blob representation of a prepared session
/// boundary.
///
/// `serde_json::Error` is not cloneable, while the single-assignment lazy cell
/// must publish the same terminal result to every racing reader. Preserve its
/// diagnostic text in a cloneable typed error instead of retrying serialization
/// after a failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("failed to encode prepared session boundary: {message}")]
pub struct SessionBoundaryEncodeError {
    message: std::sync::Arc<str>,
}

impl SessionBoundaryEncodeError {
    fn from_serde(error: serde_json::Error) -> Self {
        Self {
            message: std::sync::Arc::from(error.to_string()),
        }
    }

    /// The serializer diagnostic retained by the prepared boundary.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// One sealed physical mutation admitted at a HeadCanonical boundary.
///
/// Ordinary appends and same-session rewrites remain disjoint carriers. The
/// shared accessors expose only the exact authority facts needed by runtime
/// adoption; stores must match the variant before consuming physical rows.
#[derive(Debug, Clone)]
pub enum PreparedHeadCanonicalPhysicalMutation {
    Ordinary(crate::session_store::PreparedHeadCanonicalMutation),
    Rewrite(crate::session_store::PreparedHeadCanonicalRewriteMutation),
}

impl PreparedHeadCanonicalPhysicalMutation {
    #[must_use]
    pub fn session_id(&self) -> &crate::types::SessionId {
        match self {
            Self::Ordinary(mutation) => mutation.session_id(),
            Self::Rewrite(mutation) => mutation.session_id(),
        }
    }

    #[must_use]
    pub fn predecessor_head(&self) -> Option<&crate::session_store::SessionHead> {
        match self {
            Self::Ordinary(mutation) => mutation.predecessor_head(),
            Self::Rewrite(mutation) => Some(mutation.predecessor_head()),
        }
    }

    #[must_use]
    pub fn predecessor_head_token(&self) -> Option<&str> {
        match self {
            Self::Ordinary(mutation) => mutation.predecessor_head_token(),
            Self::Rewrite(mutation) => Some(mutation.predecessor_head_token()),
        }
    }

    #[must_use]
    pub fn successor_head(&self) -> &crate::session_store::SessionHead {
        match self {
            Self::Ordinary(mutation) => mutation.successor_head(),
            Self::Rewrite(mutation) => mutation.successor_head(),
        }
    }

    #[must_use]
    pub fn successor_head_token(&self) -> &str {
        match self {
            Self::Ordinary(mutation) => mutation.successor_head_token(),
            Self::Rewrite(mutation) => mutation.successor_head_token(),
        }
    }

    #[must_use]
    pub fn ordinary(&self) -> Option<&crate::session_store::PreparedHeadCanonicalMutation> {
        match self {
            Self::Ordinary(mutation) => Some(mutation),
            Self::Rewrite(_) => None,
        }
    }

    #[must_use]
    pub fn rewrite(&self) -> Option<&crate::session_store::PreparedHeadCanonicalRewriteMutation> {
        match self {
            Self::Ordinary(_) => None,
            Self::Rewrite(mutation) => Some(mutation),
        }
    }

    pub(crate) fn validate_live_successor(
        &self,
        session: &crate::Session,
    ) -> Result<(), crate::SessionStoreError> {
        match self {
            Self::Ordinary(mutation) => mutation.validate_live_successor(session),
            Self::Rewrite(mutation) => mutation.validate_live_successor(session),
        }
    }

    pub fn acknowledge_session(
        &self,
        session: &mut crate::Session,
        committed_head_token: &str,
    ) -> Result<(), crate::SessionStoreError> {
        match self {
            Self::Ordinary(mutation) => mutation.acknowledge_session(session, committed_head_token),
            Self::Rewrite(mutation) => mutation.acknowledge_session(session, committed_head_token),
        }
    }
}

impl From<crate::session_store::PreparedHeadCanonicalMutation>
    for PreparedHeadCanonicalPhysicalMutation
{
    fn from(mutation: crate::session_store::PreparedHeadCanonicalMutation) -> Self {
        Self::Ordinary(mutation)
    }
}

impl From<crate::session_store::PreparedHeadCanonicalRewriteMutation>
    for PreparedHeadCanonicalPhysicalMutation
{
    fn from(mutation: crate::session_store::PreparedHeadCanonicalRewriteMutation) -> Self {
        Self::Rewrite(mutation)
    }
}

/// A bounded store-prepared HeadCanonical mutation.
///
/// Runtime/store authority is deliberately absent. The store transaction owns
/// predecessor observation, fencing, and the committed receipt; this carrier
/// binds only the already-prepared physical delta to its live domain Session.
#[derive(Debug, Clone)]
pub struct PreparedHeadCanonicalBoundary {
    mutation: PreparedHeadCanonicalPhysicalMutation,
    compaction_projection_intents: std::sync::Arc<[crate::CompactionProjectionIntent]>,
    catalog_labels: std::collections::BTreeMap<String, String>,
    catalog_lifecycle_terminal: Option<crate::SessionLifecycleTerminal>,
}

impl PreparedHeadCanonicalBoundary {
    #[must_use]
    pub fn mutation(&self) -> &PreparedHeadCanonicalPhysicalMutation {
        &self.mutation
    }

    /// Validated small outbox facts carried by the prepared successor.
    #[must_use]
    pub fn compaction_projection_intents(&self) -> &[crate::CompactionProjectionIntent] {
        self.compaction_projection_intents.as_ref()
    }

    /// Exact bounded label projection captured from the same live successor.
    #[must_use]
    pub fn catalog_labels(&self) -> &std::collections::BTreeMap<String, String> {
        &self.catalog_labels
    }

    /// Exact bounded lifecycle projection captured from the same live successor.
    #[must_use]
    pub const fn catalog_lifecycle_terminal(&self) -> Option<crate::SessionLifecycleTerminal> {
        self.catalog_lifecycle_terminal
    }
}

/// One disjoint session-persistence boundary.
///
/// Whole-blob backends receive either a typed document with lazy single-encode
/// bytes or explicitly untyped compatibility bytes. Head-canonical backends
/// receive only a sealed prepared suffix and small successor authority; that
/// variant cannot expose a `Session` or materialize whole-document bytes.
/// Keeping the variants disjoint makes an accidental O(document) fallback on
/// the ordinary O(delta) path a typed error rather than a performance
/// convention.
#[derive(Debug, Clone)]
enum BoundSessionCommitKind {
    WholeBlobTyped {
        session: std::sync::Arc<crate::Session>,
        whole_blob: std::sync::Arc<
            std::sync::OnceLock<
                Result<
                    std::sync::Arc<crate::SerializedSessionArtifact>,
                    SessionBoundaryEncodeError,
                >,
            >,
        >,
    },
    WholeBlobUntyped {
        whole_blob: std::sync::Arc<
            std::sync::OnceLock<
                Result<
                    std::sync::Arc<crate::SerializedSessionArtifact>,
                    SessionBoundaryEncodeError,
                >,
            >,
        >,
    },
    HeadCanonical {
        boundary: std::sync::Arc<PreparedHeadCanonicalBoundary>,
    },
    /// Final promotion of a provisional physical tail already written by the
    /// exact active run. This variant deliberately carries neither a Session
    /// nor a lazy WholeBlob artifact nor a HeadCanonical delta.
    ProvisionalPromotion {
        receipt: crate::RunCheckpointReceipt,
    },
}

#[derive(Debug, Clone)]
pub struct BoundSessionCommit {
    kind: BoundSessionCommitKind,
    #[cfg(test)]
    whole_blob_encode_count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl BoundSessionCommit {
    /// Seal a typed session as the exact document this boundary will commit.
    ///
    /// This mint intentionally does not serialize. Head-canonical stores can
    /// consume the typed document without ever constructing a whole blob;
    /// whole-blob stores materialize it exactly once through
    /// [`Self::whole_blob_bytes`].
    ///
    /// The fallible return is retained for source compatibility with callers
    /// that previously observed eager JSON serialization here. Construction no
    /// longer has a serialization failure mode.
    pub fn sealed(session: std::sync::Arc<crate::Session>) -> Result<Self, serde_json::Error> {
        Ok(Self {
            kind: BoundSessionCommitKind::WholeBlobTyped {
                session,
                whole_blob: std::sync::Arc::new(std::sync::OnceLock::new()),
            },
            #[cfg(test)]
            whole_blob_encode_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        })
    }

    /// Bytes carrying no typed certification: a consumer that needs a
    /// `Session` must deserialize and validate these bytes itself.
    #[must_use]
    pub fn untyped(snapshot: Vec<u8>) -> Self {
        Self {
            kind: BoundSessionCommitKind::WholeBlobUntyped {
                whole_blob: std::sync::Arc::new(std::sync::OnceLock::from(Ok(
                    std::sync::Arc::new(crate::SerializedSessionArtifact::from_raw_bytes(snapshot)),
                ))),
            },
            #[cfg(test)]
            whole_blob_encode_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// Seal the exact latest provisional receipt as the final persistence
    /// boundary for its run.
    #[must_use]
    pub fn provisional_promotion(receipt: crate::RunCheckpointReceipt) -> Self {
        Self {
            kind: BoundSessionCommitKind::ProvisionalPromotion { receipt },
            #[cfg(test)]
            whole_blob_encode_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// Convert a typed whole-blob carrier into a bounded head mutation.
    ///
    /// This compatibility constructor validates the typed successor and then
    /// drops it; the returned carrier is the disjoint head-only variant. New
    /// live actor paths should call [`Self::head_canonical_from_session`]
    /// directly while borrowing the actor-owned session.
    pub fn with_head_canonical_mutation(
        self,
        mutation: crate::session_store::PreparedHeadCanonicalMutation,
    ) -> Result<Self, crate::SessionStoreError> {
        let mutation_session_id = mutation.session_id().clone();
        let invalid = |reason: String| crate::SessionStoreError::InvalidTranscriptRewrite {
            id: mutation_session_id.clone(),
            reason,
        };
        let session = match &self.kind {
            BoundSessionCommitKind::WholeBlobTyped { session, .. } => {
                std::sync::Arc::clone(session)
            }
            BoundSessionCommitKind::WholeBlobUntyped { .. } => {
                return Err(invalid(
                    "head-canonical persistence requires a typed session boundary".to_string(),
                ));
            }
            BoundSessionCommitKind::HeadCanonical { .. } => {
                return Err(invalid(
                    "head-canonical mutation was already attached to this boundary".to_string(),
                ));
            }
            BoundSessionCommitKind::ProvisionalPromotion { .. } => {
                return Err(invalid(
                    "provisional promotion cannot be converted into a head-canonical mutation"
                        .to_string(),
                ));
            }
        };
        Self::head_canonical_from_session(session.as_ref(), mutation)
    }

    /// Mint a bounded head-canonical carrier from a borrowed live session.
    ///
    /// The session is used only while validating the prepared mutation and
    /// small compaction outbox facts. It is deliberately not retained by the
    /// returned carrier: ordinary head-canonical persistence must never turn
    /// an O(delta) suffix into an O(document) `Session` clone or whole-blob
    /// encode merely to cross the runtime boundary.
    pub fn head_canonical_from_session(
        session: &crate::Session,
        mutation: crate::session_store::PreparedHeadCanonicalMutation,
    ) -> Result<Self, crate::SessionStoreError> {
        Self::head_canonical_physical_from_session(session, mutation.into())
    }

    /// Mint a bounded HeadCanonical carrier from either disjoint physical
    /// mutation kind.
    pub fn head_canonical_physical_from_session(
        session: &crate::Session,
        mutation: PreparedHeadCanonicalPhysicalMutation,
    ) -> Result<Self, crate::SessionStoreError> {
        let boundary = Self::prepare_head_canonical_boundary(session, mutation)?;
        Ok(Self {
            kind: BoundSessionCommitKind::HeadCanonical {
                boundary: std::sync::Arc::new(boundary),
            },
            #[cfg(test)]
            whole_blob_encode_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        })
    }

    /// Mint a bounded same-session rewrite carrier from a borrowed live
    /// session without retaining or encoding the accumulated document.
    pub fn head_canonical_rewrite_from_session(
        session: &crate::Session,
        mutation: crate::session_store::PreparedHeadCanonicalRewriteMutation,
    ) -> Result<Self, crate::SessionStoreError> {
        Self::head_canonical_physical_from_session(session, mutation.into())
    }

    fn prepare_head_canonical_boundary(
        session: &crate::Session,
        mutation: PreparedHeadCanonicalPhysicalMutation,
    ) -> Result<PreparedHeadCanonicalBoundary, crate::SessionStoreError> {
        let mutation_session_id = mutation.session_id().clone();
        let invalid = |reason: String| crate::SessionStoreError::InvalidTranscriptRewrite {
            id: mutation_session_id.clone(),
            reason,
        };
        if session.id() != mutation.session_id() {
            return Err(invalid(format!(
                "prepared mutation belongs to session {}, not sealed session {}",
                mutation.session_id(),
                session.id()
            )));
        }

        mutation.validate_live_successor(session)?;

        let compaction_projection_intents = session
            .validated_compaction_projection_intents()
            .map_err(|error| {
                invalid(format!(
                    "head-canonical successor carries invalid compaction projection intents: {error}"
                ))
            })?
            .into();
        let catalog_labels = session
            .metadata()
            .get("session_labels")
            .map(|value| {
                serde_json::from_value::<std::collections::BTreeMap<String, String>>(value.clone())
                    .map_err(|error| {
                        invalid(format!(
                            "head-canonical successor carries malformed catalog labels: {error}"
                        ))
                    })
            })
            .transpose()?
            .unwrap_or_default();
        let catalog_lifecycle_terminal = session.try_lifecycle_terminal().map_err(|error| {
            invalid(format!(
                "head-canonical successor carries malformed lifecycle-terminal metadata: {error}"
            ))
        })?;

        Ok(PreparedHeadCanonicalBoundary {
            mutation,
            compaction_projection_intents,
            catalog_labels,
            catalog_lifecycle_terminal,
        })
    }

    /// Prepared bounded mutation and independent authority proofs, when this
    /// boundary is eligible for `HeadCanonicalV1`.
    #[must_use]
    pub fn head_canonical(&self) -> Option<&PreparedHeadCanonicalBoundary> {
        match &self.kind {
            BoundSessionCommitKind::HeadCanonical { boundary } => Some(boundary.as_ref()),
            BoundSessionCommitKind::WholeBlobTyped { .. }
            | BoundSessionCommitKind::WholeBlobUntyped { .. }
            | BoundSessionCommitKind::ProvisionalPromotion { .. } => None,
        }
    }

    /// Store-issued provisional physical identity carried by a final promotion
    /// boundary.
    #[must_use]
    pub fn provisional_promotion_receipt(&self) -> Option<&crate::RunCheckpointReceipt> {
        match &self.kind {
            BoundSessionCommitKind::ProvisionalPromotion { receipt } => Some(receipt),
            BoundSessionCommitKind::WholeBlobTyped { .. }
            | BoundSessionCommitKind::WholeBlobUntyped { .. }
            | BoundSessionCommitKind::HeadCanonical { .. } => None,
        }
    }

    /// Verify that an acknowledgement names this exact prepared successor.
    ///
    /// The head-only carrier does not retain the live session. The actor owner
    /// applies only the prepared row/component acknowledgement after this exact
    /// store-issued token check succeeds.
    pub fn acknowledge_head_canonical_commit(
        &self,
        committed_head_cas_token: &str,
    ) -> Result<(), crate::SessionStoreError> {
        let boundary = self.head_canonical().ok_or_else(|| {
            crate::SessionStoreError::Internal(
                "session boundary has no head-canonical mutation to acknowledge".to_string(),
            )
        })?;
        if boundary.mutation().successor_head_token() != committed_head_cas_token {
            return Err(crate::SessionStoreError::TranscriptRevisionConflict {
                id: boundary.mutation().session_id().clone(),
                expected: boundary.mutation().successor_head_token().to_string(),
                actual: committed_head_cas_token.to_string(),
            });
        }
        Ok(())
    }

    /// Materialize the whole-blob representation, if the selected backend
    /// requires one.
    ///
    /// A typed carrier serializes its exact `Session` into this single-assignment
    /// buffer. An untyped carrier returns the bytes supplied to
    /// [`Self::untyped`]. Calling this on the disjoint head-canonical variant
    /// is a typed error.
    pub fn whole_blob_bytes(&self) -> Result<&[u8], SessionBoundaryEncodeError> {
        let (whole_blob, session) = match &self.kind {
            BoundSessionCommitKind::WholeBlobTyped {
                session,
                whole_blob,
            } => (whole_blob, Some(session)),
            BoundSessionCommitKind::WholeBlobUntyped { whole_blob } => (whole_blob, None),
            BoundSessionCommitKind::HeadCanonical { .. } => {
                return Err(SessionBoundaryEncodeError {
                    message: std::sync::Arc::from(
                        "head-canonical boundary has no whole-blob representation",
                    ),
                });
            }
            BoundSessionCommitKind::ProvisionalPromotion { .. } => {
                return Err(SessionBoundaryEncodeError {
                    message: std::sync::Arc::from(
                        "provisional promotion boundary has no whole-blob representation",
                    ),
                });
            }
        };
        whole_blob
            .get_or_init(|| {
                let Some(session) = session else {
                    return Err(SessionBoundaryEncodeError {
                        message: std::sync::Arc::from(
                            "untyped whole-blob carrier lost its compatibility bytes",
                        ),
                    });
                };
                let snapshot = session
                    .to_persisted_artifact()
                    .map_err(SessionBoundaryEncodeError::from_serde)?;
                #[cfg(test)]
                self.whole_blob_encode_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(std::sync::Arc::new(snapshot))
            })
            .as_ref()
            .map(|snapshot| snapshot.bytes())
            .map_err(Clone::clone)
    }

    /// The sealed WholeBlob bytes together with their single-pass physical
    /// row digest.
    ///
    /// Runtime/store WholeBlob paths should consume this artifact directly
    /// and reuse [`crate::SerializedSessionArtifact::row_sha256_token`] rather
    /// than hashing [`Self::whole_blob_bytes`] again.
    pub fn whole_blob_artifact(
        &self,
    ) -> Result<&crate::SerializedSessionArtifact, SessionBoundaryEncodeError> {
        let _ = self.whole_blob_bytes()?;
        let whole_blob = match &self.kind {
            BoundSessionCommitKind::WholeBlobTyped { whole_blob, .. }
            | BoundSessionCommitKind::WholeBlobUntyped { whole_blob } => whole_blob,
            BoundSessionCommitKind::HeadCanonical { .. } => {
                return Err(SessionBoundaryEncodeError {
                    message: std::sync::Arc::from(
                        "head-canonical boundary has no whole-blob representation",
                    ),
                });
            }
            BoundSessionCommitKind::ProvisionalPromotion { .. } => {
                return Err(SessionBoundaryEncodeError {
                    message: std::sync::Arc::from(
                        "provisional promotion boundary has no whole-blob representation",
                    ),
                });
            }
        };
        match whole_blob.get() {
            Some(Ok(artifact)) => Ok(artifact.as_ref()),
            Some(Err(error)) => Err(error.clone()),
            None => Err(SessionBoundaryEncodeError {
                message: std::sync::Arc::from(
                    "whole-blob cell remained empty after successful materialization",
                ),
            }),
        }
    }

    /// Consume this carrier into a shared whole-blob representation.
    ///
    /// This is the owned counterpart to [`Self::whole_blob_bytes`]. It avoids
    /// copying an already materialized blob; compatibility APIs that still
    /// require `Vec<u8>` may need one final bridge copy.
    pub fn into_whole_blob_bytes(
        self,
    ) -> Result<std::sync::Arc<Vec<u8>>, SessionBoundaryEncodeError> {
        let _ = self.whole_blob_bytes()?;
        let whole_blob = match &self.kind {
            BoundSessionCommitKind::WholeBlobTyped { whole_blob, .. }
            | BoundSessionCommitKind::WholeBlobUntyped { whole_blob } => whole_blob,
            BoundSessionCommitKind::HeadCanonical { .. } => {
                return Err(SessionBoundaryEncodeError {
                    message: std::sync::Arc::from(
                        "head-canonical boundary has no whole-blob representation",
                    ),
                });
            }
            BoundSessionCommitKind::ProvisionalPromotion { .. } => {
                return Err(SessionBoundaryEncodeError {
                    message: std::sync::Arc::from(
                        "provisional promotion boundary has no whole-blob representation",
                    ),
                });
            }
        };
        match whole_blob.get() {
            Some(Ok(snapshot)) => Ok(snapshot.bytes_arc()),
            Some(Err(error)) => Err(error.clone()),
            None => Err(SessionBoundaryEncodeError {
                message: std::sync::Arc::from(
                    "whole-blob cell remained empty after successful materialization",
                ),
            }),
        }
    }

    #[cfg(test)]
    fn whole_blob_encode_count(&self) -> usize {
        self.whole_blob_encode_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The typed session, when the producer certified a WholeBlob document;
    /// identical by construction to [`Self::whole_blob_bytes`].
    #[must_use]
    pub fn session(&self) -> Option<&crate::Session> {
        match &self.kind {
            BoundSessionCommitKind::WholeBlobTyped { session, .. } => Some(session.as_ref()),
            BoundSessionCommitKind::WholeBlobUntyped { .. }
            | BoundSessionCommitKind::HeadCanonical { .. }
            | BoundSessionCommitKind::ProvisionalPromotion { .. } => None,
        }
    }

    /// Borrow the certified session as a shared handle.
    #[must_use]
    pub fn session_arc(&self) -> Option<&std::sync::Arc<crate::Session>> {
        match &self.kind {
            BoundSessionCommitKind::WholeBlobTyped { session, .. } => Some(session),
            BoundSessionCommitKind::WholeBlobUntyped { .. }
            | BoundSessionCommitKind::HeadCanonical { .. }
            | BoundSessionCommitKind::ProvisionalPromotion { .. } => None,
        }
    }

    /// Clone the shared handle to the certified Session without consuming this
    /// carrier or reparsing its WholeBlob bytes.
    #[must_use]
    pub fn session_arc_cloned(&self) -> Option<std::sync::Arc<crate::Session>> {
        self.session_arc().cloned()
    }

    /// Consume the pair into the certified session handle, if any.
    #[must_use]
    pub fn into_session_arc(self) -> Option<std::sync::Arc<crate::Session>> {
        match self.kind {
            BoundSessionCommitKind::WholeBlobTyped { session, .. } => Some(session),
            BoundSessionCommitKind::WholeBlobUntyped { .. }
            | BoundSessionCommitKind::HeadCanonical { .. }
            | BoundSessionCommitKind::ProvisionalPromotion { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CoreApplyOutput {
    /// Unsequenced receipt proving boundary application. The runtime driver
    /// mints the final sequenced [`super::run_receipt::RunBoundaryReceipt`]
    /// from the generated machine's per-run boundary counter at commit time
    /// (dogma K10 — executors cannot produce the boundary sequence).
    pub receipt: RunBoundaryReceiptDraft,
    /// The session persistence mutation to commit atomically with the receipt
    /// and input-state updates, held as one disjoint sealed value.
    ///
    /// Private, and readable only through [`Self::committed`] /
    /// [`Self::whole_blob_bytes`] / [`Self::session`]: as two assignable `pub`
    /// halves the seal was a convention a producer could break by overwriting
    /// the bytes after attaching the typed session, or by moving a typed
    /// session into a struct literal beside foreign bytes. One private field
    /// makes re-pairing unrepresentable — a consumer that validates the typed
    /// half and persists the bytes is validating and persisting the same
    /// document by construction.
    ///
    /// Whole-blob variants preserve typed/byte pairing and pay for at most one
    /// encode. The head-canonical variant contains no `Session` at all: it
    /// carries only the prepared suffix and successor authority, so neither a
    /// consumer nor an error fallback can accidentally turn an ordinary
    /// append into O(document) work.
    committed: Option<BoundSessionCommit>,
    /// Terminal payload observation produced by runtime-backed execution.
    ///
    /// `None` means the primitive committed successfully but did not produce
    /// a result payload (for example immediate context appends). Runtime
    /// surfaces must route this payload shape through generated machine
    /// authority before choosing a public completion result class.
    pub terminal: Option<CoreApplyTerminal>,
}

/// Durable receipt for one exact interaction-terminal publication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreInteractionTerminalPublicationReceipt {
    interaction_id: InteractionId,
    terminal_seq: u64,
    payload_digest: String,
}

impl CoreInteractionTerminalPublicationReceipt {
    pub fn try_new(event: &AgentEvent, terminal_seq: u64) -> Result<Self, CoreExecutorError> {
        if terminal_seq == 0 {
            return Err(CoreExecutorError::Internal(
                "interaction terminal durable sequence must be non-zero".to_string(),
            ));
        }
        let interaction_id = match event {
            AgentEvent::InteractionComplete { interaction_id, .. }
            | AgentEvent::InteractionCallbackPending { interaction_id, .. }
            | AgentEvent::InteractionFailed { interaction_id, .. } => *interaction_id,
            _ => {
                return Err(CoreExecutorError::Internal(
                    "interaction terminal publication receipt requires an Interaction terminal event"
                        .to_string(),
                ));
            }
        };
        let encoded = serde_json::to_vec(event).map_err(|error| {
            CoreExecutorError::Internal(format!(
                "failed to encode interaction terminal publication receipt: {error}"
            ))
        })?;
        Ok(Self {
            interaction_id,
            terminal_seq,
            payload_digest: format!("{:x}", Sha256::digest(encoded)),
        })
    }

    pub fn interaction_id(&self) -> InteractionId {
        self.interaction_id
    }

    pub fn terminal_seq(&self) -> u64 {
        self.terminal_seq
    }

    pub fn payload_digest(&self) -> &str {
        &self.payload_digest
    }
}

/// Typed failure while preparing or resolving an exact live turn boundary.
///
/// Only [`CoreBoundaryStageError::Unavailable`] permits a caller to fall back
/// to queued delivery. `Stale` means an exact actor/run/generation witness was
/// invalidated, while `Fault` means the preparation mechanism itself failed;
/// neither may be laundered into ordinary unavailability.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoreBoundaryStageError {
    #[error("active turn boundary is unavailable: {reason}")]
    Unavailable { reason: String },
    #[error("active turn boundary authority is stale: {reason}")]
    Stale { reason: String },
    #[error("active turn boundary preparation failed: {reason}")]
    Fault { reason: String },
}

impl CoreBoundaryStageError {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self::Unavailable {
            reason: reason.into(),
        }
    }

    pub fn stale(reason: impl Into<String>) -> Self {
        Self::Stale {
            reason: reason.into(),
        }
    }

    pub fn fault(reason: impl Into<String>) -> Self {
        Self::Fault {
            reason: reason.into(),
        }
    }

    #[must_use]
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable { .. })
    }
}

pub(crate) trait CoreBoundaryStageCommitAuthority: Send {
    fn commit(&mut self) -> Result<(), CoreBoundaryStageError>;
    fn abort(&mut self) -> Result<(), CoreBoundaryStageError>;
}

/// Successful prepare result for one exact parked model boundary.
///
/// The value is deliberately non-`Clone` and `#[must_use]`: it owns the only
/// commit/abort authority for the parked `{actor, run, generation}`. Dropping
/// it synchronously aborts the preparation and wakes the runner.
///
/// `commit` is the publication linearization point, not a claim that the LLM
/// consumed the context. A hard cancel that linearizes after publication but
/// before the runner's final synchronous consume still cancels that
/// active-turn-only context; the runner-owned consumption witness distinguishes
/// those outcomes.
#[must_use = "a prepared boundary must be committed or aborted; dropping it aborts"]
pub struct CoreBoundaryStageOutput {
    /// Optional serialized session snapshot to commit atomically with the
    /// generated receipt and input-state updates.
    session_snapshot: Option<Vec<u8>>,
    authority: Option<Box<dyn CoreBoundaryStageCommitAuthority>>,
}

impl CoreBoundaryStageOutput {
    pub(crate) fn prepared(
        session_snapshot: Option<Vec<u8>>,
        authority: Box<dyn CoreBoundaryStageCommitAuthority>,
    ) -> Self {
        Self {
            session_snapshot,
            authority: Some(authority),
        }
    }

    #[must_use]
    pub fn session_snapshot(&self) -> Option<&[u8]> {
        self.session_snapshot.as_deref()
    }

    /// Publish the prepared candidate exactly once and unblock its runner.
    ///
    /// Success means the exact parked actor accepted publication. Delivery to
    /// the model remains cancellable until the runner consumes its separate
    /// model-boundary witness at the final call seam.
    pub fn commit(mut self) -> Result<(), CoreBoundaryStageError> {
        let Some(mut authority) = self.authority.take() else {
            return Err(CoreBoundaryStageError::stale(
                "prepared boundary authority was already resolved",
            ));
        };
        authority.commit()
    }

    pub fn abort(mut self) -> Result<(), CoreBoundaryStageError> {
        let Some(mut authority) = self.authority.take() else {
            return Err(CoreBoundaryStageError::stale(
                "prepared boundary authority was already resolved",
            ));
        };
        authority.abort()
    }
}

impl std::fmt::Debug for CoreBoundaryStageOutput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CoreBoundaryStageOutput")
            .field(
                "session_snapshot_len",
                &self.session_snapshot.as_ref().map(Vec::len),
            )
            .field("authority", &self.authority.as_ref().map(|_| "prepared"))
            .finish()
    }
}

impl CoreApplyOutput {
    /// An output that commits no session document.
    pub fn new(receipt: RunBoundaryReceiptDraft, terminal: Option<CoreApplyTerminal>) -> Self {
        Self {
            receipt,
            committed: None,
            terminal,
        }
    }

    /// An output whose committed session document is UNCERTIFIED: the bytes
    /// carry no typed half, so a consumer that needs a `Session` deserializes
    /// and validates them itself.
    ///
    /// Producers that hold the typed session use [`Self::with_session`]
    /// instead; it seals the pair and is the only way a typed session ever
    /// accompanies bytes.
    pub fn with_untyped_snapshot(
        receipt: RunBoundaryReceiptDraft,
        untyped_snapshot: Option<Vec<u8>>,
        terminal: Option<CoreApplyTerminal>,
    ) -> Self {
        Self {
            receipt,
            committed: untyped_snapshot.map(BoundSessionCommit::untyped),
            terminal,
        }
    }

    pub fn with_run_result(
        receipt: RunBoundaryReceiptDraft,
        untyped_snapshot: Option<Vec<u8>>,
        run_result: RunResult,
    ) -> Self {
        Self::with_untyped_snapshot(
            receipt,
            untyped_snapshot,
            Some(CoreApplyTerminal::RunResult(Box::new(run_result))),
        )
    }

    pub fn with_callback_pending(
        receipt: RunBoundaryReceiptDraft,
        untyped_snapshot: Option<Vec<u8>>,
        tool_use_id: impl Into<String>,
        tool_name: impl Into<String>,
        args: Value,
    ) -> Self {
        Self::with_untyped_snapshot(
            receipt,
            untyped_snapshot,
            Some(CoreApplyTerminal::CallbackPending {
                tool_use_id: tool_use_id.into(),
                tool_name: tool_name.into(),
                args,
            }),
        )
    }

    pub fn with_callback_batch_pending(
        receipt: RunBoundaryReceiptDraft,
        untyped_snapshot: Option<Vec<u8>>,
        pending_tool_calls: Vec<crate::error::PendingCallbackToolCall>,
    ) -> Self {
        Self::with_untyped_snapshot(
            receipt,
            untyped_snapshot,
            Some(CoreApplyTerminal::CallbackBatchPending { pending_tool_calls }),
        )
    }

    pub fn without_terminal(
        receipt: RunBoundaryReceiptDraft,
        untyped_snapshot: Option<Vec<u8>>,
    ) -> Self {
        Self::with_untyped_snapshot(receipt, untyped_snapshot, None)
    }

    /// Commit the typed session as one sealed prepared boundary document.
    ///
    /// Whole-blob serialization is deferred until the selected persistence
    /// profile requests it. Any uncertified bytes a constructor installed
    /// earlier are replaced wholesale — typed authority and lazy bytes remain
    /// one private carrier, so no producer can certify one transcript while a
    /// different one is committed.
    pub fn with_session(
        mut self,
        session: std::sync::Arc<crate::Session>,
    ) -> Result<Self, serde_json::Error> {
        self.committed = Some(BoundSessionCommit::sealed(session)?);
        Ok(self)
    }

    /// Install an already sealed session boundary carrier.
    ///
    /// This is the profile-aware counterpart to [`Self::with_session`].
    /// Producers that prepared a bounded head-canonical mutation must retain
    /// that mutation on the exact typed carrier handed to RuntimeStore;
    /// reminting from only the `Session` would silently discard its physical
    /// predecessor CAS and suffix proof.
    #[must_use]
    pub fn with_bound_session(mut self, committed: BoundSessionCommit) -> Self {
        self.committed = Some(committed);
        self
    }

    /// The sealed session document this boundary commits, if any.
    #[must_use]
    pub fn committed(&self) -> Option<&BoundSessionCommit> {
        self.committed.as_ref()
    }

    /// Lazily materialize the exact whole-blob bytes this boundary commits.
    pub fn whole_blob_bytes(&self) -> Result<Option<&[u8]>, SessionBoundaryEncodeError> {
        self.committed
            .as_ref()
            .map(BoundSessionCommit::whole_blob_bytes)
            .transpose()
    }

    /// The typed WholeBlob session sealed to [`Self::whole_blob_bytes`], when
    /// the producer certified one.
    #[must_use]
    pub fn session(&self) -> Option<&crate::Session> {
        self.committed
            .as_ref()
            .and_then(BoundSessionCommit::session)
    }

    /// Consume the sealed session document, leaving the receipt and terminal
    /// behind.
    #[must_use]
    pub fn into_committed(self) -> Option<BoundSessionCommit> {
        self.committed
    }

    /// Consume into the receipt, the sealed session document, and the terminal
    /// observation. The document stays sealed across the handoff.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        RunBoundaryReceiptDraft,
        Option<BoundSessionCommit>,
        Option<CoreApplyTerminal>,
    ) {
        (self.receipt, self.committed, self.terminal)
    }
}

/// Cloneable live endpoint for cooperative in-flight turn boundaries.
///
/// ```compile_fail
/// use meerkat_core::lifecycle::CoreExecutorBoundaryHandle;
///
/// async fn boundary_handles_cannot_hard_cancel(handle: &dyn CoreExecutorBoundaryHandle) {
///     handle
///         .hard_cancel_current_run("wrong authority".to_string())
///         .await
///         .unwrap();
/// }
/// ```
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait CoreExecutorBoundaryHandle: Send + Sync {
    /// Request cooperative cancellation for one exact active run.
    async fn cancel_after_boundary(
        &self,
        expected_run_id: &RunId,
        reason: String,
    ) -> Result<(), CoreExecutorError>;

    /// Prepare request-only runtime context for one exact cooperative LLM
    /// boundary and return only after the actor is parked immediately before
    /// consumption. The non-clone result owns explicit commit/abort authority.
    ///
    /// This context is never Session state and therefore carries no durable
    /// session snapshot.
    async fn prepare_transient_turn_context_at_boundary(
        &self,
        _expected_run_id: &RunId,
        _contexts: Vec<TurnRequestContext>,
    ) -> Result<CoreBoundaryStageOutput, CoreBoundaryStageError> {
        Err(CoreBoundaryStageError::unavailable(
            "live transient turn-context preparation is unsupported by this executor",
        ))
    }
}

/// Cloneable live endpoint for hard-cancelling the active run immediately.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait CoreExecutorInterruptHandle: Send + Sync {
    async fn hard_cancel_current_run(&self, reason: String) -> Result<(), CoreExecutorError>;
}

/// Cloneable capability for exact durable interaction-terminal publication.
///
/// Runtime control paths may need to terminalize queued or staged directed
/// inputs while the owning executor is in flight (destroy/unregister) or after
/// its loop channels have been detached. Keeping this authority on a separate
/// handle prevents those paths from borrowing or duplicating the executor
/// while still routing publication through the executor's owning session
/// surface.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait CoreExecutorPublicationHandle: Send + Sync {
    async fn publish_interaction_terminals(
        &self,
        events: &[AgentEvent],
    ) -> Result<Vec<CoreInteractionTerminalPublicationReceipt>, CoreExecutorError>;
}

/// Cloneable service/surface cleanup authority retained by the runtime entry.
///
/// Unlike adapter unregister, this handle owns only the executor's live actor
/// and surface-local state. `MeerkatMachine` invokes it inside the exact
/// attachment's generated unregister window, and can retry it after a failed
/// or externally initiated drain without resurrecting the executor object.
/// Implementations must therefore be idempotent, including when a prior
/// attempt completed only part of its sidecar cleanup before returning an
/// error.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait CoreExecutorPostStopCleanupHandle: Send + Sync {
    async fn cleanup_after_runtime_stop_terminalized(&self) -> Result<(), CoreExecutorError>;

    /// Cleanup when the runtime loop already owns this session's stable outer
    /// turn-finalization boundary. Implementations backed by that boundary must
    /// not reacquire it.
    async fn cleanup_after_runtime_stop_terminalized_under_turn_finalization_boundary(
        &self,
    ) -> Result<(), CoreExecutorError> {
        self.cleanup_after_runtime_stop_terminalized().await
    }
}

/// Opaque RAII witness that one session actor's turn-finalization interval is
/// exclusively owned. The runtime holds this from before queue/effect staging
/// through machine commit, compatibility checkpoint, exact terminal receipt
/// persistence, and waiter resolution.
pub trait CoreExecutorTurnFinalizationGuard: Send {}

impl<T: Send> CoreExecutorTurnFinalizationGuard for T {}

/// Cloneable endpoint for the stable per-session turn-finalization boundary.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait CoreExecutorTurnFinalizationBoundaryHandle: Send + Sync {
    async fn acquire(
        &self,
    ) -> Result<Box<dyn CoreExecutorTurnFinalizationGuard>, CoreExecutorError>;
}

/// The interface core exposes for the runtime layer to apply run primitives.
///
/// The runtime layer creates an implementation that wraps an `Agent` and
/// translates `RunPrimitive` into session mutations. This trait is defined
/// in core so both layers can depend on it without circular deps.
///
/// # Object Safety
/// This trait is object-safe to allow `Box<dyn CoreExecutor>` usage.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait CoreExecutor: Send + Sync {
    /// Optional live cooperative-boundary endpoint.
    ///
    /// Implementations return this only when the underlying live turn can be
    /// signaled while `apply()` is in flight and will also wake any yielding
    /// turn so the boundary request can be observed.
    fn boundary_handle(&self) -> Option<Arc<dyn CoreExecutorBoundaryHandle>> {
        None
    }

    /// Optional live hard-interrupt endpoint.
    ///
    /// Hard cancel is intentionally live-handle-only. It is not available on
    /// the queued in-loop executor channel because user/session interrupt
    /// semantics require prompt delivery during a long in-flight turn.
    fn interrupt_handle(&self) -> Option<Arc<dyn CoreExecutorInterruptHandle>> {
        None
    }

    /// Optional cloneable authority for exact durable terminal publication.
    fn publication_handle(&self) -> Option<Arc<dyn CoreExecutorPublicationHandle>> {
        None
    }

    /// Whether `MeerkatMachine` should retain and fence this attachment's exact
    /// post-stop service cleanup authority.
    ///
    /// Opted-in executors expose a cloneable attachment-local cleanup handle.
    /// The machine fences it by the attachment incarnation it created, so a
    /// stale cleanup cannot remove replacement state. Ordinary runtime stop
    /// cleans the service incarnation while preserving the registered
    /// `Stopped` machine state; explicit unregister owns the later `Draining`
    /// transition and registration removal.
    fn machine_managed_post_stop_unregister(&self) -> bool {
        false
    }

    /// Cloneable service/surface cleanup authority for machine-managed
    /// post-stop unregister.
    fn post_stop_cleanup_handle(&self) -> Option<Arc<dyn CoreExecutorPostStopCleanupHandle>> {
        None
    }

    /// Stable boundary shared with direct and non-turn session mutations.
    fn turn_finalization_boundary_handle(
        &self,
    ) -> Option<Arc<dyn CoreExecutorTurnFinalizationBoundaryHandle>> {
        None
    }

    /// Apply a run primitive to the conversation.
    ///
    /// Returns a receipt proving the application, including a digest of the
    /// conversation state after mutation.
    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError>;

    /// Persist or project the committed session snapshot after the runtime
    /// control plane has durably committed the machine boundary.
    ///
    /// RuntimeStore remains the authority for runtime-backed turns; this hook
    /// is for compatibility projections such as `SessionStore` snapshots that
    /// must not be written before the machine commit succeeds. Recovery may
    /// invoke this with the authoritative RuntimeStore snapshot after outbox
    /// finalization so a stale compatibility snapshot cannot resurrect an
    /// already-finalized compaction intent.
    async fn checkpoint_committed_session_snapshot(
        &mut self,
        _session_snapshot: std::sync::Arc<Vec<u8>>,
    ) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    /// Acknowledge an ordinary WholeBlob boundary from its store-issued
    /// fixed-size authority.
    ///
    /// The runtime compares this authority with the exact prepared artifact
    /// before invoking the executor. Durable implementations then confirm the
    /// revision/digest through the store's bounded authority seam and publish
    /// executor-owned post-commit effects. Ordinary finalization must not
    /// reload or compare the accumulated document.
    async fn acknowledge_whole_blob_session_boundary(
        &mut self,
        _committed_store_revision: u64,
        _committed_blob_sha256: &str,
    ) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    /// Acknowledge a session boundary whose canonical head and transcript
    /// suffix were already committed inside the RuntimeStore transaction.
    ///
    /// Unlike [`Self::checkpoint_committed_session_snapshot`], this hook
    /// carries no whole document and performs no compatibility projection.
    /// It lets the executor publish post-commit side effects (for example,
    /// staged context lifecycle events) against the exact small authority
    /// returned by the store. Implementations that can be paired with a
    /// head-canonical runtime must override it; the default fails closed so a
    /// successful durable commit is never silently reported as fully
    /// finalized when executor-owned side effects remain staged.
    async fn acknowledge_head_canonical_session_boundary(
        &mut self,
        _committed_head_token: &str,
    ) -> Result<(), CoreExecutorError> {
        Err(CoreExecutorError::Internal(
            "executor cannot acknowledge a head-canonical session boundary".to_string(),
        ))
    }

    /// Acknowledge metadata-only promotion of an exact provisional tail.
    ///
    /// The runtime has already verified that the store-returned authority
    /// matches the actor-carried receipt. This hook carries only the fixed-size
    /// committed revision/token so executor-owned staged effects can publish
    /// and actor-local committed-base fencing can advance without re-encoding
    /// WholeBlob state or reapplying a HeadCanonical mutation.
    async fn acknowledge_provisional_session_boundary(
        &mut self,
        _committed_store_revision: u64,
        _committed_authority_token: &str,
    ) -> Result<(), CoreExecutorError> {
        Err(CoreExecutorError::Internal(
            "executor cannot acknowledge a promoted provisional session boundary".to_string(),
        ))
    }

    /// Reconcile and finalize semantic-memory compaction stages named by the
    /// exact RuntimeStore atomic outbox. The empty slice is authoritative: a
    /// durable implementation must use it to abort any invisible stage left by
    /// a crash before the runtime boundary committed.
    async fn reconcile_committed_compaction_projections(
        &mut self,
        intents: &[crate::memory::CompactionProjectionIntent],
    ) -> Result<(), CoreExecutorError> {
        if intents.is_empty() {
            Ok(())
        } else {
            Err(CoreExecutorError::Internal(
                "executor cannot reconcile committed compaction projections".to_string(),
            ))
        }
    }

    /// Roll back and abort any invisible compaction stage after the runtime
    /// boundary commit was rejected and the authoritative outbox was observed
    /// empty. This is deliberately separate from committed reconciliation so
    /// an empty post-error observation can never be mistaken for commit
    /// authority.
    async fn abort_uncommitted_compaction_projections(&mut self) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    /// Abort every executor-owned projection staged by a run whose atomic
    /// runtime boundary was rejected.
    ///
    /// The default preserves compatibility with executors that can stage only
    /// compaction. Runtime-backed session executors override this to also
    /// remove any uncommitted live transcript and context-event projections.
    /// Implementations must be cancellation-safe and retry-idempotent: once an
    /// attempt observes one sub-projection aborted, cancellation before the
    /// whole cleanup returns must leave enough mechanical progress to continue
    /// without requiring an already-discarded live carrier.
    async fn abort_rejected_run_projections(&mut self) -> Result<(), CoreExecutorError> {
        self.abort_uncommitted_compaction_projections().await
    }

    /// Durably publish exact per-input Interaction terminal events after
    /// generated runtime completion authority has observed finalization.
    /// Implementations must make replay idempotent by interaction ID and
    /// reject a mismatching existing payload.
    async fn publish_interaction_terminals(
        &mut self,
        events: &[AgentEvent],
    ) -> Result<Vec<CoreInteractionTerminalPublicationReceipt>, CoreExecutorError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        Err(CoreExecutorError::Internal(
            "exact interaction terminal publication is unsupported by this executor".to_string(),
        ))
    }

    /// Request cancellation at the next cooperative boundary.
    async fn cancel_after_boundary(&mut self, reason: String) -> Result<(), CoreExecutorError>;

    /// Ask this runtime executor to stop accepting work.
    async fn stop_runtime_executor(&mut self, reason: String) -> Result<(), CoreExecutorError>;

    /// Cleanup of executor-owned external/session material that is safe only
    /// after the runtime control plane has durably terminalized the stop.
    ///
    /// This hook must not unregister the runtime session. The machine-owned
    /// runtime-loop cleanup coordinator invokes it; ordinary stop preserves
    /// the registered `Stopped` session, while explicit or executor-required
    /// unregister separately owns registration removal. Recursive unregister
    /// from this hook is rejected fail-closed.
    async fn cleanup_after_runtime_stop_terminalized(&mut self) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    // Verify CoreExecutor is object-safe
    fn _assert_object_safe(_: &dyn CoreExecutor) {}

    #[test]
    fn prepared_session_boundary_serializes_exactly_once_across_clones() {
        let Ok(commit) = BoundSessionCommit::sealed(std::sync::Arc::new(crate::Session::new()))
        else {
            panic!("sealing a typed boundary no longer serializes and cannot fail");
        };
        let cloned = commit.clone();

        assert_eq!(commit.whole_blob_encode_count(), 0);
        assert!(commit.whole_blob_bytes().is_ok());
        assert!(cloned.whole_blob_bytes().is_ok());
        assert_eq!(commit.whole_blob_encode_count(), 1);
        assert_eq!(cloned.whole_blob_encode_count(), 1);
    }

    #[test]
    fn core_executor_error_display() {
        let err = CoreExecutorError::ApplyFailed {
            cause: CoreApplyFailureCause::runtime_turn("bad input"),
        };
        assert_eq!(err.to_string(), "Apply failed: bad input");

        let err = CoreExecutorError::ControlFailed {
            cause: CoreControlFailureCause::runtime_control("not running"),
        };
        assert_eq!(err.to_string(), "Control failed: not running");

        let err = CoreExecutorError::Stopped;
        assert_eq!(err.to_string(), "Executor is stopped");

        let err = CoreExecutorError::Cancelled;
        assert_eq!(err.to_string(), "Run was cancelled");

        let err = CoreExecutorError::Internal("oops".into());
        assert_eq!(err.to_string(), "Internal error: oops");
    }

    #[test]
    fn apply_failed_carries_typed_cause() {
        let err = CoreExecutorError::ApplyFailed {
            cause: CoreApplyFailureCause::runtime_context_apply("context write failed"),
        };

        match err {
            CoreExecutorError::ApplyFailed { cause } => {
                assert_eq!(cause.kind, CoreApplyFailureCauseKind::RuntimeContextApply);
                assert_eq!(cause.message(), "context write failed");
            }
            other => panic!("expected typed apply failure, got {other:?}"),
        }
    }

    #[test]
    fn cancelled_session_error_remains_typed_at_runtime_executor_boundary() {
        let err = CoreExecutorError::apply_failed_from_session_error(SessionError::Agent(
            AgentError::Cancelled,
        ));

        assert!(err.is_cancelled());
        assert_eq!(
            err.apply_failure_cause().kind,
            CoreApplyFailureCauseKind::RuntimeTurn
        );
    }

    #[test]
    fn corrupted_live_session_signal_stops_instead_of_retrying_apply() {
        let err = CoreExecutorError::apply_failed_from_session_error(
            SessionError::runtime_executor_stopped("terminal witness mismatch"),
        );

        assert!(matches!(err, CoreExecutorError::Stopped));
    }

    #[test]
    fn durable_projection_authority_unknown_requests_canonical_runtime_teardown() {
        let err = CoreExecutorError::apply_failed_from_session_error(SessionError::Agent(
            AgentError::session_durable_projection_authority_unknown(
                "durable transcript projection split",
            ),
        ));

        assert!(err.requires_runtime_teardown());
        assert_eq!(
            CoreExecutorTeardownReason::DurableProjectionAuthorityUnknown.as_str(),
            "DurableProjectionAuthorityUnknown"
        );
        assert_eq!(
            CoreExecutorTeardownReason::from_wire_str("DurableProjectionAuthorityUnknown"),
            Some(CoreExecutorTeardownReason::DurableProjectionAuthorityUnknown)
        );
        assert!(matches!(
            err,
            CoreExecutorError::TeardownRequired {
                reason: CoreExecutorTeardownReason::DurableProjectionAuthorityUnknown,
                ..
            }
        ));
    }

    #[test]
    fn hook_denial_agent_error_maps_to_typed_apply_failure_cause() {
        let error = AgentError::HookDenied {
            hook_id: crate::hooks::HookId::new("guard"),
            point: crate::hooks::HookPoint::PreToolExecution,
            reason_code: crate::hooks::HookReasonCode::PolicyViolation,
            message: "blocked by hook".to_string(),
            payload: None,
        };

        let cause = CoreApplyFailureCause::from_agent_error(&error);
        assert_eq!(cause.kind, CoreApplyFailureCauseKind::HookDenied);
        assert!(cause.message().contains("blocked by hook"));
    }

    #[test]
    fn hook_runtime_agent_error_maps_to_typed_apply_failure_cause() {
        let error = AgentError::HookExecutionFailed {
            hook_id: crate::hooks::HookId::new("guard"),
            reason: "missing runtime".to_string(),
        };

        let cause = CoreApplyFailureCause::from_agent_error(&error);
        assert_eq!(cause.kind, CoreApplyFailureCauseKind::HookRuntimeFailure);
        assert!(cause.message().contains("missing runtime"));
    }
}
