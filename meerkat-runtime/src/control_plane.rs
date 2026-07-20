//! Concrete helpers for the runtime control-plane seam.
//!
//! Control-plane semantics are now part of `MeerkatMachine`, but the runtime
//! still coordinates out-of-band executor control through `RuntimeLoop` plus
//! the per-session driver. These helpers keep the concrete stop/preemption
//! behavior in one place.

use crate::effect::{RuntimeEffect, RuntimeEffectInner};
use crate::meerkat_machine::{SharedCompletionRegistry, SharedDriver, machine_stop_runtime};
use crate::tokio::sync::mpsc;
use crate::traits::RuntimeDriverError;
use meerkat_core::time_compat::Instant;

const RUNTIME_STOPPED_REASON: &str = "runtime stopped";
const RUNTIME_UNREGISTERED_REASON: &str = "runtime session unregistered";

async fn await_before_deadline<F>(deadline: Option<Instant>, future: F) -> Option<F::Output>
where
    F: std::future::Future,
{
    let Some(deadline) = deadline else {
        return Some(future.await);
    };
    let remaining = deadline.saturating_duration_since(Instant::now());
    crate::tokio::time::timeout(remaining, future).await.ok()
}

fn generated_runtime_termination_reason(
    driver: &crate::meerkat_machine::driver::DriverEntry,
) -> &'static str {
    let authority = driver.shared_dsl_authority();
    let authority = authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if authority.state().registration_phase
        == crate::meerkat_machine::dsl::RegistrationPhase::Draining
    {
        RUNTIME_UNREGISTERED_REASON
    } else {
        RUNTIME_STOPPED_REASON
    }
}

#[derive(Debug, thiserror::Error)]
enum RunlessTerminalConvergenceError {
    #[error("retryable runless terminal failure during {context}: {error}")]
    Retryable {
        context: &'static str,
        error: RuntimeDriverError,
    },
    #[error("stale runless terminal authority during {context}: {error}")]
    StaleAuthority {
        context: &'static str,
        error: RuntimeDriverError,
    },
    #[error("runless terminal corruption during {context}: {error}")]
    Corrupt {
        context: &'static str,
        error: RuntimeDriverError,
    },
}

impl RunlessTerminalConvergenceError {
    fn from_driver(context: &'static str, error: RuntimeDriverError) -> Self {
        match error {
            error @ (RuntimeDriverError::NotReady { .. }
            | RuntimeDriverError::NotFound { .. }
            | RuntimeDriverError::Destroyed
            | RuntimeDriverError::StaleAuthority { .. }) => Self::StaleAuthority { context, error },
            error @ (RuntimeDriverError::ValidationFailed { .. }
            | RuntimeDriverError::RecoveryCorruption { .. }
            | RuntimeDriverError::RecoveryRepairBlocked { .. }) => Self::Corrupt { context, error },
            error @ (RuntimeDriverError::UnregisterFinalizationOutcomeUnknown { .. }
            | RuntimeDriverError::UnregisterInProgress { .. }
            | RuntimeDriverError::RuntimeStopInProgress { .. }
            | RuntimeDriverError::RecoveryBackoff { .. }
            | RuntimeDriverError::Internal(_)) => Self::Retryable { context, error },
        }
    }

    fn from_generated_authority(context: &'static str, error: RuntimeDriverError) -> Self {
        match error {
            error @ (RuntimeDriverError::NotReady { .. }
            | RuntimeDriverError::NotFound { .. }
            | RuntimeDriverError::Destroyed
            | RuntimeDriverError::StaleAuthority { .. }) => Self::StaleAuthority { context, error },
            other => Self::Corrupt {
                context,
                error: other,
            },
        }
    }

    fn corrupt(context: &'static str, reason: impl Into<String>) -> Self {
        Self::Corrupt {
            context,
            error: RuntimeDriverError::RecoveryCorruption {
                reason: reason.into(),
            },
        }
    }

    fn retryable(context: &'static str, reason: impl Into<String>) -> Self {
        Self::Retryable {
            context,
            error: RuntimeDriverError::Internal(reason.into()),
        }
    }

    fn into_driver_error(self) -> RuntimeDriverError {
        match self {
            Self::Retryable { error, .. }
            | Self::StaleAuthority { error, .. }
            | Self::Corrupt { error, .. } => error,
        }
    }
}

async fn await_runless_terminal_handoff(
    handoff: crate::tokio::task::JoinHandle<Result<(), RuntimeDriverError>>,
    context: &'static str,
) -> Result<(), RuntimeDriverError> {
    handoff.await.map_err(|error| {
        RuntimeDriverError::Internal(format!(
            "{context} task ended before its cancellation-safe handoff completed: {error}"
        ))
    })?
}

/// Once the external terminal append has returned exact receipts, move receipt
/// persistence and waiter delivery into an independently owned task. Dropping
/// the runtime-loop/control future therefore cannot strand the local waiter
/// bundle after compacting its durable outbox to `Published`.
///
/// The task takes locks in the existing driver -> completion-registry order and
/// retains both across the receipt CAS. After that awaited CAS returns there is
/// no cancellation point before the synchronous waiter handoff.
fn spawn_published_runless_terminal_handoff(
    driver: SharedDriver,
    completions: Option<SharedCompletionRegistry>,
    candidate_owner_input_id: meerkat_core::lifecycle::InputId,
    completion_input_ids: Vec<meerkat_core::lifecycle::InputId>,
    receipts: Vec<
        meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt,
    >,
    bundle: crate::completion::AuthorizedRuntimeTerminalBundle,
) -> crate::tokio::task::JoinHandle<Result<(), RuntimeDriverError>> {
    crate::tokio::spawn(async move {
        let mut driver = driver.lock().await;
        match completions {
            Some(completions) => {
                let mut completions = completions.lock().await;
                driver
                    .mark_interaction_terminal_outboxes_published(
                        &candidate_owner_input_id,
                        &receipts,
                    )
                    .await?;
                completions
                    .resolve_authorized_runtime_terminal_bundle(completion_input_ids, bundle);
            }
            None => {
                driver
                    .mark_interaction_terminal_outboxes_published(
                        &candidate_owner_input_id,
                        &receipts,
                    )
                    .await?;
            }
        }
        Ok(())
    })
}

/// Nondirected runtime termination has no interaction outbox to act as a
/// recovery carrier. Spawn its generated-authority projection and waiter
/// delivery before the caller reaches another await, so forced runtime-loop
/// cancellation cannot lose the only recipient list after the lifecycle commit.
fn spawn_nondirected_runless_terminal_handoff(
    driver: SharedDriver,
    completions: Option<SharedCompletionRegistry>,
    completion_input_ids: Vec<meerkat_core::lifecycle::InputId>,
    reason: String,
) -> crate::tokio::task::JoinHandle<Result<(), RuntimeDriverError>> {
    crate::tokio::spawn(async move {
        let driver = driver.lock().await;
        let authority = crate::meerkat_machine::driver::machine_resolve_runtime_completion_result(
            &driver,
            None,
            crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::RuntimeTerminated,
            crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
        )?;
        let bundle = crate::completion::authorize_runtime_terminal_bundle(
            &[],
            None,
            authority,
            None,
            Some(&reason),
        )
        .map_err(|error| RuntimeDriverError::RecoveryCorruption {
            reason: format!("runtime termination projection failed: {error}"),
        })?;
        if let Some(completions) = completions {
            let mut completions = completions.lock().await;
            completions.resolve_authorized_runtime_terminal_bundle(completion_input_ids, bundle);
        }
        Ok(())
    })
}

