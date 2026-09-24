use std::sync::Arc;

use meerkat_core::lifecycle::CoreExecutorInterruptHandle;
use meerkat_core::types::SessionId;

use crate::meerkat_machine::MeerkatMachine;
use crate::runtime_state::RuntimeState;
use crate::traits::RuntimeDriverError;

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
        let result = if run_is_current {
            match callback_result {
                Ok(true) => Ok(true),
                Ok(false) => Err(RuntimeDriverError::InterruptDispatchOutcomeUnknown {
                    run_id: expected_run_id.clone(),
                    reason: "executor reported the exact run non-current while machine authority still binds it"
                        .to_string(),
                }),
                Err(error) => Err(error),
            }
        } else {
            Ok(false)
        };
        if matches!(result, Ok(true)) {
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
