//! SessionServiceRuntimeExt — v9 runtime extension for SessionService.
//!
//! This trait extends the existing SessionService with runtime-specific
//! operations. It lives in meerkat-runtime (NOT in core) to maintain
//! the separation: core owns SessionService, runtime owns runtime extensions.

use meerkat_core::lifecycle::InputId;
use meerkat_core::types::SessionId;

use crate::accept::AcceptOutcome;
use crate::input::Input;
use crate::input_state::InputState;
use crate::runtime_state::RuntimeState;
use crate::traits::{ResetReport, RetireReport, RuntimeDriverError};

/// Runtime mode for a session service instance.
///
/// This branch is runtime-backed only. All sessions use v9 runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeMode {
    /// Full v9 runtime-backed mode.
    V9Compliant,
}

/// v9 runtime extensions for SessionService.
///
/// Surfaces query this to decide whether to expose v9 runtime methods.
/// Legacy-mode surfaces MUST NOT advertise v9 capabilities.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait SessionServiceRuntimeExt: Send + Sync {
    /// Get the runtime mode.
    fn runtime_mode(&self) -> RuntimeMode;

    /// Accept an input for a session.
    async fn accept_input(
        &self,
        session_id: &SessionId,
        input: Input,
    ) -> Result<AcceptOutcome, RuntimeDriverError>;

    /// Get the runtime state for a session.
    async fn runtime_state(
        &self,
        session_id: &SessionId,
    ) -> Result<RuntimeState, RuntimeDriverError>;

    /// Retire a session's runtime.
    async fn retire_runtime(
        &self,
        session_id: &SessionId,
    ) -> Result<RetireReport, RuntimeDriverError>;

    /// Reset a session's runtime.
    async fn reset_runtime(
        &self,
        session_id: &SessionId,
    ) -> Result<ResetReport, RuntimeDriverError>;

    /// Get the state of a specific input.
    async fn input_state(
        &self,
        session_id: &SessionId,
        input_id: &InputId,
    ) -> Result<Option<InputState>, RuntimeDriverError>;

    /// List all active (non-terminal) inputs for a session.
    async fn list_active_inputs(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<InputId>, RuntimeDriverError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verify trait is object-safe
    fn _assert_object_safe(_: &dyn SessionServiceRuntimeExt) {}

    #[test]
    fn runtime_mode_equality() {
        assert_eq!(RuntimeMode::V9Compliant, RuntimeMode::V9Compliant);
    }
}
