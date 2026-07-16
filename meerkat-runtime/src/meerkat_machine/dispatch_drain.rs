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
                expected_attachment,
                mob_id,
            } => {
                if let Some(expected) = expected_attachment.as_ref()
                    && (!expected.belongs_to(self) || expected.session_id() != &session_id)
                {
                    return Err(RuntimeDriverError::StaleAuthority {
                        reason: format!(
                            "peer-ingress attachment witness does not belong to runtime session '{session_id}'"
                        ),
                    });
                }
                // Guard: session must exist.
                if !self.sessions.read().await.contains_key(&session_id) {
                    if expected_attachment.is_some() {
                        return Err(RuntimeDriverError::StaleAuthority {
                            reason: format!(
                                "runtime executor attachment disappeared before peer-ingress publication for session '{session_id}'"
                            ),
                        });
                    }
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                // Generated SetPeerIngressContext/Attach/Spawn transitions own
                // the Destroyed exception. They accept only a persistent-host
                // supervisor-cleanup drain backed by closed durable authority;
                // the shell must not pre-classify terminal admission here.

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                if let Some(expected) = expected_attachment.as_ref() {
                    let exact_attachment = if let Some(gate) = gate.as_ref() {
                        let sessions = self.sessions.read().await;
                        sessions.get(&session_id).is_some_and(|entry| {
                            Arc::ptr_eq(&entry.mutation_gate, gate)
                                && entry.epoch_id == expected.epoch_id
                                && entry.generated_executor_registration_active()
                                && matches!(
                                    &entry.attachment_slot,
                                    RuntimeLoopAttachmentSlot::Attached(attachment)
                                        if attachment.id == expected.attachment_id
                                            && !attachment.wake_tx.is_closed()
                                            && !attachment.effect_tx.is_closed()
                                )
                        })
                    } else {
                        false
                    };
                    if !exact_attachment {
                        return Err(RuntimeDriverError::StaleAuthority {
                            reason: format!(
                                "runtime executor attachment changed before peer-ingress publication for session '{session_id}'"
                            ),
                        });
                    }
                }

                if let Err(reason) = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::SetPeerIngressContext {
                            keep_alive,
                        },
                        "SetPeerIngressContext",
                    )
                    .await
                {
                    return Err(self
                        .classify_session_dsl_rejection(&session_id, reason)
                        .await);
                }

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
                            comms_runtime_id: comms_runtime_id.clone(),
                            mob_id,
                        }
                    } else {
                        crate::meerkat_machine::dsl::MeerkatMachineInput::AttachSessionIngress {
                            comms_runtime_id: comms_runtime_id.clone(),
                        }
                    };

                    let (runtime_changed, same_owner, authority_snapshot) = {
                        let sessions = self.sessions.read().await;
                        let entry =
                            sessions
                                .get(&session_id)
                                .ok_or(RuntimeDriverError::NotReady {
                                    state: RuntimeState::Destroyed,
                                })?;
                        let authority = entry
                            .dsl_authority
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        let state = authority.state();
                        let same_owner = match (&attach_input, state.peer_ingress_owner_kind) {
                            (
                                crate::meerkat_machine::dsl::MeerkatMachineInput::AttachSessionIngress { .. },
                                crate::meerkat_machine::dsl::PeerIngressOwnerKind::SessionOwned,
                            ) => true,
                            (
                                crate::meerkat_machine::dsl::MeerkatMachineInput::AttachMobIngress { mob_id, .. },
                                crate::meerkat_machine::dsl::PeerIngressOwnerKind::MobOwned,
                            ) => state.peer_ingress_mob_id.as_ref() == Some(mob_id),
                            _ => false,
                        };
                        (
                            state
                                .peer_ingress_comms_runtime_id
                                .as_ref()
                                .is_some_and(|current| current != &comms_runtime_id),
                            same_owner,
                            state.clone(),
                        )
                    };
                    if runtime_changed {
                        // First prove the exact generated attach against a
                        // cloned authority. SessionOwned -> MobOwned promotion
                        // is a direct accepted transition and must still
                        // quiesce the old runtime's drain + rotation carrier.
                        // Only an otherwise-identical owner replacing its
                        // runtime needs the explicit Detach -> Attach fallback.
                        let mut direct_preview =
                            crate::meerkat_machine::dsl::MeerkatMachineAuthority::recover_from_state(
                                authority_snapshot.clone(),
                            )
                            .map_err(|error| RuntimeDriverError::ValidationFailed {
                                reason: error.to_string(),
                            })?;
                        let direct_attach =
                            crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                                &mut direct_preview,
                                attach_input.clone(),
                            );
                        let detach_before_attach = match direct_attach {
                            Ok(_) => false,
                            Err(_) if same_owner => {
                                let mut fallback_preview =
                                    crate::meerkat_machine::dsl::MeerkatMachineAuthority::recover_from_state(
                                        authority_snapshot,
                                    )
                                    .map_err(|error| RuntimeDriverError::ValidationFailed {
                                        reason: error.to_string(),
                                    })?;
                                crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                                    &mut fallback_preview,
                                    crate::meerkat_machine::dsl::MeerkatMachineInput::DetachIngress,
                                )
                                .map_err(|error| {
                                    RuntimeDriverError::ValidationFailed {
                                        reason: error.to_string(),
                                    }
                                })?;
                                crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                                    &mut fallback_preview,
                                    attach_input.clone(),
                                )
                                .map_err(|error| {
                                    RuntimeDriverError::ValidationFailed {
                                        reason: error.to_string(),
                                    }
                                })?;
                                true
                            }
                            Err(error) => {
                                return Err(RuntimeDriverError::ValidationFailed {
                                    reason: error.to_string(),
                                });
                            }
                        };

                        // The caller holds the session mutation gate. Abort and
                        // JOIN both old-runtime producers before publishing any
                        // generated authority for the replacement runtime.
                        self.abort_and_join_session_comms_producers(&session_id, false)
                            .await;
                        if detach_before_attach {
                            self.stage_session_dsl_input(
                                &session_id,
                                crate::meerkat_machine::dsl::MeerkatMachineInput::DetachIngress,
                                "DetachIngressForPeerRuntimeReplacement",
                            )
                            .await
                            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                        }
                    }
                    self.stage_session_dsl_input(&session_id, attach_input, "AttachPeerIngress")
                        .await
                        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                } else if !keep_alive {
                    // Prove DetachIngress before touching mechanics. Then stop
                    // and join BOTH the drain and rotation carrier while the
                    // mutation gate is held; only after quiescence publish the
                    // detached generated authority.
                    let authority = self
                        .session_dsl_authority(&session_id)
                        .await
                        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                    {
                        let authority = authority
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        let mut preview =
                            crate::meerkat_machine::dsl::MeerkatMachineAuthority::recover_from_state(
                                authority.state().clone(),
                            )
                            .map_err(|error| RuntimeDriverError::ValidationFailed {
                                reason: error.to_string(),
                            })?;
                        crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                            &mut preview,
                            crate::meerkat_machine::dsl::MeerkatMachineInput::DetachIngress,
                        )
                        .map_err(|error| {
                            RuntimeDriverError::ValidationFailed {
                                reason: error.to_string(),
                            }
                        })?;
                    }
                    if !self
                        .stop_and_abort_session_comms_producers(&session_id, false)
                        .await
                    {
                        return Ok(MeerkatMachineCommandResult::Spawned(false));
                    }
                    self.stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::DetachIngress,
                        "DetachIngress",
                    )
                    .await
                    .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                    return Ok(MeerkatMachineCommandResult::Spawned(false));
                }

                // Ownership transitions above have succeeded (or been
                // idempotently rejected); dispatch the mechanical
                // drain-task lifecycle side-effect. `update_peer_ingress_
                // context_inner` stages `SpawnDrain` when enabling and
                // applies the already-gated stop/abort path when disabling.
                // On DSL rejection it returns false without mutating
                // shell slot state (preserving the bdd460951 invariant
                // "no shell mutation after DSL rejection"). Its return
                // value is the typed `Spawned(bool)` result the caller
                // of `maybe_spawn_comms_drain` observes.
                let spawned = self
                    .update_peer_ingress_context_inner(&session_id, keep_alive, comms_runtime)
                    .await?;
                Ok(MeerkatMachineCommandResult::Spawned(spawned))
            }
            MeerkatMachineCommand::NotifyDrainExited { session_id, reason } => {
                // D2b: a drain-exit observation arriving after the session's
                // sessions-map entry is gone is a legitimate post-teardown
                // interleaving (the unregister drain aborts the drain task and
                // removes the entry; a straggling exit can still be reported).
                // It is observation-shaped and benign — surface it as an
                // accepted no-op, not a `Destroyed` error.
                if !self.sessions.read().await.contains_key(&session_id) {
                    tracing::debug!(
                        %session_id,
                        ?reason,
                        "post-teardown drain-exit observation (benign no-op)"
                    );
                    return Ok(MeerkatMachineCommandResult::Unit);
                }

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                // Stage-first: NotifyDrainExited is not declared from
                // Destroyed (DrainBindingInvariant); the machine rejects it
                // there and the rejection is classified as the terminal
                // `Destroyed` truth.
                if let Err(reason) = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::NotifyDrainExited {
                            reason: crate::meerkat_machine::dsl::DrainExitReason::from(reason),
                        },
                        "NotifyDrainExited",
                    )
                    .await
                {
                    return Err(self
                        .classify_session_dsl_rejection(&session_id, reason)
                        .await);
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
                let session_ids: Vec<meerkat_core::types::SessionId> = {
                    let sessions = self.sessions.read().await;
                    sessions.keys().cloned().collect()
                };
                for session_id in session_ids {
                    let Some(gate) = self.session_mutation_gate(&session_id).await else {
                        continue;
                    };
                    let _gate_guard = gate.lock().await;
                    let _ = self
                        .stop_and_abort_session_comms_producers(&session_id, true)
                        .await;
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
                let Some(gate) = self.session_mutation_gate(&session_id).await else {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                };
                let _gate_guard = gate.lock().await;
                let _ = self
                    .stop_and_abort_session_comms_producers(&session_id, true)
                    .await;
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

    /// Stop generated drain authority (when running), then abort both
    /// session-scoped comms producers. The caller must hold the session
    /// mutation gate so no stale drain can install a new rotation carrier
    /// between the authority check and mechanical quiescence.
    pub(super) async fn stop_and_abort_session_comms_producers(
        &self,
        session_id: &meerkat_core::types::SessionId,
        bound_drain_wait: bool,
    ) -> bool {
        let drain_is_running = self
            .drain_authority_state(session_id)
            .await
            .is_some_and(|state| state.phase == crate::meerkat_machine::dsl::DrainPhase::Running);
        if drain_is_running && !self.stage_drain_stop_dsl(session_id).await {
            return false;
        }
        self.abort_and_join_session_comms_producers(session_id, bound_drain_wait)
            .await;
        true
    }

    /// Abort the drain and supervisor-rotation carrier together, then join
    /// them outside the session registry lock. Runtime replacement uses an
    /// unbounded drain join because new generated runtime authority must not be
    /// published while any old-runtime producer remains live. Explicit stop
    /// retains its established bounded drain grace, while the async rotation
    /// carrier is always joined before return.
    pub(super) async fn abort_and_join_session_comms_producers(
        &self,
        session_id: &meerkat_core::types::SessionId,
        bound_drain_wait: bool,
    ) {
        let (drain_handle, rotation_slot) = {
            let mut sessions = self.sessions.write().await;
            let Some(entry) = sessions.get_mut(session_id) else {
                return;
            };
            (
                entry.drain_slot.abort_keeping_handle(),
                Arc::clone(&entry.supervisor_rotation_task),
            )
        };
        let rotation_handle = rotation_slot.abort_keeping_handle().await;

        if let Some(drain_handle) = drain_handle {
            if bound_drain_wait {
                const COMMS_DRAIN_ABORT_GRACE: std::time::Duration =
                    std::time::Duration::from_secs(2);
                if crate::tokio::time::timeout(COMMS_DRAIN_ABORT_GRACE, drain_handle)
                    .await
                    .is_err()
                {
                    tracing::warn!(
                        %session_id,
                        "comms drain task did not quiesce within the abort grace window; proceeding (task already aborted)"
                    );
                }
            } else {
                let _ = drain_handle.await;
            }
        }
        if let Some(rotation_handle) = rotation_handle {
            let _ = rotation_handle.await;
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
