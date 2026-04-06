//! CoreExecutor trait — the interface core exposes to the runtime layer.
//!
//! The runtime layer implements this trait (as `AgentCoreExecutor`) to bridge
//! RunPrimitive into Agent session mutations. The trait lives in core so both
//! layers can reference it without circular dependencies.

use super::RunId;
use super::run_control::RunControlCommand;
use super::run_primitive::RunPrimitive;
use super::run_receipt::RunBoundaryReceipt;
use crate::types::RunResult;
use serde_json::Value;

/// Errors from CoreExecutor operations.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum CoreExecutorError {
    /// The primitive could not be applied (conversation mutation failed).
    #[error("Apply failed: {reason}")]
    ApplyFailed { reason: String },

    /// The control command could not be executed.
    #[error("Control failed: {reason}")]
    ControlFailed { reason: String },

    /// The executor is in a terminal state and cannot accept more work.
    #[error("Executor is stopped")]
    Stopped,

    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Successful result of applying a run primitive.
#[derive(Debug, Clone)]
pub enum CoreApplyTerminal {
    /// The run completed and produced a result.
    RunResult(RunResult),
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
    /// Compatibility mirror of the completed run result, if one exists.
    ///
    /// Canonical terminal semantics live in `terminal`. Keep this field while
    /// runtime-backed surfaces migrate off the legacy `Option<RunResult>` seam.
    pub run_result: Option<RunResult>,
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
            run_result: Some(run_result.clone()),
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
            run_result: None,
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
            run_result: None,
            terminal: None,
        }
    }
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
    /// Apply a run primitive to the conversation.
    ///
    /// Returns a receipt proving the application, including a digest of the
    /// conversation state after mutation.
    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError>;

    /// Execute an out-of-band control command.
    async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verify CoreExecutor is object-safe
    fn _assert_object_safe(_: &dyn CoreExecutor) {}

    #[test]
    fn core_executor_error_display() {
        let err = CoreExecutorError::ApplyFailed {
            reason: "bad input".into(),
        };
        assert_eq!(err.to_string(), "Apply failed: bad input");

        let err = CoreExecutorError::ControlFailed {
            reason: "not running".into(),
        };
        assert_eq!(err.to_string(), "Control failed: not running");

        let err = CoreExecutorError::Stopped;
        assert_eq!(err.to_string(), "Executor is stopped");

        let err = CoreExecutorError::Internal("oops".into());
        assert_eq!(err.to_string(), "Internal error: oops");
    }
}
