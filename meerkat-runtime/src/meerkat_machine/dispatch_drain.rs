use super::*;

impl MeerkatMachine {
    pub(super) async fn execute_meerkat_machine_drain_command(
        self: &Arc<Self>,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        match command {
            MeerkatMachineCommand::SetPeerIngressContext {
                session_id,
                keep_alive,
                comms_runtime,
            } => {
                // Guard: session must exist.
                if !self.sessions.read().await.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                // Guard: DrainBindingInvariant — no drain mutation on destroyed
                // sessions.
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                let previous_dsl_state = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::SetPeerIngressContext {
                            keep_alive,
                        },
                        "SetPeerIngressContext",
                    )
                    .await
                    .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                let result = MeerkatMachineCommandResult::Spawned(
                    self.update_peer_ingress_context_inner(&session_id, keep_alive, comms_runtime)
                        .await,
                );
                if let Err(err) = self.sync_session_dsl_projection(&session_id).await {
                    self.restore_session_dsl_state(&session_id, previous_dsl_state)
                        .await;
                    return Err(err);
                }
                Ok(result)
            }
            MeerkatMachineCommand::NotifyDrainExited { session_id, reason } => {
                // Guard: session must exist.
                if !self.sessions.read().await.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                // Guard: DrainBindingInvariant — no drain mutation on destroyed
                // sessions.
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                let reason_str = format!("{reason:?}");
                let previous_dsl_state = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::NotifyDrainExited {
                            reason: reason_str.clone(),
                        },
                        "NotifyDrainExited",
                    )
                    .await
                    .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                self.notify_comms_drain_exited_inner(&session_id, reason)
                    .await;
                if let Err(err) = self.sync_session_dsl_projection(&session_id).await {
                    self.restore_session_dsl_state(&session_id, previous_dsl_state)
                        .await;
                    return Err(err);
                }
                Ok(MeerkatMachineCommandResult::Unit)
            }
            _ => unreachable!("non-drain command routed to drain handler"),
        }
    }

    pub(super) async fn execute_meerkat_machine_drain_local_command(
        &self,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        match command {
            MeerkatMachineCommand::AbortAll => {
                let mut slots = self.comms_drain_slots.write().await;
                for (_, slot) in slots.iter_mut() {
                    abort_slot(slot);
                }
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::Abort { session_id } => {
                // Guard: session must be registered.
                if !self.sessions.read().await.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                let mut slots = self.comms_drain_slots.write().await;
                if let Some(slot) = slots.get_mut(&session_id) {
                    abort_slot(slot);
                }
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::Wait { session_id } => {
                // Guard: session must be registered.
                if !self.sessions.read().await.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                let handle = {
                    let mut slots = self.comms_drain_slots.write().await;
                    slots
                        .get_mut(&session_id)
                        .and_then(|slot| slot.handle.take())
                };
                if let Some(handle) = handle {
                    let _ = handle.await;
                }
                let mut slots = self.comms_drain_slots.write().await;
                if let Some(slot) = slots.get_mut(&session_id)
                    && slot.phase
                        == meerkat_core::comms_drain_lifecycle_authority::CommsDrainPhase::Running
                {
                    tracing::warn!(
                        "comms_drain: task exited without notifying authority (likely panicked), \
                         submitting Failed safety net"
                    );
                    slot.mark_task_exit_if_running_for_safety(
                        meerkat_core::comms_drain_lifecycle_authority::DrainExitReason::Failed,
                    );
                }
                Ok(MeerkatMachineCommandResult::Unit)
            }
            _ => unreachable!("non-drain-local command routed to drain-local handler"),
        }
    }
}
