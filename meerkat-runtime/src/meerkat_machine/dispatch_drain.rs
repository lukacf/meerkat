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
                Ok(MeerkatMachineCommandResult::Spawned(
                    self.update_peer_ingress_context_inner(&session_id, keep_alive, comms_runtime)
                        .await,
                ))
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
                self.notify_comms_drain_exited_inner(&session_id, reason)
                    .await;
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
                    && slot.authority.phase()
                        == meerkat_core::comms_drain_lifecycle_authority::CommsDrainPhase::Running
                {
                    tracing::warn!(
                        "comms_drain: task exited without notifying authority (likely panicked), \
                         submitting Failed safety net"
                    );
                    match protocol_comms_drain_spawn::notify_task_exited(
                        &mut slot.authority,
                        DrainExitReason::Failed,
                    ) {
                        Ok(effects) => {
                            apply_runtime_drain_effects(slot, &effects);
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "comms drain authority rejected safety-net TaskExited"
                            );
                        }
                    }
                }
                Ok(MeerkatMachineCommandResult::Unit)
            }
            _ => unreachable!("non-drain-local command routed to drain-local handler"),
        }
    }
}
