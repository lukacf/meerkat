//! CoreExecutor trait — the interface core exposes to the runtime layer.
//!
//! The runtime layer implements this trait (as `AgentCoreExecutor`) to bridge
//! RunPrimitive into Agent session mutations. The trait lives in core so both
//! layers can reference it without circular dependencies.

use super::RunId;
use super::run_primitive::RunPrimitive;
use super::run_receipt::RunBoundaryReceiptDraft;
use crate::error::AgentError;
use crate::service::SessionError;
use crate::session::PendingSystemContextAppend;
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
}

impl CoreExecutorTeardownReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ArchivedSession => "ArchivedSession",
            Self::SessionUnavailable => "SessionUnavailable",
        }
    }

    pub fn from_wire_str(value: &str) -> Option<Self> {
        match value {
            "ArchivedSession" => Some(Self::ArchivedSession),
            "SessionUnavailable" => Some(Self::SessionUnavailable),
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

    pub fn apply_failed_from_session_error(error: SessionError) -> Self {
        if error.requests_runtime_executor_stop() {
            return Self::Stopped;
        }
        match error {
            SessionError::Agent(AgentError::Cancelled) => Self::Cancelled,
            SessionError::Agent(AgentError::StickyModelFallbackAuthorityUnknown { message }) => {
                Self::session_unavailable_requires_teardown(message)
            }
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
    CallbackPending { tool_name: String, args: Value },
}

#[derive(Debug, Clone)]
pub struct CoreApplyOutput {
    /// Unsequenced receipt proving boundary application. The runtime driver
    /// mints the final sequenced [`super::run_receipt::RunBoundaryReceipt`]
    /// from the generated machine's per-run boundary counter at commit time
    /// (dogma K10 — executors cannot produce the boundary sequence).
    pub receipt: RunBoundaryReceiptDraft,
    /// Optional serialized session snapshot to durably commit atomically with
    /// the receipt and input-state updates.
    pub session_snapshot: Option<Vec<u8>>,
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
    pub fn with_run_result(
        receipt: RunBoundaryReceiptDraft,
        session_snapshot: Option<Vec<u8>>,
        run_result: RunResult,
    ) -> Self {
        Self {
            receipt,
            session_snapshot,
            terminal: Some(CoreApplyTerminal::RunResult(Box::new(run_result))),
        }
    }

    pub fn with_callback_pending(
        receipt: RunBoundaryReceiptDraft,
        session_snapshot: Option<Vec<u8>>,
        tool_name: impl Into<String>,
        args: Value,
    ) -> Self {
        Self {
            receipt,
            session_snapshot,
            terminal: Some(CoreApplyTerminal::CallbackPending {
                tool_name: tool_name.into(),
                args,
            }),
        }
    }

    pub fn without_terminal(
        receipt: RunBoundaryReceiptDraft,
        session_snapshot: Option<Vec<u8>>,
    ) -> Self {
        Self {
            receipt,
            session_snapshot,
            terminal: None,
        }
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

    /// Prepare runtime-owned system context for one exact cooperative LLM
    /// boundary and return only after the actor is parked immediately before
    /// consumption. The non-clone result owns explicit commit/abort authority.
    ///
    /// Implementations that can serialize the staged session snapshot return
    /// it so the runtime control plane can commit the snapshot atomically with
    /// the consumed input state. Implementations without durable session
    /// authority may return `None`.
    async fn prepare_system_context_at_boundary(
        &self,
        _expected_run_id: &RunId,
        _appends: Vec<PendingSystemContextAppend>,
    ) -> Result<CoreBoundaryStageOutput, CoreBoundaryStageError> {
        Err(CoreBoundaryStageError::unavailable(
            "live boundary system-context preparation is unsupported by this executor",
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

    /// Whether `MeerkatMachine` should close the exact runtime-loop
    /// registration after this executor's post-stop cleanup returns.
    ///
    /// Persistent surface executors opt in when their cleanup owns only the
    /// service-side live actor. The machine then fences unregister by the
    /// attachment incarnation it created, so the loop never awaits its own
    /// `JoinHandle` and a stale cleanup cannot remove a replacement loop.
    /// Executors with a broader incarnation protocol (for example mob member
    /// sidecars) keep the default and own their fenced teardown themselves.
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
        _session_snapshot: &[u8],
    ) -> Result<(), CoreExecutorError> {
        Ok(())
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
