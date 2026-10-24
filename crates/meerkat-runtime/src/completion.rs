//! Input completion waiters — allows callers to await the execution outcome of
//! an accepted input.
//!
//! When a surface accepts an input via the runtime, it can optionally receive a
//! `CompletionHandle`. It normally resolves when the input reaches a terminal
//! state (Consumed or Abandoned). If an execution attempt fails durably and the
//! machine requeues the input, the process-local handle resolves that generated
//! attempt failure without terminalizing the durable input. This bridges the
//! async accept/await pattern needed for surfaces that want synchronous-feeling
//! turn execution through the runtime.
//!
//! `CompletionRegistry` is waiter plumbing only. Production code must never
//! treat waiter presence, waiter counts, or sender membership as semantic
//! runtime truth.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex as StdMutex, Weak};

use meerkat_core::lifecycle::InputId;
use meerkat_core::lifecycle::RunId;
use meerkat_core::lifecycle::core_executor::CoreApplyTerminal;
use meerkat_core::types::{RunResult, SessionId};
use meerkat_core::{TurnErrorMetadata, TurnTerminalCauseKind, TurnTerminalOutcome};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg(test)]
use crate::meerkat_machine::driver::RuntimeCompletionResultAttempt;
use crate::meerkat_machine::driver::{
    RuntimeCompletionResultAuthority, RuntimeCompletionResultRealized,
};
use crate::meerkat_machine::dsl::RuntimeCompletionResultClass;
use crate::tokio::sync::oneshot;

/// Mechanical failure while waiting for completion plumbing.
///
/// This is intentionally separate from [`CompletionOutcome`]: a closed waiter
/// channel or missing generated completion authority is not a public runtime
/// result class.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
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
    /// The input completed and its result is preserved, but the result may
    /// not be disclosed to this reader: the scoped destination it was
    /// admitted under is not the session document's current binding. This
    /// is not a waiter failure: the input is not failed and its completion
    /// cleanup already ran.
    #[error("the input's {observed:?} result is withheld: its scoped destination is not current")]
    DisclosureRefused {
        /// The body-free generated completion observation: what the input
        /// reached, without its result or callback arguments.
        observed: crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome,
    },
    /// The run's boundary commit was rejected and rolled back, so none of the
    /// run landed. The session needs a reload, and the input stays durably
    /// admitted: it runs again after the reload. Do not resubmit it; read it
    /// after the reload. `detail` is the commit's rejection.
    #[error(
        "the boundary commit of run {run_id} rolled back ({detail}); the input runs again after its session reloads"
    )]
    BoundaryRolledBack { run_id: RunId, detail: String },
    /// The run's boundary commit or terminalization reported no durable
    /// outcome within its store's declared deadline, and it may still land.
    /// The session needs a reload, and the input stays durably admitted. Do
    /// not resubmit it; read its outcome after the reload.
    #[error(
        "the durable outcome of run {run_id} is unknown: its store did not answer within its commit deadline; read the input after its session reloads"
    )]
    DurableOutcomeUnknown { run_id: RunId },
    /// The input was still queued when its runtime stopped, and the stop's
    /// unregister kept it durably queued for the next start
    /// ([`UnregisterQueuedInputs::Requeue`](crate::UnregisterQueuedInputs::Requeue)).
    /// It is neither failed nor consumed: the next start runs it, or ends it
    /// as abandoned with reason `requeue_expired` when it is older than the
    /// stop's max age by then. Do not resubmit it; read it after the restart.
    #[error(
        "the input stays queued across the runtime stop at {stopped_at} and runs on the next start; do not resubmit it"
    )]
    RequeuedAcrossStop {
        /// When the stop kept the input queued.
        stopped_at: chrono::DateTime<chrono::Utc>,
    },
    /// The input joined run `run_id` at a model boundary, and a round commit
    /// of the run wrote its content into the run's provisional tail. The run
    /// was cancelled, and its tail was closed as an interrupted run with
    /// every committed row kept: the input was consumed with the run, its
    /// content is in the session's committed history, and it does not run
    /// again. Do not resubmit it.
    #[error(
        "the input joined run {run_id}, whose committed rounds took its content; the run was closed as interrupted, so it does not run again"
    )]
    ConsumedWithInterruptedRun { run_id: RunId },
}

impl CompletionWaitError {
    /// The generated wait-failure observation of a waiter failure; `None`
    /// for a disclosure refusal, which is not one.
    pub fn wait_failure_observation(
        &self,
    ) -> Option<crate::meerkat_machine::dsl::RuntimeCompletionWaitFailureObservation> {
        match self {
            Self::ChannelClosed => Some(
                crate::meerkat_machine::dsl::RuntimeCompletionWaitFailureObservation::ChannelClosed,
            ),
            Self::AttachmentReplaced
            | Self::AuthorityUnavailable(_)
            | Self::BoundaryRolledBack { .. }
            | Self::DurableOutcomeUnknown { .. }
            | Self::ConsumedWithInterruptedRun { .. } => Some(
                crate::meerkat_machine::dsl::RuntimeCompletionWaitFailureObservation::AuthorityUnavailable,
            ),
            Self::RequeuedAcrossStop { .. } => Some(
                crate::meerkat_machine::dsl::RuntimeCompletionWaitFailureObservation::RequeuedAcrossStop,
            ),
            Self::DisclosureRefused { .. } => None,
        }
    }

    /// Stable typed predicate for a refused result disclosure.
    #[must_use]
    pub const fn is_disclosure_refused(&self) -> bool {
        matches!(self, Self::DisclosureRefused { .. })
    }

    /// Stable typed predicate for retryable attachment replacement. Surfaces
    /// must not infer this class from an error string.
    #[must_use]
    pub const fn is_attachment_replaced(&self) -> bool {
        matches!(self, Self::AttachmentReplaced)
    }

    /// Stable typed predicate: the run's boundary rolled back, so it is known
    /// not to have landed. The input stays admitted.
    #[must_use]
    pub const fn is_boundary_rolled_back(&self) -> bool {
        matches!(self, Self::BoundaryRolledBack { .. })
    }

    /// Stable typed predicate: whether the run's boundary landed is unknown
    /// until its session reloads. The input stays admitted.
    #[must_use]
    pub const fn is_durable_outcome_unknown(&self) -> bool {
        matches!(self, Self::DurableOutcomeUnknown { .. })
    }

    /// Stable typed predicate: a stop kept the input durably queued for the
    /// next start. The input is not failed.
    #[must_use]
    pub const fn is_requeued_across_stop(&self) -> bool {
        matches!(self, Self::RequeuedAcrossStop { .. })
    }

    /// Stable typed predicate: the input was consumed with a cancelled run
    /// whose committed round holds its content, and the run was closed as
    /// interrupted. The input does not run again.
    #[must_use]
    pub const fn is_consumed_with_interrupted_run(&self) -> bool {
        matches!(self, Self::ConsumedWithInterruptedRun { .. })
    }

