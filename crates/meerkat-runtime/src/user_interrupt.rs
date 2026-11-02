use std::sync::Arc;

use meerkat_core::lifecycle::CoreExecutorInterruptHandle;
use meerkat_core::types::SessionId;

use crate::meerkat_machine::MeerkatMachine;
use crate::runtime_state::RuntimeState;
use crate::traits::RuntimeDriverError;

/// What a run-fenced stop dispatch observed and registered under the session
/// mutation gate.
#[derive(Default)]
pub(super) struct RunStopCapture {
    /// The machine's current run at the compare.
    pub(super) current_run_id: Option<meerkat_core::RunId>,
    /// Whether `StopCurrentRunForRun` committed for the expected run.
    pub(super) staged: bool,
    /// The runtime phase observed when generated authority refused the stop
    /// after the compare had matched.
    pub(super) refused_state: Option<RuntimeState>,
    /// One completion waiter per input staged for the stopped run.
    pub(super) contributors: Vec<(
        meerkat_core::lifecycle::InputId,
        crate::completion::CompletionHandle,
    )>,
}

impl MeerkatMachine {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn reconcile_user_interrupt_dispatch(
        &self,
        session_id: &SessionId,
        dispatch_id: uuid::Uuid,
        expected_run_id: &meerkat_core::RunId,
        captured_gate: &crate::meerkat_machine::SessionMutationGate,
        captured_authority: &Arc<
            std::sync::Mutex<crate::meerkat_machine::dsl::MeerkatMachineAuthority>,
        >,
        attachment_id: Option<crate::meerkat_machine::RuntimeLoopAttachmentId>,
        provisional_claim_id: Option<uuid::Uuid>,
        interrupt_handle: &Arc<dyn CoreExecutorInterruptHandle>,
        result_tx: &crate::tokio::sync::watch::Sender<Option<Result<bool, RuntimeDriverError>>>,
        expected_member: Option<
            &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        >,
        callback_result: Result<bool, RuntimeDriverError>,
    ) -> Result<bool, RuntimeDriverError> {
        let member_lease = match expected_member {
            Some(expected_member) => match self
                .acquire_member_effect_authority_lease(session_id, Some(expected_member))
                .await
            {
                Ok(lease) => Some(lease),
                Err(_) => {
                    let result = Ok(false);
                    // The member is no longer current, so the answer does not
                    // need the gate; only clearing the dispatch slot does. A
                    // session that degraded while another holder keeps the
                    // gate keeps the slot until it is reloaded.
                    let Ok(_guard) = self
                        .lock_live_session_mutation_gate(session_id, captured_gate)
                        .await
                    else {
                        result_tx.send_replace(Some(result.clone()));
                        return result;
                    };
                    let mut sessions = self.sessions.write().await;
                    result_tx.send_replace(Some(result.clone()));
                    if let Some(entry) = sessions.get_mut(session_id)
                        && entry.mutation_gate.same_gate(captured_gate)
                        && entry
                            .pending_user_interrupt_dispatch
                            .as_ref()
                            .is_some_and(|pending| pending.dispatch_id == dispatch_id)
                    {
                        entry.pending_user_interrupt_dispatch = None;
                    }
                    return result;
                }
            },
            None => None,
        };
        let gate = match member_lease.as_ref() {
            Some(lease) => &lease.session_mutation_gate,
            None => captured_gate,
        };
        let gate_guard = match self.lock_live_session_mutation_gate(session_id, gate).await {
            Ok(guard) => guard,
            Err(required) => {
                // The session degraded while another holder keeps its gate:
                // the dispatch cannot be reconciled with its exact attachment
                // before the session is reloaded. The run's next
                // ready-gated wait starts the degraded settlement, which
                // hard-cancels the exact run.
                let result = Err(super::degraded_gate_wait_refusal(required));
                result_tx.send_replace(Some(result.clone()));
                return result;
            }
        };

        let exact_current = {
            let mut sessions = self.sessions.write().await;
            let Some(entry) = sessions.get_mut(session_id) else {
                let result = Ok(false);
                result_tx.send_replace(Some(result.clone()));
                return result;
            };
            let pending_matches = entry
                .pending_user_interrupt_dispatch
                .as_ref()
                .is_some_and(|pending| pending.dispatch_id == dispatch_id);
            let handle_matches = entry
                .interrupt_handle()
                .is_some_and(|current| Arc::ptr_eq(&current, interrupt_handle));
            let attachment_matches = entry.mutation_gate.same_gate(captured_gate)
                && Arc::ptr_eq(&entry.dsl_authority, captured_authority)
                && entry.live_attachment_id() == attachment_id
                && entry.provisional_materialization_claim_id == provisional_claim_id
                && handle_matches;
            pending_matches && attachment_matches
        };
        if !exact_current {
            let result = Ok(false);
            result_tx.send_replace(Some(result.clone()));
            return result;
        }
        if let (Some(lease), Some(expected_member)) = (&member_lease, expected_member)
            && let Err(error) = self.validate_member_effect_authority_lease_current(
                session_id,
                lease,
                Some(expected_member),
            )
        {
            let mut sessions = self.sessions.write().await;
            if let Some(entry) = sessions.get_mut(session_id)
                && entry
                    .pending_user_interrupt_dispatch
                    .as_ref()
                    .is_some_and(|pending| pending.dispatch_id == dispatch_id)
            {
                entry.pending_user_interrupt_dispatch = None;
            }
            result_tx.send_replace(Some(Err(error.clone())));
            return Err(error);
        }
        let run_is_current = {
            let authority = captured_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            matches!(
                crate::meerkat_machine::dsl_authority::runtime_phase_from_authority(&authority),
                RuntimeState::Running | RuntimeState::Retired
            ) && crate::meerkat_machine::dsl_authority::current_run_id_from_authority(&authority)
                .as_ref()
                == Some(expected_run_id)
        };
        let result = match (run_is_current, callback_result) {
            // The executor fences the callback to the exact run, and the
            // attachment, handle and dispatch slot still match. So a delivered
            // interrupt stays delivered even when the run has already reached
            // its terminal (typically because of this interrupt) before this
            // reconcile reacquired the gate.
            (_, Ok(true)) => Ok(true),
            (true, Ok(false)) => Err(RuntimeDriverError::InterruptDispatchOutcomeUnknown {
                run_id: expected_run_id.clone(),
                reason: "executor reported the exact run non-current while machine authority still binds it"
                    .to_string(),
            }),
            (true, Err(error)) => Err(error),
            (false, Ok(false) | Err(_)) => Ok(false),
        };
        // Only a delivered interrupt whose run is still bound keeps its slot
        // for same-run retries to join. Once the run has left machine
        // authority no retry can join it (the compare answers `false`), so the
        // slot is released like a failed dispatch.
        if run_is_current && matches!(result, Ok(true)) {
            result_tx.send_replace(Some(result.clone()));
        } else {
            let mut sessions = self.sessions.write().await;
            if let Some(entry) = sessions.get_mut(session_id)
                && entry
                    .pending_user_interrupt_dispatch
                    .as_ref()
                    .is_some_and(|pending| pending.dispatch_id == dispatch_id)
            {
                entry.pending_user_interrupt_dispatch = None;
            }
            // Publish while the sessions write lock is still held. A woken
            // retry cannot observe the result until the failed dispatch slot
            // is already gone, so it can safely reissue exactly once.
            result_tx.send_replace(Some(result.clone()));
        }
        drop(gate_guard);
        drop(member_lease);
        result
    }

