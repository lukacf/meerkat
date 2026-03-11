//! CoreExecutor trait — the interface core exposes to the runtime layer.
//!
//! The runtime layer implements this trait (as `AgentCoreExecutor`) to bridge
//! RunPrimitive into Agent session mutations. The trait lives in core so both
//! layers can reference it without circular dependencies.

use super::run_control::RunControlCommand;
use super::run_primitive::RunPrimitive;
use super::run_receipt::RunBoundaryReceipt;

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
        primitive: RunPrimitive,
    ) -> Result<RunBoundaryReceipt, CoreExecutorError>;

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