/// Publish one already-committed runless runtime-termination batch and only
/// then deliver the same generated decision to its completion waiters.
///
/// `completion_input_ids` is the exact full recipient set, including
/// nondirected siblings. Interaction events are emitted only for the directed
/// subset represented by durable outboxes. A missing publisher is therefore a
/// hard failure only when that subset is nonempty; the outboxes remain the
/// recovery anchor and no waiter is resolved ahead of publication.
pub(crate) async fn publish_and_resolve_runless_runtime_termination(
    driver: &SharedDriver,
    completions: Option<&SharedCompletionRegistry>,
    publication_handle: Option<&dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>,
    completion_input_ids: &[meerkat_core::lifecycle::InputId],
    candidate_owner_input_id: Option<&meerkat_core::lifecycle::InputId>,
    reason: &str,
) -> Result<(), RuntimeDriverError> {
    publish_and_resolve_runless_runtime_termination_before(
        driver,
        completions,
        publication_handle,
        completion_input_ids,
        candidate_owner_input_id,
        reason,
        None,
    )
    .await
}

/// Request-bounded publication of one already-committed runless terminal.
/// A deadline error leaves the durable outbox as the exact retry authority.
pub(crate) async fn publish_and_resolve_runless_runtime_termination_before(
    driver: &SharedDriver,
    completions: Option<&SharedCompletionRegistry>,
    publication_handle: Option<&dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>,
    completion_input_ids: &[meerkat_core::lifecycle::InputId],
    candidate_owner_input_id: Option<&meerkat_core::lifecycle::InputId>,
    reason: &str,
    publication_deadline: Option<Instant>,
) -> Result<(), RuntimeDriverError> {
    if completion_input_ids.is_empty() {
        return Ok(());
    }

    let (persisted_completion_input_ids, interaction_ids) = if let Some(candidate_owner_input_id) =
        candidate_owner_input_id
    {
        let batch = {
            let mut driver = driver.lock().await;
            driver
                .adopt_interaction_terminal_batch_for_owner(candidate_owner_input_id)
                .await?;
            driver.interaction_terminal_batch_for_owner(candidate_owner_input_id)?
        };
        if publication_handle.is_none() {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "directed runtime termination has no exact terminal publication capability"
                    .to_string(),
            });
        }
        batch
    } else {
        (completion_input_ids.to_vec(), Vec::new())
    };
    if persisted_completion_input_ids != completion_input_ids {
        return Err(RuntimeDriverError::RecoveryCorruption {
            reason: "runtime termination caller recipients differ from the durable exact completion batch"
                .to_string(),
        });
    }

    if candidate_owner_input_id.is_none() {
        let handoff = spawn_nondirected_runless_terminal_handoff(
            driver.clone(),
            completions.cloned(),
            persisted_completion_input_ids,
            reason.to_string(),
        );
        return await_runless_terminal_handoff(handoff, "nondirected runtime-terminal waiter")
            .await;
    }

    let authority =
        crate::meerkat_machine::driver::machine_resolve_runtime_terminated_completion_result(
            driver,
        )
        .await?;
    let bundle = crate::completion::authorize_runtime_terminal_bundle(
        &interaction_ids,
        None,
        authority,
        None,
        Some(reason),
    )
    .map_err(|error| RuntimeDriverError::RecoveryCorruption {
        reason: format!("runtime termination projection failed: {error}"),
    })?;

    if let Some(candidate_owner_input_id) = candidate_owner_input_id {
        let events = bundle.interaction_events();
        if events.len() != interaction_ids.len() {
            return Err(RuntimeDriverError::RecoveryCorruption {
                reason: "authorized runtime termination changed the exact directed recipient set"
                    .to_string(),
            });
        }
        {
            let mut driver = driver.lock().await;
            driver
                .finalize_interaction_terminal_outboxes(
                    candidate_owner_input_id,
                    events,
                    crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
                )
                .await?;
        }
        let publication_handle =
            publication_handle.ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "directed runtime termination has no exact terminal publication capability"
                    .to_string(),
            })?;
        let publication = publication_handle.publish_interaction_terminals(events);
        let publication = await_before_deadline(publication_deadline, publication)
            .await
            .ok_or_else(|| {
                RuntimeDriverError::Internal(
                    "interaction terminal publication did not complete before the convergence deadline"
                        .to_string(),
                )
            })?;
        let receipts = publication.map_err(|error| {
            RuntimeDriverError::Internal(format!(
                "interaction terminal durable publication failed: {error}"
            ))
        })?;
        let handoff = spawn_published_runless_terminal_handoff(
            driver.clone(),
            completions.cloned(),
            (*candidate_owner_input_id).clone(),
            persisted_completion_input_ids,
            receipts,
            bundle,
        );
        return await_runless_terminal_handoff(handoff, "published runtime-terminal receipt").await;
    }

    Err(RuntimeDriverError::RecoveryCorruption {
        reason: "nondirected runtime-terminal batch escaped its owned handoff".to_string(),
    })
}

/// Retry every durable runless terminal batch through an attached publication
/// capability. This is the convergence path for a control operation whose
/// lifecycle/input commit landed before publication or receipt persistence.
async fn drain_recovered_runless_runtime_terminations_classified(
    driver: &SharedDriver,
    completions: Option<&SharedCompletionRegistry>,
    publication_handle: Option<&dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>,
    publication_deadline: Option<Instant>,
) -> Result<(), RunlessTerminalConvergenceError> {
    let batches = driver
        .lock()
        .await
        .interaction_terminal_recovery_batches()
        .await
        .map_err(|error| {
            RunlessTerminalConvergenceError::from_driver(
                "loading committed recovery carriers",
                error,
            )
        })?;
    for batch in batches {
        let crate::meerkat_machine::driver::InteractionTerminalRecoveryBatch {
            batch_key,
            input_ids,
            interaction_ids,
            terminal,
            terminal_observation,
            runtime_termination_reason,
            completion_error_metadata,
            phase,
        } = batch;
        let crate::input_state::InteractionTerminalBatchKey::RuntimeTermination {
            candidate_owner_input_id,
        } = batch_key
        else {
            // Run-scoped recovery needs the executor checkpoint seam and stays
            // owned by RuntimeLoop.
            continue;
        };
        if terminal.is_some()
            || completion_error_metadata.is_some()
            || terminal_observation
                != crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::RuntimeTerminated
        {
            return Err(RunlessTerminalConvergenceError::corrupt(
                "validating a committed recovery carrier",
                "runless terminal batch carried a non-runtime-termination candidate",
            ));
        }
        let reason = runtime_termination_reason.ok_or_else(|| {
            RunlessTerminalConvergenceError::corrupt(
                "validating a committed recovery carrier",
                "runless terminal batch lost its exact termination reason",
            )
        })?;
        let publication_handle = publication_handle.ok_or_else(|| {
            RunlessTerminalConvergenceError::corrupt(
                "resolving exact publication capability",
                "pending directed runtime termination has no publication capability",
            )
        })?;
        let authority =
            crate::meerkat_machine::driver::machine_resolve_runtime_terminated_completion_result(
                driver,
            )
            .await
            .map_err(|error| {
                RunlessTerminalConvergenceError::from_generated_authority(
                    "resolving generated runtime-termination authority",
                    error,
                )
            })?;
        let bundle = crate::completion::authorize_runtime_terminal_bundle(
            &interaction_ids,
            None,
            authority,
            None,
            Some(&reason),
        )
        .map_err(|error| {
            RunlessTerminalConvergenceError::corrupt(
                "projecting a committed recovery carrier",
                format!("runless terminal projection failed: {error}"),
            )
        })?;
        let events = bundle.interaction_events();
        if let crate::meerkat_machine::driver::InteractionTerminalRecoveryPhase::Finalized {
            events: recovered_events,
            finalization,
            finalization_error,
        } = phase
        {
            let payloads_match = if recovered_events.len() == events.len() {
                recovered_events.iter().zip(events).try_fold(
                    true,
                    |matches, (recovered, expected)| {
                        let recovered_id =
                            crate::input_state::interaction_terminal_event_id(recovered);
                        let expected_id =
                            crate::input_state::interaction_terminal_event_id(expected);
                        let recovered_digest =
                            crate::input_state::interaction_terminal_payload_digest(recovered)
                                .map_err(|reason| {
                                    RunlessTerminalConvergenceError::corrupt(
                                        "validating a finalized recovery payload",
                                        reason,
                                    )
                                })?;
                        let expected_digest =
                            crate::input_state::interaction_terminal_payload_digest(expected)
                                .map_err(|reason| {
                                    RunlessTerminalConvergenceError::corrupt(
                                        "validating generated terminal payload",
                                        reason,
                                    )
                                })?;
                        Ok::<_, RunlessTerminalConvergenceError>(
                            matches
                                && recovered_id == expected_id
                                && recovered_digest == expected_digest,
                        )
                    },
                )?
            } else {
                false
            };
            if finalization
                != crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded
                || finalization_error.is_some()
                || !payloads_match
            {
                return Err(RunlessTerminalConvergenceError::corrupt(
                    "validating finalized recovery authority",
                    "recovered runless terminal payload differs from generated authority",
                ));
            }
        } else {
            driver
                .lock()
                .await
                .finalize_interaction_terminal_outboxes(
                    &candidate_owner_input_id,
                    events,
                    crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
                )
                .await
                .map_err(|error| {
                    RunlessTerminalConvergenceError::from_driver(
                        "finalizing the committed terminal carrier",
                        error,
                    )
                })?;
        }
        let publication = publication_handle.publish_interaction_terminals(events);
        let publication = await_before_deadline(publication_deadline, publication)
            .await
            .ok_or_else(|| {
                RunlessTerminalConvergenceError::retryable(
                    "publishing the exact terminal batch",
                    "recovered interaction terminal publication did not complete before the convergence deadline",
                )
            })?;
        let receipts = publication.map_err(|error| {
            RunlessTerminalConvergenceError::retryable(
                "publishing the exact terminal batch",
                format!("recovered interaction terminal publication failed: {error}"),
            )
        })?;
        let handoff = spawn_published_runless_terminal_handoff(
            driver.clone(),
            completions.cloned(),
            candidate_owner_input_id,
            input_ids,
            receipts,
            bundle,
        );
        await_runless_terminal_handoff(handoff, "recovered runtime-terminal receipt")
            .await
            .map_err(|error| {
                RunlessTerminalConvergenceError::from_driver(
                    "persisting exact publication receipts and resolving waiters",
                    error,
                )
            })?;
    }
    Ok(())
}

