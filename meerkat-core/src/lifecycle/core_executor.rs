//! CoreExecutor trait — the interface core exposes to the runtime layer.
//!
//! The runtime layer implements this trait (as `AgentCoreExecutor`) to bridge
//! RunPrimitive into Agent session mutations. The trait lives in core so both
//! layers can reference it without circular dependencies.

use super::RunId;
use super::run_primitive::RunPrimitive;
use super::run_receipt::RunBoundaryReceipt;
use crate::error::AgentError;
use crate::service::SessionError;
use crate::types::RunResult;
use serde_json::Value;
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

    pub fn apply_failed_from_session_error(error: SessionError) -> Self {
        match error {
            SessionError::Agent(AgentError::Cancelled) => Self::Cancelled,
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

    pub fn control_failed(cause: CoreControlFailureCause) -> Self {
        Self::ControlFailed { cause }
    }

    pub fn control_failed_runtime(message: impl Into<String>) -> Self {
        Self::control_failed(CoreControlFailureCause::runtime_control(message))
    }

    pub fn apply_failure_cause(&self) -> CoreApplyFailureCause {
        match self {
            Self::ApplyFailed { cause } => cause.clone(),
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
    RunResult(RunResult),
    /// A resume-pending request reached the session with no pending boundary.
    NoPendingBoundary,
    /// The run committed a continuation boundary and is waiting for external
    /// tool results before it can continue.
    CallbackPending { tool_name: String, args: Value },
}

#[derive(Debug, Clone)]
pub struct CoreApplyOutput {
    /// The authoritative receipt proving boundary application.
    pub receipt: RunBoundaryReceipt,
    /// Optional serialized session snapshot to durably commit atomically with
    /// the receipt and input-state updates.
    pub session_snapshot: Option<Vec<u8>>,
    /// Canonical terminal outcome for runtime-backed execution.
    ///
    /// `None` means the primitive committed successfully but did not produce a
    /// turn terminal (for example immediate context appends).
    pub terminal: Option<CoreApplyTerminal>,
}

impl CoreApplyOutput {
    pub fn with_run_result(
        receipt: RunBoundaryReceipt,
        session_snapshot: Option<Vec<u8>>,
        run_result: RunResult,
    ) -> Self {
        Self {
            receipt,
            session_snapshot,
            terminal: Some(CoreApplyTerminal::RunResult(run_result)),
        }
    }

    pub fn with_callback_pending(
        receipt: RunBoundaryReceipt,
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
        receipt: RunBoundaryReceipt,
        session_snapshot: Option<Vec<u8>>,
    ) -> Self {
        Self {
            receipt,
            session_snapshot,
            terminal: None,
        }
    }
}

/// Cloneable live endpoint for asking an executor to stop at the next
/// cooperative turn boundary.
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
    async fn cancel_after_boundary(&self, reason: String) -> Result<(), CoreExecutorError>;
}

/// Cloneable live endpoint for hard-cancelling the active run immediately.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait CoreExecutorInterruptHandle: Send + Sync {
    async fn hard_cancel_current_run(&self, reason: String) -> Result<(), CoreExecutorError>;
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

    /// Apply a run primitive to the conversation.
    ///
    /// Returns a receipt proving the application, including a digest of the
    /// conversation state after mutation.
    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError>;

    /// Request cancellation at the next cooperative boundary.
    async fn cancel_after_boundary(&mut self, reason: String) -> Result<(), CoreExecutorError>;

    /// Stop this runtime executor and release live session state.
    async fn stop_runtime_executor(&mut self, reason: String) -> Result<(), CoreExecutorError>;
}

#[cfg(test)]
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
