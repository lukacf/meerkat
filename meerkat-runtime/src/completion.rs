//! Input completion waiters — allows callers to await terminal outcome of an accepted input.
//!
//! When a surface accepts an input via the runtime, it can optionally receive a
//! `CompletionHandle` that resolves when the input reaches a terminal state
//! (Consumed or Abandoned). This bridges the async accept/await pattern needed
//! for surfaces that want synchronous-feeling turn execution through the runtime.
//!
//! `CompletionRegistry` is waiter plumbing only. Production code must never
//! treat waiter presence, waiter counts, or sender membership as semantic
//! runtime truth.

use std::collections::HashMap;
use std::future::Future;

use meerkat_core::lifecycle::InputId;
#[cfg(test)]
use meerkat_core::lifecycle::RunId;
use meerkat_core::lifecycle::core_executor::CoreApplyTerminal;
use meerkat_core::types::{RunResult, SessionId};
use meerkat_core::{TurnErrorMetadata, TurnTerminalCauseKind, TurnTerminalOutcome};
use serde_json::Value;

use crate::meerkat_machine::driver::{
    RuntimeCompletionResultAttempt, RuntimeCompletionResultAuthority,
    RuntimeCompletionResultRealized,
};
use crate::meerkat_machine::dsl::RuntimeCompletionResultClass;
use crate::tokio::sync::oneshot;

/// Mechanical failure while waiting for completion plumbing.
///
/// This is intentionally separate from [`CompletionOutcome`]: a closed waiter
/// channel or missing generated completion authority is not a public runtime
/// result class.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompletionWaitError {
    #[error("completion channel closed without an authorized result")]
    ChannelClosed,
    /// The exact executor attachment that admitted this request was replaced
    /// before its waiter resolved. The old input is durably cancelled; a retry
    /// is a new request owned by the successor attachment. Replacement
    /// execution must never be reported as the old request's success.
    #[error("executor attachment was replaced before request completion; retry the request")]
    AttachmentReplaced,
    #[error("{0}")]
    AuthorityUnavailable(String),
}

impl CompletionWaitError {
    pub fn wait_failure_observation(
        &self,
    ) -> crate::meerkat_machine::dsl::RuntimeCompletionWaitFailureObservation {
        match self {
            Self::ChannelClosed => {
                crate::meerkat_machine::dsl::RuntimeCompletionWaitFailureObservation::ChannelClosed
            }
            Self::AttachmentReplaced | Self::AuthorityUnavailable(_) => {
                crate::meerkat_machine::dsl::RuntimeCompletionWaitFailureObservation::AuthorityUnavailable
            }
        }
    }

    /// Stable typed predicate for retryable attachment replacement. Surfaces
    /// must not infer this class from an error string.
    #[must_use]
    pub const fn is_attachment_replaced(&self) -> bool {
        matches!(self, Self::AttachmentReplaced)
    }
}

/// Outcome delivered to a completion waiter.
#[derive(Debug, Clone)]
pub enum CompletionOutcome {
    /// The input was successfully consumed and produced a result.
    Completed(Box<RunResult>),
    /// The input was consumed but produced no RunResult (e.g. context-append ops).
    CompletedWithoutResult,
    /// The input reached a callback boundary and requires external tool
    /// fulfillment before the turn can continue.
    CallbackPending {
        tool_use_id: String,
        tool_name: String,
        args: Value,
    },
    /// One assistant tool-use batch is waiting on multiple external callbacks.
    CallbackBatchPending {
        pending_tool_calls: Vec<meerkat_core::error::PendingCallbackToolCall>,
    },
    /// The input reached the canonical cancellation terminal.
    Cancelled,
    /// The input was abandoned before completing, carrying typed failure
    /// metadata so every surface sees the same structured turn error the
    /// sibling [`AbandonedWithError`](Self::AbandonedWithError) carries.
    Abandoned {
        reason: String,
        error: TurnErrorMetadata,
    },
    /// The input was abandoned before completing, with typed failure metadata.
    AbandonedWithError {
        reason: String,
        error: TurnErrorMetadata,
    },
    /// The turn produced output, but a later runtime finalization step (the
    /// durable commit) failed, so the run is NOT durably terminal. The produced
    /// result is deliberately NOT carried on this outcome: a finalization
    /// failure must be treated as failure by every surface, never surfaced as a
    /// usable success result (that would be a false belief of success).
    CompletedWithFinalizationFailure { error: TurnErrorMetadata },
    /// The runtime was stopped or destroyed while the input was pending,
    /// carrying typed failure metadata describing the termination cause.
    RuntimeTerminated {
        reason: String,
        error: TurnErrorMetadata,
    },
}

/// Project one generated-authority completion onto the canonical
/// interaction-scoped terminal event. Both the comms live stream and durable
/// placed-turn observation use this single mapping so result/failure semantics
/// cannot drift between surfaces.
pub fn interaction_terminal_event(
    interaction_id: meerkat_core::interaction::InteractionId,
    outcome: CompletionOutcome,
) -> meerkat_core::event::AgentEvent {
    use meerkat_core::event::{AgentEvent, InteractionFailureReason};

    match outcome {
        CompletionOutcome::Completed(result) => {
            if let Some(extraction_error) = result.extraction_error {
                AgentEvent::InteractionFailed {
                    interaction_id,
                    reason: InteractionFailureReason::ExtractionFailed {
                        last_output: extraction_error.last_output,
                        attempts: extraction_error.attempts,
                        reason: extraction_error.reason,
                    },
                }
            } else {
                AgentEvent::InteractionComplete {
                    interaction_id,
                    result: result.text,
                    structured_output: result.structured_output,
                }
            }
        }
        CompletionOutcome::CompletedWithoutResult => AgentEvent::InteractionComplete {
            interaction_id,
            result: String::new(),
            structured_output: None,
        },
        CompletionOutcome::CallbackPending {
            tool_use_id,
            tool_name,
            args,
        } => AgentEvent::InteractionCallbackPending {
            interaction_id,
            pending_tool_calls: vec![meerkat_core::error::PendingCallbackToolCall {
                tool_use_id,
                tool_name: tool_name.clone(),
                args: args.clone(),
            }],
            tool_name,
            args,
        },
        CompletionOutcome::CallbackBatchPending { pending_tool_calls } => {
            let first = pending_tool_calls.first().cloned().unwrap_or(
                meerkat_core::error::PendingCallbackToolCall {
                    tool_use_id: String::new(),
                    tool_name: "callback_batch".to_string(),
                    args: Value::Null,
                },
            );
            AgentEvent::InteractionCallbackPending {
                interaction_id,
                tool_name: first.tool_name,
                args: first.args,
                pending_tool_calls,
            }
        }
        CompletionOutcome::Cancelled => AgentEvent::InteractionFailed {
            interaction_id,
            reason: InteractionFailureReason::Cancelled,
        },
        CompletionOutcome::Abandoned { reason, .. }
        | CompletionOutcome::AbandonedWithError { reason, .. }
        | CompletionOutcome::RuntimeTerminated { reason, .. } => AgentEvent::InteractionFailed {
            interaction_id,
            reason: InteractionFailureReason::abandoned(reason),
        },
        CompletionOutcome::CompletedWithFinalizationFailure { error } => {
            let detail = error
                .detail
                .unwrap_or_else(|| "turn finalization failed".to_string());
            AgentEvent::InteractionFailed {
                interaction_id,
                reason: InteractionFailureReason::finalization_failed(detail),
            }
        }
    }
}

/// Runtime-minted observation for post-completion cleanup.
///
/// Cleanup code can inspect this generated-facing observation, but it cannot
/// rewrite the public [`CompletionOutcome`] that the waiter received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionCleanupObservation {
    owner_session_id: SessionId,
    owner_agent_runtime_id: Option<crate::meerkat_machine::dsl::AgentRuntimeId>,
    owner_fence_token: Option<crate::meerkat_machine::dsl::FenceToken>,
    owner_runtime_generation: Option<crate::meerkat_machine::dsl::Generation>,
    owner_runtime_epoch_id: Option<crate::meerkat_machine::dsl::RuntimeEpochId>,
    observed_outcome: crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome,
}