pub(crate) async fn drain_recovered_runless_runtime_terminations(
    driver: &SharedDriver,
    completions: Option<&SharedCompletionRegistry>,
    publication_handle: Option<&dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>,
) -> Result<(), RuntimeDriverError> {
    drain_recovered_runless_runtime_terminations_classified(
        driver,
        completions,
        publication_handle,
        None,
    )
    .await
    .map_err(RunlessTerminalConvergenceError::into_driver_error)
}

async fn has_committed_runless_recovery_carrier(
    driver: &SharedDriver,
) -> Result<bool, RunlessTerminalConvergenceError> {
    let driver = driver.lock().await;
    if !matches!(
        driver.runtime_state(),
        crate::RuntimeState::Stopped
            | crate::RuntimeState::Retired
            | crate::RuntimeState::Destroyed
    ) {
        // Stop dispatch stages its exact outboxes before the runtime loop sees
        // the effect. Those rows are not a retry authority until the lifecycle
        // transaction commits; a failed stop persist leaves the runtime in its
        // previous non-terminal state and must return immediately.
        return Ok(false);
    }
    let stored_inputs = driver
        .as_driver()
        .stored_input_states_snapshot()
        .map_err(|error| {
            RunlessTerminalConvergenceError::from_driver(
                "checking for a committed recovery carrier",
                error,
            )
        })?;
    Ok(stored_inputs.into_iter().any(|stored| {
        stored
            .state
            .interaction_terminal_outbox
            .is_some_and(|outbox| {
                matches!(
                    outbox.batch_key,
                    crate::input_state::InteractionTerminalBatchKey::RuntimeTermination { .. }
                ) && !matches!(
                    outbox.phase,
                    crate::input_state::InteractionTerminalOutboxPhase::Published { .. }
                )
            })
    }))
}

/// Canonical terminalization for an async stop after the executor has accepted
/// the stop control command.
pub(crate) async fn terminalize_async_stop_once(
    driver: &SharedDriver,
    completions: Option<&SharedCompletionRegistry>,
    publication_handle: Option<&dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>,
) -> Result<(), RuntimeDriverError> {
    let (completion_input_ids, candidate_owner_input_id, termination_reason) = {
        let mut driver = driver.lock().await;
        let completion_input_ids = driver.as_driver().active_input_ids();
        let termination_reason = generated_runtime_termination_reason(&driver).to_string();
        let prepared = driver.prepare_runless_runtime_terminated_interaction_outboxes(
            &completion_input_ids,
            termination_reason.clone(),
        )?;
        if !matches!(
            driver.runtime_state(),
            crate::RuntimeState::Stopped | crate::RuntimeState::Destroyed
        ) && let Err(error) = machine_stop_runtime(&mut driver).await
        {
            driver.rollback_prepared_runless_interaction_terminal_outboxes(prepared);
            return Err(error);
        }
        let candidate_owner_input_id =
            crate::meerkat_machine::driver::DriverEntry::commit_prepared_runless_interaction_terminal_outboxes(prepared);
        (
            completion_input_ids,
            candidate_owner_input_id,
            termination_reason,
        )
    };

    publish_and_resolve_runless_runtime_termination(
        driver,
        completions,
        publication_handle,
        &completion_input_ids,
        candidate_owner_input_id.as_ref(),
        &termination_reason,
    )
    .await
}

/// Converge only a runless terminal whose exact committed carrier survived the
/// one-shot stop transaction. A pre-commit failure has no carrier and returns
/// immediately; once the carrier exists, transient publication/receipt errors
/// retry until the exact receipt is durable and the complete waiter bundle has
/// been resolved.
async fn converge_committed_runless_runtime_terminations(
    driver: &SharedDriver,
    completions: Option<&SharedCompletionRegistry>,
    publication_handle: Option<&dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>,
    initial_result: Result<(), RuntimeDriverError>,
) -> Result<(), RuntimeDriverError> {
    let initial_error = initial_result.err();
    let carrier_exists = has_committed_runless_recovery_carrier(driver)
        .await
        .map_err(RunlessTerminalConvergenceError::into_driver_error)?;
    if !carrier_exists {
        return initial_error.map_or(Ok(()), Err);
    }

    if let Some(error) = initial_error {
        match RunlessTerminalConvergenceError::from_driver(
            "the initial stop terminalization attempt",
            error,
        ) {
            RunlessTerminalConvergenceError::Retryable { .. } => {}
            fatal => return Err(fatal.into_driver_error()),
        }
    }

    converge_known_committed_runless_runtime_terminations_before(
        driver,
        completions,
        publication_handle,
        None,
    )
    .await
}

/// Retry an exact committed runless-terminal carrier. Request paths provide a
/// deadline; teardown may pass `None` because it still owns the carrier.
pub(crate) async fn converge_known_committed_runless_runtime_terminations_before(
    driver: &SharedDriver,
    completions: Option<&SharedCompletionRegistry>,
    publication_handle: Option<&dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>,
    deadline: Option<Instant>,
) -> Result<(), RuntimeDriverError> {
    let mut delay_ms = 25_u64;
    loop {
        if deadline.is_some_and(|deadline| deadline <= Instant::now()) {
            return Err(RuntimeDriverError::Internal(
                "committed runless terminal recovery did not converge before its deadline"
                    .to_string(),
            ));
        }
        match drain_recovered_runless_runtime_terminations_classified(
            driver,
            completions,
            publication_handle,
            deadline,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(error @ RunlessTerminalConvergenceError::Retryable { .. }) => {
                let Some(sleep_for) = retry_delay_before_deadline(delay_ms, deadline) else {
                    return Err(RuntimeDriverError::Internal(format!(
                        "committed runless terminal recovery did not converge before its deadline: {error}"
                    )));
                };
                tracing::error!(
                    error = %error,
                    retry_delay_ms = sleep_for.as_millis(),
                    "committed runless terminal recovery remains pending"
                );
                crate::tokio::time::sleep(sleep_for).await;
                delay_ms = delay_ms.saturating_mul(2).min(1_000);
            }
            Err(error @ RunlessTerminalConvergenceError::StaleAuthority { .. }) => {
                tracing::warn!(
                    error = %error,
                    "runless terminal recovery stopped after losing exact authority"
                );
                return Err(error.into_driver_error());
            }
            Err(error @ RunlessTerminalConvergenceError::Corrupt { .. }) => {
                tracing::error!(
                    error = %error,
                    "runless terminal recovery failed closed on durable corruption"
                );
                return Err(error.into_driver_error());
            }
        }
    }
}

