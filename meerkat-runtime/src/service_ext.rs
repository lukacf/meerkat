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
    ImageOperationRoutingRequest, ImageOperationRoutingResult, SessionLlmReconfigureReport,
    SessionLlmReconfigureRequest, SwitchTurnRequest,
};
use crate::runtime_state::RuntimeState;
use crate::traits::{ResetReport, RetireReport, RuntimeDriverError};

/// v9 runtime extensions for SessionService.
///
/// This branch is runtime-backed only: every implementation is a v9
/// runtime surface, so the methods below are unconditionally available.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait SessionServiceRuntimeExt: Send + Sync {
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

    /// Get the runtime-owned resolved LLM capability surface for a session.
    async fn resolved_session_llm_capabilities(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<crate::meerkat_machine_types::SessionLlmCapabilitySurface>, RuntimeDriverError>
    {
        Err(RuntimeDriverError::Internal(
            "resolved session llm capabilities are not implemented by this runtime adapter".into(),
        ))
    }

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

    /// Resolve a caller-supplied idempotency key to its admitted input and
    /// return that input's stored state (terminal outcome, last run id,
    /// boundary sequence).
    ///
    /// This is the durable reconciliation query for interrupted work: the
    /// machine-owned idempotency binding and the input's terminal facts
    /// survive restart (persistent runtimes re-enter them on recovery), so
    /// after re-registering a session a host can ask "did the interaction I
    /// submitted under this key reach a terminal state, and which?" without
    /// keeping its own run journal. Read-only: never registers a binding.
    async fn input_state_by_idempotency_key(
        &self,
        session_id: &SessionId,
        idempotency_key: &str,
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

    async fn deny_image_operation_plan(
        &self,
        _session_id: &SessionId,
        _operation_id: meerkat_core::image_generation::ImageOperationId,
        _reason: meerkat_core::image_generation::ImageOperationDenialReason,
    ) -> Result<meerkat_core::image_generation::ImageOperationPhase, RuntimeDriverError> {
        Err(RuntimeDriverError::Internal(
            "image operation plan denial is not supported by this runtime adapter".into(),
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

    async fn classify_image_operation_terminal(
        &self,
        _session_id: &SessionId,
        _operation_id: meerkat_core::image_generation::ImageOperationId,
        _observation: meerkat_core::image_generation::ImageProviderTerminalObservation,
        _provider_text: meerkat_core::image_generation::ProviderTextDisposition,
    ) -> Result<meerkat_core::image_generation::ImageOperationTerminalClass, RuntimeDriverError>
    {
        Err(RuntimeDriverError::Internal(
            "image operation terminal classification is not supported by this runtime adapter"
                .into(),
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
}