    /// Hard-cancel the session's current run. A run whose session store's
    /// fence reads closed (a store fence gap) is stopped through a
    /// suspension instead, and a run marked resumable while no run is
    /// current (a gap's, or a restart's not resumed yet) has its stop
    /// recorded: either run never resumes, and its inputs end cancelled once
    /// the store takes writes again. Refused with `NotReady` when no run is
    /// current and none is marked.
    pub async fn hard_cancel_current_run(
        &self,
        session_id: &SessionId,
        reason: impl Into<String>,
    ) -> Result<(), RuntimeDriverError> {
        if self
            .dispatch_user_interrupt(session_id, None, None, reason.into())
            .await?
        {
            return Ok(());
        }

        let state = self
            .existing_session_runtime_state(session_id)
            .await
            .unwrap_or(RuntimeState::Destroyed);
        if state == RuntimeState::Destroyed {
            Err(RuntimeDriverError::Destroyed)
        } else {
            Err(RuntimeDriverError::NotReady { state })
        }
    }

    /// Assert a hard cancel only while `expected_run_id` remains the exact
    /// machine-owned current run.
    ///
    /// Returns `true` when the interrupt was delivered to that run and `false`
    /// when the run was already unbound/terminal (including when another run
    /// has since become current). The compare and interrupt admission share the
    /// per-session mutation gate, so a stale bridge retry cannot race the
    /// comparison and cancel a newer run.
    pub async fn hard_cancel_run_if_current(
        &self,
        session_id: &SessionId,
        expected_run_id: &meerkat_core::RunId,
        reason: impl Into<String>,
    ) -> Result<bool, RuntimeDriverError> {
        self.dispatch_user_interrupt(session_id, Some(expected_run_id), None, reason.into())
            .await
    }