fn retry_delay_before_deadline(
    delay_ms: u64,
    deadline: Option<Instant>,
) -> Option<std::time::Duration> {
    let requested = std::time::Duration::from_millis(delay_ms);
    let Some(deadline) = deadline else {
        return Some(requested);
    };
    let remaining = deadline.checked_duration_since(Instant::now())?;
    (!remaining.is_zero()).then(|| requested.min(remaining))
}

/// Own the complete accepted Stop transaction independently of the runtime-loop
/// task that requested it. External unregister may force-abort a stuck loop;
/// detaching this exact transaction ensures such cancellation cannot interrupt
/// an in-memory lifecycle mutation before its persistence await, a nondirected
/// waiter-only handoff, or a directed receipt/waiter convergence retry.
pub(crate) async fn terminalize_async_stop(
    driver: &SharedDriver,
    completions: Option<&SharedCompletionRegistry>,
    publication_handle: Option<
        std::sync::Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>,
    >,
    turn_finalization_guard: Option<
        Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard>,
    >,
) -> Result<
    Option<Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard>>,
    RuntimeDriverError,
> {
    let driver = driver.clone();
    let completions = completions.cloned();
    let terminalization = crate::tokio::spawn(async move {
        // This is the exact guard acquired by RuntimeLoop before accepting the
        // Stop effect. The independently owned transaction returns it to a live
        // caller for cleanup, while forced outer-loop cancellation leaves it in
        // the child through exact publication, receipt, and waiter handoff.
        // A prior accepted stop may already own an exact committed runless
        // carrier. Preserve its original generated reason across a later
        // unregister transition instead of attempting to replace that public
        // terminal with a second lifecycle classification.
        let initial_result = match has_committed_runless_recovery_carrier(&driver).await {
            Ok(true) => Ok(()),
            Ok(false) => {
                terminalize_async_stop_once(
                    &driver,
                    completions.as_ref(),
                    publication_handle.as_deref(),
                )
                .await
            }
            Err(error) => Err(error.into_driver_error()),
        };
        let result = converge_committed_runless_runtime_terminations(
            &driver,
            completions.as_ref(),
            publication_handle.as_deref(),
            initial_result,
        )
        .await;
        (result, turn_finalization_guard)
    });
    let (result, turn_finalization_guard) = terminalization.await.map_err(|error| {
        RuntimeDriverError::Internal(format!(
            "accepted runtime Stop terminalization task ended before its cancellation-safe handoff completed: {error}"
        ))
    })?;
    result?;
    Ok(turn_finalization_guard)
}

/// Deliver a non-terminal executor effect.
///
/// Stop effects are rejected here by construction. The runtime loop's typed
/// handoff path is the only owner allowed to call the executor stop hook,
/// terminalize machine authority, and relinquish the exact executor for
/// external cleanup.
pub(crate) async fn apply_executor_effect(
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    effect: RuntimeEffect,
) -> Result<(), RuntimeDriverError> {
    match effect.into_inner() {
        RuntimeEffectInner::CancelAfterBoundary { reason } => {
            executor
                .cancel_after_boundary(reason)
                .await
                .map_err(|err| {
                    RuntimeDriverError::Internal(format!(
                        "failed to apply cancel-after-boundary executor effect: {err}"
                    ))
                })?;
            Ok(())
        }
        RuntimeEffectInner::StopRuntimeExecutor { .. } => Err(RuntimeDriverError::Internal(
            "stop-runtime-executor effect requires the runtime-loop terminal handoff path"
                .to_string(),
        )),
    }
}

/// Typed handoff produced when a runtime-loop task realizes an executor
/// effect. Required post-terminalization cleanup is deliberately excluded from
/// this task: the loop must first relinquish its executor so an external
/// teardown owner can run cleanup without ever awaiting the loop's own
/// `JoinHandle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeLoopEffectOutcome {
    Continue,
    StopTerminalizedNeedsExternalCleanup,
}

#[derive(Debug)]
pub(crate) struct RuntimeLoopEffectFailure {
    pub(crate) error: RuntimeDriverError,
    /// Present only when the executor stop hook itself failed before machine
    /// terminalization. The retained exact executor must receive this same
    /// stop request again before cleanup may run.
    pub(crate) executor_stop_retry_reason: Option<String>,
}

/// Realize the required executor stop hook at the control-plane ownership
/// boundary. Runtime-loop and unregister mechanics must delegate here rather
/// than invoking the executor control primitive directly.
pub(crate) async fn apply_runtime_executor_stop_hook(
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    reason: String,
) -> Result<(), meerkat_core::lifecycle::CoreExecutorError> {
    executor.stop_runtime_executor(reason).await
}

pub(crate) async fn apply_runtime_loop_executor_effect(
    driver: &SharedDriver,
    completions: Option<&SharedCompletionRegistry>,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    effect: RuntimeEffect,
    turn_finalization_guard: Option<
        Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard>,
    >,
) -> Result<RuntimeLoopEffectOutcome, RuntimeLoopEffectFailure> {
    match effect.into_inner() {
        RuntimeEffectInner::CancelAfterBoundary { reason } => {
            executor
                .cancel_after_boundary(reason)
                .await
                .map_err(|err| RuntimeLoopEffectFailure {
                    error: RuntimeDriverError::Internal(format!(
                        "failed to apply cancel-after-boundary executor effect: {err}"
                    )),
                    executor_stop_retry_reason: None,
                })?;
            Ok(RuntimeLoopEffectOutcome::Continue)
        }
        RuntimeEffectInner::StopRuntimeExecutor { reason } => {
            if let Err(error) = apply_runtime_executor_stop_hook(executor, reason.clone()).await {
                return Err(RuntimeLoopEffectFailure {
                    error: RuntimeDriverError::Internal(format!(
                        "failed to apply stop-runtime-executor effect: {error}"
                    )),
                    executor_stop_retry_reason: Some(reason),
                });
            }
            let publication_handle = executor.publication_handle();
            let _turn_finalization_guard = terminalize_async_stop(
                driver,
                completions,
                publication_handle,
                turn_finalization_guard,
            )
            .await
            .map_err(|error| RuntimeLoopEffectFailure {
                error,
                executor_stop_retry_reason: None,
            })?;
            Ok(RuntimeLoopEffectOutcome::StopTerminalizedNeedsExternalCleanup)
        }
    }
}

/// Typed outcome of draining ready executor effects.
///
/// Distinguishes the three terminal shapes a drain can produce so the loop
/// driver never conflates a closed effect channel with an applied stop:
/// channel closure must still route through the canonical stop/terminalize
/// path (StopRuntimeExecutor + [`terminalize_async_stop`] + waiter resolution)
/// instead of silently exiting as if a stop effect had already been applied.
#[derive(Debug)]
pub(crate) enum EffectDrainOutcome {
    /// A stop effect was received but NOT applied. The caller drops its
    /// authority guard first, realizes the stop hook/terminalization, then
    /// hands required cleanup to the external teardown owner.
    StopEffectPending(RuntimeEffect),
    /// No effects were pending; the channel remains open. The loop may
    /// continue processing queued work.
    Empty,
    /// The effect sender was dropped (control plane gone). The loop must
    /// terminalize through the stop path rather than exit bare.
    ChannelClosed,
}

