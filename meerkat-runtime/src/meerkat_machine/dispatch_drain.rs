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
                // The attach/detach transitions below encode exact
                // idempotence in DSL guards. Any remaining rejection is an
                // authoritative denial (for example a session attach trying
                // to downgrade a mob-owned ingress) and must stop before the
                // drain shell mutates task state.
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
                    self.stage_session_dsl_input(&session_id, attach_input, "AttachPeerIngress")
                        .await
                        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                } else if !keep_alive {
                    // keep_alive=false + comms_runtime=None → caller is
                    // tearing down the drain. Fire `DetachIngress` to
                    // clear any active ownership. The DSL accepts exact
                    // no-op detach; other rejection stops the shell here.
                    self.stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::DetachIngress,
                        "DetachIngress",
                    )
                    .await
                    .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
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
                // Stage StopDrain for each session whose generated drain
                // authority says Running. The shell slot is only task
                // mechanics and does not decide lifecycle.
                let session_states: Vec<(meerkat_core::types::SessionId, bool)> = {
                    let sessions = self.sessions.read().await;
                    sessions
                        .iter()
                        .map(|(sid, entry)| {
                            let authority = entry
                                .dsl_authority
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            (
                                sid.clone(),
                                authority.state().drain_phase
                                    == crate::meerkat_machine::dsl::DrainPhase::Running,
                            )
                        })
                        .collect()
                };
                let mut accepted_session_ids = Vec::new();
                for (sid, running) in &session_states {
                    if !running || self.stage_drain_stop_dsl(sid).await {
                        accepted_session_ids.push(sid.clone());
                    }
                }
                let mut sessions = self.sessions.write().await;
                for sid in accepted_session_ids {
                    if let Some(entry) = sessions.get_mut(&sid) {
                        abort_slot(&mut entry.drain_slot);
                    }
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
                // Stage StopDrain if generated drain authority says Running.
                let drain_is_running =
                    self.drain_authority_state(&session_id)
                        .await
                        .is_some_and(|state| {
                            state.phase == crate::meerkat_machine::dsl::DrainPhase::Running
                        });
                if drain_is_running && !self.stage_drain_stop_dsl(&session_id).await {
                    return Ok(MeerkatMachineCommandResult::Unit);
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
                        .and_then(|entry| entry.drain_slot.take_handle())
                };
                if let Some(handle) = handle {
                    let _ = handle.await;
                }
                // Re-read post-await to safety-net a panicked task that
                // never notified the authority. The generated
                // `NotifyDrainExited { Failed }` transition owns the
                // resulting clean-vs-respawnable state; the shell then only
                // clears task-handle mechanics after acceptance.
                let drain_is_running =
                    self.drain_authority_state(&session_id)
                        .await
                        .is_some_and(|state| {
                            state.phase == crate::meerkat_machine::dsl::DrainPhase::Running
                        });
                if drain_is_running {
                    let dsl_accepted = self
                        .stage_session_dsl_input(
                            &session_id,
                            crate::meerkat_machine::dsl::MeerkatMachineInput::NotifyDrainExited {
                                reason: crate::meerkat_machine::dsl::DrainExitReason::Failed,
                            },
                            "NotifyDrainExited(safety)",
                        )
                        .await
                        .map_err(|err| {
                            tracing::warn!(
                                error = %err,
                                "DSL rejected drain exit safety net"
                            );
                            err
                        })
                        .is_ok();
                    tracing::warn!(
                        "comms_drain: task exited without notifying authority (likely panicked), \
                         submitting Failed safety net"
                    );
                    if dsl_accepted {
                        self.project_comms_drain_failed_safety_net(&session_id)
                            .await;
                    }
                }
                Ok(MeerkatMachineCommandResult::Unit)
            }
            _ => unreachable!("non-drain-local command routed to drain-local handler"),
        }
    }

    /// Fire the typed `StopDrain` DSL input for `session_id` if the session
    /// still has a live DSL authority. Returns whether the machine accepted
    /// the transition; callers use that gate before applying shell-side abort
    /// projection.
    async fn stage_drain_stop_dsl(&self, session_id: &meerkat_core::types::SessionId) -> bool {
        if let Err(err) = self
            .stage_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::StopDrain,
                "StopDrain",
            )
            .await
        {
            tracing::warn!(
                %session_id,
                error = %err,
                "DSL rejected StopDrain; skipping drain abort"
            );
            false
        } else {
            true
        }
    }
}