    /// Stop `expected_run_id` atomically, terminalizing every contributor
    /// already bound to it.
    ///
    /// Under the session mutation gate this compares the machine's current
    /// run with `expected_run_id` and commits the generated
    /// `StopCurrentRunForRun`, then dispatches the exact-run hard interrupt.
    /// From that point the run admits no durable Steer join; at its terminal,
    /// a joined Steer whose append is not in the surviving image is abandoned
    /// as `Cancelled` instead of re-entering its lane, and a failed attempt
    /// never replays its staged batch. So no contributor of the stopped run
    /// starts a successor. A retained join is consumed with the run as usual.
    ///
    /// The call returns after every contributor staged at the stop reached
    /// its canonical terminal ([`crate::RunStopReceipt::Stopped`]). Once the
    /// stop is committed, an interrupt dispatch that fails, times out, or
    /// finds the executor not yet inside the run does not fail the call: the
    /// run still ends as a stopped run, and the contributors' terminals are
    /// the outcome. If the executor had not entered the run, the run executes
    /// until its own terminal before the call returns. A stop refused because
    /// a runtime stop or teardown took the run is
    /// [`crate::RunStopReceipt::NotStoppable`]. A late
    /// stop, whose run is no longer current, touches nothing and returns
    /// [`crate::RunStopReceipt::NotCurrent`]: queued input and newer runs are
    /// never interrupted. Input admitted but not joined to the run is not a
    /// contributor and stays queued.
    ///
    /// A run with no runtime-loop contributors (a direct service turn) is
    /// interrupted and reported as `Stopped` with no contributors.
    ///
    /// A run a store fence gap suspended (or one a restart marked resumable
    /// that has not resumed yet), and a run whose session store's fence
    /// reads closed while it runs, are stopped without waiting: the stop is
    /// recorded at once and the call returns `Stopped` with no contributors.
    /// Such a run never resumes. Its inputs end `Abandoned` with reason
    /// `Cancelled` when the runtime settles it (at its next lap for a
    /// restart's run, once the store takes writes again for a gap's): read
    /// their outcome later. A process that ends first, or another writer
    /// that takes the session over, runs them instead. A graceful requeue
    /// stop, or the Keep rollback of a start refused before it served
    /// ([`crate::UnregisterQueuedInputs::Keep`]), that begins before that
    /// settlement ends them `Abandoned` with reason `Stopped`, and leaves the
    /// run itself to the next start, which repairs its tail as an interrupted
    /// run; no run terminal is published for it. A stop that races the gap's
    /// own suspension of the run may fail
    /// with an internal error naming a contributor whose wait was answered
    /// `RequeuedAcrossStop`. A cancel at the next boundary accepted just
    /// before that suspension can likewise fail when its dispatch to the
    /// dropped turn fails, while the run is still settled as cancelled.
    ///
    /// At a store fence gap the stop falls back to the hard interrupt in
    /// three cases: the surface cannot suspend a run
    /// ([`meerkat_core::lifecycle::CoreExecutorInterruptHandle::suspend_run_if_current`]
    /// answers `false`), the stop joins a hard interrupt of the same run
    /// already in flight, or the machine refuses the suspension (a published
    /// live-boundary join not yet retained). The cancel is then realized only
    /// in memory: the gap refuses its writes, the session reloads, and the
    /// reload resumes the stopped run, although the call already answered
    /// `Stopped`.
    pub async fn stop_run(
        &self,
        session_id: &SessionId,
        expected_run_id: &meerkat_core::RunId,
        reason: impl Into<String>,
    ) -> Result<crate::run_stop::RunStopReceipt, RuntimeDriverError> {
        self.stop_run_inner(session_id, expected_run_id, None, reason.into())
            .await
    }