    /// When the stop kept the input queued, for a
    /// [`RequeuedAcrossStop`](Self::RequeuedAcrossStop) answer; `None` for
    /// every other failure.
    #[must_use]
    pub fn requeued_stopped_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        match self {
            Self::RequeuedAcrossStop { stopped_at } => Some(*stopped_at),
            _ => None,
        }
    }
}

/// Outcome delivered to a completion waiter.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "completion_type", rename_all = "snake_case")]
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
        /// The call's dedupe key, as on
        /// [`meerkat_core::error::PendingCallbackToolCall`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dedupe_key: Option<meerkat_core::ToolCallDedupeKey>,
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
    /// The accepted execution attempt ended with typed failure metadata.
    ///
    /// The durable input may be abandoned or machine-requeued; this public
    /// result describes the caller's exact attempt, not a second lifecycle
    /// authority.
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

/// Public completion plus the exact committed input terminal observed by the
/// completion witness.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct CompletionObservation {
    outcome: CompletionOutcome,
    input_terminal_outcome: Option<crate::input_state::InputTerminalOutcome>,
}

impl CompletionObservation {
    #[must_use]
    pub fn outcome(&self) -> &CompletionOutcome {
        &self.outcome
    }

    #[must_use]
    pub fn input_terminal_outcome(&self) -> Option<&crate::input_state::InputTerminalOutcome> {
        self.input_terminal_outcome.as_ref()
    }

    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        CompletionOutcome,
        Option<crate::input_state::InputTerminalOutcome>,
    ) {
        (self.outcome, self.input_terminal_outcome)
    }
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
            dedupe_key,
        } => AgentEvent::InteractionCallbackPending {
            interaction_id,
            pending_tool_calls: vec![meerkat_core::error::PendingCallbackToolCall {
                tool_use_id,
                tool_name: tool_name.clone(),
                args: args.clone(),
                settlement_failures: Vec::new(),
                dedupe_key,
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
                    settlement_failures: Vec::new(),
                    dedupe_key: None,
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
///
/// The private delivery always carries the known outcome and its body-free
/// cleanup observation through relays; the result payload is gated only
/// where it is released to a public reader.
#[derive(Debug)]
struct CompletionDelivery {
    outcome: CompletionOutcome,
    cleanup_observation: CompletionCleanupObservation,
    terminal_outcome: Option<crate::input_state::InputTerminalOutcome>,
    disclosure: Option<DeferredDisclosure>,
}

impl CompletionDelivery {
    fn into_release(self) -> CompletionRelease {
        CompletionRelease {
            observed: self.cleanup_observation.observed_outcome(),
            outcome: self.outcome,
            disclosure: self.disclosure,
        }
    }
}

type DisclosureDecision = std::pin::Pin<Box<dyn Future<Output = bool> + Send>>;

/// A disclosure decision captured when a completion is delivered and taken
/// when its result payload is released (after waits and cleanup relays),
/// against the actual current registration.
#[derive(Clone)]
pub(crate) struct DeferredDisclosure(Arc<dyn Fn() -> DisclosureDecision + Send + Sync>);

impl DeferredDisclosure {
    pub(crate) fn new<F, Fut>(permitted: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = bool> + Send + 'static,
    {
        Self(Arc::new(move || Box::pin(permitted())))
    }

    /// A decision that refuses: an input whose disclosure could not be
    /// decided is never disclosed.
    fn refused() -> Self {
        Self::new(|| async { false })
    }

    pub(crate) async fn permitted(&self) -> bool {
        (self.0)().await
    }
}

impl std::fmt::Debug for DeferredDisclosure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DeferredDisclosure")
    }
}

/// Captures, for one delivered input, how its result payload's disclosure is
/// decided at release.
/// `None` when the input's disclosure cannot be decided from what the
/// machine holds (the input is not admitted, or already archived).
pub(crate) type CompletionDisclosureGate =
    Arc<dyn Fn(&InputId) -> Option<DeferredDisclosure> + Send + Sync>;

/// A delivered completion outcome whose result payload has not been released
/// yet. [`Self::release`] releases it to the public reader, or refuses the
/// disclosure with [`CompletionWaitError::DisclosureRefused`] while the
/// input's completion and preserved result stand. Its `Debug` is opaque: it
/// shows only the body-free observation, never the unreleased outcome.
pub struct CompletionRelease {
    outcome: CompletionOutcome,
    observed: crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome,
    disclosure: Option<DeferredDisclosure>,
}

impl std::fmt::Debug for CompletionRelease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompletionRelease")
            .field("observed", &self.observed)
            .finish_non_exhaustive()
    }
}

impl CompletionRelease {
    /// The body-free generated completion observation of this outcome.
    #[must_use]
    pub fn observed(&self) -> crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome {
        self.observed
    }

    /// Release the outcome to this reader, deciding against the actual
    /// current registration now. Only a result payload (a run result or
    /// callback arguments) is ever withheld; lifecycle-only outcomes are
    /// always released.
    pub async fn release(self) -> Result<CompletionOutcome, CompletionWaitError> {
        if let Some(disclosure) = &self.disclosure
            && self.outcome.carries_result_payload()
            && !disclosure.permitted().await
        {
            return Err(CompletionWaitError::DisclosureRefused {
                observed: self.observed,
            });
        }
        Ok(self.outcome)
    }
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
    rx: Option<oneshot::Receiver<Result<CompletionDelivery, CompletionWaitError>>>,
    waiter_token: Option<CompletionWaiterToken>,
}

#[derive(Debug)]
struct CompletionWaiterToken {
    registry: Weak<StdMutex<CompletionWaiterMap>>,
    disclosures: Weak<StdMutex<CapturedDisclosureMap>>,
    input_id: InputId,
    waiter_id: uuid::Uuid,
}

impl CompletionHandle {
    #[cfg(test)]
    pub(crate) fn pending_for_test() -> PendingCompletionForTest {
        let (tx, rx) = oneshot::channel();
        PendingCompletionForTest {
            handle: Some(Self {
                rx: Some(rx),
                waiter_token: None,
            }),
            tx,
        }
    }

    async fn try_wait_delivery(mut self) -> Result<CompletionDelivery, CompletionWaitError> {
        let Some(rx) = self.rx.take() else {
            return Err(CompletionWaitError::ChannelClosed);
        };
        rx.await.unwrap_or(Err(CompletionWaitError::ChannelClosed))
    }

    /// Wait for the generated execution result or report mechanical waiter
    /// failure. A result this reader may not receive is refused with
    /// [`CompletionWaitError::DisclosureRefused`].
    pub async fn try_wait(self) -> Result<CompletionOutcome, CompletionWaitError> {
        self.try_wait_delivery()
            .await?
            .into_release()
            .release()
            .await
    }

