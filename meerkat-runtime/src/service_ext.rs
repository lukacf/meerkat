//! SessionServiceRuntimeExt — v9 runtime extension for SessionService.
//!
//! This trait extends the existing SessionService with runtime-specific
//! operations. It lives in meerkat-runtime (NOT in core) to maintain
//! the separation: core owns SessionService, runtime owns runtime extensions.

use meerkat_core::lifecycle::InputId;
use meerkat_core::types::SessionId;

use crate::accept::AcceptOutcome;
use crate::completion::CompletionHandle;
use crate::input::Input;
use crate::input_state::StoredInputState;
use crate::meerkat_machine_types::{
    ImageOperationRoutingRequest, ImageOperationRoutingResult, RealtimeAttachmentStatus,
    SessionLlmReconfigureReport, SessionLlmReconfigureRequest, SwitchTurnRequest,
};
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

    /// Accept an input and optionally return a completion handle that resolves
    /// when the admitted work reaches a terminal runtime outcome.
    async fn accept_input_with_completion(
        &self,
        session_id: &SessionId,
        input: Input,
    ) -> Result<(AcceptOutcome, Option<CompletionHandle>), RuntimeDriverError>;

    /// Get the runtime state for a session.
    async fn runtime_state(
        &self,
        session_id: &SessionId,
    ) -> Result<RuntimeState, RuntimeDriverError>;

    /// Get the live attachment status for a session.
    async fn realtime_attachment_status(
        &self,
        session_id: &SessionId,
    ) -> Result<RealtimeAttachmentStatus, RuntimeDriverError>;

    /// Wave-c C-9c R4: fully-projected public channel status. RPC / MCP
    /// `realtime/status` responders use this so `attempt_count` /
    /// `next_retry_at` / `deadline_at` come from DSL state projected off
    /// the reconnect retry machine's progress, not hard-coded defaults.
    async fn realtime_channel_status(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_contracts::RealtimeChannelStatus, RuntimeDriverError>;

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

    /// Get the state of a specific input, bundled with its DSL-owned seed
    /// (phase / run association / boundary sequence).
    async fn input_state(
        &self,
        session_id: &SessionId,
        input_id: &InputId,
    ) -> Result<Option<StoredInputState>, RuntimeDriverError>;

    /// List all active (non-terminal) inputs for a session.
    async fn list_active_inputs(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<InputId>, RuntimeDriverError>;

    /// Canonically reconfigure the LLM identity for a registered live session.
    async fn reconfigure_session_llm_identity(
        &self,
        session_id: &SessionId,
        request: SessionLlmReconfigureRequest,
    ) -> Result<SessionLlmReconfigureReport, RuntimeDriverError>;

    async fn configure_model_routing_baseline(
        &self,
        _session_id: &SessionId,
        _baseline_model: meerkat_core::lifecycle::run_primitive::ModelId,
        _realtime_capable: bool,
    ) -> Result<(), RuntimeDriverError> {
        Err(RuntimeDriverError::Internal(
            "model routing baseline is not supported by this runtime adapter".into(),
        ))
    }

    async fn session_model_routing_status(
        &self,
        _session_id: &SessionId,
    ) -> Result<meerkat_core::image_generation::SessionModelRoutingStatus, RuntimeDriverError> {
        Err(RuntimeDriverError::Internal(
            "model routing status is not supported by this runtime adapter".into(),
        ))
    }

    async fn request_switch_turn(
        &self,
        _session_id: &SessionId,
        _request: SwitchTurnRequest,
    ) -> Result<meerkat_core::image_generation::SwitchTurnControlResult, RuntimeDriverError> {
        Err(RuntimeDriverError::Internal(
            "switch_turn is not supported by this runtime adapter".into(),
        ))
    }

    async fn admit_model_routing_assistant_turn(
        &self,
        _session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        Err(RuntimeDriverError::Internal(
            "model routing turn admission is not supported by this runtime adapter".into(),
        ))
    }

    async fn begin_image_operation(
        &self,
        _session_id: &SessionId,
        _request: ImageOperationRoutingRequest,
    ) -> Result<ImageOperationRoutingResult, RuntimeDriverError> {
        Err(RuntimeDriverError::Internal(
            "image operation routing is not supported by this runtime adapter".into(),
        ))
    }

    async fn activate_image_operation_override(
        &self,
        _session_id: &SessionId,
        _operation_id: meerkat_core::image_generation::ImageOperationId,
    ) -> Result<meerkat_core::image_generation::ImageOperationPhase, RuntimeDriverError> {
        Err(RuntimeDriverError::Internal(
            "image operation activation is not supported by this runtime adapter".into(),
        ))
    }

    async fn complete_image_operation(
        &self,
        _session_id: &SessionId,
        _operation_id: meerkat_core::image_generation::ImageOperationId,
        _terminal: meerkat_core::image_generation::ImageOperationTerminalClass,
    ) -> Result<meerkat_core::image_generation::ImageOperationPhase, RuntimeDriverError> {
        Err(RuntimeDriverError::Internal(
            "image operation completion is not supported by this runtime adapter".into(),
        ))
    }

    async fn restore_image_operation_override(
        &self,
        _session_id: &SessionId,
        _operation_id: meerkat_core::image_generation::ImageOperationId,
    ) -> Result<meerkat_core::image_generation::ImageOperationPhase, RuntimeDriverError> {
        Err(RuntimeDriverError::Internal(
            "image operation restore is not supported by this runtime adapter".into(),
        ))
    }
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