impl CompletionCleanupObservation {
    fn from_realized_result(realized: RuntimeCompletionResultRealized) -> Self {
        Self {
            owner_session_id: realized.session_id().clone(),
            owner_agent_runtime_id: realized.agent_runtime_id().cloned(),
            owner_fence_token: realized.fence_token(),
            owner_runtime_generation: realized.runtime_generation(),
            owner_runtime_epoch_id: realized.runtime_epoch_id().cloned(),
            observed_outcome: realized.cleanup_observation(),
        }
    }

    /// Return whether this runtime-minted observation proves that `session_id`
    /// reached the machine-owned runtime-termination completion class.
    ///
    /// Cleanup relays use this narrow proof when the machine-owned unregister
    /// saga wins the race and removes the runtime registration after the
    /// executor's external-only cleanup, before the completion relay inspects
    /// the registry. It must not be replaced with a generic "registration is
    /// absent" check: absence alone carries no terminal authority.
    #[must_use]
    pub fn proves_runtime_termination_for(&self, session_id: &SessionId) -> bool {
        self.owner_session_id == *session_id
            && self.observed_outcome
                == crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome::RuntimeTerminated
    }

    pub(crate) fn owner_session_id(&self) -> &SessionId {
        &self.owner_session_id
    }

    pub(crate) fn owner_agent_runtime_id(
        &self,
    ) -> Option<&crate::meerkat_machine::dsl::AgentRuntimeId> {
        self.owner_agent_runtime_id.as_ref()
    }

    pub(crate) fn owner_fence_token(&self) -> Option<crate::meerkat_machine::dsl::FenceToken> {
        self.owner_fence_token
    }

    pub(crate) fn owner_runtime_generation(
        &self,
    ) -> Option<crate::meerkat_machine::dsl::Generation> {
        self.owner_runtime_generation
    }

    pub(crate) fn owner_runtime_epoch_id(
        &self,
    ) -> Option<&crate::meerkat_machine::dsl::RuntimeEpochId> {
        self.owner_runtime_epoch_id.as_ref()
    }

    pub(crate) fn observed_outcome(
        &self,
    ) -> crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome {
        self.observed_outcome
    }
}

/// Result carried on completion waiter plumbing after generated authority has
/// selected both the public result class and cleanup observation.
#[derive(Debug)]
struct CompletionDelivery {
    outcome: CompletionOutcome,
    cleanup_observation: CompletionCleanupObservation,
}

/// Snapshot of one input's registered completion waiters.
///
/// This is a diagnostic/supporting-carrier view only. Waiter counts are never
/// semantic runtime truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionWaiterEntrySnapshot {
    pub input_id: InputId,
    pub waiter_count: usize,
}

/// Diagnostic snapshot of the completion waiter registry.
///
/// This makes the carrier explicit for MeerkatMachine mapping work without
/// promoting waiter plumbing into canonical runtime semantics.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompletionRegistrySnapshot {
    pub input_count: usize,
    pub waiter_count: usize,
    pub waiting_inputs: Vec<CompletionWaiterEntrySnapshot>,
}

/// Handle for awaiting the completion of an accepted input.
#[derive(Debug)]
pub struct CompletionHandle {
    rx: oneshot::Receiver<Result<CompletionDelivery, CompletionWaitError>>,
}

impl CompletionHandle {
    async fn try_wait_delivery(self) -> Result<CompletionDelivery, CompletionWaitError> {
        self.rx
            .await
            .unwrap_or(Err(CompletionWaitError::ChannelClosed))
    }

    /// Wait for the input to reach a terminal state or report mechanical waiter failure.
    pub async fn try_wait(self) -> Result<CompletionOutcome, CompletionWaitError> {
        self.try_wait_delivery()
            .await
            .map(|delivery| delivery.outcome)
    }

    /// Wait for completion and return the generated cleanup observation carried
    /// with the authorized public outcome.
    pub async fn try_wait_with_cleanup_observation(
        self,
    ) -> Result<(CompletionOutcome, CompletionCleanupObservation), CompletionWaitError> {
        let delivery = self.try_wait_delivery().await?;
        Ok((delivery.outcome, delivery.cleanup_observation))
    }

    /// Wait for the input to reach a terminal state or report mechanical waiter failure.
    pub async fn wait(self) -> Result<CompletionOutcome, CompletionWaitError> {
        self.try_wait().await
    }

    /// Wait for a test handle that is expected to resolve through generated authority.
    #[cfg(test)]
    pub(crate) async fn wait_authorized(self) -> CompletionOutcome {
        self.wait()
            .await
            .expect("completion waiter closed without an authorized result")
    }

    /// Relay completion through a cleanup future before resolving the returned
    /// handle. This lets surfaces transfer cleanup ownership immediately after
    /// accepting runtime work while still returning a completion handle.
    pub fn with_cleanup<F, Fut>(self, cleanup: F) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        crate::tokio::spawn(async move {
            let outcome = self.try_wait_delivery().await;
            cleanup().await;
            let _ = tx.send(outcome);
        });
        Self { rx }
    }

    /// Relay completion through a cleanup future that can inspect the outcome.
    ///
    /// The cleanup future receives a runtime-minted cleanup observation and
    /// cannot replace the completion result.
    pub fn with_outcome_cleanup<F, Fut>(self, cleanup: F) -> Self
    where
        F: FnOnce(CompletionCleanupObservation) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        crate::tokio::spawn(async move {
            let outcome = match self.try_wait_delivery().await {
                Ok(delivery) => {
                    cleanup(delivery.cleanup_observation.clone()).await;
                    Ok(delivery)
                }
                Err(error) => Err(error),
            };
            let _ = tx.send(outcome);
        });
        Self { rx }
    }

    /// Relay completion through cleanup that can observe either generated
    /// completion-cleanup evidence or a typed waiter failure.
    pub fn with_completion_cleanup<F, Fut>(self, cleanup: F) -> Self
    where
        F: FnOnce(Result<CompletionCleanupObservation, CompletionWaitError>) -> Fut
            + Send
            + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        crate::tokio::spawn(async move {
            let outcome = self.try_wait_delivery().await;
            match &outcome {
                Ok(delivery) => cleanup(Ok(delivery.cleanup_observation.clone())).await,
                Err(error) => cleanup(Err(error.clone())).await,
            }
            let _ = tx.send(outcome);
        });
        Self { rx }
    }

    /// Relay completion through required, fallible cleanup before publishing
    /// the outcome. A cleanup failure withholds a successful runtime result;
    /// when waiter delivery and cleanup both fail, both causes are preserved.
    pub fn with_resultful_completion_cleanup<F, Fut>(self, cleanup: F) -> Self
    where
        F: FnOnce(Result<CompletionCleanupObservation, CompletionWaitError>) -> Fut
            + Send
            + 'static,
        Fut: Future<Output = Result<(), CompletionWaitError>> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        crate::tokio::spawn(async move {
            let outcome = self.try_wait_delivery().await;
            let cleanup_input = match &outcome {
                Ok(delivery) => Ok(delivery.cleanup_observation.clone()),
                Err(error) => Err(error.clone()),
            };
            let cleanup_result = cleanup(cleanup_input).await;
            let gated = match (outcome, cleanup_result) {
                (Ok(delivery), Ok(())) => Ok(delivery),
                (Ok(_), Err(cleanup_error)) => Err(cleanup_error),
                (Err(primary_error), Ok(())) => Err(primary_error),
                (Err(primary_error), Err(cleanup_error)) => {
                    Err(CompletionWaitError::AuthorityUnavailable(format!(
                        "{primary_error}; additionally failed required completion cleanup: {cleanup_error}"
                    )))
                }
            };
            let _ = tx.send(gated);
        });
        Self { rx }
    }

    #[cfg(test)]
    fn already_resolved_internal(
        outcome: CompletionOutcome,
        realized: RuntimeCompletionResultRealized,
    ) -> Self {
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(Ok(CompletionDelivery {
            outcome,
            cleanup_observation: CompletionCleanupObservation::from_realized_result(realized),
        }));
        Self { rx }
    }

    #[cfg(test)]
    pub(crate) fn already_resolved_with_generated_class(
        outcome: CompletionOutcome,
        expected_class: crate::meerkat_machine::dsl::RuntimeCompletionResultClass,
        terminal: crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation,
        finalization: crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation,
    ) -> Result<Self, crate::RuntimeDriverError> {
        let run_id = if terminal
            == crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::RuntimeTerminated
        {
            None
        } else {
            Some(RunId::new())
        };
        let authority =
            crate::meerkat_machine::driver::machine_resolve_pre_resolved_runtime_completion_result(
                run_id.as_ref(),
                terminal,
                finalization,
            )?;
        let attempt = authority.begin_surface_resolution();
        if !attempt.allows(expected_class) {
            let generated_class = attempt.class();
            attempt.fail();
            return Err(crate::RuntimeDriverError::Internal(format!(
                "generated runtime completion authority returned {generated_class:?}, expected {expected_class:?}",
            )));
        }
        Ok(Self::already_resolved_internal(outcome, attempt.realize()))
    }

    #[cfg(test)]
    pub(crate) fn already_completed_without_result() -> Result<Self, crate::RuntimeDriverError> {
        Self::already_resolved_with_generated_class(
            CompletionOutcome::CompletedWithoutResult,
            crate::meerkat_machine::dsl::RuntimeCompletionResultClass::CompletedWithoutResult,
            crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::NoResult,
            crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
        )
    }

    #[cfg(test)]
    pub(crate) fn already_runtime_apply_failed(
        reason: String,
        error: TurnErrorMetadata,
    ) -> Result<Self, crate::RuntimeDriverError> {
        Self::already_resolved_with_generated_class(
            CompletionOutcome::AbandonedWithError { reason, error },
            crate::meerkat_machine::dsl::RuntimeCompletionResultClass::AbandonedWithError,
            crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::NoResult,
            crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Failed,
        )
    }

    #[cfg(test)]
    pub(crate) fn already_runtime_terminated(
        reason: String,
    ) -> Result<Self, crate::RuntimeDriverError> {
        Self::already_resolved_with_generated_class(
            CompletionOutcome::runtime_terminated(&reason),
            crate::meerkat_machine::dsl::RuntimeCompletionResultClass::RuntimeTerminated,
            crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::RuntimeTerminated,
            crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
        )
    }

    #[cfg(test)]
    pub(crate) fn already_callback_pending(
        tool_use_id: String,
        tool_name: String,
        args: Value,
    ) -> Result<Self, crate::RuntimeDriverError> {
        Self::already_resolved_with_generated_class(
            CompletionOutcome::CallbackPending {
                tool_use_id,
                tool_name,
                args,
            },
            crate::meerkat_machine::dsl::RuntimeCompletionResultClass::CallbackPending,
            crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::CallbackPending,
            crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
        )
    }
}

