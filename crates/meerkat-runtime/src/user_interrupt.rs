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

#[cfg(test)]
const USER_INTERRUPT_ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(100);
#[cfg(not(test))]
const USER_INTERRUPT_ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl MeerkatMachine {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn reconcile_user_interrupt_dispatch(
        &self,
        session_id: &SessionId,
        dispatch_id: uuid::Uuid,
        expected_run_id: &meerkat_core::RunId,
        captured_gate: &Arc<crate::tokio::sync::Mutex<()>>,
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
                    let _guard = Arc::clone(captured_gate).lock_owned().await;
                    let mut sessions = self.sessions.write().await;
                    let result = Ok(false);
                    result_tx.send_replace(Some(result.clone()));
                    if let Some(entry) = sessions.get_mut(session_id)
                        && Arc::ptr_eq(&entry.mutation_gate, captured_gate)
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
        let gate_guard = match member_lease.as_ref() {
            Some(lease) => Arc::clone(&lease.session_mutation_gate).lock_owned().await,
            None => Arc::clone(captured_gate).lock_owned().await,
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
            let attachment_matches = Arc::ptr_eq(&entry.mutation_gate, captured_gate)
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
        let driver = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).map(|entry| entry.driver.clone())
        };
        if let Some(driver) = driver
            && let Some(stored) = driver.lock().await.as_driver().stored_input_state(input_id)
        {
            return stored.seed.terminal_outcome;
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

    pub(super) async fn await_user_interrupt_dispatch(
        mut result_rx: crate::tokio::sync::watch::Receiver<
            Option<Result<bool, RuntimeDriverError>>,
        >,
        expected_run_id: &meerkat_core::RunId,
    ) -> Result<bool, RuntimeDriverError> {
        if let Some(result) = result_rx.borrow().clone() {
            return result;
        }
        match crate::tokio::time::timeout(USER_INTERRUPT_ACK_TIMEOUT, result_rx.changed()).await {
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
                    USER_INTERRUPT_ACK_TIMEOUT.as_millis()
                ),
            }),
        }
    }
}