/// Drain any ready executor effects before starting another unit of
/// ordinary queued work.
pub(crate) async fn drain_ready_executor_effects(
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    effect_rx: &mut mpsc::Receiver<RuntimeEffect>,
) -> Result<EffectDrainOutcome, RuntimeDriverError> {
    loop {
        match effect_rx.try_recv() {
            Ok(effect) => {
                if effect.is_stop() {
                    // Never applied here: the caller holds the session
                    // mutation gate during drains. Stop hooks may re-enter
                    // machine control, and cleanup must remain an explicit
                    // post-loop external handoff.
                    return Ok(EffectDrainOutcome::StopEffectPending(effect));
                }
                apply_executor_effect(executor, effect).await?;
            }
            Err(mpsc::error::TryRecvError::Empty) => return Ok(EffectDrainOutcome::Empty),
            Err(mpsc::error::TryRecvError::Disconnected) => {
                return Ok(EffectDrainOutcome::ChannelClosed);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::{InputId, RunId, run_primitive::RunPrimitive};

    /// Deterministically fails every `apply` while keeping the stop path
    /// healthy — models a poison payload whose run terminally fails on each
    /// stage attempt (the field shape behind the defer-wedge class tests).
    pub(crate) struct ApplyFailingExecutor {
        apply_calls: Arc<AtomicUsize>,
        stop_calls: Arc<AtomicUsize>,
    }

    impl ApplyFailingExecutor {
        pub(crate) fn new(apply_calls: Arc<AtomicUsize>, stop_calls: Arc<AtomicUsize>) -> Self {
            Self {
                apply_calls,
                stop_calls,
            }
        }
    }

    #[async_trait::async_trait]
    impl CoreExecutor for ApplyFailingExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Err(CoreExecutorError::apply_failed_runtime_turn(
                "synthetic deterministic apply failure",
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            self.stop_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    pub(crate) struct StopFailingExecutor {
        stop_calls: Arc<AtomicUsize>,
        apply_calls: Arc<AtomicUsize>,
    }

    impl StopFailingExecutor {
        pub(crate) fn new(stop_calls: Arc<AtomicUsize>, apply_calls: Arc<AtomicUsize>) -> Self {
            Self {
                stop_calls,
                apply_calls,
            }
        }
    }

    #[async_trait::async_trait]
    impl CoreExecutor for StopFailingExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Err(CoreExecutorError::apply_failed_runtime_turn(
                "stop failure regression must not apply queued work",
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            self.stop_calls.fetch_add(1, Ordering::SeqCst);
            Err(CoreExecutorError::control_failed_runtime(
                "synthetic stop effect failure",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::ephemeral::EphemeralRuntimeDriver;
    use crate::effect::{RuntimeEffect, runtime_effect_for_test};
    use crate::identifiers::LogicalRuntimeId;
    use crate::meerkat_machine::driver::DriverEntry;
    use crate::meerkat_machine::dsl::RuntimeEffectKind;
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::{RunId, run_primitive::RunPrimitive};
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn shared_driver() -> SharedDriver {
        Arc::new(crate::tokio::sync::Mutex::new(DriverEntry::Ephemeral(
            EphemeralRuntimeDriver::new(LogicalRuntimeId::new("effect-test")),
        )))
    }

    fn seed_attached_authority(
        driver: &mut EphemeralRuntimeDriver,
        session_id: &meerkat_core::types::SessionId,
        runtime_id: &LogicalRuntimeId,
        fence_token: u64,
        generation: u64,
        epoch_id: &str,
    ) {
        let mut authority =
            crate::meerkat_machine::dsl_authority::new_registered_authority(session_id)
                .expect("test authority must register its session");
        crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
            &mut authority,
            crate::meerkat_machine::dsl::MeerkatMachineInput::PrepareBindings {
                agent_runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from(
                    runtime_id.to_string(),
                ),
                fence_token: crate::meerkat_machine::dsl::FenceToken::from(fence_token),
                generation: Some(crate::meerkat_machine::dsl::Generation::from(generation)),
                runtime_epoch_id: Some(crate::meerkat_machine::dsl::RuntimeEpochId::from(
                    epoch_id.to_string(),
                )),
                session_id: crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
            },
        )
        .expect("test authority must attach through the normal binding transition");
        *driver
            .shared_dsl_authority()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = authority;
        driver.set_control_projection(crate::RuntimeState::Attached, None, None);
    }

    fn runtime_effect(kind: RuntimeEffectKind, reason: &str) -> RuntimeEffect {
        runtime_effect_for_test(kind, reason)
    }

    struct RetryRecordingTerminalPublisher {
        attempts: AtomicUsize,
        published: StdMutex<Vec<meerkat_core::event::AgentEvent>>,
        retry_entered: Arc<crate::tokio::sync::Notify>,
        retry_release: Arc<crate::tokio::sync::Notify>,
    }

    impl RetryRecordingTerminalPublisher {
        fn new(
            retry_entered: Arc<crate::tokio::sync::Notify>,
            retry_release: Arc<crate::tokio::sync::Notify>,
        ) -> Self {
            Self {
                attempts: AtomicUsize::new(0),
                published: StdMutex::new(Vec::new()),
                retry_entered,
                retry_release,
            }
        }
    }

    #[async_trait::async_trait]
    impl meerkat_core::lifecycle::CoreExecutorPublicationHandle for RetryRecordingTerminalPublisher {
        async fn publish_interaction_terminals(
            &self,
            events: &[meerkat_core::event::AgentEvent],
        ) -> Result<
            Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
            CoreExecutorError,
        > {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                // Model the exact crash cut: the durable append committed,
                // but the caller lost the publication receipts before it
                // could persist its Published phase.
                let mut published = self
                    .published
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                published.extend_from_slice(events);
                return Err(CoreExecutorError::Internal(
                    "synthetic receipt loss after durable terminal append".to_string(),
                ));
            }
            self.retry_entered.notify_one();
            self.retry_release.notified().await;
            let published = self
                .published
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if serde_json::to_vec(published.as_slice()).map_err(|error| {
                CoreExecutorError::Internal(format!(
                    "failed to encode durable terminal replay: {error}"
                ))
            })? != serde_json::to_vec(events).map_err(|error| {
                CoreExecutorError::Internal(format!(
                    "failed to encode incoming terminal replay: {error}"
                ))
            })? {
                return Err(CoreExecutorError::Internal(
                    "exact terminal replay differed from the durable append".to_string(),
                ));
            }
            drop(published);
            events
                .iter()
                .enumerate()
                .map(|(index, event)| {
                    meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt::try_new(
                        event,
                        index as u64 + 1,
                    )
                })
                .collect()
        }
    }

    struct DropRecordingTurnFinalizationGuard {
        dropped: Arc<AtomicBool>,
        dropped_notify: Arc<crate::tokio::sync::Notify>,
    }

    impl Drop for DropRecordingTurnFinalizationGuard {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
            self.dropped_notify.notify_one();
        }
    }

    #[tokio::test]
    async fn mixed_stop_converges_lost_receipt_before_waiters_and_cleanup() {
        use crate::completion::CompletionOutcome;
        use crate::input::{Input, PromptInput};
        use crate::traits::RuntimeDriver;
        use meerkat_core::event::{AgentEvent, InteractionFailureReason};
        use meerkat_core::types::{ContentInput, SessionId};

        let runtime_id = LogicalRuntimeId::new("mixed-stop-terminal-publication");
        let session_id = SessionId::new();
        let mut raw_driver = EphemeralRuntimeDriver::new(runtime_id.clone());
        seed_attached_authority(
            &mut raw_driver,
            &session_id,
            &runtime_id,
            7,
            3,
            "mixed-stop-epoch",
        );

        let directed_uuid = meerkat_core::time_compat::new_uuid_v7();
        let directed = crate::mob_adapter::create_tracked_flow_step_input(
            "directed-step",
            ContentInput::Text("directed work".to_string()),
            "mixed-stop-flow",
            None,
            &directed_uuid.to_string(),
        )
        .expect("tracked flow step");
        let directed_input_id = directed.id().clone();
        let nondirected = Input::Prompt(PromptInput::new("ordinary sibling", None));
        let nondirected_input_id = nondirected.id().clone();
        assert!(
            RuntimeDriver::accept_input(&mut raw_driver, directed)
                .await
                .expect("accept directed input")
                .is_accepted()
        );
        assert!(
            RuntimeDriver::accept_input(&mut raw_driver, nondirected)
                .await
                .expect("accept nondirected input")
                .is_accepted()
        );

        let driver = Arc::new(crate::tokio::sync::Mutex::new(DriverEntry::Ephemeral(
            raw_driver,
        )));
        let completions = Arc::new(crate::tokio::sync::Mutex::new(
            crate::completion::CompletionRegistry::new(),
        ));
        let (directed_waiter, nondirected_waiter) = {
            let mut registry = completions.lock().await;
            (
                registry.register(directed_input_id.clone()),
                registry.register(nondirected_input_id.clone()),
            )
        };
        let retry_entered = Arc::new(crate::tokio::sync::Notify::new());
        let retry_release = Arc::new(crate::tokio::sync::Notify::new());
        let publisher = Arc::new(RetryRecordingTerminalPublisher::new(
            Arc::clone(&retry_entered),
            Arc::clone(&retry_release),
        ));
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let cleanup_calls = Arc::new(AtomicUsize::new(0));
        let stop_driver = Arc::clone(&driver);
        let stop_completions = Arc::clone(&completions);
        let publication_handle: Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle> =
            publisher.clone();
        let mut executor = RecordingExecutor {
            boundary_calls: Arc::new(AtomicUsize::new(0)),
            stop_calls: Arc::clone(&stop_calls),
            cleanup_calls: Arc::clone(&cleanup_calls),
            fail_boundary: false,
            publication_handle: Some(publication_handle),
        };
        let stop_task = crate::tokio::spawn(async move {
            apply_runtime_loop_executor_effect(
                &stop_driver,
                Some(&stop_completions),
                &mut executor,
                runtime_effect(RuntimeEffectKind::StopRuntimeExecutor, "stop"),
                None,
            )
            .await
        });

        crate::tokio::time::timeout(std::time::Duration::from_secs(1), retry_entered.notified())
            .await
            .expect("committed runless carrier should enter its publication retry");
        assert_eq!(
            publisher
                .published
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            1,
            "the crash cut occurs after the exact durable append"
        );
        {
            let driver = driver.lock().await;
            let outbox = driver
                .as_driver()
                .stored_input_states_snapshot()
                .expect("snapshot finalized retry carrier")
                .into_iter()
                .find_map(|stored| stored.state.interaction_terminal_outbox)
                .expect("directed sibling keeps its retry carrier");
            assert!(matches!(
                outbox.phase,
                crate::input_state::InteractionTerminalOutboxPhase::Finalized { .. }
            ));
            assert!(outbox.candidate.is_some());
            assert!(outbox.completion_input_ids.is_some());
        }
        assert_eq!(
            completions.lock().await.debug_waiter_count(),
            2,
            "publication failure must not resolve either mixed-batch waiter"
        );
        assert_eq!(
            cleanup_calls.load(Ordering::SeqCst),
            0,
            "post-stop cleanup must remain behind exact publication and waiter resolution"
        );
        assert!(
            !stop_task.is_finished(),
            "stop application must remain pending while its committed carrier is unpublished"
        );

        retry_release.notify_one();
        let outcome = stop_task
            .await
            .expect("stop convergence task should join")
            .expect("retry exact runless terminal publication");
        assert_eq!(
            outcome,
            RuntimeLoopEffectOutcome::StopTerminalizedNeedsExternalCleanup
        );
        {
            let driver = driver.lock().await;
            let outbox = driver
                .as_driver()
                .stored_input_states_snapshot()
                .expect("snapshot published retry carrier")
                .into_iter()
                .find_map(|stored| stored.state.interaction_terminal_outbox)
                .expect("published directed proof remains compacted");
            assert!(matches!(
                outbox.phase,
                crate::input_state::InteractionTerminalOutboxPhase::Published { .. }
            ));
            assert!(outbox.candidate.is_none());
            assert!(outbox.completion_input_ids.is_none());
        }

        for outcome in [
            directed_waiter.wait_authorized().await,
            nondirected_waiter.wait_authorized().await,
        ] {
            match outcome {
                CompletionOutcome::RuntimeTerminated { reason, .. } => {
                    assert_eq!(reason, "runtime stopped");
                }
                other => panic!("expected exact RuntimeTerminated outcome, got {other:?}"),
            }
        }
        assert_eq!(completions.lock().await.debug_waiter_count(), 0);
        assert_eq!(stop_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            cleanup_calls.load(Ordering::SeqCst),
            0,
            "runtime-loop terminal convergence must leave cleanup to the external handoff owner"
        );
        assert_eq!(publisher.attempts.load(Ordering::SeqCst), 2);
        let published = publisher
            .published
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            published.len(),
            1,
            "only the directed sibling emits an event"
        );
        match &published[0] {
            AgentEvent::InteractionFailed {
                interaction_id,
                reason: InteractionFailureReason::Abandoned { detail },
            } => {
                assert_eq!(interaction_id.0, directed_input_id.0);
                assert_eq!(detail, "runtime stopped");
                assert_ne!(interaction_id.0, nondirected_input_id.0);
            }
            other => panic!("unexpected directed terminal event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn cancelling_stop_caller_does_not_cancel_committed_terminal_handoff() {
        use crate::completion::CompletionOutcome;
        use crate::traits::RuntimeDriver;
        use meerkat_core::types::{ContentInput, SessionId};

        let runtime_id = LogicalRuntimeId::new("cancel-safe-stop-terminal-publication");
        let session_id = SessionId::new();
        let mut raw_driver = EphemeralRuntimeDriver::new(runtime_id.clone());
        seed_attached_authority(
            &mut raw_driver,
            &session_id,
            &runtime_id,
            17,
            9,
            "cancel-safe-stop-epoch",
        );
        let directed_uuid = meerkat_core::time_compat::new_uuid_v7();
        let directed = crate::mob_adapter::create_tracked_flow_step_input(
            "cancel-safe-directed-step",
            ContentInput::Text("directed work".to_string()),
            "cancel-safe-stop-flow",
            None,
            &directed_uuid.to_string(),
        )
        .expect("tracked flow step");
        let directed_input_id = directed.id().clone();
        assert!(
            RuntimeDriver::accept_input(&mut raw_driver, directed)
                .await
                .expect("accept directed input")
                .is_accepted()
        );

        let driver = Arc::new(crate::tokio::sync::Mutex::new(DriverEntry::Ephemeral(
            raw_driver,
        )));
        let completions = Arc::new(crate::tokio::sync::Mutex::new(
            crate::completion::CompletionRegistry::new(),
        ));
        let waiter = completions.lock().await.register(directed_input_id.clone());
        let retry_entered = Arc::new(crate::tokio::sync::Notify::new());
        let retry_release = Arc::new(crate::tokio::sync::Notify::new());
        let publisher = Arc::new(RetryRecordingTerminalPublisher::new(
            Arc::clone(&retry_entered),
            Arc::clone(&retry_release),
        ));
        let publication_handle: Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle> =
            publisher.clone();
        let cleanup_calls = Arc::new(AtomicUsize::new(0));
        let mut executor = RecordingExecutor {
            boundary_calls: Arc::new(AtomicUsize::new(0)),
            stop_calls: Arc::new(AtomicUsize::new(0)),
            cleanup_calls: Arc::clone(&cleanup_calls),
            fail_boundary: false,
            publication_handle: Some(publication_handle),
        };
        let stop_driver = Arc::clone(&driver);
        let stop_completions = Arc::clone(&completions);
        let turn_guard_dropped = Arc::new(AtomicBool::new(false));
        let turn_guard_dropped_notify = Arc::new(crate::tokio::sync::Notify::new());
        let turn_guard: Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard> =
            Box::new(DropRecordingTurnFinalizationGuard {
                dropped: Arc::clone(&turn_guard_dropped),
                dropped_notify: Arc::clone(&turn_guard_dropped_notify),
            });
        let stop_task = crate::tokio::spawn(async move {
            apply_runtime_loop_executor_effect(
                &stop_driver,
                Some(&stop_completions),
                &mut executor,
                runtime_effect(RuntimeEffectKind::StopRuntimeExecutor, "stop"),
                Some(turn_guard),
            )
            .await
        });

        crate::tokio::time::timeout(std::time::Duration::from_secs(1), retry_entered.notified())
            .await
            .expect("committed carrier should enter exact publication retry");
        stop_task.abort();
        assert!(
            stop_task
                .await
                .expect_err("outer Stop caller should be cancelled")
                .is_cancelled()
        );
        assert!(
            !turn_guard_dropped.load(Ordering::SeqCst),
            "forced outer-loop cancellation must not release B while terminal publication is pending"
        );
        retry_release.notify_one();

        let outcome = crate::tokio::time::timeout(
            std::time::Duration::from_secs(1),
            waiter.wait_authorized(),
        )
        .await
        .expect("detached terminal handoff must resolve its waiter after caller cancellation");
        assert!(matches!(
            outcome,
            CompletionOutcome::RuntimeTerminated { ref reason, .. }
                if reason == "runtime stopped"
        ));
        let outbox = driver
            .lock()
            .await
            .as_driver()
            .stored_input_states_snapshot()
            .expect("snapshot published cancellation-safe carrier")
            .into_iter()
            .find_map(|stored| stored.state.interaction_terminal_outbox)
            .expect("published directed proof remains compacted");
        assert!(matches!(
            outbox.phase,
            crate::input_state::InteractionTerminalOutboxPhase::Published { .. }
        ));
        assert_eq!(completions.lock().await.debug_waiter_count(), 0);
        assert_eq!(publisher.attempts.load(Ordering::SeqCst), 2);
        if !turn_guard_dropped.load(Ordering::SeqCst) {
            crate::tokio::time::timeout(
                std::time::Duration::from_secs(1),
                turn_guard_dropped_notify.notified(),
            )
            .await
            .expect("detached terminal transaction must release B after waiter handoff");
        }
        assert!(turn_guard_dropped.load(Ordering::SeqCst));
        assert_eq!(
            cleanup_calls.load(Ordering::SeqCst),
            0,
            "cancelled caller cannot run cleanup before its detached handoff completes"
        );
    }

    #[tokio::test]
    async fn staged_runless_outbox_without_lifecycle_commit_is_not_retried() {
        use crate::traits::RuntimeDriver;
        use meerkat_core::types::{ContentInput, SessionId};

        let runtime_id = LogicalRuntimeId::new("uncommitted-stop-carrier");
        let session_id = SessionId::new();
        let mut raw_driver = EphemeralRuntimeDriver::new(runtime_id.clone());
        seed_attached_authority(
            &mut raw_driver,
            &session_id,
            &runtime_id,
            13,
            5,
            "uncommitted-stop-epoch",
        );
        let directed_uuid = meerkat_core::time_compat::new_uuid_v7();
        let directed = crate::mob_adapter::create_tracked_flow_step_input(
            "uncommitted-directed",
            ContentInput::Text("uncommitted directed work".to_string()),
            "uncommitted-flow",
            None,
            &directed_uuid.to_string(),
        )
        .expect("tracked flow step");
        let directed_input_id = directed.id().clone();
        assert!(
            RuntimeDriver::accept_input(&mut raw_driver, directed)
                .await
                .expect("accept directed input")
                .is_accepted()
        );
        let mut driver_entry = DriverEntry::Ephemeral(raw_driver);
        let prepared = driver_entry
            .prepare_runless_runtime_terminated_interaction_outboxes(
                std::slice::from_ref(&directed_input_id),
                "runtime stopped".to_string(),
            )
            .expect("stage runless outbox before lifecycle commit");
        DriverEntry::commit_prepared_runless_interaction_terminal_outboxes(prepared)
            .expect("directed staging should mint a carrier owner");
        assert_eq!(driver_entry.runtime_state(), crate::RuntimeState::Attached);
        let driver = Arc::new(crate::tokio::sync::Mutex::new(driver_entry));
        let error = crate::tokio::time::timeout(
            std::time::Duration::from_millis(100),
            converge_committed_runless_runtime_terminations(
                &driver,
                None,
                None,
                Err(RuntimeDriverError::Internal(
                    "synthetic pre-commit failure".to_string(),
                )),
            ),
        )
        .await
        .expect("an outbox without a lifecycle commit must not enter the retry loop")
        .expect_err("the original pre-commit failure must be preserved");

        assert!(matches!(
            error,
            RuntimeDriverError::Internal(message)
                if message == "synthetic pre-commit failure"
        ));
    }

    #[test]
    fn runless_terminal_errors_classify_transient_stale_and_corrupt() {
        assert!(matches!(
            RunlessTerminalConvergenceError::from_driver(
                "test",
                RuntimeDriverError::Internal("transient store failure".to_string()),
            ),
            RunlessTerminalConvergenceError::Retryable { .. }
        ));
        assert!(matches!(
            RunlessTerminalConvergenceError::from_driver(
                "test",
                RuntimeDriverError::StaleAuthority {
                    reason: "lost CAS".to_string(),
                },
            ),
            RunlessTerminalConvergenceError::StaleAuthority { .. }
        ));
        assert!(matches!(
            RunlessTerminalConvergenceError::from_driver(
                "test",
                RuntimeDriverError::RecoveryCorruption {
                    reason: "payload mismatch".to_string(),
                },
            ),
            RunlessTerminalConvergenceError::Corrupt { .. }
        ));
    }

    struct RecordingExecutor {
        boundary_calls: Arc<AtomicUsize>,
        stop_calls: Arc<AtomicUsize>,
        cleanup_calls: Arc<AtomicUsize>,
        fail_boundary: bool,
        publication_handle: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for RecordingExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            unreachable!("effect tests do not apply primitives")
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            self.boundary_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_boundary {
                return Err(CoreExecutorError::control_failed_runtime(
                    "synthetic boundary cancel failure",
                ));
            }
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            self.stop_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn publication_handle(
            &self,
        ) -> Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>> {
            self.publication_handle.clone()
        }

        async fn cleanup_after_runtime_stop_terminalized(
            &mut self,
        ) -> Result<(), CoreExecutorError> {
            self.cleanup_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct CleanupFailingExecutor;

    #[async_trait::async_trait]
    impl CoreExecutor for CleanupFailingExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            unreachable!("effect tests do not apply primitives")
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn cleanup_after_runtime_stop_terminalized(
            &mut self,
        ) -> Result<(), CoreExecutorError> {
            Err(CoreExecutorError::control_failed_runtime(
                "synthetic retained cleanup failure",
            ))
        }
    }

    #[tokio::test]
    async fn apply_executor_effect_dispatches_boundary_and_rejects_stop() {
        let boundary_calls = Arc::new(AtomicUsize::new(0));
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let cleanup_calls = Arc::new(AtomicUsize::new(0));
        let mut executor = RecordingExecutor {
            boundary_calls: Arc::clone(&boundary_calls),
            stop_calls: Arc::clone(&stop_calls),
            cleanup_calls: Arc::clone(&cleanup_calls),
            fail_boundary: false,
            publication_handle: None,
        };
        apply_executor_effect(
            &mut executor,
            runtime_effect(RuntimeEffectKind::CancelAfterBoundary, "boundary"),
        )
        .await
        .expect("boundary effect should apply");

        assert_eq!(boundary_calls.load(Ordering::SeqCst), 1);
        assert_eq!(stop_calls.load(Ordering::SeqCst), 0);
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 0);

        let error = apply_executor_effect(
            &mut executor,
            runtime_effect(RuntimeEffectKind::StopRuntimeExecutor, "stop"),
        )
        .await
        .expect_err("direct stop application must require terminal handoff");

        assert!(error.to_string().contains("terminal handoff path"));
        assert_eq!(boundary_calls.load(Ordering::SeqCst), 1);
        assert_eq!(stop_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            cleanup_calls.load(Ordering::SeqCst),
            0,
            "direct effect application must never own terminal cleanup"
        );
    }

    #[tokio::test]
    async fn runtime_loop_terminal_handoff_does_not_run_cleanup_inline() {
        let driver = shared_driver();
        let mut executor = CleanupFailingExecutor;

        let outcome = apply_runtime_loop_executor_effect(
            &driver,
            None,
            &mut executor,
            runtime_effect(RuntimeEffectKind::StopRuntimeExecutor, "stop"),
            None,
        )
        .await
        .expect("terminal handoff must not invoke external cleanup inline");

        assert_eq!(
            outcome,
            RuntimeLoopEffectOutcome::StopTerminalizedNeedsExternalCleanup
        );
        assert_eq!(
            driver.lock().await.runtime_state(),
            crate::RuntimeState::Stopped
        );
    }

    #[tokio::test]
    async fn apply_executor_effect_boundary_cancel_failure_is_fail_closed() {
        let boundary_calls = Arc::new(AtomicUsize::new(0));
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let cleanup_calls = Arc::new(AtomicUsize::new(0));
        let mut executor = RecordingExecutor {
            boundary_calls: Arc::clone(&boundary_calls),
            stop_calls,
            cleanup_calls,
            fail_boundary: true,
            publication_handle: None,
        };
        let err = apply_executor_effect(
            &mut executor,
            runtime_effect(RuntimeEffectKind::CancelAfterBoundary, "boundary"),
        )
        .await
        .expect_err("boundary cancel effect failure must not be warn-only");

        assert!(
            err.to_string()
                .contains("failed to apply cancel-after-boundary executor effect"),
            "unexpected error: {err}"
        );
        assert_eq!(boundary_calls.load(Ordering::SeqCst), 1);
    }

    /// Regression for #287: a closed effect channel must surface as a distinct
    /// `ChannelClosed` outcome, NOT collapse into the same `AppliedStop` signal a
    /// real stop effect produces. Under the old `Result<bool, _>` shape both
    /// returned `Ok(true)` and the loop exited bare without terminalizing.
    #[tokio::test]
    async fn drain_ready_executor_effects_channel_closed_is_distinct_from_applied_stop() {
        let driver = shared_driver();

        // Stop-pending case: a stop effect is delivered, then the sender is
        // dropped. The drain must NOT apply the stop (the stop hook can
        // re-enter machine control while drain callers hold session authority);
        // it surfaces the effect for the runtime-loop handoff path after the
        // guard is released.
        {
            let stop_calls = Arc::new(AtomicUsize::new(0));
            let mut executor = RecordingExecutor {
                boundary_calls: Arc::new(AtomicUsize::new(0)),
                stop_calls: Arc::clone(&stop_calls),
                cleanup_calls: Arc::new(AtomicUsize::new(0)),
                fail_boundary: false,
                publication_handle: None,
            };
            let (tx, mut rx) = mpsc::channel::<RuntimeEffect>(4);
            tx.send(runtime_effect(
                RuntimeEffectKind::StopRuntimeExecutor,
                "stop",
            ))
            .await
            .expect("send stop effect");
            drop(tx);

            let outcome = drain_ready_executor_effects(&mut executor, &mut rx)
                .await
                .expect("drain with pending stop should succeed");
            let EffectDrainOutcome::StopEffectPending(effect) = outcome else {
                panic!("a stop effect must surface as StopEffectPending, got {outcome:?}");
            };
            assert_eq!(
                stop_calls.load(Ordering::SeqCst),
                0,
                "the drain must never apply the stop itself"
            );
            let outcome =
                apply_runtime_loop_executor_effect(&driver, None, &mut executor, effect, None)
                    .await
                    .expect("runtime-loop handoff applies the pending stop guard-free");
            assert_eq!(
                outcome,
                RuntimeLoopEffectOutcome::StopTerminalizedNeedsExternalCleanup
            );
            assert_eq!(stop_calls.load(Ordering::SeqCst), 1);
        }

        // Closed-channel-only case: the sender is dropped without ever
        // delivering a stop effect. The drain must report `ChannelClosed`,
        // distinguishable from `AppliedStop`, and must NOT invoke the
        // executor stop.
        {
            let stop_calls = Arc::new(AtomicUsize::new(0));
            let mut executor = RecordingExecutor {
                boundary_calls: Arc::new(AtomicUsize::new(0)),
                stop_calls: Arc::clone(&stop_calls),
                cleanup_calls: Arc::new(AtomicUsize::new(0)),
                fail_boundary: false,
                publication_handle: None,
            };
            let (tx, mut rx) = mpsc::channel::<RuntimeEffect>(4);
            drop(tx);

            let outcome = drain_ready_executor_effects(&mut executor, &mut rx)
                .await
                .expect("drain on closed channel should succeed");
            assert!(
                matches!(outcome, EffectDrainOutcome::ChannelClosed),
                "a closed effect channel must be distinguishable from a pending stop: {outcome:?}"
            );
            assert_eq!(
                stop_calls.load(Ordering::SeqCst),
                0,
                "bare channel closure does not itself apply a stop effect"
            );
        }
    }

    #[tokio::test]
    async fn runless_terminal_retry_selects_exact_batch_without_consuming_unrelated_batches() {
        use crate::input::{Input, PromptInput};
        use crate::traits::RuntimeDriver;
        use meerkat_core::types::{ContentInput, SessionId};

        let runtime_id = LogicalRuntimeId::new("multiple-runless-terminal-batches");
        let session_id = SessionId::new();
        let mut raw_driver = EphemeralRuntimeDriver::new(runtime_id.clone());
        seed_attached_authority(
            &mut raw_driver,
            &session_id,
            &runtime_id,
            11,
            4,
            "multiple-runless-epoch",
        );

        let mut batches = Vec::new();
        for (label, prompt) in [("first", "first sibling"), ("second", "second sibling")] {
            let directed_uuid = meerkat_core::time_compat::new_uuid_v7();
            let directed = crate::mob_adapter::create_tracked_flow_step_input(
                &format!("{label}-directed"),
                ContentInput::Text(format!("{label} directed work")),
                &format!("{label}-flow"),
                None,
                &directed_uuid.to_string(),
            )
            .expect("tracked flow step");
            let directed_input_id = directed.id().clone();
            let nondirected = Input::Prompt(PromptInput::new(prompt, None));
            let nondirected_input_id = nondirected.id().clone();
            assert!(
                RuntimeDriver::accept_input(&mut raw_driver, directed)
                    .await
                    .expect("accept directed input")
                    .is_accepted()
            );
            assert!(
                RuntimeDriver::accept_input(&mut raw_driver, nondirected)
                    .await
                    .expect("accept nondirected input")
                    .is_accepted()
            );
            batches.push((
                vec![directed_input_id, nondirected_input_id],
                format!("{label} termination"),
            ));
        }

        let mut driver = DriverEntry::Ephemeral(raw_driver);
        let mut owners = Vec::new();
        for (input_ids, reason) in &batches {
            let prepared = driver
                .prepare_runless_runtime_terminated_interaction_outboxes(input_ids, reason.clone())
                .expect("unrelated runless batch should stage independently");
            owners.push(
                DriverEntry::commit_prepared_runless_interaction_terminal_outboxes(prepared)
                    .expect("directed batch has a candidate owner"),
            );
        }
        assert_ne!(owners[0], owners[1]);

        let exact_retry = driver
            .prepare_runless_runtime_terminated_interaction_outboxes(
                &batches[0].0,
                batches[0].1.clone(),
            )
            .expect("exact retry should select its existing batch");
        assert_eq!(
            DriverEntry::commit_prepared_runless_interaction_terminal_outboxes(exact_retry),
            Some(owners[0].clone())
        );

        let Err(reason_conflict) = driver.prepare_runless_runtime_terminated_interaction_outboxes(
            &batches[0].0,
            "different reason".to_string(),
        ) else {
            panic!("same recipients with a different reason must fail closed");
        };
        assert!(matches!(
            reason_conflict,
            RuntimeDriverError::ValidationFailed { .. }
        ));

        let Err(overlap) = driver.prepare_runless_runtime_terminated_interaction_outboxes(
            &[batches[0].0[0].clone(), batches[1].0[1].clone()],
            batches[0].1.clone(),
        ) else {
            panic!("partially overlapping runless batch must fail closed");
        };
        assert!(matches!(
            overlap,
            RuntimeDriverError::ValidationFailed { .. }
        ));
    }
}