impl CompletionOutcome {
    /// Mint a [`RuntimeTerminated`](Self::RuntimeTerminated) outcome from a
    /// termination reason, attaching the typed terminal failure metadata every
    /// surface keys off (runtime stop/destroy is a fatal terminal boundary).
    fn runtime_terminated(reason: &str) -> Self {
        Self::RuntimeTerminated {
            reason: reason.to_string(),
            error: TurnErrorMetadata::terminal(
                TurnTerminalCauseKind::FatalFailure,
                TurnTerminalOutcome::Failed,
                reason,
            ),
        }
    }

    pub fn abandoned_reason(&self) -> Option<&str> {
        match self {
            Self::Abandoned { reason, .. } | Self::AbandonedWithError { reason, .. } => {
                Some(reason)
            }
            _ => None,
        }
    }

    pub fn error_metadata(&self) -> Option<&TurnErrorMetadata> {
        match self {
            Self::Abandoned { error, .. }
            | Self::AbandonedWithError { error, .. }
            | Self::CompletedWithFinalizationFailure { error, .. }
            | Self::RuntimeTerminated { error, .. } => Some(error),
            _ => None,
        }
    }
}

fn authorized_completion_outcome(
    terminal: Option<&CoreApplyTerminal>,
    authority: RuntimeCompletionResultAuthority,
    finalization_error: Option<TurnErrorMetadata>,
    runtime_termination_reason: Option<&str>,
) -> Result<(CompletionOutcome, CompletionCleanupObservation), CompletionWaitError> {
    let attempt = authority.begin_surface_resolution();
    if runtime_termination_reason.is_some()
        && attempt.class() != RuntimeCompletionResultClass::RuntimeTerminated
    {
        let actual = attempt.class();
        attempt.fail();
        return Err(CompletionWaitError::AuthorityUnavailable(format!(
            "runtime completion authority resolved {actual:?} with a runtime-termination reason"
        )));
    }
    let outcome = match attempt.class() {
        RuntimeCompletionResultClass::Completed => {
            let Some(CoreApplyTerminal::RunResult(result)) = terminal else {
                attempt.fail();
                return Err(CompletionWaitError::AuthorityUnavailable(
                    "runtime completion authority resolved Completed without result payload"
                        .to_string(),
                ));
            };
            CompletionOutcome::Completed(Box::new(result.as_ref().clone()))
        }
        RuntimeCompletionResultClass::CompletedWithoutResult => {
            if !matches!(terminal, Some(CoreApplyTerminal::NoPendingBoundary) | None) {
                attempt.fail();
                return Err(CompletionWaitError::AuthorityUnavailable(
                    "runtime completion authority resolved CompletedWithoutResult with terminal payload"
                        .to_string(),
                ));
            }
            CompletionOutcome::CompletedWithoutResult
        }
        RuntimeCompletionResultClass::CallbackPending => match terminal {
            Some(CoreApplyTerminal::CallbackPending {
                tool_use_id,
                tool_name,
                args,
            }) => CompletionOutcome::CallbackPending {
                tool_use_id: tool_use_id.clone(),
                tool_name: tool_name.clone(),
                args: args.clone(),
            },
            Some(CoreApplyTerminal::CallbackBatchPending { pending_tool_calls }) => {
                CompletionOutcome::CallbackBatchPending {
                    pending_tool_calls: pending_tool_calls.clone(),
                }
            }
            _ => {
                attempt.fail();
                return Err(CompletionWaitError::AuthorityUnavailable(
                        "runtime completion authority resolved CallbackPending without callback payload"
                            .to_string(),
                    ));
            }
        },
        RuntimeCompletionResultClass::Cancelled => {
            if terminal.is_some() || finalization_error.is_some() {
                attempt.fail();
                return Err(CompletionWaitError::AuthorityUnavailable(
                    "runtime completion authority resolved Cancelled with payload".to_string(),
                ));
            }
            CompletionOutcome::Cancelled
        }
        RuntimeCompletionResultClass::AbandonedWithError => {
            if matches!(terminal, Some(CoreApplyTerminal::RunResult(_))) {
                attempt.fail();
                return Err(CompletionWaitError::AuthorityUnavailable(
                    "runtime completion authority resolved AbandonedWithError with result payload"
                        .to_string(),
                ));
            }
            let Some(error) = finalization_error else {
                attempt.fail();
                return Err(CompletionWaitError::AuthorityUnavailable(
                    "runtime completion authority resolved AbandonedWithError without typed error"
                        .to_string(),
                ));
            };
            let reason = error
                .detail
                .clone()
                .unwrap_or_else(|| "runtime finalization failed".to_string());
            CompletionOutcome::AbandonedWithError { reason, error }
        }
        RuntimeCompletionResultClass::CompletedWithFinalizationFailure => {
            let Some(CoreApplyTerminal::RunResult(_)) = terminal else {
                attempt.fail();
                return Err(CompletionWaitError::AuthorityUnavailable(
                    "runtime completion authority resolved CompletedWithFinalizationFailure without result payload"
                        .to_string(),
                ));
            };
            let Some(error) = finalization_error else {
                attempt.fail();
                return Err(CompletionWaitError::AuthorityUnavailable(
                    "runtime completion authority resolved finalization failure without typed error"
                        .to_string(),
                ));
            };
            CompletionOutcome::CompletedWithFinalizationFailure { error }
        }
        RuntimeCompletionResultClass::RuntimeTerminated => {
            if terminal.is_some() || finalization_error.is_some() {
                attempt.fail();
                return Err(CompletionWaitError::AuthorityUnavailable(
                    "runtime completion authority resolved RuntimeTerminated with payload"
                        .to_string(),
                ));
            }
            let Some(reason) = runtime_termination_reason.filter(|reason| !reason.is_empty())
            else {
                attempt.fail();
                return Err(CompletionWaitError::AuthorityUnavailable(
                    "runtime completion authority resolved RuntimeTerminated without its exact reason"
                        .to_string(),
                ));
            };
            CompletionOutcome::runtime_terminated(reason)
        }
    };
    Ok((
        outcome,
        CompletionCleanupObservation::from_realized_result(attempt.realize()),
    ))
}