    /// Wait for completion and return the generated cleanup observation carried
    /// with the authorized public outcome. The cleanup observation is always
    /// returned; the outcome is released (or its disclosure refused) by the
    /// caller after its cleanup, with [`CompletionRelease::release`].
    pub async fn try_wait_with_cleanup_observation(
        self,
    ) -> Result<(CompletionRelease, CompletionCleanupObservation), CompletionWaitError> {
        let delivery = self.try_wait_delivery().await?;
        let cleanup_observation = delivery.cleanup_observation.clone();
        Ok((delivery.into_release(), cleanup_observation))
    }

    /// Wait for completion together with the exact committed input terminal
    /// observed by the terminal-completion witness. The result payload is
    /// released as in [`Self::try_wait`].
    pub async fn try_wait_with_terminal_outcome(
        self,
    ) -> Result<CompletionObservation, CompletionWaitError> {
        let delivery = self.try_wait_delivery().await?;
        let input_terminal_outcome = delivery.terminal_outcome.clone();
        Ok(CompletionObservation {
            outcome: delivery.into_release().release().await?,
            input_terminal_outcome,
        })
    }

    /// Wait for the input's terminal as a wake only, without releasing its
    /// result payload.
    pub(crate) async fn wait_terminal_wake(self) {
        let _wake = self.try_wait_delivery().await;
    }

    /// Wait for the generated execution result or report mechanical waiter failure.
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
        Self {
            rx: Some(rx),
            waiter_token: None,
        }
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
        Self {
            rx: Some(rx),
            waiter_token: None,
        }
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
        Self {
            rx: Some(rx),
            waiter_token: None,
        }
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
        Self {
            rx: Some(rx),
            waiter_token: None,
        }
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
            terminal_outcome: None,
            disclosure: None,
        }));
        Self {
            rx: Some(rx),
            waiter_token: None,
        }
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
                dedupe_key: None,
            },
            crate::meerkat_machine::dsl::RuntimeCompletionResultClass::CallbackPending,
            crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::CallbackPending,
            crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
        )
    }
}