    /// [`Self::stop_run`] additionally pinned to one exact host-member
    /// residency, for the supervisor bridge's `StopMemberRun` receiver. The
    /// residency comparison and the stop commit share the session mutation
    /// gate.
    pub async fn stop_run_for_member_incarnation(
        &self,
        session_id: &SessionId,
        expected_run_id: &meerkat_core::RunId,
        expected_member: &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        reason: impl Into<String>,
    ) -> Result<crate::run_stop::RunStopReceipt, RuntimeDriverError> {
        self.stop_run_inner(
            session_id,
            expected_run_id,
            Some(expected_member),
            reason.into(),
        )
        .await
    }

    async fn stop_run_inner(
        &self,
        session_id: &SessionId,
        expected_run_id: &meerkat_core::RunId,
        expected_member: Option<
            &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        >,
        reason: String,
    ) -> Result<crate::run_stop::RunStopReceipt, RuntimeDriverError> {
        let mut capture = RunStopCapture::default();
        let dispatched = self
            .dispatch_user_interrupt_with_stop(
                session_id,
                Some(expected_run_id),
                expected_member,
                reason,
                Some(&mut capture),
            )
            .await;
        if !capture.staged {
            dispatched?;
            if let Some(state) = capture.refused_state
                && capture.current_run_id.as_ref() == Some(expected_run_id)
            {
                return Ok(crate::run_stop::RunStopReceipt::NotStoppable {
                    run_id: expected_run_id.clone(),
                    state,
                });
            }
            return Ok(crate::run_stop::RunStopReceipt::NotCurrent {
                run_id: expected_run_id.clone(),
                current_run_id: capture.current_run_id,
            });
        }
        // The stop is committed, so the run's contributors terminalize under
        // stop semantics whatever the interrupt dispatch reports. Their
        // terminals are the outcome; the dispatch result is diagnostic only.
        // `Ok(false)` means the run was no longer current at the executor
        // callback, or the executor had not entered it yet; an unknown or
        // failed dispatch leaves the run to reach its terminal on its own.
        // Either way the run ends as a stopped run: its unretained joins are
        // cancelled and a failed attempt is not replayed.
        match dispatched {
            Ok(true) => {}
            Ok(false) => tracing::debug!(
                %session_id,
                run_id = %expected_run_id,
                "committed run stop found no executor interrupt target; awaiting the run terminal"
            ),
            Err(error) => tracing::warn!(
                %session_id,
                run_id = %expected_run_id,
                %error,
                "committed run stop could not confirm its interrupt; awaiting the run terminal"
            ),
        }
        let mut contributors = Vec::with_capacity(capture.contributors.len());
        for (input_id, waiter) in capture.contributors {
            let (outcome, observed_terminal) = waiter
                .try_wait_with_terminal_outcome()
                .await
                .map_err(|error| {
                    if error.is_disclosure_refused() {
                        // The contributor completed; only its result is
                        // withheld from this reader.
                        return RuntimeDriverError::ScopedResultWithheld {
                            input_id: input_id.clone(),
                        };
                    }
                    RuntimeDriverError::Internal(format!(
                        "stopped run {expected_run_id} contributor {input_id} completion failed: {error}"
                    ))
                })?
                .into_parts();
            let terminal = match observed_terminal {
                Some(terminal) => Some(terminal),
                None => self.committed_input_terminal(session_id, &input_id).await,
            };
            contributors.push(crate::run_stop::RunStopContributor {
                input_id,
                outcome,
                terminal,
            });
        }
        Ok(crate::run_stop::RunStopReceipt::Stopped {
            run_id: expected_run_id.clone(),
            contributors,
        })
    }

