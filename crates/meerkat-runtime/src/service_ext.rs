//! SessionServiceRuntimeExt — v9 runtime extension for SessionService.
//!
//! This trait extends the existing SessionService with runtime-specific
//! operations. It lives in meerkat-runtime (NOT in core) to maintain
//! the separation: core owns SessionService, runtime owns runtime extensions.

use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::types::SessionId;

use crate::accept::AcceptOutcome;
use crate::completion::CompletionHandle;
use crate::completion::CompletionOutcome;
use crate::input::Input;
use crate::input_state::StoredInputState;
use crate::meerkat_machine_types::{
    ImageOperationRoutingRequest, ImageOperationRoutingResult, SessionLlmReconfigureReport,
    SessionLlmReconfigureRequest, SwitchTurnRequest,
};
use crate::runtime_state::RuntimeState;
use crate::terminal_status::{
    InteractionSelector, InteractionTerminalReport, RunTerminalReport, Sourced,
};
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

    /// Return the exact rich public completion previously selected for this
    /// input, without registering a waiter or reviving an unregistered runtime.
    /// Inputs archived out of an attached runtime are read from the same
    /// durable receipt authority used after runtime teardown.
    ///
    /// `Ok(None)` means an admitted input has no finalized receipt yet,
    /// including the durable pre-finalization window. A terminal 0.8.10 row
    /// whose rich result was never recorded is repair-blocked rather than
    /// reported as retryable absence. `InputTerminalOutcome::Consumed` is not
    /// evidence for any particular public completion class.
    async fn input_terminal_completion(
        &self,
        session_id: &SessionId,
        input_id: &InputId,
    ) -> Result<Option<CompletionOutcome>, RuntimeDriverError>;

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

    /// Durable-only witness for one caller-supplied idempotency key.
    ///
    /// Unlike [`Self::input_state_by_idempotency_key`], this never consults
    /// live machine state: `Some` means a committed row exists in the
    /// runtime's store-owned input index and therefore survives process
    /// restart, so a retry under the same key collapses onto it even on a
    /// fresh process. `None` means only that no such durable evidence is
    /// observable - a store-less (ephemeral) runtime always answers `None`
    /// because it retains nothing across restart.
    ///
    /// The default under-claims rather than over-claims: an adapter that
    /// cannot offer durable evidence must never be read as proof of
    /// durability. [`crate::bounded_submit::submit_bounded`] classifies an
    /// expired caller bound with this read.
    async fn durable_input_state_by_idempotency_key(
        &self,
        _session_id: &SessionId,
        _idempotency_key: &str,
    ) -> Result<Option<StoredInputState>, RuntimeDriverError> {
        Ok(None)
    }

    /// Durable terminal-status query for one interaction.
    ///
    /// Registered sessions answer from live DSL truth; unregistered sessions
    /// on a machine with a persistent RuntimeStore answer from the durably
    /// committed input-state witnesses WITHOUT reviving the runtime. A
    /// never-admitted session id fails typed `NotFound`; unregistered
    /// sessions on a store-less (ephemeral) machine keep the `NotReady`
    /// class. `Ok(None)` means the session is known but no input matches the
    /// selector.
    async fn interaction_terminal_status(
        &self,
        session_id: &SessionId,
        selector: InteractionSelector,
    ) -> Result<Option<Sourced<InteractionTerminalReport>>, RuntimeDriverError>;

    /// Durable terminal-status query for a run.
    ///
    /// Evaluates the input-state witnesses whose `last_run_id` references
    /// `run_id` (live snapshot when registered, durable store rows
    /// otherwise) through the canonical pure evaluator. An unknown run on a
    /// known session reports `NoDurableWitness` — callers must not read that
    /// as `Failed` (re-staging rebinds `last_run_id`).
    async fn run_terminal_status(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> Result<Sourced<RunTerminalReport>, RuntimeDriverError>;

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

    /// Every committed handoff whose durable log entry has no terminal record.
    ///
    /// Reads the committed log and returns typed candidates. It answers only
    /// "what does the durable log still owe"; whether a candidate is actionable
    /// is decided by the realization seam, which additionally requires the
    /// originating run's committed boundary receipt.
    ///
    /// The default is an empty list because a runtime adapter that carries no
    /// durable session can never hold a committed handoff — that is a true
    /// answer, not a stub.
    async fn committed_model_routing_handoffs_awaiting_decision(
        &self,
        _session_id: &SessionId,
    ) -> Result<Vec<crate::meerkat_machine_types::CommittedModelRoutingHandoff>, RuntimeDriverError>
    {
        Ok(Vec::new())
    }

    /// Reconcile crash-window terminals and report what is still owed, from a
    /// SINGLE read of the committed log.
    ///
    /// The pre-dequeue seam runs on every lap for every session, so reading the
    /// durable log twice — once to reconcile, once to list pending — doubles a
    /// per-lap cost that is almost always answered "nothing to do". One read
    /// serves both, and it is the same read, so the two answers cannot disagree
    /// with each other.
    async fn reconcile_and_list_committed_model_routing_handoffs(
        &self,
        _session_id: &SessionId,
    ) -> Result<Vec<crate::meerkat_machine_types::CommittedModelRoutingHandoff>, RuntimeDriverError>
    {
        Ok(Vec::new())
    }

    /// Bring generated authority into agreement with durable terminals that
    /// were committed before the machine recorded them.
    ///
    /// The realization chain persists its durable terminal BEFORE marking the
    /// generated one, so a crash between those two steps leaves a request that
    /// is settled on disk and still `Imported`/`Claimed` in the machine. That
    /// request is invisible to the awaiting-decision seam — it owes nothing —
    /// so nothing would ever finish it.
    ///
    /// This drives ONLY the matching generated terminal from the exact durable
    /// record. It re-applies no identity and re-decides no denial: the durable
    /// record is the fact, and this is the machine catching up to it.
    ///
    /// The default reconciles nothing, matching the read default above.
    async fn reconcile_committed_model_routing_terminals(
        &self,
        _session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        Ok(())
    }

    /// Realize one committed handoff while the caller already holds this
    /// session's turn-finalization boundary.
    ///
    /// Unlike the read above, the default here REFUSES: reaching it means a
    /// candidate was produced and then could not be acted on, and silently
    /// reporting success would strand the request forever.
    async fn realize_committed_model_routing_handoff_under_turn_finalization_boundary(
        &self,
        _session_id: &SessionId,
        _handoff: crate::meerkat_machine_types::CommittedModelRoutingHandoff,
    ) -> Result<crate::meerkat_machine_types::ModelRoutingHandoffRealization, RuntimeDriverError>
    {
        Err(RuntimeDriverError::Internal(
            "committed model-routing handoff realization is not supported by this runtime adapter"
                .into(),
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

/// Build the shared pre-dequeue realization handle for one runtime-backed
/// session.
///
/// This is the single implementation every runtime-backed surface returns.
/// Duplicating it per surface would let one skin quietly ship a session where a
/// committed handoff is never realized, and that failure is invisible: the
/// session simply keeps answering on the old model as if nothing was ever
/// requested.
///
/// It lives here, beside `MeerkatMachine`, rather than in the facade because it
/// needs only the machine and the session id — the machine already owns the
/// generated lifecycle, the boundary receipts, and the reconfigure host that
/// reads and writes the durable log. Keeping it here is also what lets the mob
/// provisioner return the same handle: mob depends on the runtime, but not on
/// the facade's `session-store` feature. The facade re-exports this function,
/// so `meerkat::surface::persistent_runtime_pre_dequeue_handle` remains the
/// name every surface calls.
#[must_use]
pub fn persistent_runtime_pre_dequeue_handle(
    adapter: std::sync::Arc<crate::MeerkatMachine>,
    session_id: SessionId,
) -> std::sync::Arc<dyn meerkat_core::lifecycle::CoreExecutorPreDequeueHandle> {
    std::sync::Arc::new(RuntimeModelRoutingHandoffPreDequeueHandle {
        adapter,
        session_id,
    })
}

struct RuntimeModelRoutingHandoffPreDequeueHandle {
    adapter: std::sync::Arc<crate::MeerkatMachine>,
    session_id: SessionId,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl meerkat_core::lifecycle::CoreExecutorPreDequeueHandle
    for RuntimeModelRoutingHandoffPreDequeueHandle
{
    async fn realize_committed_handoffs_under_turn_finalization_boundary(
        &self,
    ) -> Result<
        meerkat_core::lifecycle::CorePreDequeueOutcome,
        meerkat_core::lifecycle::CoreExecutorError,
    > {
        use meerkat_core::lifecycle::{CoreExecutorError, CorePreDequeueOutcome};

        // One read of the committed log answers both questions: which terminals
        // the machine has not caught up to, and what is still owed. A settled
        // request never appears in the pending list, so reconciliation cannot
        // be gated on there being pending work.
        let pending = self
            .adapter
            .reconcile_and_list_committed_model_routing_handoffs(&self.session_id)
            .await
            .map_err(|error| CoreExecutorError::control_failed_runtime(error.to_string()))?;
        if pending.is_empty() {
            return Ok(CorePreDequeueOutcome::NothingPending);
        }
        let mut realized_any = false;
        for handoff in pending {
            let request_id = handoff.request_id;
            match self
                .adapter
                .realize_committed_model_routing_handoff_under_turn_finalization_boundary(
                    &self.session_id,
                    handoff,
                )
                .await
                .map_err(|error| {
                    // A failed realization must not let the pending input
                    // proceed under an identity the session was told to leave.
                    CoreExecutorError::control_failed_runtime(error.to_string())
                })? {
                crate::ModelRoutingHandoffRealization::Realized { .. }
                | crate::ModelRoutingHandoffRealization::Denied { .. } => realized_any = true,
                crate::ModelRoutingHandoffRealization::AlreadyExact => {}
                crate::ModelRoutingHandoffRealization::Held { reason } => {
                    return Err(CoreExecutorError::control_failed_runtime(format!(
                        "committed model-routing handoff {request_id:?} is held: {reason}"
                    )));
                }
            }
        }
        Ok(if realized_any {
            CorePreDequeueOutcome::Realized
        } else {
            CorePreDequeueOutcome::NothingPending
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verify trait is object-safe
    fn _assert_object_safe(_: &dyn SessionServiceRuntimeExt) {}
}