/// One generated-authority terminal decision projected onto every public
/// consumer of a runtime completion. Interaction events and waiter delivery
/// must come from this same non-cloneable bundle so no caller can preview the
/// generated result twice and accidentally publish one class while returning
/// another.
#[must_use = "authorized runtime terminal bundle must be published and/or delivered"]
pub(crate) struct AuthorizedRuntimeTerminalBundle {
    outcome: CompletionOutcome,
    cleanup_observation: CompletionCleanupObservation,
    interaction_events: Vec<meerkat_core::event::AgentEvent>,
}

impl AuthorizedRuntimeTerminalBundle {
    pub(crate) fn interaction_events(&self) -> &[meerkat_core::event::AgentEvent] {
        &self.interaction_events
    }

    #[cfg(test)]
    fn into_interaction_events(self) -> Vec<meerkat_core::event::AgentEvent> {
        self.interaction_events
    }
}

/// Consume generated completion authority exactly once and create the shared
/// event/waiter terminal bundle for the whole runtime batch.
pub(crate) fn authorize_runtime_terminal_bundle(
    interaction_ids: &[meerkat_core::interaction::InteractionId],
    terminal: Option<&CoreApplyTerminal>,
    authority: RuntimeCompletionResultAuthority,
    finalization_error: Option<TurnErrorMetadata>,
    runtime_termination_reason: Option<&str>,
) -> Result<AuthorizedRuntimeTerminalBundle, CompletionWaitError> {
    let (outcome, cleanup_observation) = authorized_completion_outcome(
        terminal,
        authority,
        finalization_error,
        runtime_termination_reason,
    )?;
    let interaction_events = interaction_ids
        .iter()
        .map(|interaction_id| interaction_terminal_event(*interaction_id, outcome.clone()))
        .collect();
    Ok(AuthorizedRuntimeTerminalBundle {
        outcome,
        cleanup_observation,
        interaction_events,
    })
}

/// Consume one generated completion authority for the exact directed batch
/// and project the same authorized outcome onto every interaction ID.
#[cfg(test)]
pub(crate) fn authorized_interaction_terminal_events(
    interaction_ids: &[meerkat_core::interaction::InteractionId],
    terminal: Option<&CoreApplyTerminal>,
    authority: RuntimeCompletionResultAuthority,
    finalization_error: Option<TurnErrorMetadata>,
) -> Result<Vec<meerkat_core::event::AgentEvent>, CompletionWaitError> {
    authorize_runtime_terminal_bundle(
        interaction_ids,
        terminal,
        authority,
        finalization_error,
        None,
    )
    .map(AuthorizedRuntimeTerminalBundle::into_interaction_events)
}

/// Registry of pending completion waiters, keyed by InputId.
///
/// Uses `Vec<Sender>` per InputId to support multiple waiters for the same input
/// (e.g. dedup of in-flight input registers a second waiter for the same InputId).
#[derive(Default)]
pub(crate) struct CompletionRegistry {
    waiters:
        HashMap<InputId, Vec<oneshot::Sender<Result<CompletionDelivery, CompletionWaitError>>>>,
}