    /// The committed terminal of one contributor: the live ledger first, then
    /// the durable row an archived input left in the store.
    async fn committed_input_terminal(
        &self,
        session_id: &SessionId,
        input_id: &meerkat_core::lifecycle::InputId,
    ) -> Option<crate::input_state::InputTerminalOutcome> {
        let target = {
            let sessions = self.sessions.read().await;
            sessions
                .get(session_id)
                .map(|entry| (entry.driver.clone(), entry.durability_health.clone()))
        };
        if let Some((driver, durability_health)) = target {
            match super::lock_driver_live(&driver, durability_health.as_ref()).await {
                Ok(driver) => {
                    if let Some(stored) = driver.as_driver().stored_input_state(input_id) {
                        return stored.seed.terminal_outcome;
                    }
                }
                // A degraded session whose driver a commit past its deadline
                // still holds: the terminal is not known until the store
                // answers, and the store read below would wait for it too.
                Err(_) => return None,
            }
        }
        let store = self.store.as_ref()?;
        match store
            .load_input_state(&Self::logical_runtime_id(session_id), input_id)
            .await
        {
            Ok(stored) => stored.and_then(|stored| stored.seed.terminal_outcome),
            Err(error) => {
                tracing::warn!(%session_id, %input_id, %error,
                    "stopped-run contributor terminal could not be read from the store");
                None
            }
        }
    }

    /// Run-fenced hard cancel additionally pinned to one exact host-member
    /// residency. Both comparisons and the interrupt stage share the same
    /// session mutation gate.
    pub(crate) async fn hard_cancel_run_if_current_for_member_incarnation(
        &self,
        session_id: &SessionId,
        expected_run_id: &meerkat_core::RunId,
        expected_member: &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        reason: impl Into<String>,
    ) -> Result<bool, RuntimeDriverError> {
        self.dispatch_user_interrupt(
            session_id,
            Some(expected_run_id),
            Some(expected_member),
            reason.into(),
        )
        .await
    }

    /// This machine's user-interrupt acknowledgement bound.
    fn user_interrupt_ack_timeout(&self) -> std::time::Duration {
        #[cfg(any(test, feature = "test-support"))]
        {
            *self
                .test_user_interrupt_ack_timeout
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }
        #[cfg(not(any(test, feature = "test-support")))]
        {
            crate::meerkat_machine::USER_INTERRUPT_ACK_TIMEOUT
        }
    }

    /// Shorten this machine's user-interrupt acknowledgement bound for a test
    /// that exercises a wedged executor callback.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_user_interrupt_ack_timeout_for_test(&self, timeout: std::time::Duration) {
        *self
            .test_user_interrupt_ack_timeout
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = timeout;
    }

    pub(super) async fn await_user_interrupt_dispatch(
        &self,
        mut result_rx: crate::tokio::sync::watch::Receiver<
            Option<Result<bool, RuntimeDriverError>>,
        >,
        expected_run_id: &meerkat_core::RunId,
    ) -> Result<bool, RuntimeDriverError> {
        if let Some(result) = result_rx.borrow().clone() {
            return result;
        }
        let ack_timeout = self.user_interrupt_ack_timeout();
        match crate::tokio::time::timeout(ack_timeout, result_rx.changed()).await {
            Ok(Ok(())) => result_rx.borrow().clone().ok_or_else(|| {
                RuntimeDriverError::Internal(
                    "hard-interrupt completion changed without publishing a result".to_string(),
                )
            })?,
            Ok(Err(_)) => Err(RuntimeDriverError::Internal(
                "process-owned hard-interrupt task ended without a result".to_string(),
            )),
            Err(_) => Err(RuntimeDriverError::InterruptDispatchOutcomeUnknown {
                run_id: expected_run_id.clone(),
                reason: format!(
                    "executor callback exceeded the {} ms acknowledgement bound; exact reconciliation continues process-owned",
                    ack_timeout.as_millis()
                ),
            }),
        }
    }
}