impl Drop for CompletionHandle {
    fn drop(&mut self) {
        if let Some(token) = self.waiter_token.take()
            && let Some(registry) = token.registry.upgrade()
        {
            let mut waiters = registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(input_waiters) = waiters.get_mut(&token.input_id) {
                input_waiters.remove(&token.waiter_id);
                if input_waiters.is_empty() {
                    waiters.remove(&token.input_id);
                    if let Some(disclosures) = token.disclosures.upgrade() {
                        disclosures
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .remove(&token.input_id);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) struct PendingCompletionForTest {
    handle: Option<CompletionHandle>,
    tx: oneshot::Sender<Result<CompletionDelivery, CompletionWaitError>>,
}

#[cfg(test)]
impl PendingCompletionForTest {
    pub(crate) fn take_handle(&mut self) -> CompletionHandle {
        self.handle
            .take()
            .expect("test pending completion handle is taken only once")
    }

    pub(crate) fn sender_is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

impl CompletionOutcome {
    /// Whether this outcome carries the run's result payload (its result or
    /// the model-produced callback arguments), as opposed to a lifecycle-only
    /// report. Only a result payload is subject to scoped disclosure.
    #[must_use]
    pub fn carries_result_payload(&self) -> bool {
        matches!(
            self,
            Self::Completed(_) | Self::CallbackPending { .. } | Self::CallbackBatchPending { .. }
        )
    }

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
                dedupe_key,
            }) => CompletionOutcome::CallbackPending {
                tool_use_id: tool_use_id.clone(),
                tool_name: tool_name.clone(),
                args: args.clone(),
                dedupe_key: dedupe_key.clone(),
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
    finalization: crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation,
    cleanup_observation: CompletionCleanupObservation,
    interaction_events: Vec<meerkat_core::event::AgentEvent>,
    terminal_completion_witness:
        crate::meerkat_machine::driver::InputTerminalCompletionAuthorizationWitness,
    receipt_commit_input: Option<Box<crate::meerkat_machine::dsl::MeerkatMachineInput>>,
}

/// Non-constructible carrier for the exact public result selected by generated
/// completion authority. Persistence accepts this token rather than a raw
/// `CompletionOutcome`, so handwritten callers cannot mint durable success.
#[must_use = "authorized terminal completion must be durably finalized"]
pub(crate) struct AuthorizedInputTerminalCompletion {
    outcome: CompletionOutcome,
    finalization: crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation,
    witness: crate::meerkat_machine::driver::InputTerminalCompletionAuthorizationWitness,
    receipt_commit_input: Option<Box<crate::meerkat_machine::dsl::MeerkatMachineInput>>,
}

impl AuthorizedInputTerminalCompletion {
    pub(crate) fn receipt_commit_input(
        &self,
    ) -> Option<&crate::meerkat_machine::dsl::MeerkatMachineInput> {
        self.receipt_commit_input.as_deref()
    }

    pub(crate) fn outcome(&self) -> &CompletionOutcome {
        &self.outcome
    }

    pub(crate) fn finalization(
        &self,
    ) -> crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation {
        self.finalization
    }

    pub(crate) fn witness(
        &self,
    ) -> &crate::meerkat_machine::driver::InputTerminalCompletionAuthorizationWitness {
        &self.witness
    }
}

impl AuthorizedRuntimeTerminalBundle {
    pub(crate) fn interaction_events(&self) -> &[meerkat_core::event::AgentEvent] {
        &self.interaction_events
    }

    pub(crate) fn terminal_completion(&self) -> AuthorizedInputTerminalCompletion {
        AuthorizedInputTerminalCompletion {
            outcome: self.outcome.clone(),
            finalization: self.finalization,
            witness: self.terminal_completion_witness.clone(),
            receipt_commit_input: self.receipt_commit_input.clone(),
        }
    }
}

/// Consume generated completion authority exactly once and create the shared
/// event/waiter terminal bundle for the whole runtime batch.
pub(crate) fn authorize_runtime_terminal_bundle(
    interaction_ids: &[meerkat_core::interaction::InteractionId],
    terminal: Option<&CoreApplyTerminal>,
    authority: RuntimeCompletionResultAuthority,
    terminal_completion_witness:
        crate::meerkat_machine::driver::InputTerminalCompletionAuthorizationWitness,
    finalization_error: Option<TurnErrorMetadata>,
    runtime_termination_reason: Option<&str>,
) -> Result<AuthorizedRuntimeTerminalBundle, CompletionWaitError> {
    if !authority.completion_batch_matches(&terminal_completion_witness) {
        return Err(CompletionWaitError::AuthorityUnavailable(
            "generated runtime completion authority did not match the exact pending batch owner/run binding"
                .to_string(),
        ));
    }
    let receipt_commit_input = authority
        .receipt_commit_input()
        .map(|input| Box::new(input.clone()));
    let result_class = authority.result_class();
    let finalization = authority.finalization();
    if finalization == crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Failed
        && !matches!(
            result_class,
            RuntimeCompletionResultClass::CompletedWithFinalizationFailure
                | RuntimeCompletionResultClass::AbandonedWithError
        )
    {
        return Err(CompletionWaitError::AuthorityUnavailable(format!(
            "generated runtime completion authority resolved {result_class:?} from a failed finalization"
        )));
    }
    if finalization
        == crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded
        && matches!(
            result_class,
            RuntimeCompletionResultClass::CompletedWithFinalizationFailure
        )
    {
        return Err(CompletionWaitError::AuthorityUnavailable(
            "generated runtime completion authority resolved a finalization-failure result from successful finalization"
                .to_string(),
        ));
    }
    let candidate = match result_class {
        RuntimeCompletionResultClass::RuntimeTerminated => {
            let Some(reason) = runtime_termination_reason.filter(|reason| !reason.is_empty())
            else {
                return Err(CompletionWaitError::AuthorityUnavailable(
                    "runtime completion authority resolved RuntimeTerminated without its exact reason"
                        .to_string(),
                ));
            };
            crate::input_state::InteractionTerminalCandidate::RuntimeTerminated {
                reason: reason.to_string(),
            }
        }
        RuntimeCompletionResultClass::Cancelled => {
            crate::input_state::InteractionTerminalCandidate::Cancelled
        }
        _ => crate::input_state::InteractionTerminalCandidate::from_core_apply_terminal(terminal),
    };
    let candidate_digest = crate::input_state::interaction_terminal_payload_digest(&candidate)
        .map_err(CompletionWaitError::AuthorityUnavailable)?;
    if candidate_digest != terminal_completion_witness.candidate_digest() {
        return Err(CompletionWaitError::AuthorityUnavailable(
            "generated runtime completion payload did not match the exact pending batch candidate"
                .to_string(),
        ));
    }
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
        finalization,
        cleanup_observation,
        interaction_events,
        terminal_completion_witness,
        receipt_commit_input,
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
    let (outcome, _) =
        authorized_completion_outcome(terminal, authority, finalization_error, None)?;
    Ok(interaction_ids
        .iter()
        .map(|interaction_id| interaction_terminal_event(*interaction_id, outcome.clone()))
        .collect())
}

/// Registry of pending completion waiters, keyed by InputId.
///
/// Uses `Vec<Sender>` per InputId to support multiple waiters for the same input
/// (e.g. dedup of in-flight input registers a second waiter for the same InputId).
#[derive(Default)]
pub(crate) struct CompletionRegistry {
    waiters: Arc<StdMutex<CompletionWaiterMap>>,
    receipt_less_observers: Arc<StdMutex<ReceiptLessTerminalObserverMap>>,
    /// Decides at release whether a delivered result payload may reach its
    /// public reader (the session document's scoped destination rule).
    /// Without a gate every result is released.
    disclosure_gate: Option<CompletionDisclosureGate>,
    /// The disclosure decision of each waited input, captured when its first
    /// waiter registers: the input is admitted and its destination recorded
    /// then, while by delivery its terminal may already have archived that
    /// record.
    disclosures: Arc<StdMutex<CapturedDisclosureMap>>,
}

type CapturedDisclosureMap = HashMap<InputId, DeferredDisclosure>;

type CompletionWaiterMap = HashMap<
    InputId,
    HashMap<uuid::Uuid, oneshot::Sender<Result<CompletionDelivery, CompletionWaitError>>>,
>;

/// Wake-only observers of an input terminal that resolves no completion
/// waiter.
///
/// Supersession and coalescing terminalize a queued input inside a later
/// admission: the machine stages no terminal-completion receipt for them and
/// no completion is delivered, so a completion waiter on that input is never
/// resolved. The terminal-receipt wait parks on one of these next to its
/// completion waiter, and the admission that committed the transition wakes
/// it. A wake carries nothing; the woken wait re-reads machine-owned state.
/// Like the waiter map, this is plumbing only and never runtime truth.
type ReceiptLessTerminalObserverMap = HashMap<InputId, HashMap<uuid::Uuid, oneshot::Sender<()>>>;

/// Wake-only registration returned by
/// [`CompletionRegistry::observe_receipt_less_terminal`]. Dropping it
/// unregisters it.
#[derive(Debug)]
pub(crate) struct ReceiptLessTerminalObserver {
    rx: Option<oneshot::Receiver<()>>,
    registry: Weak<StdMutex<ReceiptLessTerminalObserverMap>>,
    input_id: InputId,
    observer_id: uuid::Uuid,
}

impl ReceiptLessTerminalObserver {
    /// Resolve on the next wake for the input, or once the registry is
    /// dropped (session teardown). Either way the caller re-reads.
    pub(crate) async fn woken(mut self) {
        if let Some(rx) = self.rx.take() {
            let _closed_or_woken = rx.await;
        }
    }
}

impl Drop for ReceiptLessTerminalObserver {
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            let mut observers = registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(input_observers) = observers.get_mut(&self.input_id) {
                input_observers.remove(&self.observer_id);
                if input_observers.is_empty() {
                    observers.remove(&self.input_id);
                }
            }
        }
    }
}

impl CompletionRegistry {
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// A registry whose result payloads are released through `gate`.
    pub(crate) fn with_disclosure_gate(gate: CompletionDisclosureGate) -> Self {
        Self {
            disclosure_gate: Some(gate),
            ..Self::default()
        }
    }

    /// The disclosure of a delivered outcome: the decision captured when its
    /// waiters registered. There is no late lookup: by delivery the input's
    /// terminal may have archived what the machine recorded for it, so an
    /// input with no captured decision is refused, never decided as
    /// unscoped. (Every production registration follows the input's
    /// admission, so this refusal is unreachable for them; it also holds on a
    /// session without a governed host, where only a pre-admission
    /// registration followed by an archive before delivery could reach it.)
    fn disclosure_for(
        &self,
        captured: Option<DeferredDisclosure>,
        outcome: &CompletionOutcome,
    ) -> Option<DeferredDisclosure> {
        if !outcome.carries_result_payload() {
            return None;
        }
        self.disclosure_gate.as_ref()?;
        Some(captured.unwrap_or_else(DeferredDisclosure::refused))
    }

    /// Take an input's waiters together with its captured disclosure, under
    /// the waiter lock (lock order: waiters, then disclosures, as in
    /// registration and handle drop).
    fn take_input_waiters(
        &self,
        input_id: &InputId,
    ) -> Option<(
        Vec<oneshot::Sender<Result<CompletionDelivery, CompletionWaitError>>>,
        Option<DeferredDisclosure>,
    )> {
        let mut waiters = self
            .waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let input_waiters = waiters.remove(input_id)?;
        let captured = self
            .disclosures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(input_id);
        Some((input_waiters.into_values().collect(), captured))
    }

    fn take_waiters(
        &self,
        input_id: &InputId,
    ) -> Option<Vec<oneshot::Sender<Result<CompletionDelivery, CompletionWaitError>>>> {
        self.take_input_waiters(input_id)
            .map(|(senders, _captured)| senders)
    }

    #[cfg(test)]
    fn send_outcome(
        senders: Vec<oneshot::Sender<Result<CompletionDelivery, CompletionWaitError>>>,
        outcome: CompletionOutcome,
        cleanup_observation: CompletionCleanupObservation,
    ) {
        Self::send_delivery(senders, outcome, cleanup_observation, None, None);
    }

    /// Deliver a possibly result-bearing outcome with the disclosure
    /// decision its waiters captured.
    fn send_input_outcome(
        &self,
        captured: Option<DeferredDisclosure>,
        senders: Vec<oneshot::Sender<Result<CompletionDelivery, CompletionWaitError>>>,
        outcome: CompletionOutcome,
        cleanup_observation: CompletionCleanupObservation,
        terminal_outcome: Option<crate::input_state::InputTerminalOutcome>,
    ) {
        let disclosure = self.disclosure_for(captured, &outcome);
        Self::send_delivery(
            senders,
            outcome,
            cleanup_observation,
            terminal_outcome,
            disclosure,
        );
    }

    fn send_delivery(
        senders: Vec<oneshot::Sender<Result<CompletionDelivery, CompletionWaitError>>>,
        outcome: CompletionOutcome,
        cleanup_observation: CompletionCleanupObservation,
        terminal_outcome: Option<crate::input_state::InputTerminalOutcome>,
        disclosure: Option<DeferredDisclosure>,
    ) {
        for tx in senders {
            let outcome = outcome.clone();
            let _ = tx.send(Ok(CompletionDelivery {
                outcome,
                cleanup_observation: cleanup_observation.clone(),
                terminal_outcome: terminal_outcome.clone(),
                disclosure: disclosure.clone(),
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

    #[cfg(test)]
    fn authority_mismatch_error(
        authority: &RuntimeCompletionResultAttempt,
        expected: RuntimeCompletionResultClass,
    ) -> CompletionWaitError {
        CompletionWaitError::AuthorityUnavailable(format!(
            "generated runtime completion authority returned {:?}, expected {expected:?}",
            authority.class()
        ))
    }

    #[cfg(test)]
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

    #[cfg(test)]
    fn cleanup_from_realized_attempt(
        authority: RuntimeCompletionResultAttempt,
    ) -> CompletionCleanupObservation {
        CompletionCleanupObservation::from_realized_result(authority.realize())
    }

    /// Register a waiter for an input. Returns the handle the caller will await.
    ///
    /// Multiple waiters can be registered for the same InputId — all observe
    /// the same generated execution result.
    pub(crate) fn register(&mut self, input_id: InputId) -> CompletionHandle {
        let (tx, rx) = oneshot::channel();
        let waiter_id = meerkat_core::time_compat::new_uuid_v7();
        // One critical section under the waiter lock (then disclosures, as in
        // handle drop): a concurrent drop of the input's last old handle
        // cannot clear the capture between the check and the waiter insert.
        let mut waiters = self
            .waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(gate) = &self.disclosure_gate {
            let mut disclosures = self
                .disclosures
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !disclosures.contains_key(&input_id)
                && let Some(decision) = gate(&input_id)
            {
                disclosures.insert(input_id.clone(), decision);
            }
        }
        waiters
            .entry(input_id.clone())
            .or_default()
            .insert(waiter_id, tx);
        drop(waiters);
        CompletionHandle {
            rx: Some(rx),
            waiter_token: Some(CompletionWaiterToken {
                registry: Arc::downgrade(&self.waiters),
                disclosures: Arc::downgrade(&self.disclosures),
                input_id,
                waiter_id,
            }),
        }
    }

    /// Register a wake-only observer for a terminal of `input_id` that
    /// delivers no completion (supersession or coalescing).
    pub(crate) fn observe_receipt_less_terminal(
        &mut self,
        input_id: InputId,
    ) -> ReceiptLessTerminalObserver {
        let (tx, rx) = oneshot::channel();
        let observer_id = meerkat_core::time_compat::new_uuid_v7();
        self.receipt_less_observers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(input_id.clone())
            .or_default()
            .insert(observer_id, tx);
        ReceiptLessTerminalObserver {
            rx: Some(rx),
            registry: Arc::downgrade(&self.receipt_less_observers),
            input_id,
            observer_id,
        }
    }

    /// Wake every observer of these inputs. Called only after the machine
    /// committed the inputs' terminal fact: a receipt-less terminal (under the
    /// driver-then-registry lock order every resolver uses), or a directed
    /// batch's finalized receipt (after its persist released the driver). An
    /// observer registers under the driver lock while the input still reads
    /// pending, so every observer of a committed terminal is woken.
    pub(crate) fn wake_receipt_less_terminal_observers<I>(&mut self, input_ids: I)
    where
        I: IntoIterator<Item = InputId>,
    {
        let mut observers = self
            .receipt_less_observers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for input_id in input_ids {
            if let Some(input_observers) = observers.remove(&input_id) {
                for tx in input_observers.into_values() {
                    let _ = tx.send(());
                }
            }
        }
    }

    /// Resolve all waiters for a completed input.
    #[cfg(test)]
    fn resolve_completed(
        &mut self,
        input_id: &InputId,
        result: RunResult,
        cleanup_observation: CompletionCleanupObservation,
    ) {
        if let Some((senders, captured)) = self.take_input_waiters(input_id) {
            self.send_input_outcome(
                captured,
                senders,
                CompletionOutcome::Completed(Box::new(result)),
                cleanup_observation,
                None,
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

    /// Resolve process-local waiters from generated runtime-result authority
    /// without minting or persisting an input-terminal receipt.
    ///
    /// This is the exact seam for a durably recorded failed execution attempt
    /// whose contributing inputs remain machine-owned and requeued. Waiter
    /// delivery is mechanical observer plumbing; it must not mutate or
    /// terminalize those input rows.
    pub(crate) fn resolve_process_local_runtime_completion_authorized<I>(
        &mut self,
        input_ids: I,
        terminal: Option<&CoreApplyTerminal>,
        authority: RuntimeCompletionResultAuthority,
        finalization_error: Option<TurnErrorMetadata>,
    ) where
        I: IntoIterator<Item = InputId>,
    {
        let input_ids: Vec<InputId> = input_ids.into_iter().collect();
        if !input_ids.iter().any(|input_id| {
            self.waiters
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key(input_id)
        }) {
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
            if let Some((senders, captured)) = self.take_input_waiters(&input_id) {
                self.send_input_outcome(
                    captured,
                    senders,
                    outcome.clone(),
                    cleanup_observation.clone(),
                    None,
                );
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
            if let Some((senders, captured)) = self.take_input_waiters(&input_id) {
                let terminal_outcome = bundle
                    .terminal_completion_witness
                    .terminal_outcome(&input_id)
                    .cloned();
                self.send_input_outcome(
                    captured,
                    senders,
                    bundle.outcome.clone(),
                    bundle.cleanup_observation.clone(),
                    terminal_outcome,
                );
            }
        }
    }

    /// Resolve all waiters for an input that completed without producing a RunResult.
    #[cfg(test)]
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

    #[cfg(test)]
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
        if let Some((senders, captured)) = self.take_input_waiters(input_id) {
            self.send_input_outcome(
                captured,
                senders,
                CompletionOutcome::CallbackPending {
                    tool_use_id,
                    tool_name,
                    args,
                    dedupe_key: None,
                },
                cleanup_observation,
                None,
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
        let all_waiters = {
            let mut waiters = self
                .waiters
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.disclosures
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
            std::mem::take(&mut *waiters)
        };
        for (_, senders) in all_waiters {
            Self::send_outcome(
                senders.into_values().collect(),
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
        let all_waiters = {
            let mut waiters = self
                .waiters
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.disclosures
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
            std::mem::take(&mut *waiters)
        };
        for (_, senders) in all_waiters {
            Self::send_error(senders.into_values().collect(), error.clone());
        }
    }

    /// Fail the waiters of `input_ids` with `error`. Returns how many of the
    /// inputs had waiters.
    pub(crate) fn fail_inputs<I>(&mut self, input_ids: I, error: CompletionWaitError) -> usize
    where
        I: IntoIterator<Item = InputId>,
    {
        let mut failed = 0;
        for input_id in input_ids {
            if let Some(senders) = self.take_waiters(&input_id) {
                Self::send_error(senders, error.clone());
                failed += 1;
            }
        }
        failed
    }

    /// Answer every waiter still registered when an unregister's drain
    /// resumes after the stop terminalization. A waiter of an input the stop
    /// kept queued for the next start gets
    /// [`CompletionWaitError::RequeuedAcrossStop`] with the stop's time; every
    /// other one gets `AuthorityUnavailable`, since the terminalization has
    /// already published every terminal recipient. Returns how many kept
    /// inputs still had waiters here.
    pub(crate) fn drain_unregistered_waiters<I>(&mut self, kept: I) -> usize
    where
        I: IntoIterator<Item = (InputId, chrono::DateTime<chrono::Utc>)>,
    {
        let mut late = 0;
        for (input_id, stopped_at) in kept {
            late += self.fail_inputs(
                [input_id],
                CompletionWaitError::RequeuedAcrossStop { stopped_at },
            );
        }
        self.fail_not_pending_waiters(
            |_| false,
            CompletionWaitError::AuthorityUnavailable(
                "runtime session unregistered after canonical terminal recipients resolved"
                    .to_string(),
            ),
        );
        late
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
        self.waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|input_id, senders| {
                if is_still_pending(input_id) {
                    return true;
                }
                self.disclosures
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(input_id);
                Self::send_error(
                    std::mem::take(senders).into_values().collect(),
                    error.clone(),
                );
                false
            });
    }

    /// Snapshot the current waiter carrier without mutating it.
    pub(crate) fn diagnostic_snapshot(&self) -> CompletionRegistrySnapshot {
        let waiters = self
            .waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut waiting_inputs: Vec<_> = waiters
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
        !self
            .waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    /// Number of pending waiters (total across all InputIds).
    ///
    /// Test-only introspection. Production code must treat the registry as
    /// waiter plumbing rather than semantic runtime truth.
    #[cfg(test)]
    pub fn debug_waiter_count(&self) -> usize {
        self.waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .map(HashMap::len)
            .sum()
    }

    /// Number of registered receipt-less terminal observers. Test-only.
    #[cfg(test)]
    pub(crate) fn debug_receipt_less_observer_count(&self) -> usize {
        self.receipt_less_observers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .map(HashMap::len)
            .sum()
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
            run_usage: None,
            request_usage: Vec::new(),
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
                settlement_failures: Vec::new(),
                dedupe_key: None,
            },
            meerkat_core::error::PendingCallbackToolCall {
                tool_use_id: "call-b".to_string(),
                tool_name: "ask_b".to_string(),
                args: serde_json::json!({"tool_use_id": "call-b", "question": "b"}),
                settlement_failures: Vec::new(),
                dedupe_key: None,
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

    /// A single pending callback call's dedupe key reaches the
    /// `interaction_callback_pending` event's pending call unchanged, and a
    /// call without one projects none. (Unrun until the candidate's
    /// validation.)
    #[test]
    fn callback_terminal_event_carries_the_pending_call_dedupe_key() {
        let interaction_id = meerkat_core::interaction::InteractionId(uuid::Uuid::new_v4());
        let arguments =
            serde_json::value::RawValue::from_string(r#"{"question":"a"}"#.to_string()).unwrap();
        let key = meerkat_core::ToolCallDedupeKey::new(
            meerkat_core::RunId::new(),
            5,
            "call-a",
            "ask_a",
            &arguments,
        )
        .unwrap();
        for dedupe_key in [Some(key), None] {
            let event = interaction_terminal_event(
                interaction_id,
                CompletionOutcome::CallbackPending {
                    tool_use_id: "call-a".to_string(),
                    tool_name: "ask_a".to_string(),
                    args: serde_json::json!({"question": "a"}),
                    dedupe_key: dedupe_key.clone(),
                },
            );
            let meerkat_core::event::AgentEvent::InteractionCallbackPending {
                pending_tool_calls,
                ..
            } = event
            else {
                panic!("expected callback-pending interaction event");
            };
            assert_eq!(pending_tool_calls.len(), 1);
            assert_eq!(pending_tool_calls[0].dedupe_key, dedupe_key);
        }
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

    /// The unregister drain answers each waiter of an input the stop kept
    /// queued with the requeued answer and the stop's time, never with the
    /// unregistered failure; every other leftover waiter gets that failure.
    #[tokio::test]
    async fn the_unregister_drain_answers_a_kept_inputs_waiters_as_requeued() {
        let mut registry = CompletionRegistry::new();
        let kept = InputId::new();
        let other = InputId::new();
        let kept_waiters = [
            registry.register(kept.clone()),
            registry.register(kept.clone()),
        ];
        let other_waiter = registry.register(other);
        let stopped_at = chrono::Utc::now();

        let late = registry.drain_unregistered_waiters([(kept, stopped_at)]);

        assert_eq!(late, 1, "the kept input still had waiters at the drain");
        for waiter in kept_waiters {
            match waiter.try_wait().await {
                Err(error) => assert_eq!(
                    error,
                    CompletionWaitError::RequeuedAcrossStop { stopped_at }
                ),
                Ok(outcome) => panic!("a kept input's waiter completed: {outcome:?}"),
            }
        }
        match other_waiter.try_wait().await {
            Err(CompletionWaitError::AuthorityUnavailable(_)) => {}
            other => panic!("expected the unregistered failure, got {other:?}"),
        }
        assert!(!registry.debug_has_waiters());
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
        let authority = authority(
            RuntimeCompletionResultClass::RuntimeTerminated,
            RuntimeCompletionObservedOutcome::RuntimeTerminated,
        );
        let witness = authority.test_completion_batch_witness(
            vec![input_id.clone()],
            crate::input_state::InteractionTerminalCandidate::RuntimeTerminated {
                reason: reason.to_string(),
            },
        );
        let bundle = authorize_runtime_terminal_bundle(
            &[interaction_id],
            None,
            authority,
            witness,
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

    #[test]
    fn runtime_terminal_bundle_rejects_a_witness_for_another_candidate() {
        let input_id = InputId::new();
        let authority = authority(
            RuntimeCompletionResultClass::RuntimeTerminated,
            RuntimeCompletionObservedOutcome::RuntimeTerminated,
        );
        let witness = authority.test_completion_batch_witness(
            vec![input_id],
            crate::input_state::InteractionTerminalCandidate::RuntimeTerminated {
                reason: "different durable candidate".to_string(),
            },
        );
        let error = match authorize_runtime_terminal_bundle(
            &[],
            None,
            authority,
            witness,
            None,
            Some("generated result reason"),
        ) {
            Ok(_) => panic!("authority authorized a differently digested pending candidate"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            CompletionWaitError::AuthorityUnavailable(reason)
                if reason.contains("exact pending batch candidate")
        ));
    }

    #[test]
    fn released_0831_abandoned_completion_json_remains_byte_identical() {
        let outcome = CompletionOutcome::AbandonedWithError {
            reason: "runtime apply failed".to_string(),
            error: TurnErrorMetadata::runtime_apply_failure("runtime apply failed"),
        };
        let encoded = serde_json::to_string(&outcome).unwrap();
        assert_eq!(
            encoded,
            r#"{"completion_type":"abandoned_with_error","reason":"runtime apply failed","error":{"kind":"runtime_apply_failure","terminal":true,"outcome":"failed","detail":"runtime apply failed"}}"#
        );
        let decoded: CompletionOutcome = serde_json::from_str(&encoded).unwrap();
        assert!(matches!(
            decoded,
            CompletionOutcome::AbandonedWithError { .. }
        ));
    }

    #[tokio::test]
    async fn additive_wait_api_returns_the_witness_derived_terminal() {
        let mut pending = CompletionHandle::pending_for_test();
        let handle = pending.take_handle();
        let authority = authority(
            RuntimeCompletionResultClass::AbandonedWithError,
            RuntimeCompletionObservedOutcome::RuntimeApplyFailed,
        );
        let attempt = authority.begin_surface_resolution();
        pending
            .tx
            .send(Ok(CompletionDelivery {
                outcome: CompletionOutcome::AbandonedWithError {
                    reason: "never executed".to_string(),
                    error: TurnErrorMetadata::runtime_apply_failure("never executed"),
                },
                cleanup_observation: CompletionCleanupObservation::from_realized_result(
                    attempt.realize(),
                ),
                terminal_outcome: Some(crate::input_state::InputTerminalOutcome::Abandoned {
                    reason: crate::input_state::InputAbandonReason::NeverExecuted,
                }),
                disclosure: None,
            }))
            .expect("test completion receiver remains live");

        let observation = handle
            .try_wait_with_terminal_outcome()
            .await
            .expect("typed waiter delivery");
        assert!(matches!(
            observation.outcome(),
            CompletionOutcome::AbandonedWithError { .. }
        ));
        assert_eq!(
            observation.input_terminal_outcome(),
            Some(&crate::input_state::InputTerminalOutcome::Abandoned {
                reason: crate::input_state::InputAbandonReason::NeverExecuted,
            })
        );
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

    /// A graceful stop answers a kept input's waiter while its session
    /// unregisters. The generated arm for that failure reads no session
    /// state, so it answers the same before and after the session is gone:
    /// release the pre-admission, session not running, requeued across the
    /// stop, resumable. The waiter's error carries the stop's time.
    #[tokio::test]
    async fn wait_failure_authority_resolves_a_kept_input_with_or_without_its_session() {
        use crate::meerkat_machine::dsl::{
            RuntimeCompletionWaitFailureObservation, RuntimeCompletionWaitFailurePublicErrorClass,
            RuntimeCompletionWaitFailurePublicReason,
        };

        let adapter = crate::meerkat_machine::MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let stopped_at = chrono::Utc::now();
        let requeued = CompletionWaitError::RequeuedAcrossStop { stopped_at };
        assert_eq!(
            requeued.wait_failure_observation(),
            Some(RuntimeCompletionWaitFailureObservation::RequeuedAcrossStop)
        );
        assert_eq!(requeued.requeued_stopped_at(), Some(stopped_at));
        assert_eq!(
            CompletionWaitError::ChannelClosed.requeued_stopped_at(),
            None
        );

        for registered in [false, true] {
            if registered {
                adapter
                    .prepare_bindings(session_id.clone())
                    .await
                    .expect("session should prepare runtime bindings");
            }
            let authority = adapter
                .resolve_runtime_completion_wait_failure(&session_id, &requeued)
                .await
                .unwrap_or_else(|error| {
                    panic!("registered={registered}: a kept input's wait must resolve: {error:?}")
                });
            assert!(
                authority.releases_pre_admission(),
                "registered={registered}"
            );
            assert_eq!(
                authority.failure,
                RuntimeCompletionWaitFailureObservation::RequeuedAcrossStop,
                "registered={registered}"
            );
            assert_eq!(
                authority.public_error_class,
                RuntimeCompletionWaitFailurePublicErrorClass::SessionNotRunning,
                "registered={registered}"
            );
            assert_eq!(
                authority.public_reason,
                RuntimeCompletionWaitFailurePublicReason::RequeuedAcrossStop,
                "registered={registered}"
            );
            assert!(authority.resumable, "registered={registered}");
        }
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

    /// The result payload is gated where it is released, not where it is
    /// delivered: a completion buffered while its destination is current is
    /// refused once the destination retires before the public read. Cleanup
    /// relays still observe the body-free cleanup evidence, the cleanup
    /// observation accessor keeps it, and lifecycle-only outcomes are never
    /// gated.
    #[tokio::test]
    async fn result_disclosure_is_decided_at_release_after_cleanup_relays() {
        let permitted = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let gate_permitted = Arc::clone(&permitted);
        let mut registry =
            CompletionRegistry::with_disclosure_gate(Arc::new(move |_input_id: &InputId| {
                let permitted = Arc::clone(&gate_permitted);
                Some(DeferredDisclosure::new(move || {
                    let permitted = permitted.load(std::sync::atomic::Ordering::SeqCst);
                    async move { permitted }
                }))
            }));
        let input_id = InputId::new();
        let (cleanup_tx, cleanup_rx) = oneshot::channel();
        let relayed = registry.register(input_id.clone()).with_completion_cleanup(
            move |observation| async move {
                let _ = cleanup_tx.send(observation.is_ok());
            },
        );
        let observed = registry.register(input_id.clone());
        let current = registry.register(input_id.clone());
        registry.resolve_completed_authorized(
            &input_id,
            make_run_result(),
            authority(
                RuntimeCompletionResultClass::Completed,
                RuntimeCompletionObservedOutcome::Completed,
            ),
        );
        assert!(
            cleanup_rx.await.expect("the relay ran its cleanup"),
            "the relay's cleanup observed the generated cleanup evidence"
        );
        match current.try_wait().await {
            Ok(CompletionOutcome::Completed(result)) => assert_eq!(result.text, "hello"),
            other => panic!("the current destination receives the result: {other:?}"),
        }

        // The destination retires after delivery was buffered, before release.
        permitted.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(matches!(
            relayed.try_wait().await,
            Err(CompletionWaitError::DisclosureRefused {
                observed: RuntimeCompletionObservedOutcome::Completed
            })
        ));
        let (release, _cleanup_observation) = observed
            .try_wait_with_cleanup_observation()
            .await
            .expect("the cleanup observation is kept for the caller's cleanup");
        let formatted = format!("{release:?}");
        assert!(
            !formatted.contains("hello"),
            "formatting an unreleased outcome never shows its payload: {formatted}"
        );
        assert_eq!(
            release.observed(),
            RuntimeCompletionObservedOutcome::Completed
        );
        let refused = release
            .release()
            .await
            .expect_err("the payload is withheld");
        assert!(refused.is_disclosure_refused());
        assert!(refused.wait_failure_observation().is_none());

        let lifecycle_only = InputId::new();
        let handle = registry.register(lifecycle_only.clone());
        registry.resolve_without_result_authorized(
            &lifecycle_only,
            authority(
                RuntimeCompletionResultClass::CompletedWithoutResult,
                RuntimeCompletionObservedOutcome::CompletedWithoutResult,
            ),
        );
        assert!(matches!(
            handle.try_wait().await,
            Ok(CompletionOutcome::CompletedWithoutResult)
        ));

        // Callback arguments are model-produced payload too: withheld, while
        // the refusal still reports the truthful callback-pending status.
        let callback = InputId::new();
        let handle = registry.register(callback.clone());
        registry.resolve_callback_pending_authorized(
            &callback,
            "call-1".to_string(),
            "lookup".to_string(),
            serde_json::json!({ "secret": "callback-argument-marker" }),
            authority(
                RuntimeCompletionResultClass::CallbackPending,
                RuntimeCompletionObservedOutcome::CallbackPending,
            ),
        );
        let refused = handle
            .try_wait()
            .await
            .expect_err("callback arguments are withheld");
        assert_eq!(
            refused,
            CompletionWaitError::DisclosureRefused {
                observed: RuntimeCompletionObservedOutcome::CallbackPending
            }
        );
        assert!(!refused.to_string().contains("callback-argument-marker"));
    }

    /// The disclosure decision is captured when the input's waiter registers,
    /// while the input's destination is recorded. Its terminal may archive
    /// that record before the result is delivered: a result for the current
    /// destination is still disclosed, and a refused capture stays refused.
    #[tokio::test]
    async fn disclosure_is_captured_at_registration_not_at_delivery() {
        let recorded = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let admitted = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let gate_recorded = Arc::clone(&recorded);
        let gate_admitted = Arc::clone(&admitted);
        let mut registry =
            CompletionRegistry::with_disclosure_gate(Arc::new(move |_input_id: &InputId| {
                // What the machine holds for the input when the gate asks: no
                // decision for an input it does not hold.
                if !gate_admitted.load(std::sync::atomic::Ordering::SeqCst) {
                    return None;
                }
                let permitted = gate_recorded.load(std::sync::atomic::Ordering::SeqCst);
                Some(DeferredDisclosure::new(move || async move { permitted }))
            }));
        let current = InputId::new();
        let handle = registry.register(current.clone());
        // The input's terminal archives its destination record before delivery.
        recorded.store(false, std::sync::atomic::Ordering::SeqCst);
        registry.resolve_completed_authorized(
            &current,
            make_run_result(),
            authority(
                RuntimeCompletionResultClass::Completed,
                RuntimeCompletionObservedOutcome::Completed,
            ),
        );
        match handle.try_wait().await {
            Ok(CompletionOutcome::Completed(result)) => assert_eq!(result.text, "hello"),
            other => panic!("the current destination receives its result: {other:?}"),
        }

        // A waiter registered when the input was not disclosable stays refused.
        let refused = InputId::new();
        let handle = registry.register(refused.clone());
        recorded.store(true, std::sync::atomic::Ordering::SeqCst);
        registry.resolve_completed_authorized(
            &refused,
            make_run_result(),
            authority(
                RuntimeCompletionResultClass::Completed,
                RuntimeCompletionObservedOutcome::Completed,
            ),
        );
        assert!(matches!(
            handle.try_wait().await,
            Err(CompletionWaitError::DisclosureRefused { .. })
        ));

        // A waiter registered for an input the machine does not hold captures
        // nothing, and delivery makes no late lookup (by then the record may
        // be archived): the result is refused, never decided as unscoped,
        // even once the input is admitted.
        admitted.store(false, std::sync::atomic::Ordering::SeqCst);
        let unknown = InputId::new();
        let handle = registry.register(unknown.clone());
        admitted.store(true, std::sync::atomic::Ordering::SeqCst);
        registry.resolve_completed_authorized(
            &unknown,
            make_run_result(),
            authority(
                RuntimeCompletionResultClass::Completed,
                RuntimeCompletionObservedOutcome::Completed,
            ),
        );
        assert!(matches!(
            handle.try_wait().await,
            Err(CompletionWaitError::DisclosureRefused { .. })
        ));
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
                ..
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
        let handle = CompletionHandle {
            rx: Some(rx),
            waiter_token: None,
        }
        .with_resultful_completion_cleanup(|completion| async move {
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