impl CompletionRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn take_waiters(
        &mut self,
        input_id: &InputId,
    ) -> Option<Vec<oneshot::Sender<Result<CompletionDelivery, CompletionWaitError>>>> {
        self.waiters.remove(input_id)
    }

    fn send_outcome(
        senders: Vec<oneshot::Sender<Result<CompletionDelivery, CompletionWaitError>>>,
        outcome: CompletionOutcome,
        cleanup_observation: CompletionCleanupObservation,
    ) {
        for tx in senders {
            let outcome = outcome.clone();
            let _ = tx.send(Ok(CompletionDelivery {
                outcome,
                cleanup_observation: cleanup_observation.clone(),
            }));
        }
    }

    fn send_error(
        senders: Vec<oneshot::Sender<Result<CompletionDelivery, CompletionWaitError>>>,
        error: CompletionWaitError,
    ) {
        for tx in senders {
            let _ = tx.send(Err(error.clone()));
        }
    }

    fn authority_mismatch_error(
        authority: &RuntimeCompletionResultAttempt,
        expected: RuntimeCompletionResultClass,
    ) -> CompletionWaitError {
        CompletionWaitError::AuthorityUnavailable(format!(
            "generated runtime completion authority returned {:?}, expected {expected:?}",
            authority.class()
        ))
    }

    fn fail_input_authority_mismatch(
        &mut self,
        input_id: &InputId,
        authority: RuntimeCompletionResultAttempt,
        expected: RuntimeCompletionResultClass,
    ) {
        let error = Self::authority_mismatch_error(&authority, expected);
        authority.fail();
        if let Some(senders) = self.take_waiters(input_id) {
            Self::send_error(senders, error);
        }
    }

    #[cfg(test)]
    fn fail_inputs_authority_mismatch<I>(
        &mut self,
        input_ids: I,
        authority: RuntimeCompletionResultAttempt,
        expected: RuntimeCompletionResultClass,
    ) where
        I: IntoIterator<Item = InputId>,
    {
        let error = Self::authority_mismatch_error(&authority, expected);
        authority.fail();
        self.fail_inputs(input_ids, error);
    }

    fn cleanup_from_realized_attempt(
        authority: RuntimeCompletionResultAttempt,
    ) -> CompletionCleanupObservation {
        CompletionCleanupObservation::from_realized_result(authority.realize())
    }

    /// Register a waiter for an input. Returns the handle the caller will await.
    ///
    /// Multiple waiters can be registered for the same InputId — all will be
    /// resolved when the input reaches a terminal state.
    pub(crate) fn register(&mut self, input_id: InputId) -> CompletionHandle {
        let (tx, rx) = oneshot::channel();
        self.waiters.entry(input_id).or_default().push(tx);
        CompletionHandle { rx }
    }

    /// Resolve all waiters for a completed input.
    #[cfg(test)]
    fn resolve_completed(
        &mut self,
        input_id: &InputId,
        result: RunResult,
        cleanup_observation: CompletionCleanupObservation,
    ) {
        if let Some(senders) = self.take_waiters(input_id) {
            Self::send_outcome(
                senders,
                CompletionOutcome::Completed(Box::new(result)),
                cleanup_observation,
            );
        }
    }

    #[cfg(test)]
    pub(crate) fn resolve_completed_authorized(
        &mut self,
        input_id: &InputId,
        result: RunResult,
        authority: RuntimeCompletionResultAuthority,
    ) {
        let expected = RuntimeCompletionResultClass::Completed;
        let attempt = authority.begin_surface_resolution();
        if !attempt.allows(expected) {
            self.fail_input_authority_mismatch(input_id, attempt, expected);
            return;
        }
        self.resolve_completed(
            input_id,
            result,
            Self::cleanup_from_realized_attempt(attempt),
        );
    }

    #[cfg(test)]
    pub(crate) fn resolve_runtime_completion_authorized<I>(
        &mut self,
        input_ids: I,
        terminal: Option<&CoreApplyTerminal>,
        authority: RuntimeCompletionResultAuthority,
        finalization_error: Option<TurnErrorMetadata>,
    ) where
        I: IntoIterator<Item = InputId>,
    {
        let input_ids: Vec<InputId> = input_ids.into_iter().collect();
        if !input_ids
            .iter()
            .any(|input_id| self.waiters.contains_key(input_id))
        {
            let attempt = authority.begin_surface_resolution();
            attempt.abandon();
            return;
        }
        let (outcome, cleanup_observation) =
            match authorized_completion_outcome(terminal, authority, finalization_error, None) {
                Ok(projected) => projected,
                Err(error) => {
                    self.fail_inputs(input_ids, error);
                    return;
                }
            };
        for input_id in input_ids {
            if let Some(senders) = self.take_waiters(&input_id) {
                Self::send_outcome(senders, outcome.clone(), cleanup_observation.clone());
            }
        }
    }

    /// Mechanically deliver a terminal decision that already consumed the
    /// generated result authority used for durable interaction publication.
    pub(crate) fn resolve_authorized_runtime_terminal_bundle<I>(
        &mut self,
        input_ids: I,
        bundle: AuthorizedRuntimeTerminalBundle,
    ) where
        I: IntoIterator<Item = InputId>,
    {
        for input_id in input_ids {
            if let Some(senders) = self.take_waiters(&input_id) {
                Self::send_outcome(
                    senders,
                    bundle.outcome.clone(),
                    bundle.cleanup_observation.clone(),
                );
            }
        }
    }

    /// Resolve all waiters for an input that completed without producing a RunResult.
    fn resolve_without_result(
        &mut self,
        input_id: &InputId,
        cleanup_observation: CompletionCleanupObservation,
    ) {
        if let Some(senders) = self.take_waiters(input_id) {
            Self::send_outcome(
                senders,
                CompletionOutcome::CompletedWithoutResult,
                cleanup_observation,
            );
        }
    }

    pub(crate) fn resolve_without_result_authorized(
        &mut self,
        input_id: &InputId,
        authority: RuntimeCompletionResultAuthority,
    ) {
        let expected = RuntimeCompletionResultClass::CompletedWithoutResult;
        let attempt = authority.begin_surface_resolution();
        if !attempt.allows(expected) {
            self.fail_input_authority_mismatch(input_id, attempt, expected);
            return;
        }
        self.resolve_without_result(input_id, Self::cleanup_from_realized_attempt(attempt));
    }

    /// Resolve all waiters for an input that reached a callback boundary.
    #[cfg(test)]
    fn resolve_callback_pending(
        &mut self,
        input_id: &InputId,
        tool_use_id: String,
        tool_name: String,
        args: Value,
        cleanup_observation: CompletionCleanupObservation,
    ) {
        if let Some(senders) = self.take_waiters(input_id) {
            Self::send_outcome(
                senders,
                CompletionOutcome::CallbackPending {
                    tool_use_id,
                    tool_name,
                    args,
                },
                cleanup_observation,
            );
        }
    }

    #[cfg(test)]
    pub(crate) fn resolve_callback_pending_authorized(
        &mut self,
        input_id: &InputId,
        tool_use_id: String,
        tool_name: String,
        args: Value,
        authority: RuntimeCompletionResultAuthority,
    ) {
        let expected = RuntimeCompletionResultClass::CallbackPending;
        let attempt = authority.begin_surface_resolution();
        if !attempt.allows(expected) {
            self.fail_input_authority_mismatch(input_id, attempt, expected);
            return;
        }
        self.resolve_callback_pending(
            input_id,
            tool_use_id,
            tool_name,
            args,
            Self::cleanup_from_realized_attempt(attempt),
        );
    }

    /// Resolve all waiters for an input that reached the cancellation terminal.
    #[cfg(test)]
    fn resolve_cancelled(
        &mut self,
        input_id: &InputId,
        cleanup_observation: CompletionCleanupObservation,
    ) {
        if let Some(senders) = self.take_waiters(input_id) {
            Self::send_outcome(senders, CompletionOutcome::Cancelled, cleanup_observation);
        }
    }

    #[cfg(test)]
    pub(crate) fn resolve_cancelled_authorized(
        &mut self,
        input_id: &InputId,
        authority: RuntimeCompletionResultAuthority,
    ) {
        let expected = RuntimeCompletionResultClass::Cancelled;
        let attempt = authority.begin_surface_resolution();
        if !attempt.allows(expected) {
            self.fail_input_authority_mismatch(input_id, attempt, expected);
            return;
        }
        self.resolve_cancelled(input_id, Self::cleanup_from_realized_attempt(attempt));
    }

    /// Resolve all waiters for an abandoned input with typed failure metadata.
    #[cfg(test)]
    fn resolve_abandoned_with_error(
        &mut self,
        input_id: &InputId,
        reason: String,
        error: TurnErrorMetadata,
        cleanup_observation: CompletionCleanupObservation,
    ) {
        if let Some(senders) = self.take_waiters(input_id) {
            Self::send_outcome(
                senders,
                CompletionOutcome::AbandonedWithError { reason, error },
                cleanup_observation,
            );
        }
    }

    #[cfg(test)]
    pub(crate) fn resolve_abandoned_with_error_authorized(
        &mut self,
        input_id: &InputId,
        reason: String,
        error: TurnErrorMetadata,
        authority: RuntimeCompletionResultAuthority,
    ) {
        let expected = RuntimeCompletionResultClass::AbandonedWithError;
        let attempt = authority.begin_surface_resolution();
        if !attempt.allows(expected) {
            self.fail_input_authority_mismatch(input_id, attempt, expected);
            return;
        }
        self.resolve_abandoned_with_error(
            input_id,
            reason,
            error,
            Self::cleanup_from_realized_attempt(attempt),
        );
    }

    /// Resolve all waiters for a turn whose output exists but finalization
    /// failed after output production.
    #[cfg(test)]
    fn resolve_completed_with_finalization_failure(
        &mut self,
        input_id: &InputId,
        error: TurnErrorMetadata,
        cleanup_observation: CompletionCleanupObservation,
    ) {
        if let Some(senders) = self.take_waiters(input_id) {
            Self::send_outcome(
                senders,
                CompletionOutcome::CompletedWithFinalizationFailure { error },
                cleanup_observation,
            );
        }
    }

    #[cfg(test)]
    pub(crate) fn resolve_completed_with_finalization_failure_authorized(
        &mut self,
        input_id: &InputId,
        error: TurnErrorMetadata,
        authority: RuntimeCompletionResultAuthority,
    ) {
        let expected = RuntimeCompletionResultClass::CompletedWithFinalizationFailure;
        let attempt = authority.begin_surface_resolution();
        if !attempt.allows(expected) {
            self.fail_input_authority_mismatch(input_id, attempt, expected);
            return;
        }
        self.resolve_completed_with_finalization_failure(
            input_id,
            error,
            Self::cleanup_from_realized_attempt(attempt),
        );
    }

    /// Resolve all pending waiters with a termination error.
    ///
    /// The public termination result class is supplied by generated
    /// MeerkatMachine authority; this registry method only fans the authorized
    /// class out to waiter channels.
    #[cfg(test)]
    pub(crate) fn resolve_all_runtime_terminated(
        &mut self,
        reason: &str,
        authority: RuntimeCompletionResultAuthority,
    ) {
        let expected = RuntimeCompletionResultClass::RuntimeTerminated;
        let attempt = authority.begin_surface_resolution();
        if !attempt.allows(expected) {
            let error = Self::authority_mismatch_error(&attempt, expected);
            attempt.fail();
            self.fail_all_waiters(error);
            return;
        }
        let cleanup_observation = Self::cleanup_from_realized_attempt(attempt);
        for (_, senders) in self.waiters.drain() {
            Self::send_outcome(
                senders,
                CompletionOutcome::runtime_terminated(reason),
                cleanup_observation.clone(),
            );
        }
    }

    #[cfg(test)]
    pub(crate) fn resolve_inputs_runtime_terminated<I>(
        &mut self,
        input_ids: I,
        reason: &str,
        authority: RuntimeCompletionResultAuthority,
    ) where
        I: IntoIterator<Item = InputId>,
    {
        let input_ids: Vec<InputId> = input_ids.into_iter().collect();
        let expected = RuntimeCompletionResultClass::RuntimeTerminated;
        let attempt = authority.begin_surface_resolution();
        if !attempt.allows(expected) {
            self.fail_inputs_authority_mismatch(input_ids, attempt, expected);
            return;
        }
        let cleanup_observation = Self::cleanup_from_realized_attempt(attempt);
        for input_id in input_ids {
            if let Some(senders) = self.take_waiters(&input_id) {
                Self::send_outcome(
                    senders,
                    CompletionOutcome::runtime_terminated(reason),
                    cleanup_observation.clone(),
                );
            }
        }
    }

    pub(crate) fn fail_all_waiters(&mut self, error: CompletionWaitError) {
        for (_, senders) in self.waiters.drain() {
            Self::send_error(senders, error.clone());
        }
    }

    pub(crate) fn fail_inputs<I>(&mut self, input_ids: I, error: CompletionWaitError)
    where
        I: IntoIterator<Item = InputId>,
    {
        for input_id in input_ids {
            if let Some(senders) = self.take_waiters(&input_id) {
                Self::send_error(senders, error.clone());
            }
        }
    }

    /// Fail waiter plumbing whose input IDs are no longer pending after a
    /// reconciliation that intentionally preserves/requeues semantic work.
    /// Recycle/recover must not fabricate a runtime-terminal public outcome:
    /// any missing waiter recipient is an internal reconciliation fault, not
    /// a newly authorized input terminal.
    pub(crate) fn fail_not_pending_waiters<F>(
        &mut self,
        mut is_still_pending: F,
        error: CompletionWaitError,
    ) where
        F: FnMut(&InputId) -> bool,
    {
        self.waiters.retain(|input_id, senders| {
            if is_still_pending(input_id) {
                return true;
            }
            Self::send_error(std::mem::take(senders), error.clone());
            false
        });
    }

    /// Snapshot the current waiter carrier without mutating it.
    pub(crate) fn diagnostic_snapshot(&self) -> CompletionRegistrySnapshot {
        let mut waiting_inputs: Vec<_> = self
            .waiters
            .iter()
            .map(|(input_id, senders)| CompletionWaiterEntrySnapshot {
                input_id: input_id.clone(),
                waiter_count: senders.len(),
            })
            .collect();
        waiting_inputs
            .sort_by(|left, right| left.input_id.to_string().cmp(&right.input_id.to_string()));

        CompletionRegistrySnapshot {
            input_count: waiting_inputs.len(),
            waiter_count: waiting_inputs.iter().map(|entry| entry.waiter_count).sum(),
            waiting_inputs,
        }
    }

    /// Check if there are any pending waiters.
    ///
    /// Test-only introspection. Production code must treat the registry as
    /// waiter plumbing rather than semantic runtime truth.
    #[cfg(test)]
    pub fn debug_has_waiters(&self) -> bool {
        !self.waiters.is_empty()
    }

    /// Number of pending waiters (total across all InputIds).
    ///
    /// Test-only introspection. Production code must treat the registry as
    /// waiter plumbing rather than semantic runtime truth.
    #[cfg(test)]
    pub fn debug_waiter_count(&self) -> usize {
        self.waiters.values().map(Vec::len).sum()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::meerkat_machine::dsl::{
        RuntimeCompletionObservedOutcome, RuntimeCompletionResultClass,
    };
    use meerkat_core::types::{SessionId, Usage};

    fn make_run_result() -> RunResult {
        RunResult {
            text: "hello".into(),
            session_id: SessionId::new(),
            usage: Usage::default(),
            turns: 1,
            tool_calls: 0,
            terminal_cause_kind: None,
            structured_output: None,
            extraction_error: None,
            schema_warnings: None,
            skill_diagnostics: None,
        }
    }

    fn authority(
        result_class: RuntimeCompletionResultClass,
        cleanup_observation: RuntimeCompletionObservedOutcome,
    ) -> RuntimeCompletionResultAuthority {
        crate::meerkat_machine::driver::test_runtime_completion_authority(
            result_class,
            cleanup_observation,
        )
    }

    #[test]
    fn callback_batch_terminal_event_preserves_complete_typed_pending_set() {
        let interaction_id = meerkat_core::interaction::InteractionId(uuid::Uuid::new_v4());
        let calls = vec![
            meerkat_core::error::PendingCallbackToolCall {
                tool_use_id: "call-a".to_string(),
                tool_name: "ask_a".to_string(),
                args: serde_json::json!({"tool_use_id": "call-a", "question": "a"}),
            },
            meerkat_core::error::PendingCallbackToolCall {
                tool_use_id: "call-b".to_string(),
                tool_name: "ask_b".to_string(),
                args: serde_json::json!({"tool_use_id": "call-b", "question": "b"}),
            },
        ];

        let event = interaction_terminal_event(
            interaction_id,
            CompletionOutcome::CallbackBatchPending {
                pending_tool_calls: calls.clone(),
            },
        );
        let meerkat_core::event::AgentEvent::InteractionCallbackPending {
            interaction_id: actual_id,
            tool_name,
            args,
            pending_tool_calls,
        } = event
        else {
            panic!("expected callback-pending interaction event");
        };
        assert_eq!(actual_id, interaction_id);
        assert_eq!(tool_name, "ask_a");
        assert_eq!(args, calls[0].args);
        assert_eq!(pending_tool_calls, calls);
    }

    #[tokio::test]
    async fn register_and_complete() {
        let mut registry = CompletionRegistry::new();
        let input_id = InputId::new();
        let handle = registry.register(input_id.clone());

        assert!(registry.debug_has_waiters());
        assert_eq!(registry.debug_waiter_count(), 1);

        let result = make_run_result();
        registry.resolve_completed_authorized(
            &input_id,
            result,
            authority(
                RuntimeCompletionResultClass::Completed,
                RuntimeCompletionObservedOutcome::Completed,
            ),
        );

        match handle.wait_authorized().await {
            CompletionOutcome::Completed(r) => assert_eq!(r.text, "hello"),
            other => panic!("Expected Completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn register_and_fail_waiter() {
        let mut registry = CompletionRegistry::new();
        let input_id = InputId::new();
        let handle = registry.register(input_id.clone());

        registry.fail_inputs(
            [input_id],
            CompletionWaitError::AuthorityUnavailable("retired".into()),
        );

        match handle.try_wait().await {
            Err(CompletionWaitError::AuthorityUnavailable(reason)) => assert_eq!(reason, "retired"),
            other => panic!("Expected wait error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn mismatched_result_authority_fails_waiter_closed() {
        let mut registry = CompletionRegistry::new();
        let input_id = InputId::new();
        let handle = registry.register(input_id.clone());

        registry.resolve_completed_authorized(
            &input_id,
            make_run_result(),
            authority(
                RuntimeCompletionResultClass::Cancelled,
                RuntimeCompletionObservedOutcome::Cancelled,
            ),
        );

        assert!(!registry.debug_has_waiters());
        match handle.try_wait().await {
            Err(CompletionWaitError::AuthorityUnavailable(reason)) => {
                assert!(reason.contains("Cancelled"));
                assert!(reason.contains("Completed"));
            }
            other => panic!("Expected authority mismatch wait error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_all_runtime_terminated() {
        let mut registry = CompletionRegistry::new();
        let h1 = registry.register(InputId::new());
        let h2 = registry.register(InputId::new());

        registry.resolve_all_runtime_terminated(
            "runtime stopped",
            authority(
                RuntimeCompletionResultClass::RuntimeTerminated,
                RuntimeCompletionObservedOutcome::RuntimeTerminated,
            ),
        );

        assert!(!registry.debug_has_waiters());

        match h1.wait_authorized().await {
            CompletionOutcome::RuntimeTerminated { reason, .. } => {
                assert_eq!(reason, "runtime stopped");
            }
            other => panic!("Expected RuntimeTerminated, got {other:?}"),
        }
        match h2.wait_authorized().await {
            CompletionOutcome::RuntimeTerminated { reason, .. } => {
                assert_eq!(reason, "runtime stopped");
            }
            other => panic!("Expected RuntimeTerminated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn runtime_terminal_bundle_preserves_one_exact_non_default_reason() {
        let input_id = InputId::new();
        let interaction_id = meerkat_core::interaction::InteractionId(input_id.0);
        let reason = "runtime destroyed by operator blue/17";
        let bundle = authorize_runtime_terminal_bundle(
            &[interaction_id],
            None,
            authority(
                RuntimeCompletionResultClass::RuntimeTerminated,
                RuntimeCompletionObservedOutcome::RuntimeTerminated,
            ),
            None,
            Some(reason),
        )
        .expect("generated runtime termination should authorize one exact terminal bundle");

        assert!(matches!(
            bundle.interaction_events(),
            [meerkat_core::event::AgentEvent::InteractionFailed {
                interaction_id: emitted_id,
                reason: meerkat_core::event::InteractionFailureReason::Abandoned { detail },
            }] if *emitted_id == interaction_id && detail == reason
        ));

        let mut registry = CompletionRegistry::new();
        let handle = registry.register(input_id.clone());
        registry.resolve_authorized_runtime_terminal_bundle([input_id], bundle);
        match handle.wait_authorized().await {
            CompletionOutcome::RuntimeTerminated {
                reason: waiter_reason,
                error,
            } => {
                assert_eq!(waiter_reason, reason);
                assert_eq!(error.detail.as_deref(), Some(reason));
            }
            other => panic!("Expected RuntimeTerminated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn mismatched_runtime_terminated_authority_fails_all_waiters_closed() {
        let mut registry = CompletionRegistry::new();
        let h1 = registry.register(InputId::new());
        let h2 = registry.register(InputId::new());

        registry.resolve_all_runtime_terminated(
            "runtime stopped",
            authority(
                RuntimeCompletionResultClass::CompletedWithoutResult,
                RuntimeCompletionObservedOutcome::CompletedWithoutResult,
            ),
        );

        assert!(!registry.debug_has_waiters());
        for handle in [h1, h2] {
            match handle.try_wait().await {
                Err(CompletionWaitError::AuthorityUnavailable(reason)) => {
                    assert!(reason.contains("CompletedWithoutResult"));
                    assert!(reason.contains("RuntimeTerminated"));
                }
                other => panic!("Expected authority mismatch wait error, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn cleanup_rejects_observation_from_another_session() {
        let adapter = crate::meerkat_machine::MeerkatMachine::ephemeral();
        let source_session_id = SessionId::new();
        let target_session_id = SessionId::new();
        adapter
            .prepare_bindings(source_session_id.clone())
            .await
            .expect("source session should prepare runtime bindings");
        adapter
            .prepare_bindings(target_session_id.clone())
            .await
            .expect("target session should prepare runtime bindings");

        let input = crate::Input::Prompt(crate::PromptInput::new(
            "source session pending completion",
            None,
        ));
        let (_outcome, handle) = adapter
            .accept_input_with_completion(&source_session_id, input)
            .await
            .expect("source input should be accepted");
        let handle = handle.expect("source input should have a completion waiter");
        adapter
            .stop_runtime_executor(&source_session_id, "source stopped")
            .await
            .expect("source stop should resolve waiter");
        let (_outcome, observation) = handle
            .try_wait_with_cleanup_observation()
            .await
            .expect("waiter should resolve with generated cleanup observation");

        assert_eq!(observation.owner_session_id(), &source_session_id);
        let err = adapter
            .resolve_runtime_completion_cleanup(
                &target_session_id,
                observation,
                false,
                crate::meerkat_machine::dsl::RuntimeCompletionLiveSessionObservation::Absent,
            )
            .await
            .expect_err("cleanup must reject an observation minted for another session");
        assert!(
            matches!(err, crate::RuntimeDriverError::ValidationFailed { .. }),
            "expected generated cleanup validation failure, got {err:?}"
        );
    }

    #[tokio::test]
    async fn cleanup_rejects_stale_same_session_observation_after_rebinding() {
        let adapter = crate::meerkat_machine::MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        adapter
            .prepare_bindings(session_id.clone())
            .await
            .expect("session should prepare initial runtime bindings");

        let input = crate::Input::Prompt(crate::PromptInput::new(
            "same session pending completion",
            None,
        ));
        let (_outcome, handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("input should be accepted");
        let handle = handle.expect("input should have a completion waiter");
        adapter
            .stop_runtime_executor(&session_id, "first runtime stopped")
            .await
            .expect("stop should resolve waiter");
        let (_outcome, stale_observation) = handle
            .try_wait_with_cleanup_observation()
            .await
            .expect("waiter should resolve with generated cleanup observation");

        adapter
            .unregister_session(&session_id)
            .await
            .expect("initial runtime binding should unregister cleanly");
        adapter
            .prepare_bindings(session_id.clone())
            .await
            .expect("session should prepare replacement runtime bindings");

        let err = adapter
            .resolve_runtime_completion_cleanup(
                &session_id,
                stale_observation,
                false,
                crate::meerkat_machine::dsl::RuntimeCompletionLiveSessionObservation::Absent,
            )
            .await
            .expect_err("cleanup must reject an observation minted for a prior runtime binding");
        assert!(
            matches!(err, crate::RuntimeDriverError::ValidationFailed { .. }),
            "expected generated cleanup validation failure, got {err:?}"
        );
    }

    #[tokio::test]
    async fn wait_failure_authority_releases_pre_admission_and_classifies_public_reason() {
        let adapter = crate::meerkat_machine::MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        adapter
            .prepare_bindings(session_id.clone())
            .await
            .expect("session should prepare runtime bindings");

        let authority = adapter
            .resolve_runtime_completion_wait_failure(
                &session_id,
                &CompletionWaitError::AuthorityUnavailable("missing generated result".into()),
            )
            .await
            .expect("wait-failure authority should resolve");

        assert!(authority.releases_pre_admission());
        assert_eq!(
            authority.public_error_class,
            crate::meerkat_machine::dsl::RuntimeCompletionWaitFailurePublicErrorClass::InternalError
        );
        assert_eq!(
            authority.public_reason,
            crate::meerkat_machine::dsl::RuntimeCompletionWaitFailurePublicReason::CompletionAuthorityUnavailable
        );
        assert!(!authority.resumable);
    }

    #[tokio::test]
    async fn wait_failure_authority_rejects_missing_session_authority() {
        let adapter = crate::meerkat_machine::MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        let err = adapter
            .resolve_runtime_completion_wait_failure(
                &session_id,
                &CompletionWaitError::ChannelClosed,
            )
            .await
            .expect_err("wait-failure authority must fail closed without a session authority");

        assert!(
            matches!(err, crate::RuntimeDriverError::ValidationFailed { .. }),
            "expected generated wait-failure validation failure, got {err:?}"
        );
    }

    #[tokio::test]
    async fn resolve_nonexistent_is_a_noop() {
        let mut registry = CompletionRegistry::new();
        registry.resolve_completed_authorized(
            &InputId::new(),
            make_run_result(),
            authority(
                RuntimeCompletionResultClass::Completed,
                RuntimeCompletionObservedOutcome::Completed,
            ),
        );
        registry.fail_inputs(
            [InputId::new()],
            CompletionWaitError::AuthorityUnavailable("gone".into()),
        );
        assert!(!registry.debug_has_waiters());
    }

    #[tokio::test]
    async fn dropped_sender_gives_wait_error() {
        let mut registry = CompletionRegistry::new();
        let input_id = InputId::new();
        let handle = registry.register(input_id);

        // Drop the registry (and thus the sender)
        drop(registry);

        assert!(matches!(
            handle.try_wait().await,
            Err(CompletionWaitError::ChannelClosed)
        ));
    }

    #[tokio::test]
    async fn multi_waiter_all_receive_result() {
        let mut registry = CompletionRegistry::new();
        let input_id = InputId::new();

        let h1 = registry.register(input_id.clone());
        let h2 = registry.register(input_id.clone());
        let h3 = registry.register(input_id.clone());

        assert_eq!(registry.debug_waiter_count(), 3);

        let result = make_run_result();
        registry.resolve_completed_authorized(
            &input_id,
            result,
            authority(
                RuntimeCompletionResultClass::Completed,
                RuntimeCompletionObservedOutcome::Completed,
            ),
        );

        assert!(!registry.debug_has_waiters());

        for handle in [h1, h2, h3] {
            match handle.wait_authorized().await {
                CompletionOutcome::Completed(r) => assert_eq!(r.text, "hello"),
                other => panic!("Expected Completed, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn resolve_without_result_sends_variant() {
        let mut registry = CompletionRegistry::new();
        let input_id = InputId::new();
        let handle = registry.register(input_id.clone());

        registry.resolve_without_result_authorized(
            &input_id,
            authority(
                RuntimeCompletionResultClass::CompletedWithoutResult,
                RuntimeCompletionObservedOutcome::CompletedWithoutResult,
            ),
        );

        match handle.wait_authorized().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => panic!("Expected CompletedWithoutResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_without_result_multi_waiter() {
        let mut registry = CompletionRegistry::new();
        let input_id = InputId::new();
        let h1 = registry.register(input_id.clone());
        let h2 = registry.register(input_id.clone());

        registry.resolve_without_result_authorized(
            &input_id,
            authority(
                RuntimeCompletionResultClass::CompletedWithoutResult,
                RuntimeCompletionObservedOutcome::CompletedWithoutResult,
            ),
        );

        for handle in [h1, h2] {
            match handle.wait_authorized().await {
                CompletionOutcome::CompletedWithoutResult => {}
                other => panic!("Expected CompletedWithoutResult, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn resolve_callback_pending_sends_variant() {
        let mut registry = CompletionRegistry::new();
        let input_id = InputId::new();
        let handle = registry.register(input_id.clone());

        registry.resolve_callback_pending_authorized(
            &input_id,
            "call-1".to_string(),
            "browser".to_string(),
            serde_json::json!({ "url": "https://example.com" }),
            authority(
                RuntimeCompletionResultClass::CallbackPending,
                RuntimeCompletionObservedOutcome::CallbackPending,
            ),
        );

        match handle.wait_authorized().await {
            CompletionOutcome::CallbackPending {
                tool_use_id,
                tool_name,
                args,
            } => {
                assert_eq!(tool_use_id, "call-1");
                assert_eq!(tool_name, "browser");
                assert_eq!(args, serde_json::json!({ "url": "https://example.com" }));
            }
            other => panic!("Expected CallbackPending, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_cancelled_sends_variant() {
        let mut registry = CompletionRegistry::new();
        let input_id = InputId::new();
        let handle = registry.register(input_id.clone());

        registry.resolve_cancelled_authorized(
            &input_id,
            authority(
                RuntimeCompletionResultClass::Cancelled,
                RuntimeCompletionObservedOutcome::Cancelled,
            ),
        );

        match handle.wait_authorized().await {
            CompletionOutcome::Cancelled => {}
            other => panic!("Expected Cancelled, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn already_resolved_handle() {
        let handle = CompletionHandle::already_completed_without_result()
            .expect("generated completion authority should classify no-result completion");
        match handle.wait_authorized().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => panic!("Expected CompletedWithoutResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn outcome_cleanup_observes_and_relays_result() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let mut registry = CompletionRegistry::new();
        let input_id = InputId::new();
        let handle = registry.register(input_id.clone());
        let observed = Arc::new(AtomicBool::new(false));
        let cleanup_observed = Arc::clone(&observed);
        let handle = handle.with_outcome_cleanup(move |observation| async move {
            if observation.observed_outcome()
                == crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome::CompletedWithoutResult
            {
                cleanup_observed.store(true, Ordering::Release);
            }
        });

        registry.resolve_without_result_authorized(
            &input_id,
            authority(
                RuntimeCompletionResultClass::CompletedWithoutResult,
                RuntimeCompletionObservedOutcome::CompletedWithoutResult,
            ),
        );
        match handle.wait_authorized().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => panic!("Expected CompletedWithoutResult, got {other:?}"),
        }
        assert!(observed.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn resultful_completion_cleanup_withholds_success_and_combines_failures() {
        let handle = CompletionHandle::already_completed_without_result()
            .expect("generated completion authority should classify no-result completion")
            .with_resultful_completion_cleanup(|_| async {
                Err(CompletionWaitError::AuthorityUnavailable(
                    "required cleanup failed".to_string(),
                ))
            });
        let error = handle
            .wait()
            .await
            .expect_err("required cleanup failure must withhold success");
        assert!(error.to_string().contains("required cleanup failed"));

        let (tx, rx) = oneshot::channel();
        drop(tx);
        let handle =
            CompletionHandle { rx }.with_resultful_completion_cleanup(|completion| async move {
                assert!(matches!(
                    completion,
                    Err(CompletionWaitError::ChannelClosed)
                ));
                Err(CompletionWaitError::AuthorityUnavailable(
                    "cleanup authority failed".to_string(),
                ))
            });
        let error = handle
            .wait()
            .await
            .expect_err("two failures must remain a failure");
        let rendered = error.to_string();
        assert!(rendered.contains("completion channel closed"));
        assert!(rendered.contains("cleanup authority failed"));
    }

    #[tokio::test]
    async fn multi_waiter_terminated_on_reset() {
        let mut registry = CompletionRegistry::new();
        let input_id = InputId::new();
        let h1 = registry.register(input_id.clone());
        let h2 = registry.register(input_id);

        registry.resolve_all_runtime_terminated(
            "runtime reset",
            authority(
                RuntimeCompletionResultClass::RuntimeTerminated,
                RuntimeCompletionObservedOutcome::RuntimeTerminated,
            ),
        );

        for handle in [h1, h2] {
            match handle.wait_authorized().await {
                CompletionOutcome::RuntimeTerminated { reason, .. } => {
                    assert_eq!(reason, "runtime reset");
                }
                other => panic!("Expected RuntimeTerminated, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn fail_not_pending_keeps_pending_waiters_without_claiming_a_terminal() {
        let mut registry = CompletionRegistry::new();
        let keep_id = InputId::new();
        let drop_id = InputId::new();

        let keep_handle = registry.register(keep_id.clone());
        let drop_handle = registry.register(drop_id.clone());
        registry.fail_not_pending_waiters(
            |input_id| input_id == &keep_id,
            CompletionWaitError::AuthorityUnavailable(
                "recycled input no longer pending".to_string(),
            ),
        );
        assert_eq!(registry.debug_waiter_count(), 1);

        match drop_handle.try_wait().await {
            Err(CompletionWaitError::AuthorityUnavailable(reason)) => {
                assert_eq!(reason, "recycled input no longer pending");
            }
            other => panic!("Expected reconciliation plumbing failure, got {other:?}"),
        }

        registry.resolve_without_result_authorized(
            &keep_id,
            authority(
                RuntimeCompletionResultClass::CompletedWithoutResult,
                RuntimeCompletionObservedOutcome::CompletedWithoutResult,
            ),
        );
        match keep_handle.wait_authorized().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => panic!("Expected CompletedWithoutResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_without_result_nonexistent_is_a_noop() {
        let mut registry = CompletionRegistry::new();
        registry.resolve_without_result_authorized(
            &InputId::new(),
            authority(
                RuntimeCompletionResultClass::CompletedWithoutResult,
                RuntimeCompletionObservedOutcome::CompletedWithoutResult,
            ),
        );
        assert!(!registry.debug_has_waiters());
    }

    #[test]
    fn abandoned_carries_typed_error_metadata() {
        let error = TurnErrorMetadata::runtime_apply_failure("apply blew up");
        let outcome = CompletionOutcome::Abandoned {
            reason: "abandoned".into(),
            error: error.clone(),
        };
        assert_eq!(outcome.abandoned_reason(), Some("abandoned"));
        assert_eq!(outcome.error_metadata(), Some(&error));
    }

    #[test]
    fn runtime_terminated_carries_typed_error_metadata() {
        let outcome = CompletionOutcome::runtime_terminated("runtime stopped");
        match &outcome {
            CompletionOutcome::RuntimeTerminated { reason, .. } => {
                assert_eq!(reason, "runtime stopped");
            }
            other => panic!("Expected RuntimeTerminated, got {other:?}"),
        }
        let metadata = outcome
            .error_metadata()
            .expect("RuntimeTerminated must carry typed turn error metadata");
        assert_eq!(metadata.kind, TurnTerminalCauseKind::FatalFailure);
        assert_eq!(metadata.outcome, Some(TurnTerminalOutcome::Failed));
        assert!(metadata.terminal);
        assert_eq!(metadata.detail.as_deref(), Some("runtime stopped"));
    }
}
