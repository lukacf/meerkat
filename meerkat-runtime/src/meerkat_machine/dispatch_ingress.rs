use super::*;
use meerkat_core::time_compat::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveBoundaryAttachmentRevalidation {
    CurrentRun,
    RunAdvancedQueued,
    RunAdvancedClaimed { wake_needed: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LiveBoundaryInputDisposition {
    QueuedFallback,
    ExactInjected,
    SuccessorClaimed { wake_needed: bool },
}

impl LiveBoundaryInputDisposition {
    pub(super) fn suppress_cancel(self) -> bool {
        matches!(self, Self::ExactInjected | Self::SuccessorClaimed { .. })
    }

    pub(super) fn suppress_wake(self) -> bool {
        matches!(
            self,
            Self::ExactInjected | Self::SuccessorClaimed { wake_needed: false }
        )
    }
}

enum RetryableLiveBoundaryPreparation {
    Prepared(meerkat_core::lifecycle::CoreBoundaryStageOutput),
    Unavailable { reason: String },
    Stale { reason: String },
}

impl MeerkatMachine {
    /// Select whether generated admission should attempt exact live-boundary
    /// staging. The machine-owned current run plus the exact captured
    /// attachment capabilities are sufficient to select the plan; the
    /// capability may still report `Unavailable`, and the post-admission
    /// transaction revalidates attachment, run, and queued input before any
    /// append is committed.
    pub(super) async fn active_turn_boundary_candidate_available(
        driver: &SharedDriver,
        has_boundary_handle: bool,
        has_live_attachment: bool,
    ) -> bool {
        has_boundary_handle && has_live_attachment && driver.lock().await.current_run_id().is_some()
    }

    async fn commit_failed_accepted_input_terminal(
        &self,
        driver: &SharedDriver,
        input_id: &InputId,
        reason: &str,
    ) -> Result<Option<InputId>, RuntimeDriverError> {
        let candidate_owner_input_id = {
            let mut driver = driver.lock().await;
            let phase = driver
                .as_driver()
                .stored_input_state(input_id)
                .map(|stored| stored.seed.phase)
                .ok_or_else(|| RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "accepted input {input_id} disappeared before failure terminalization"
                    ),
                })?;
            if phase != InputLifecycleState::Queued {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "accepted input {input_id} reached {phase:?} before failure terminalization"
                    ),
                });
            }
            let prepared = driver.prepare_runless_runtime_terminated_interaction_outboxes(
                std::slice::from_ref(input_id),
                reason.to_string(),
            )?;
            match driver
                .abandon_queued_input(input_id, InputAbandonReason::Cancelled)
                .await
            {
                Ok(true) => {}
                Ok(false) => {
                    driver.rollback_prepared_runless_interaction_terminal_outboxes(prepared);
                    return Err(RuntimeDriverError::StaleAuthority {
                        reason: format!(
                            "accepted input {input_id} was no longer queued at failure commit"
                        ),
                    });
                }
                Err(error) => {
                    driver.rollback_prepared_runless_interaction_terminal_outboxes(prepared);
                    return Err(error);
                }
            }
            DriverEntry::commit_prepared_runless_interaction_terminal_outboxes(prepared)
        };
        Ok(candidate_owner_input_id)
    }

    async fn revalidate_live_boundary_attachment(
        &self,
        session_id: &SessionId,
        witness: &RuntimeLiveBoundaryAttachmentWitness,
        expected_run_id: &RunId,
        input_id: &InputId,
    ) -> Result<LiveBoundaryAttachmentRevalidation, RuntimeDriverError> {
        {
            let sessions = self.sessions.read().await;
            let entry =
                sessions
                    .get(session_id)
                    .ok_or_else(|| RuntimeDriverError::StaleAuthority {
                        reason: format!(
                            "live-boundary attachment disappeared for session '{session_id}'"
                        ),
                    })?;
            let exact_boundary_handle = entry
                .boundary_handle()
                .is_some_and(|handle| Arc::ptr_eq(&handle, &witness.boundary_handle));
            if !Arc::ptr_eq(&entry.mutation_gate, &witness.mutation_gate)
                || !Arc::ptr_eq(&entry.driver, &witness.driver)
                || !Arc::ptr_eq(&entry.dsl_authority, &witness.dsl_authority)
                || entry.live_attachment_id() != Some(witness.attachment_id)
                || !exact_boundary_handle
            {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "live-boundary executor attachment changed for session '{session_id}'"
                    ),
                });
            }
            entry.require_durability_ready().map_err(|required| {
                RuntimeDriverError::RecoveryRepairBlocked {
                    evidence_digest: None,
                    reason: required.to_string(),
                }
            })?;
        }

        let driver = witness.driver.lock().await;
        let current_run_id = driver.current_run_id();
        let Some(phase) = driver
            .as_driver()
            .stored_input_state(input_id)
            .map(|stored| stored.seed.phase)
        else {
            if current_run_id.as_ref() != Some(expected_run_id) {
                // Terminal successor progress may retire the input before this
                // stale preparation reacquires M. The old run no longer owns
                // it, so preserve that completed machine transition. Retain
                // the outer wake only when other queued work still needs it.
                let wake_needed = current_run_id.is_none()
                    && driver.has_queued_input_outside(std::slice::from_ref(input_id));
                return Ok(LiveBoundaryAttachmentRevalidation::RunAdvancedClaimed { wake_needed });
            }
            return Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "accepted live-boundary input {input_id} disappeared during its original run"
                ),
            });
        };
        if current_run_id.as_ref() == Some(expected_run_id) {
            if phase == InputLifecycleState::Queued {
                return Ok(LiveBoundaryAttachmentRevalidation::CurrentRun);
            }
            return Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "accepted live-boundary input {input_id} changed phase during its original run before commit: {phase:?}"
                ),
            });
        }

        if phase == InputLifecycleState::Queued {
            return Ok(LiveBoundaryAttachmentRevalidation::RunAdvancedQueued);
        }

        // M is intentionally released while the actor prepares the transient
        // boundary. During that window the old run may finish and the ordinary
        // queued path may claim this exact input for a successor run. Accept
        // only machine-owned progress: active phases must belong to the exact
        // current successor, Consumed must name a successor, and runless
        // terminal phases are already resolved by their generated transition.
        let last_run_id = driver.input_last_run_id(input_id);
        let successor_owns_input = match phase {
            InputLifecycleState::Staged
            | InputLifecycleState::Applied
            | InputLifecycleState::AppliedPendingConsumption => current_run_id
                .as_ref()
                .is_some_and(|current_run_id| last_run_id.as_ref() == Some(current_run_id)),
            InputLifecycleState::Consumed => last_run_id
                .as_ref()
                .is_some_and(|last_run_id| last_run_id != expected_run_id),
            InputLifecycleState::Superseded
            | InputLifecycleState::Coalesced
            | InputLifecycleState::Abandoned => true,
            InputLifecycleState::Accepted | InputLifecycleState::Queued => false,
        };
        if successor_owns_input {
            let wake_needed = current_run_id.is_none()
                && driver.has_queued_input_outside(std::slice::from_ref(input_id));
            return Ok(LiveBoundaryAttachmentRevalidation::RunAdvancedClaimed { wake_needed });
        }

        Err(RuntimeDriverError::StaleAuthority {
            reason: format!(
                "accepted live-boundary input {input_id} reached {phase:?} without successor-run ownership"
            ),
        })
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "terminalization keeps every exact driver, mutation-gate, publication, and completion authority explicit"
    )]
    async fn terminalize_failed_accepted_input(
        &self,
        session_id: &SessionId,
        driver: &SharedDriver,
        completions: &SharedCompletionRegistry,
        publication_handle: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>>,
        mutation_gate: &Arc<crate::tokio::sync::Mutex<()>>,
        held_mutation_gate: crate::tokio::sync::OwnedMutexGuard<()>,
        input_id: &InputId,
        reason: String,
    ) -> Result<(), RuntimeDriverError> {
        let candidate_owner_input_id = self
            .commit_failed_accepted_input_terminal(driver, input_id, &reason)
            .await?;
        let dispatch = match (
            candidate_owner_input_id.as_ref(),
            publication_handle.clone(),
        ) {
            (Some(_), Some(publication_handle)) => {
                Some(self.prepare_runless_terminal_publication_dispatch(
                    driver,
                    completions,
                    mutation_gate,
                    publication_handle,
                )?)
            }
            _ => None,
        };
        drop(held_mutation_gate);
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        let result = if let Some((result_rx, start_tx)) = dispatch {
            if let Some(start_tx) = start_tx {
                let _ = start_tx.send(());
            }
            self.await_runless_terminal_publication_dispatch(
                &LogicalRuntimeId::for_session(session_id),
                result_rx,
                Some(deadline),
            )
            .await
        } else if candidate_owner_input_id.is_some() {
            crate::control_plane::converge_known_committed_runless_runtime_terminations_before(
                driver,
                Some(completions),
                None,
                Some(deadline),
            )
            .await
        } else {
            crate::control_plane::publish_and_resolve_runless_runtime_termination_before(
                driver,
                Some(completions),
                publication_handle.as_deref(),
                std::slice::from_ref(input_id),
                None,
                &reason,
                Some(deadline),
            )
            .await
        };
        result.map_err(|error| {
            tracing::error!(
                %session_id,
                %input_id,
                %error,
                "durable accepted-input failure terminal remains recoverable from its outbox"
            );
            error
        })
    }

    // Keep the exact attachment, completion, publication, and fallback-wake
    // owners explicit; grouping them would mint a second transaction shape.
    #[allow(clippy::too_many_arguments)]
    async fn finish_live_boundary_failure(
        &self,
        session_id: &SessionId,
        witness: &RuntimeLiveBoundaryAttachmentWitness,
        completions: &SharedCompletionRegistry,
        publication_handle: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>>,
        held_mutation_gate: crate::tokio::sync::OwnedMutexGuard<()>,
        input_id: &InputId,
        primary: RuntimeDriverError,
        fallback_wake: &mut AcceptedIngressFallbackWakeGuard,
    ) -> RuntimeDriverError {
        let reason = primary.to_string();
        match self
            .terminalize_failed_accepted_input(
                session_id,
                &witness.driver,
                completions,
                publication_handle,
                &witness.mutation_gate,
                held_mutation_gate,
                input_id,
                reason.clone(),
            )
            .await
        {
            Ok(()) => {
                fallback_wake.disarm();
                primary
            }
            Err(terminalization_error) => {
                let remains_queued = witness
                    .driver
                    .lock()
                    .await
                    .as_driver()
                    .stored_input_state(input_id)
                    .is_some_and(|stored| stored.seed.phase == InputLifecycleState::Queued);
                if !remains_queued {
                    fallback_wake.disarm();
                }
                RuntimeDriverError::Internal(format!(
                    "{reason}; exact accepted-input terminalization also failed: {terminalization_error}"
                ))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn normalize_live_boundary_queued_fallback(
        &self,
        session_id: &SessionId,
        witness: &RuntimeLiveBoundaryAttachmentWitness,
        completions: &SharedCompletionRegistry,
        publication_handle: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>>,
        held_mutation_gate: crate::tokio::sync::OwnedMutexGuard<()>,
        input_id: &InputId,
        fallback_wake: &mut AcceptedIngressFallbackWakeGuard,
    ) -> Result<crate::tokio::sync::OwnedMutexGuard<()>, RuntimeDriverError> {
        if let Err(error) = witness
            .driver
            .lock()
            .await
            .machine_normalize_live_boundary_unavailable(input_id)
            .await
        {
            return Err(self
                .finish_live_boundary_failure(
                    session_id,
                    witness,
                    completions,
                    publication_handle,
                    held_mutation_gate,
                    input_id,
                    error,
                    fallback_wake,
                )
                .await);
        }
        Ok(held_mutation_gate)
    }

    /// Attempt one exact active-turn context injection.
    ///
    /// The caller enters with M. Preparation runs without M so the session actor
    /// can park and call back into runtime mechanics. The same exact attachment
    /// is then revalidated under M before the durable driver/store commit. If
    /// no run is active yet, the input remains queued for the outer runtime wake.
    /// `Unavailable`, `Stale`, and a run advancing during preparation invalidate
    /// only the transient delivery attempt, so the durably accepted input remains
    /// queued. `Fault` still converges the exact accepted input to a durable
    /// terminal before surfacing failure.
    async fn finalize_live_boundary_completion_owned(
        driver: &SharedDriver,
        completions: &SharedCompletionRegistry,
        input_id: InputId,
        run_id: RunId,
        finalization: crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation,
        finalization_error: Option<meerkat_core::TurnErrorMetadata>,
    ) -> Result<(), RuntimeDriverError> {
        let driver = Arc::clone(driver);
        let completions = Arc::clone(completions);
        crate::tokio::spawn(async move {
            let mut driver_guard = driver.lock_owned().await;
            let completion_guard = completions.lock_owned().await;
            let terminal_completion_witness = driver_guard
                .input_terminal_completion_authorization_witness(std::slice::from_ref(&input_id))?;
            let authority =
                crate::meerkat_machine::driver::machine_resolve_runtime_completion_result(
                    &driver_guard,
                    Some(&run_id),
                    crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::NoResult,
                    finalization,
                )?;
            let bundle = crate::completion::authorize_runtime_terminal_bundle(
                &[],
                None,
                authority,
                terminal_completion_witness,
                finalization_error,
                None,
            )
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "live-boundary completion projection failed: {error}"
                ))
            })?;
            driver_guard
                .finalize_input_terminal_completion_batch(bundle.terminal_completion())
                .await?;
            let _driver_guard = driver_guard;
            let mut completion_guard = completion_guard;
            completion_guard.resolve_authorized_runtime_terminal_bundle([input_id], bundle);
            Ok::<(), RuntimeDriverError>(())
        })
        .await
        .map_err(|error| {
            RuntimeDriverError::Internal(format!(
                "owned live-boundary completion task ended before publishing its outcome: {error}"
            ))
        })?
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn commit_live_boundary_input_if_available(
        &self,
        session_id: &SessionId,
        witness: &RuntimeLiveBoundaryAttachmentWitness,
        mut held_mutation_gate: crate::tokio::sync::OwnedMutexGuard<()>,
        input_id: &InputId,
        completions: &SharedCompletionRegistry,
        publication_handle: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>>,
        fallback_wake: &mut AcceptedIngressFallbackWakeGuard,
    ) -> Result<
        (
            crate::tokio::sync::OwnedMutexGuard<()>,
            LiveBoundaryInputDisposition,
        ),
        RuntimeDriverError,
    > {
        let live_boundary_plan = {
            let driver = witness.driver.lock().await;
            let Some(run_id) = driver.current_run_id() else {
                tracing::debug!(
                    session_id = %session_id,
                    input_id = %input_id,
                    "no active run is available for exact live-boundary context; retaining same-boundary Steer batching for the fresh runtime turn"
                );
                return Ok((
                    held_mutation_gate,
                    LiveBoundaryInputDisposition::QueuedFallback,
                ));
            };
            // Arbitrate only an actual active-run boundary. Before a run has
            // acquired queue authority, multiple Steer inputs intentionally
            // remain eligible for one generated same-boundary batch.
            let steer_queue = driver.driver_ingress().steer_queue();
            if steer_queue.iter().any(|candidate| candidate == input_id)
                && steer_queue.first() != Some(input_id)
            {
                drop(driver);
                held_mutation_gate = self
                    .normalize_live_boundary_queued_fallback(
                        session_id,
                        witness,
                        completions,
                        publication_handle.clone(),
                        held_mutation_gate,
                        input_id,
                        fallback_wake,
                    )
                    .await?;
                tracing::debug!(
                    session_id = %session_id,
                    input_id = %input_id,
                    "later live-boundary candidate retained durable FIFO behind earlier active-run admission"
                );
                return Ok((
                    held_mutation_gate,
                    LiveBoundaryInputDisposition::QueuedFallback,
                ));
            }
            let Some(projection) = driver.driver_ingress().primitive_projection(input_id) else {
                let error = RuntimeDriverError::Internal(format!(
                    "accepted live-boundary input {input_id} has no primitive projection"
                ));
                drop(driver);
                let error = self
                    .finish_live_boundary_failure(
                        session_id,
                        witness,
                        completions,
                        publication_handle,
                        held_mutation_gate,
                        input_id,
                        error,
                        fallback_wake,
                    )
                    .await;
                return Err(error);
            };
            let Some(semantics) = driver.driver_ingress().runtime_semantics(input_id) else {
                let error = RuntimeDriverError::Internal(format!(
                    "accepted live-boundary input {input_id} has no runtime semantics"
                ));
                drop(driver);
                let error = self
                    .finish_live_boundary_failure(
                        session_id,
                        witness,
                        completions,
                        publication_handle,
                        held_mutation_gate,
                        input_id,
                        error,
                        fallback_wake,
                    )
                    .await;
                return Err(error);
            };
            let contexts =
                crate::input::projection_to_transient_turn_context(&projection, semantics)
                    .into_iter()
                    .collect::<Vec<_>>();
            if contexts.is_empty() {
                // Idle-normalized steers and peer-terminal facts are ordinary
                // queued transcript work. Leave the fallback armed so the
                // runtime loop realizes that durable path.
                return Ok((
                    held_mutation_gate,
                    LiveBoundaryInputDisposition::QueuedFallback,
                ));
            }
            (run_id, contexts)
        };
        let (run_id, contexts) = live_boundary_plan;

        tracing::debug!(
            session_id = %session_id,
            run_id = %run_id,
            input_id = %input_id,
            context_count = contexts.len(),
            "preparing exact parked live-boundary request context"
        );

        // The boundary callback may re-enter MeerkatMachine. Drop M before its
        // first poll; the process-owned outer ingress task retains all witnesses.
        drop(held_mutation_gate);
        let prepared = witness
            .boundary_handle
            .prepare_transient_turn_context_at_boundary(&run_id, contexts)
            .await;
        let mut held_mutation_gate = Arc::clone(&witness.mutation_gate).lock_owned().await;

        let revalidation = match self
            .revalidate_live_boundary_attachment(session_id, witness, &run_id, input_id)
            .await
        {
            Ok(revalidation) => revalidation,
            Err(error) => {
                drop(prepared);
                let error = self
                    .finish_live_boundary_failure(
                        session_id,
                        witness,
                        completions,
                        publication_handle,
                        held_mutation_gate,
                        input_id,
                        error,
                        fallback_wake,
                    )
                    .await;
                return Err(error);
            }
        };

        match revalidation {
            LiveBoundaryAttachmentRevalidation::RunAdvancedQueued => {
                drop(prepared);
                held_mutation_gate = self
                    .normalize_live_boundary_queued_fallback(
                        session_id,
                        witness,
                        completions,
                        publication_handle.clone(),
                        held_mutation_gate,
                        input_id,
                        fallback_wake,
                    )
                    .await?;
                tracing::debug!(
                    session_id = %session_id,
                    run_id = %run_id,
                    input_id = %input_id,
                    "live-boundary run advanced during preparation; normalized durable queued fallback"
                );
                return Ok((
                    held_mutation_gate,
                    LiveBoundaryInputDisposition::QueuedFallback,
                ));
            }
            LiveBoundaryAttachmentRevalidation::RunAdvancedClaimed { wake_needed } => {
                if let Ok(prepared) = prepared
                    && let Err(error) = prepared.abort()
                {
                    tracing::warn!(
                        session_id = %session_id,
                        run_id = %run_id,
                        input_id = %input_id,
                        error = %error,
                        "failed to abort stale parked boundary after successor claim"
                    );
                }
                tracing::debug!(
                    session_id = %session_id,
                    run_id = %run_id,
                    input_id = %input_id,
                    wake_needed,
                    "live-boundary run advanced during preparation; successor run already owns accepted input"
                );
                // The successor owns the input, so the old-run cancel must not
                // race it. A currently active successor already owns its
                // executor wake. Once the successor has retired, retain the
                // outer wake only when other queued work still needs it.
                return Ok((
                    held_mutation_gate,
                    LiveBoundaryInputDisposition::SuccessorClaimed { wake_needed },
                ));
            }
            LiveBoundaryAttachmentRevalidation::CurrentRun => {}
        }

        let prepared = match prepared {
            Ok(prepared) => RetryableLiveBoundaryPreparation::Prepared(prepared),
            Err(meerkat_core::lifecycle::CoreBoundaryStageError::Unavailable { reason }) => {
                RetryableLiveBoundaryPreparation::Unavailable { reason }
            }
            Err(meerkat_core::lifecycle::CoreBoundaryStageError::Stale { reason }) => {
                RetryableLiveBoundaryPreparation::Stale { reason }
            }
            Err(meerkat_core::lifecycle::CoreBoundaryStageError::Fault { reason }) => {
                let error = RuntimeDriverError::Internal(format!(
                    "live-boundary preparation failed for input {input_id}: {reason}"
                ));
                let error = self
                    .finish_live_boundary_failure(
                        session_id,
                        witness,
                        completions,
                        publication_handle,
                        held_mutation_gate,
                        input_id,
                        error,
                        fallback_wake,
                    )
                    .await;
                return Err(error);
            }
        };

        let prepared = match prepared {
            RetryableLiveBoundaryPreparation::Prepared(prepared) => prepared,
            RetryableLiveBoundaryPreparation::Unavailable { reason } => {
                held_mutation_gate = self
                    .normalize_live_boundary_queued_fallback(
                        session_id,
                        witness,
                        completions,
                        publication_handle.clone(),
                        held_mutation_gate,
                        input_id,
                        fallback_wake,
                    )
                    .await?;
                tracing::debug!(
                    session_id = %session_id,
                    run_id = %run_id,
                    input_id = %input_id,
                    reason = %reason,
                    "exact live boundary unavailable; normalized durable queued fallback"
                );
                return Ok((
                    held_mutation_gate,
                    LiveBoundaryInputDisposition::QueuedFallback,
                ));
            }
            RetryableLiveBoundaryPreparation::Stale { reason } => {
                held_mutation_gate = self
                    .normalize_live_boundary_queued_fallback(
                        session_id,
                        witness,
                        completions,
                        publication_handle.clone(),
                        held_mutation_gate,
                        input_id,
                        fallback_wake,
                    )
                    .await?;
                tracing::debug!(
                    session_id = %session_id,
                    run_id = %run_id,
                    input_id = %input_id,
                    reason = %reason,
                    "exact live boundary became stale; normalized durable queued fallback"
                );
                return Ok((
                    held_mutation_gate,
                    LiveBoundaryInputDisposition::QueuedFallback,
                ));
            }
        };

        let session_snapshot = prepared.session_snapshot().map(|snapshot| {
            meerkat_core::lifecycle::core_executor::BoundSessionCommit::untyped(snapshot.to_vec())
        });
        let realization = {
            let mut driver = witness.driver.lock().await;
            driver
                .machine_realize_live_boundary_context_injected(
                    &run_id,
                    std::slice::from_ref(input_id),
                    session_snapshot,
                    session_id,
                )
                .await
        };
        if let Err(error) = realization {
            // Driver implementations roll this atomic realization back to the
            // queued checkpoint on failure. Abort the parked session candidate
            // before terminalizing the exact accepted input; a store/driver
            // fault must never be laundered into the wake-and-replay path.
            drop(prepared);
            let error = self
                .finish_live_boundary_failure(
                    session_id,
                    witness,
                    completions,
                    publication_handle,
                    held_mutation_gate,
                    input_id,
                    error,
                    fallback_wake,
                )
                .await;
            return Err(error);
        }

        // This synchronous call is the session-side publication linearization
        // point. Nothing fallible may intervene after the durable machine/store
        // commit above. Under the exact parked witness it is invariant-infallible.
        if let Err(error) = prepared.commit() {
            // The durable realization already consumed the input. A wake can
            // no longer recover it and must not be mistaken for queued
            // fallback. Persist the exact finalization-failure result before
            // returning the violated-witness error.
            fallback_wake.disarm();
            let detail = format!(
                "durable live-boundary commit for input {input_id} lost its exact session publication authority: {error}"
            );
            let finalization_error =
                meerkat_core::TurnErrorMetadata::runtime_apply_failure(detail.clone());
            return match Self::finalize_live_boundary_completion_owned(
                &witness.driver,
                completions,
                input_id.clone(),
                run_id,
                crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Failed,
                Some(finalization_error),
            )
            .await
            {
                Ok(()) => Err(RuntimeDriverError::Internal(detail)),
                Err(receipt_error) => Err(RuntimeDriverError::Internal(format!(
                    "{detail}; exact completion receipt finalization also failed: {receipt_error}"
                ))),
            };
        }
        fallback_wake.disarm();

        Self::finalize_live_boundary_completion_owned(
            &witness.driver,
            completions,
            input_id.clone(),
            run_id,
            crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
            None,
        )
        .await?;
        Ok((
            held_mutation_gate,
            LiveBoundaryInputDisposition::ExactInjected,
        ))
    }

    async fn require_directed_terminal_publication_capability(
        &self,
        session_id: &SessionId,
        input: &Input,
    ) -> Result<(), RuntimeDriverError> {
        let directed = crate::input::validated_directed_interaction_id(input)
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?
            .is_some();
        if !directed {
            return Ok(());
        }
        let publication_available = self
            .sessions
            .read()
            .await
            .get(session_id)
            .and_then(RuntimeSessionEntry::publication_handle)
            .is_some();
        if publication_available {
            Ok(())
        } else {
            Err(RuntimeDriverError::ValidationFailed {
                reason: format!(
                    "session {session_id} cannot accept directed input without a terminal publication capability"
                ),
            })
        }
    }

    fn classify_ingress_dsl_rejection(state: RuntimeState, reason: String) -> RuntimeDriverError {
        match crate::meerkat_machine::classify_runtime_lifecycle_state(state) {
            Ok(facts) => match facts.ingress_admission {
                crate::meerkat_machine::dsl::RuntimeIngressAdmission::Destroyed => {
                    RuntimeDriverError::Destroyed
                }
                crate::meerkat_machine::dsl::RuntimeIngressAdmission::NotReady => {
                    RuntimeDriverError::NotReady { state }
                }
                crate::meerkat_machine::dsl::RuntimeIngressAdmission::Open => {
                    RuntimeDriverError::ValidationFailed { reason }
                }
            },
            Err(error) => RuntimeDriverError::Internal(error),
        }
    }

    fn reject_visible_terminal_ingress(state: RuntimeState) -> Result<(), RuntimeDriverError> {
        let facts =
            crate::meerkat_machine::classify_runtime_lifecycle_state(state).map_err(|reason| {
                RuntimeDriverError::Internal(format!(
                    "generated runtime ingress admission classification failed for {state}: {reason}"
                ))
            })?;
        match facts.ingress_admission {
            crate::meerkat_machine::dsl::RuntimeIngressAdmission::Destroyed => {
                Err(RuntimeDriverError::Destroyed)
            }
            crate::meerkat_machine::dsl::RuntimeIngressAdmission::NotReady => {
                Err(RuntimeDriverError::NotReady { state })
            }
            crate::meerkat_machine::dsl::RuntimeIngressAdmission::Open => Ok(()),
        }
    }

    pub(super) async fn reject_unregistration_drain_ingress(
        &self,
        session_id: &SessionId,
        state: RuntimeState,
    ) -> Result<(), RuntimeDriverError> {
        let dsl_state = self.session_dsl_state(session_id).await.map_err(|reason| {
            RuntimeDriverError::Internal(format!(
                "failed to read generated registration phase for ingress admission: {reason}"
            ))
        })?;
        if dsl_state.registration_phase == crate::meerkat_machine::dsl::RegistrationPhase::Draining
        {
            return Err(RuntimeDriverError::NotReady { state });
        }
        Ok(())
    }

    /// Preview generated admission feedback for shell capacity mechanics.
    ///
    /// The caller does not classify policy/defaults itself; it only observes
    /// whether generated `ResolveAdmissionPlan` would ask the runtime to wake,
    /// interrupt, or process immediately.
    pub async fn input_requires_active_pre_admission(
        &self,
        session_id: &SessionId,
        input: &Input,
    ) -> Result<bool, RuntimeDriverError> {
        self.input_requires_active_pre_admission_with_wake_policy(session_id, input, false)
            .await
    }

    /// Preview generated admission feedback for no-wake shell capacity mechanics.
    ///
    /// This mirrors `accept_input_without_wake`: the shell supplies only the
    /// command mode, while generated `ResolveAdmissionPlan` owns the semantic
    /// pre-admission answer.
    pub async fn input_requires_active_pre_admission_without_wake(
        &self,
        session_id: &SessionId,
        input: &Input,
    ) -> Result<bool, RuntimeDriverError> {
        self.input_requires_active_pre_admission_with_wake_policy(session_id, input, true)
            .await
    }

    async fn input_requires_active_pre_admission_with_wake_policy(
        &self,
        session_id: &SessionId,
        input: &Input,
        without_wake: bool,
    ) -> Result<bool, RuntimeDriverError> {
        let driver = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            entry.driver.clone()
        };
        let gate_guard = self
            .lock_current_session_driver_gate(session_id, &driver)
            .await?;

        let visible_state = self
            .existing_session_visible_runtime_state(session_id)
            .await
            .unwrap_or(RuntimeState::Destroyed);
        Self::reject_visible_terminal_ingress(visible_state)?;
        let _gate_guard = gate_guard;

        let driver = driver.lock().await;
        let resolved = if without_wake {
            driver.resolve_admission_without_wake_with_active_turn_boundary(input, false)?
        } else {
            driver.resolve_admission_with_active_turn_boundary(input, false)?
        };
        Ok(resolved.requires_active_runtime_pre_admission())
    }

    /// Execute the complete ingress transaction on the process-owned machine
    /// runtime. Once this method has accepted the command, dropping the caller
    /// may discard only the acknowledgement: driver admission, completion
    /// registration, generated DSL publication, effect convergence, and wake
    /// are allowed to finish as one owned transaction.
    pub(super) async fn execute_meerkat_machine_ingress_command(
        &self,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        let spawner = MachineCleanupTaskSpawner::acquire()?;
        let machine = self.clone();
        spawner
            .spawn(async move {
                machine
                    .execute_meerkat_machine_ingress_command_owned(command)
                    .await
            })
            .await
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "process-owned ingress transaction ended without a result: {error}"
                ))
            })?
    }

    async fn execute_meerkat_machine_ingress_command_owned(
        &self,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        match command {
            MeerkatMachineCommand::AcceptWithCompletion {
                session_id,
                input,
                register_completion,
                member_residency,
                expected_attachment,
            } => {
                if let Some(expected_attachment) = expected_attachment.as_ref()
                    && (!expected_attachment.belongs_to(self)
                        || expected_attachment.session_id() != &session_id)
                {
                    return Err(RuntimeDriverError::StaleAuthority {
                        reason: format!(
                            "input admission attachment witness does not belong to runtime session '{session_id}'"
                        ),
                    });
                }
                let _member_residency_lease = match &member_residency {
                    MemberResidencyExpectation::Unfenced => None,
                    MemberResidencyExpectation::PeerOnly => Some(
                        self.acquire_member_effect_authority_lease(&session_id, None)
                            .await?,
                    ),
                    MemberResidencyExpectation::Placed(expected) => Some(
                        self.acquire_member_effect_authority_lease(&session_id, Some(expected))
                            .await?,
                    ),
                };
                tracing::debug!(
                    session_id = %session_id,
                    input_id = %input.id(),
                    register_completion,
                    "MeerkatMachine::AcceptWithCompletion loading session entry"
                );
                let (driver, completions) = {
                    let sessions = self.sessions.read().await;
                    let entry = sessions
                        .get(&session_id)
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?;
                    (entry.driver.clone(), entry.completions.clone())
                };
                tracing::debug!(
                    session_id = %session_id,
                    input_id = %input.id(),
                    "MeerkatMachine::AcceptWithCompletion loaded session entry"
                );

                tracing::debug!(
                    session_id = %session_id,
                    input_id = %input.id(),
                    "MeerkatMachine::AcceptWithCompletion resolving mutation gate"
                );
                let (gate, preacquired_gate_guard) = match &_member_residency_lease {
                    Some(lease) => (Arc::clone(&lease.session_mutation_gate), None),
                    None => {
                        let guard = self
                            .lock_current_durability_ready_session_mutation_gate(&session_id)
                            .await?;
                        let gate = {
                            let sessions = self.sessions.read().await;
                            let entry =
                                sessions
                                    .get(&session_id)
                                    .ok_or(RuntimeDriverError::NotReady {
                                        state: RuntimeState::Destroyed,
                                    })?;
                            Arc::clone(&entry.mutation_gate)
                        };
                        (gate, Some(guard))
                    }
                };
                #[cfg(test)]
                if _member_residency_lease.is_some() {
                    let test_gate = self
                        .test_fenced_accept_after_lease
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take();
                    if let Some((acquired, release)) = test_gate {
                        let _ = acquired.send(());
                        let _ = release.await;
                    }
                }
                tracing::debug!(
                    session_id = %session_id,
                    input_id = %input.id(),
                    has_gate = true,
                    "MeerkatMachine::AcceptWithCompletion resolved mutation gate"
                );
                let mut gate_guard = match preacquired_gate_guard {
                    Some(guard) => Some(guard),
                    None => Some(Arc::clone(&gate).lock_owned().await),
                };
                let hook_input = crate::hook_observation::RuntimeInputHookFacts::from_input(&input);
                let (
                    wake_tx,
                    effect_tx,
                    boundary_handle,
                    attachment_id,
                    dsl_authority,
                    publication_handle,
                    post_commit_hooks,
                ) = {
                    let sessions = self.sessions.read().await;
                    let entry =
                        sessions
                            .get(&session_id)
                            .ok_or(RuntimeDriverError::StaleAuthority {
                                reason: format!(
                                    "fenced input admission lost runtime session '{session_id}'"
                                ),
                            })?;
                    if !Arc::ptr_eq(&entry.mutation_gate, &gate)
                        || !Arc::ptr_eq(&entry.driver, &driver)
                    {
                        return Err(RuntimeDriverError::StaleAuthority {
                            reason: format!(
                                "fenced input admission runtime session '{session_id}' was replaced"
                            ),
                        });
                    }
                    entry.require_durability_ready().map_err(|required| {
                        RuntimeDriverError::RecoveryRepairBlocked {
                            evidence_digest: None,
                            reason: required.to_string(),
                        }
                    })?;
                    if let Some(expected_attachment) = expected_attachment.as_ref() {
                        let exact_attachment = entry.epoch_id == expected_attachment.epoch_id
                            && matches!(
                                &entry.attachment_slot,
                                RuntimeLoopAttachmentSlot::Attached(attachment)
                                    if attachment.id == expected_attachment.attachment_id
                                        && !attachment.wake_tx.is_closed()
                                        && !attachment.effect_tx.is_closed()
                            );
                        if !exact_attachment {
                            return Err(RuntimeDriverError::StaleAuthority {
                                reason: format!(
                                    "runtime executor attachment changed before input admission for session '{session_id}'"
                                ),
                            });
                        }
                    }
                    (
                        entry.wake_sender(),
                        entry.effect_sender(),
                        entry.boundary_handle(),
                        entry.live_attachment_id(),
                        Arc::clone(&entry.dsl_authority),
                        entry.publication_handle(),
                        Arc::clone(&entry.post_commit_hooks),
                    )
                };
                tracing::debug!(
                    session_id = %session_id,
                    input_id = %input.id(),
                    "MeerkatMachine::AcceptWithCompletion acquired mutation gate"
                );

                tracing::debug!(
                    session_id = %session_id,
                    input_id = %input.id(),
                    "MeerkatMachine::AcceptWithCompletion reading runtime state"
                );
                let state = self
                    .existing_session_runtime_state(&session_id)
                    .await
                    .unwrap_or(RuntimeState::Destroyed);
                let visible_state = self
                    .existing_session_visible_runtime_state(&session_id)
                    .await
                    .unwrap_or(RuntimeState::Destroyed);
                tracing::debug!(
                    session_id = %session_id,
                    input_id = %input.id(),
                    runtime_state = ?state,
                    visible_state = ?visible_state,
                    "MeerkatMachine::AcceptWithCompletion read runtime state"
                );
                if let Err(error) = Self::reject_visible_terminal_ingress(visible_state) {
                    crate::hook_observation::dispatch_runtime_input_error(
                        &post_commit_hooks,
                        &hook_input,
                        &error,
                    );
                    return Err(error);
                }
                if let Err(error) = self
                    .reject_unregistration_drain_ingress(&session_id, state)
                    .await
                {
                    crate::hook_observation::dispatch_runtime_input_error(
                        &post_commit_hooks,
                        &hook_input,
                        &error,
                    );
                    return Err(error);
                }
                if let Err(error) = self
                    .require_directed_terminal_publication_capability(&session_id, &input)
                    .await
                {
                    crate::hook_observation::dispatch_runtime_input_error(
                        &post_commit_hooks,
                        &hook_input,
                        &error,
                    );
                    return Err(error);
                }
                // Acquire process-owned execution before mutating either the
                // driver or generated DSL. Once admission commits, handing
                // actor-bound work off must be infallible.
                let cleanup_spawner = MachineCleanupTaskSpawner::acquire()?;

                // This observation selects the generated live-boundary plan;
                // it is not authority to publish. The exact post-admission
                // transaction below revalidates the attachment, run, and
                // queued input before committing request-only turn context.
                let active_turn_boundary_available =
                    Self::active_turn_boundary_candidate_available(
                        &driver,
                        boundary_handle.is_some(),
                        attachment_id.is_some(),
                    )
                    .await;

                let (
                    flags,
                    stages_run_boundary,
                    outcome,
                    handle,
                    accepted_input_id,
                    signal,
                    mut fallback_wake,
                ) = {
                    let mut driver = driver.lock().await;
                    // origin/main observability: surface the idle/running
                    // disposition (idle, attached non-steer, or attached peer)
                    // for ingress-admission diagnostics, including the
                    // remote-comms / paired-TCP peer admission path. The
                    // semantic decision itself is NOT taken here: it flows
                    // through the canonical machine seam
                    // (resolve_admission_with_active_turn_boundary), which
                    // derives idle vs running internally from the recovered
                    // authority plus active_turn_boundary_available (P0
                    // Invariant 1 — no handwritten admission resolution).
                    let input_kind = input.kind();
                    let runtime_idle = !active_turn_boundary_available
                        && (state == RuntimeState::Idle
                            || (state == RuntimeState::Attached
                                && !matches!(
                                    input.handling_mode(),
                                    Some(meerkat_core::types::HandlingMode::Steer)
                                ))
                            || (state == RuntimeState::Attached
                                && matches!(input, Input::Peer(_))));
                    tracing::debug!(
                        session_id = %session_id,
                        input_kind = ?input_kind,
                        runtime_state = ?state,
                        visible_state = ?visible_state,
                        runtime_idle,
                        active_turn_boundary_available,
                        "resolving runtime ingress admission via canonical machine seam"
                    );
                    let resolved = match driver.resolve_admission_with_active_turn_boundary(
                        &input,
                        active_turn_boundary_available,
                    ) {
                        Ok(resolved) => resolved,
                        Err(error) => {
                            crate::hook_observation::dispatch_runtime_input_error(
                                &post_commit_hooks,
                                &hook_input,
                                &error,
                            );
                            return Err(error);
                        }
                    };
                    let flags = resolved.coarse_flags();
                    let stages_run_boundary = resolved.stages_run_boundary();
                    self.preview_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::AcceptWithCompletion {
                            input_id: crate::meerkat_machine::dsl::InputId::from_domain(
                                &InputId::new(),
                            ),
                            request_immediate_processing: flags.request_immediate_processing,
                            interrupt_yielding: flags.interrupt_yielding,
                            wake_if_idle: flags.wake_if_idle,
                        },
                        "AcceptWithCompletion",
                    )
                    .await
                    .map_err(|reason| {
                        let reason = format!(
                            "{reason}; input_kind={}; immediate={}; interrupt_yielding={}; wake_if_idle={}",
                            input.kind(),
                            flags.request_immediate_processing,
                            flags.interrupt_yielding,
                            flags.wake_if_idle,
                        );
                        Self::classify_ingress_dsl_rejection(state, reason)
                    })?;
                    let result = match driver
                        .accept_resolved_input(input, resolved)
                        .await
                        .map_err(Self::normalize_destroyed_error)
                    {
                        Ok(r) => r,
                        Err(err) => return Err(err),
                    };

                    match &result {
                        AcceptOutcome::Accepted { input_id, seed, .. } => {
                            let accepted_input_id = input_id.clone();
                            let fallback_wake = AcceptedIngressFallbackWakeGuard::new(
                                wake_tx.clone(),
                                flags.request_immediate_processing
                                    || flags.interrupt_yielding
                                    || flags.wake_if_idle,
                            );
                            let is_terminal =
                                crate::meerkat_machine::input_seed_behavioral_terminality_via_authority(
                                    &accepted_input_id,
                                    seed,
                                )
                                .map_err(RuntimeDriverError::Internal)?;
                            let handle = if is_terminal || !register_completion {
                                None
                            } else {
                                Some({
                                    let mut completions = completions.lock().await;
                                    completions.register(accepted_input_id.clone())
                                })
                            };
                            (
                                flags,
                                stages_run_boundary,
                                result,
                                handle,
                                Some(accepted_input_id),
                                crate::driver::ephemeral::PostAdmissionSignal::None,
                                fallback_wake,
                            )
                        }
                        AcceptOutcome::Deduplicated {
                            existing_id,
                            existing_seed,
                            ..
                        } => {
                            let is_terminal =
                                crate::meerkat_machine::input_seed_behavioral_terminality_via_authority(
                                    existing_id,
                                    existing_seed,
                                )
                                .map_err(RuntimeDriverError::Internal)?;

                            if is_terminal || !register_completion {
                                (
                                    flags,
                                    stages_run_boundary,
                                    result,
                                    None,
                                    None,
                                    crate::driver::ephemeral::PostAdmissionSignal::None,
                                    AcceptedIngressFallbackWakeGuard::new(None, false),
                                )
                            } else {
                                let handle = {
                                    let mut completions = completions.lock().await;
                                    completions.register(existing_id.clone())
                                };
                                (
                                    flags,
                                    stages_run_boundary,
                                    result,
                                    Some(handle),
                                    None,
                                    crate::driver::ephemeral::PostAdmissionSignal::None,
                                    AcceptedIngressFallbackWakeGuard::new(None, false),
                                )
                            }
                        }
                        AcceptOutcome::Rejected { reason } => {
                            crate::hook_observation::dispatch_runtime_input_outcome(
                                &post_commit_hooks,
                                &hook_input,
                                &result,
                            );
                            return Err(RuntimeDriverError::ValidationFailed {
                                reason: reason.to_string(),
                            });
                        }
                    }
                };
                let (signal, cancel_plan) = if let Some(input_id) = accepted_input_id.clone() {
                    let staged = self
                        .stage_session_dsl_transition(
                            &session_id,
                            crate::meerkat_machine::dsl::MeerkatMachineInput::AcceptWithCompletion {
                                input_id: crate::meerkat_machine::dsl::InputId::from_domain(
                                    &input_id,
                                ),
                                request_immediate_processing: flags.request_immediate_processing,
                                interrupt_yielding: flags.interrupt_yielding,
                                wake_if_idle: flags.wake_if_idle,
                            },
                            "AcceptWithCompletion",
                        )
                        .await
                        .map_err(|reason| {
                            RuntimeDriverError::Internal(format!(
                                "canonical AcceptWithCompletion stage failed after admission: {reason}"
                            ))
                        })?;
                    let effects = staged.effects.clone();
                    let signal = Self::post_admission_signal_from_effects(&effects);
                    let runtime_effect =
                        crate::effect::runtime_effect_projection_optional_from_dsl_effects(
                            &effects,
                        )
                        .map_err(|reason| {
                            RuntimeDriverError::Internal(format!(
                                "canonical AcceptWithCompletion emitted invalid runtime effect facts: {reason}"
                            ))
                        })?;
                    // Build the exact abort guard before the first await after
                    // the DSL transition minted its pending dispatch fact.
                    let cancel_plan = runtime_effect
                        .map(|projected_effect| {
                            let committed_state = staged.committed_snapshot.state();
                            let expected_run_id = committed_state
                                .current_run_id
                                .as_ref()
                                .and_then(
                                    crate::meerkat_machine::dsl_authority::current_run_id_from_dsl,
                                )
                                .ok_or_else(|| {
                                    RuntimeDriverError::Internal(
                                        "AcceptWithCompletion emitted boundary cancel without a valid exact active run id"
                                            .to_string(),
                                    )
                                })?;
                            let dispatch_generation =
                                committed_state.boundary_cancel_dispatch_generation;
                            let dispatch_lifecycle_phase = committed_state.lifecycle_phase;
                            let pending_dispatch = PendingBoundaryCancelDispatchGuard::new(
                                Arc::clone(&dsl_authority),
                                dispatch_generation,
                            );
                            let effect_tx = effect_tx.clone().ok_or(
                                RuntimeDriverError::NotReady {
                                    state: RuntimeState::Destroyed,
                                },
                            )?;
                            let attachment_id = attachment_id.ok_or_else(|| {
                                RuntimeDriverError::StaleAuthority {
                                    reason: format!(
                                        "AcceptWithCompletion lost the runtime attachment for session '{session_id}'"
                                    ),
                                }
                            })?;
                            Ok(RuntimeAcceptedBoundaryCancelPlan {
                                witness: RuntimeEffectDispatchAttachmentWitness {
                                    mutation_gate: Arc::clone(&gate),
                                    driver: driver.clone(),
                                    dsl_authority: Arc::clone(&dsl_authority),
                                    attachment_id,
                                    effect_tx,
                                },
                                boundary_handle: boundary_handle.clone(),
                                pending_dispatch,
                                expected_run_id,
                                projected_effect,
                                dispatch_generation,
                                dispatch_lifecycle_phase,
                            })
                        })
                        .transpose()?;
                    self.commit_session_dsl_transition(
                        &session_id,
                        staged,
                        "AcceptWithCompletion",
                    )
                    .await
                    .map_err(|reason| {
                        RuntimeDriverError::Internal(format!(
                            "canonical AcceptWithCompletion effect dispatch failed after admission: {reason}"
                        ))
                    })?;
                    {
                        let mut driver = driver.lock().await;
                        driver.absorb_post_admission_effects(&effects);
                    }
                    (signal, cancel_plan)
                } else {
                    (signal, None)
                };
                crate::hook_observation::dispatch_runtime_input_outcome(
                    &post_commit_hooks,
                    &hook_input,
                    &outcome,
                );

                let live_boundary_plan = if signal.should_interrupt_yielding()
                    && stages_run_boundary
                    && let (Some(input_id), Some(boundary_handle), Some(attachment_id)) = (
                        accepted_input_id.as_ref(),
                        boundary_handle.clone(),
                        attachment_id,
                    ) {
                    Some(RuntimeAcceptedLiveBoundaryPlan {
                        witness: RuntimeLiveBoundaryAttachmentWitness {
                            mutation_gate: Arc::clone(&gate),
                            driver: driver.clone(),
                            dsl_authority: Arc::clone(&dsl_authority),
                            attachment_id,
                            boundary_handle,
                        },
                        input_id: input_id.clone(),
                        publication_handle: publication_handle.clone(),
                    })
                } else {
                    None
                };

                if accepted_input_id.is_some() {
                    let held_mutation_gate = gate_guard.take().ok_or_else(|| {
                        RuntimeDriverError::Internal(
                            "AcceptWithCompletion lost its held session mutation gate before asynchronous boundary work"
                                .to_string(),
                        )
                    })?;
                    self.spawn_accepted_ingress_boundary_work(
                        cleanup_spawner,
                        &session_id,
                        held_mutation_gate,
                        live_boundary_plan,
                        cancel_plan,
                        completions.clone(),
                        wake_tx,
                        signal.should_wake(),
                        fallback_wake,
                    );
                } else {
                    fallback_wake.disarm();
                    drop(gate_guard.take());
                }

                Ok(MeerkatMachineCommandResult::AcceptWithCompletion {
                    outcome,
                    handle,
                    admission_signal: signal,
                })
            }
            MeerkatMachineCommand::AcceptWithoutWake { session_id, input } => {
                let (driver, completions, mutation_gate, publication_handle, post_commit_hooks) = {
                    let sessions = self.sessions.read().await;
                    let entry = sessions
                        .get(&session_id)
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?;
                    (
                        entry.driver.clone(),
                        entry.completions.clone(),
                        Arc::clone(&entry.mutation_gate),
                        entry.publication_handle(),
                        Arc::clone(&entry.post_commit_hooks),
                    )
                };
                let hook_input = crate::hook_observation::RuntimeInputHookFacts::from_input(&input);
                let gate_guard = self
                    .lock_current_session_driver_gate(&session_id, &driver)
                    .await?;

                let state = self
                    .existing_session_runtime_state(&session_id)
                    .await
                    .unwrap_or(RuntimeState::Destroyed);
                let visible_state = self
                    .existing_session_visible_runtime_state(&session_id)
                    .await
                    .unwrap_or(RuntimeState::Destroyed);
                if let Err(error) = Self::reject_visible_terminal_ingress(visible_state) {
                    crate::hook_observation::dispatch_runtime_input_error(
                        &post_commit_hooks,
                        &hook_input,
                        &error,
                    );
                    return Err(error);
                }
                if let Err(error) = self
                    .reject_unregistration_drain_ingress(&session_id, state)
                    .await
                {
                    crate::hook_observation::dispatch_runtime_input_error(
                        &post_commit_hooks,
                        &hook_input,
                        &error,
                    );
                    return Err(error);
                }
                if let Err(error) = self
                    .require_directed_terminal_publication_capability(&session_id, &input)
                    .await
                {
                    crate::hook_observation::dispatch_runtime_input_error(
                        &post_commit_hooks,
                        &hook_input,
                        &error,
                    );
                    return Err(error);
                }
                let (outcome, accepted_input_id) = {
                    let mut driver = driver.lock().await;
                    let resolved = match driver
                        .resolve_admission_without_wake_with_active_turn_boundary(&input, false)
                    {
                        Ok(resolved) => resolved,
                        Err(error) => {
                            crate::hook_observation::dispatch_runtime_input_error(
                                &post_commit_hooks,
                                &hook_input,
                                &error,
                            );
                            return Err(error);
                        }
                    };
                    let preview = self
                        .preview_session_dsl_input(
                            &session_id,
                            crate::meerkat_machine::dsl::MeerkatMachineInput::AcceptWithoutWake {
                                input_id: crate::meerkat_machine::dsl::InputId::from_domain(
                                    &InputId::new(),
                                ),
                            },
                            "AcceptWithoutWake",
                        )
                        .await
                        .map_err(|reason| Self::classify_ingress_dsl_rejection(state, reason));
                    if let Err(error) = preview {
                        crate::hook_observation::dispatch_runtime_input_error(
                            &post_commit_hooks,
                            &hook_input,
                            &error,
                        );
                        return Err(error);
                    }
                    let result = match driver
                        .accept_resolved_input(input, resolved)
                        .await
                        .map_err(Self::normalize_destroyed_error)
                    {
                        Ok(r) => r,
                        Err(err) => return Err(err),
                    };
                    if let AcceptOutcome::Rejected { reason } = &result {
                        crate::hook_observation::dispatch_runtime_input_outcome(
                            &post_commit_hooks,
                            &hook_input,
                            &result,
                        );
                        return Err(RuntimeDriverError::ValidationFailed {
                            reason: reason.to_string(),
                        });
                    }
                    let accepted_input_id = match &result {
                        AcceptOutcome::Accepted { input_id, .. } => Some(input_id.clone()),
                        AcceptOutcome::Deduplicated { .. } => None,
                        AcceptOutcome::Rejected { .. } => unreachable!("handled above"),
                    };
                    (result, accepted_input_id)
                };
                if let Some(input_id) = accepted_input_id {
                    let effects = match self
                        .apply_session_dsl_input(
                            &session_id,
                            crate::meerkat_machine::dsl::MeerkatMachineInput::AcceptWithoutWake {
                                input_id: crate::meerkat_machine::dsl::InputId::from_domain(
                                    &input_id,
                                ),
                            },
                            "AcceptWithoutWake",
                        )
                        .await
                    {
                        Ok((_, effects)) => effects,
                        Err(reason) => {
                            let primary = RuntimeDriverError::Internal(format!(
                                "canonical AcceptWithoutWake apply failed after admission: {reason}"
                            ));
                            let primary_reason = primary.to_string();
                            match self
                                .terminalize_failed_accepted_input(
                                    &session_id,
                                    &driver,
                                    &completions,
                                    publication_handle,
                                    &mutation_gate,
                                    gate_guard,
                                    &input_id,
                                    primary_reason.clone(),
                                )
                                .await
                            {
                                Ok(()) => return Err(primary),
                                Err(terminalization_error) => {
                                    let remains_queued = driver
                                        .lock()
                                        .await
                                        .as_driver()
                                        .stored_input_state(&input_id)
                                        .is_some_and(|stored| {
                                            stored.seed.phase == InputLifecycleState::Queued
                                        });
                                    if remains_queued {
                                        // The durable admission still belongs
                                        // to the caller. Returning its exact
                                        // accepted outcome is the only safe
                                        // no-wake fallback when terminal
                                        // transfer itself could not commit.
                                        tracing::error!(
                                            %session_id,
                                            %input_id,
                                            %terminalization_error,
                                            "AcceptWithoutWake DSL publication failed and exact terminal transfer failed; returning accepted authority"
                                        );
                                        crate::hook_observation::dispatch_runtime_input_outcome(
                                            &post_commit_hooks,
                                            &hook_input,
                                            &outcome,
                                        );
                                        return Ok(MeerkatMachineCommandResult::AcceptOutcome(
                                            outcome,
                                        ));
                                    }
                                    return Err(RuntimeDriverError::Internal(format!(
                                        "{primary_reason}; exact accepted-input terminalization also failed: {terminalization_error}"
                                    )));
                                }
                            }
                        }
                    };
                    {
                        let mut driver = driver.lock().await;
                        driver.absorb_post_admission_effects(&effects);
                    }
                    let signal = Self::post_admission_signal_from_effects(&effects);
                    debug_assert!(
                        !signal.should_wake()
                            && !signal.should_interrupt_yielding()
                            && !signal.should_process_immediately(),
                        "AcceptWithoutWake unexpectedly emitted a post-admission signal"
                    );
                }

                crate::hook_observation::dispatch_runtime_input_outcome(
                    &post_commit_hooks,
                    &hook_input,
                    &outcome,
                );
                Ok(MeerkatMachineCommandResult::AcceptOutcome(outcome))
            }
            _ => unreachable!("non-ingress command routed to ingress handler"),
        }
    }
}
