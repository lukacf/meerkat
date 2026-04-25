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
                mob_id,
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

                self.stage_session_dsl_input(
                    &session_id,
                    crate::meerkat_machine::dsl::MeerkatMachineInput::SetPeerIngressContext {
                        keep_alive,
                    },
                    "SetPeerIngressContext",
                )
                .await
                .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

                // W2-G (issue #264): peer-ingress ownership is tracked by
                // the DSL via `peer_ingress_owner_kind` +
                // `peer_ingress_comms_runtime_id` + `peer_ingress_mob_id`.
                // The `SetPeerIngressContext` transition above is a
                // keep-alive-only self-loop that doesn't mutate those
                // fields; the ownership-establishing transitions are
                // `AttachSessionIngress` / `AttachMobIngress` at
                // `meerkat-runtime/src/meerkat_machine/dsl.rs:6375-6407`.
                //
                // When a `comms_runtime` is provided:
                //   - If `mob_id` is present → fire `AttachMobIngress` to
                //     record `MobOwned { comms_runtime_id, mob_id }`.
                //   - Otherwise → fire `AttachSessionIngress` to record
                //     `SessionOwned { comms_runtime_id }`.
                // Ignore the DSL's transition-rejected case gracefully —
                // the two guards (`owner_is_unattached` on Session,
                // `owner_allows_mob_attach` on Mob) turn no-op when the
                // owner is already at the target kind or a higher level,
                // which is the idempotent semantic these helpers have
                // always meant to carry.
                //
                // When `comms_runtime` is `None` the caller intends
                // keep-alive-only; no ownership transition fires.
                if let Some(ref runtime) = comms_runtime {
                    let comms_runtime_id =
                        crate::meerkat_machine::dsl::CommsRuntimeId::from_runtime(runtime);
                    let attach_input = if let Some(mob_id) = mob_id {
                        crate::meerkat_machine::dsl::MeerkatMachineInput::AttachMobIngress {
                            comms_runtime_id,
                            mob_id,
                        }
                    } else {
                        crate::meerkat_machine::dsl::MeerkatMachineInput::AttachSessionIngress {
                            comms_runtime_id,
                        }
                    };
                    // The Attach transitions have `guard` clauses that
                    // reject no-op re-application (already at target
                    // owner kind); treat transition rejections as
                    // expected idempotent behavior rather than a
                    // command failure.
                    let _ = self
                        .stage_session_dsl_input(&session_id, attach_input, "AttachPeerIngress")
                        .await;
                } else if !keep_alive {
                    // keep_alive=false + comms_runtime=None → caller is
                    // tearing down the drain. Fire `DetachIngress` to
                    // clear any active ownership. Its guard rejects
                    // no-op (already Unattached) with the same
                    // idempotent semantic as the Attach transitions.
                    let _ = self
                        .stage_session_dsl_input(
                            &session_id,
                            crate::meerkat_machine::dsl::MeerkatMachineInput::DetachIngress,
                            "DetachIngress",
                        )
                        .await;
                }

                // Ownership transitions above have succeeded (or been
                // idempotently rejected); dispatch the mechanical
                // drain-task lifecycle side-effect. `update_peer_ingress_
                // context_inner` stages `SpawnDrain` / `Abort` on the DSL
                // and, on DSL accept, spawns / aborts the drain task.
                // On DSL rejection it returns false without mutating
                // shell slot state (preserving the bdd460951 invariant
                // "no shell mutation after DSL rejection"). Its return
                // value is the typed `Spawned(bool)` result the caller
                // of `maybe_spawn_comms_drain` observes.
                let spawned = self
                    .update_peer_ingress_context_inner(&session_id, keep_alive, comms_runtime)
                    .await;
                Ok(MeerkatMachineCommandResult::Spawned(spawned))
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

                self.stage_session_dsl_input(
                    &session_id,
                    crate::meerkat_machine::dsl::MeerkatMachineInput::NotifyDrainExited {
                        reason: crate::meerkat_machine::dsl::DrainExitReason::from(reason),
                    },
                    "NotifyDrainExited",
                )
                .await
                .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
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
                // Stage StopDrain for each session whose drain is Running.
                let session_ids: Vec<meerkat_core::types::SessionId> = {
                    let sessions = self.sessions.read().await;
                    sessions
                        .iter()
                        .filter(|(_, entry)| {
                            entry.drain_slot.phase
                                == crate::meerkat_machine::CommsDrainPhase::Running
                        })
                        .map(|(sid, _)| sid.clone())
                        .collect()
                };
                for sid in &session_ids {
                    self.stage_drain_stop_dsl(sid).await;
                }
                let mut sessions = self.sessions.write().await;
                for (_, entry) in sessions.iter_mut() {
                    abort_slot(&mut entry.drain_slot);
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
                    let sessions = self.sessions.read().await;
                    sessions
                        .get(&session_id)
                        .map(|entry| {
                            entry.drain_slot.phase
                                == crate::meerkat_machine::CommsDrainPhase::Running
                        })
                        .unwrap_or(false)
                };
                if drain_is_running {
                    self.stage_drain_stop_dsl(&session_id).await;
                }
                let mut sessions = self.sessions.write().await;
                if let Some(entry) = sessions.get_mut(&session_id) {
                    abort_slot(&mut entry.drain_slot);
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
                    let mut sessions = self.sessions.write().await;
                    sessions
                        .get_mut(&session_id)
                        .and_then(|entry| entry.drain_slot.handle.take())
                };
                if let Some(handle) = handle {
                    let _ = handle.await;
                }
                // Re-read post-await to safety-net a panicked task that
                // never notified the authority. Record the stage-if-running
                // decision under the same lock acquisition so authority
                // and slot updates stay ordered: DSL input first, then
                // mark the slot exited. Dropping and re-acquiring the
                // write guard between DSL and slot mutation preserves the
                // pre-collapse pattern where the sibling map was touched
                // after the `sessions` lock was released.
                let is_respawnable = {
                    let sessions = self.sessions.read().await;
                    sessions.get(&session_id).and_then(|entry| {
                        (entry.drain_slot.phase == crate::meerkat_machine::CommsDrainPhase::Running)
                            .then_some(
                                entry.drain_slot.mode
                                    == Some(crate::meerkat_machine::CommsDrainMode::PersistentHost),
                            )
                    })
                };
                if let Some(is_respawnable) = is_respawnable {
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
                    {
                        let mut sessions = self.sessions.write().await;
                        if let Some(entry) = sessions.get_mut(&session_id) {
                            let mut authority = entry
                                .dsl_authority
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            if let Err(err) =
                                crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                                    &mut *authority,
                                    dsl_input,
                                )
                            {
                                tracing::warn!(
                                    error = %crate::meerkat_machine::dsl_authority::map_error(err, context),
                                    "DSL rejected drain exit safety net"
                                );
                            }
                        }
                    }
                    tracing::warn!(
                        "comms_drain: task exited without notifying authority (likely panicked), \
                         submitting Failed safety net"
                    );
                    let mut sessions = self.sessions.write().await;
                    if let Some(entry) = sessions.get_mut(&session_id) {
                        entry.drain_slot.mark_task_exit_if_running_for_safety(
                            crate::meerkat_machine::DrainExitReason::Failed,
                        );
                    }
                }
                Ok(MeerkatMachineCommandResult::Unit)
            }
            _ => unreachable!("non-drain-local command routed to drain-local handler"),
        }
    }

    /// Fire the typed `StopDrain` DSL input for `session_id` if the session
    /// still has a live DSL authority. A guard rejection (e.g. the drain
    /// was never `Running` for this session) is downgraded to a warn so the
    /// caller's abort-cleanup flow proceeds — `StopDrain` is idempotent by
    /// DSL construction, so "already stopped" is the only legitimate
    /// rejection shape at this seam.
    async fn stage_drain_stop_dsl(&self, session_id: &meerkat_core::types::SessionId) {
        let mut sessions = self.sessions.write().await;
        let Some(entry) = sessions.get_mut(session_id) else {
            return;
        };
        let mut authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Err(err) = crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
            &mut *authority,
            crate::meerkat_machine::dsl::MeerkatMachineInput::StopDrain,
        ) {
            tracing::warn!(
                %session_id,
                error = %crate::meerkat_machine::dsl_authority::map_error(err, "StopDrain"),
                "DSL rejected StopDrain; proceeding with abort"
            );
        }
    }
}
