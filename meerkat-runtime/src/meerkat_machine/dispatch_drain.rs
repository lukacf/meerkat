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
                // Stage StopDrain for each session whose drain is Running.
                let session_ids: Vec<meerkat_core::types::SessionId> = {
                    let slots = self.comms_drain_slots.read().await;
                    slots
                        .iter()
                        .filter(|(_, slot)| {
                            slot.phase
                                == meerkat_core::comms_drain_lifecycle_authority::CommsDrainPhase::Running
                        })
                        .map(|(sid, _)| sid.clone())
                        .collect()
                };
                for sid in &session_ids {
                    self.stage_drain_stop_dsl(sid).await;
                }
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
                // Stage StopDrain if drain is Running.
                let drain_is_running = {
                    let slots = self.comms_drain_slots.read().await;
                    slots
                        .get(&session_id)
                        .map(|s| {
                            s.phase
                                == meerkat_core::comms_drain_lifecycle_authority::CommsDrainPhase::Running
                        })
                        .unwrap_or(false)
                };
                if drain_is_running {
                    self.stage_drain_stop_dsl(&session_id).await;
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
                    // Stage DSL DrainExitedClean for safety net before
                    // mutating the slot. Determine respawnable based on
                    // mode + Failed reason (safety net always uses Failed).
                    let is_respawnable = slot.mode
                        == Some(
                            meerkat_core::comms_drain_lifecycle_authority::CommsDrainMode::PersistentHost,
                        );
                    let drain_phase_str = format!("{:?}", slot.phase);
                    drop(slots);
                    {
                        let dsl_input = if is_respawnable {
                            crate::meerkat_machine::dsl::MeerkatMachineInput::DrainExitedRespawnable
                        } else {
                            crate::meerkat_machine::dsl::MeerkatMachineInput::DrainExitedClean
                        };
                        let context = if is_respawnable {
                            "DrainExitedRespawnable(safety)"
                        } else {
                            "DrainExitedClean(safety)"
                        };
                        let mut sessions = self.sessions.write().await;
                        if let Some(entry) = sessions.get_mut(&session_id) {
                            entry.dsl_authority.state.drain_phase = drain_phase_str;
                            match crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                                &mut *entry.dsl_authority,
                                dsl_input,
                            ) {
                                Ok(transition) => {
                                    entry.dsl_authority.state.lifecycle_phase = transition.to_phase;
                                }
                                Err(err) => {
                                    tracing::warn!(
                                        error = %crate::meerkat_machine::dsl_authority::map_error(err, context),
                                        "DSL rejected drain exit safety net"
                                    );
                                }
                            }
                        }
                    }
                    tracing::warn!(
                        "comms_drain: task exited without notifying authority (likely panicked), \
                         submitting Failed safety net"
                    );
                    let mut slots = self.comms_drain_slots.write().await;
                    if let Some(slot) = slots.get_mut(&session_id) {
                        slot.mark_task_exit_if_running_for_safety(
                            meerkat_core::comms_drain_lifecycle_authority::DrainExitReason::Failed,
                        );
                    }
                }
                Ok(MeerkatMachineCommandResult::Unit)
            }
            _ => unreachable!("non-drain-local command routed to drain-local handler"),
        }
    }

    /// Stage a DSL `StopDrain` input for the given session.
    ///
    /// Patches the DSL drain_phase from the slot before applying, since
    /// `sync_session_dsl_projection` does not yet project drain substate.
    async fn stage_drain_stop_dsl(&self, session_id: &meerkat_core::types::SessionId) {
        let drain_phase_str = {
            let slots = self.comms_drain_slots.read().await;
            slots
                .get(session_id)
                .map(|s| format!("{:?}", s.phase))
                .unwrap_or_else(|| "Inactive".to_string())
        };
        let mut sessions = self.sessions.write().await;
        if let Some(entry) = sessions.get_mut(session_id) {
            entry.dsl_authority.state.drain_phase = drain_phase_str;
            match crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *entry.dsl_authority,
                crate::meerkat_machine::dsl::MeerkatMachineInput::StopDrain,
            ) {
                Ok(transition) => {
                    entry.dsl_authority.state.lifecycle_phase = transition.to_phase;
                }
                Err(err) => {
                    tracing::warn!(
                        %session_id,
                        error = %crate::meerkat_machine::dsl_authority::map_error(err, "StopDrain"),
                        "DSL rejected StopDrain; proceeding with abort"
                    );
                }
            }
        }
    }
}
