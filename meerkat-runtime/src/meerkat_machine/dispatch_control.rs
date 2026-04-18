use super::*;

impl MeerkatMachine {
    pub(super) async fn execute_meerkat_machine_control_command(
        &self,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeControlPlaneError> {
        match command {
            MeerkatMachineCommand::Ingest { runtime_id, input } => {
                let (session_id, driver, _completions, wake_tx, control_tx) = {
                    let (sid, d, c, w) = self.lookup_entry(&runtime_id).await?;
                    let ctrl = {
                        let sessions = self.sessions.read().await;
                        sessions
                            .get(&sid)
                            .and_then(RuntimeSessionEntry::control_sender)
                    };
                    (sid, d, c, w, ctrl)
                };

                // DSL-first: stage Ingest input before driver mutation.
                // The DSL transition guards phase ∈ {Idle, Attached, Running}
                // which subsumes the old manual Retired/Stopped/Destroyed check.
                let provisional_work_id = uuid::Uuid::new_v4().to_string();
                let previous_dsl_state = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::Ingest {
                            runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(
                                &runtime_id,
                            ),
                            work_id: crate::meerkat_machine::dsl::WorkId::from(provisional_work_id),
                            origin: "ingest".to_string(),
                        },
                        "Ingest",
                    )
                    .await;
                let previous_dsl_state = match previous_dsl_state {
                    Ok(state) => state,
                    Err(_) => {
                        let state = self
                            .existing_session_runtime_state(&session_id)
                            .await
                            .unwrap_or(RuntimeState::Destroyed);
                        return Err(RuntimeControlPlaneError::InvalidState { state });
                    }
                };

                let request_immediate_processing =
                    crate::accept::requests_immediate_processing(&input);
                let (outcome, signal) = {
                    let mut drv = driver.lock().await;
                    let result = match drv.as_driver_mut().accept_input(input).await {
                        Ok(result) => result,
                        Err(err) => {
                            drop(drv);
                            self.restore_session_dsl_state(&session_id, previous_dsl_state)
                                .await;
                            return Err(RuntimeControlPlaneError::Internal(err.to_string()));
                        }
                    };
                    let signal =
                        Self::machine_owned_admission_signal(&result, request_immediate_processing);
                    (result, signal)
                };

                // If the driver rejected the input, rollback DSL state.
                if matches!(&outcome, AcceptOutcome::Rejected { .. }) {
                    self.restore_session_dsl_state(&session_id, previous_dsl_state)
                        .await;
                }

                if signal.should_wake()
                    && let Some(ref tx) = wake_tx
                {
                    let _ = tx.try_send(());
                }
                if signal.should_interrupt_yielding()
                    && let Some(ref tx) = control_tx
                {
                    let _ = tx.try_send(
                        meerkat_core::lifecycle::run_control::RunControlCommand::InterruptYielding,
                    );
                }

                Ok(MeerkatMachineCommandResult::AcceptOutcome(outcome))
            }
            MeerkatMachineCommand::PublishEvent { event } => {
                let runtime_id = event.runtime_id.clone();
                let (session_id, driver, _completions, _wake_tx) =
                    self.lookup_entry(&runtime_id).await?;

                // DSL-first: stage PublishEvent before driver mutation.
                // Compute event_kind before consuming the event.
                let event_kind = format!("{:?}", std::mem::discriminant(&event.event));
                let previous_dsl_state = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::PublishEvent {
                            kind: event_kind,
                        },
                        "PublishEvent",
                    )
                    .await
                    .map_err(RuntimeControlPlaneError::Internal)?;

                let mut drv = driver.lock().await;
                if let Err(err) = drv.as_driver_mut().on_runtime_event(event).await {
                    drop(drv);
                    self.restore_session_dsl_state(&session_id, previous_dsl_state)
                        .await;
                    return Err(RuntimeControlPlaneError::Internal(err.to_string()));
                }
                drop(drv);

                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::Retire { runtime_id } => {
                let (session_id, driver, completions, wake_tx) =
                    self.lookup_entry(&runtime_id).await?;

                // Acquire the per-session mutation gate to serialize the
                // full DSL-stage → driver-mutate → DSL-sync span.
                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                let state = self
                    .existing_session_runtime_state(&session_id)
                    .await
                    .unwrap_or(RuntimeState::Destroyed);
                if matches!(state, RuntimeState::Destroyed | RuntimeState::Stopped) {
                    return Err(RuntimeControlPlaneError::InvalidState { state });
                }
                let previous_dsl_state = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::Retire,
                        "Retire",
                    )
                    .await
                    .map_err(RuntimeControlPlaneError::Internal)?;

                let mut drv = driver.lock().await;
                let mut report = match machine_retire(&mut drv).await {
                    Ok(report) => report,
                    Err(err) => {
                        drop(drv);
                        self.restore_session_dsl_state(&session_id, previous_dsl_state)
                            .await;
                        return Err(RuntimeControlPlaneError::Internal(err.to_string()));
                    }
                };
                drop(drv);

                if report.inputs_pending_drain > 0 {
                    if let Some(ref tx) = wake_tx
                        && tx.send(()).await.is_ok()
                    {
                        return Ok(MeerkatMachineCommandResult::RetireReport(report));
                    }

                    let mut drv = driver.lock().await;
                    let abandoned = drv
                        .abandon_pending_inputs(crate::input_state::InputAbandonReason::Retired)
                        .await
                        .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
                    drop(drv);
                    let mut comp = completions.lock().await;
                    comp.resolve_all_terminated("retired without runtime loop");
                    report.inputs_abandoned += abandoned;
                    report.inputs_pending_drain = 0;
                }
                Ok(MeerkatMachineCommandResult::RetireReport(report))
            }
            MeerkatMachineCommand::Recycle { runtime_id } => {
                let (session_id, driver, completions, wake_tx) =
                    self.lookup_entry(&runtime_id).await?;

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                let state = self
                    .existing_session_runtime_state(&session_id)
                    .await
                    .unwrap_or(RuntimeState::Destroyed);
                if matches!(state, RuntimeState::Destroyed | RuntimeState::Running) {
                    return Err(RuntimeControlPlaneError::InvalidState { state });
                }
                let previous_dsl_state = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::Recycle,
                        "Recycle",
                    )
                    .await
                    .map_err(RuntimeControlPlaneError::Internal)?;

                let (transferred, active_after_recycle) = {
                    let mut drv = driver.lock().await;
                    let transferred = match machine_recycle_preserving_work(&mut drv).await {
                        Ok(transferred) => transferred,
                        Err(err) => {
                            drop(drv);
                            self.restore_session_dsl_state(&session_id, previous_dsl_state)
                                .await;
                            return Err(RuntimeControlPlaneError::Internal(err.to_string()));
                        }
                    };

                    let active_after_recycle = drv.as_driver().active_input_ids();
                    (transferred, active_after_recycle)
                };

                {
                    let pending_after: HashSet<InputId> =
                        active_after_recycle.into_iter().collect();
                    let mut comp = completions.lock().await;
                    comp.resolve_not_pending(
                        |input_id| pending_after.contains(input_id),
                        "recycled input no longer pending",
                    );
                }

                if let Some(ref tx) = wake_tx {
                    let _ = tx.try_send(());
                }
                Ok(MeerkatMachineCommandResult::RecycleReport(RecycleReport {
                    inputs_transferred: transferred,
                }))
            }
            MeerkatMachineCommand::Reset { runtime_id } => {
                let (session_id, driver, completions, _wake_tx) =
                    self.lookup_entry(&runtime_id).await?;

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                let state = self
                    .existing_session_runtime_state(&session_id)
                    .await
                    .unwrap_or(RuntimeState::Destroyed);
                if matches!(state, RuntimeState::Destroyed | RuntimeState::Running) {
                    return Err(RuntimeControlPlaneError::InvalidState { state });
                }
                let previous_dsl_state = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::Reset,
                        "Reset",
                    )
                    .await
                    .map_err(RuntimeControlPlaneError::Internal)?;

                let mut drv = driver.lock().await;
                let report = match machine_reset(&mut drv).await {
                    Ok(report) => report,
                    Err(err) => {
                        drop(drv);
                        self.restore_session_dsl_state(&session_id, previous_dsl_state)
                            .await;
                        return Err(RuntimeControlPlaneError::Internal(err.to_string()));
                    }
                };
                drop(drv);

                let mut comp = completions.lock().await;
                comp.resolve_all_terminated("runtime reset");
                Ok(MeerkatMachineCommandResult::ResetReport(report))
            }
            MeerkatMachineCommand::Recover { runtime_id } => {
                let (session_id, driver, completions, wake_tx) =
                    self.lookup_entry(&runtime_id).await?;

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                let state = self
                    .existing_session_runtime_state(&session_id)
                    .await
                    .unwrap_or(RuntimeState::Destroyed);
                if matches!(state, RuntimeState::Destroyed | RuntimeState::Running) {
                    return Err(RuntimeControlPlaneError::InvalidState { state });
                }
                let previous_dsl_state = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::Recover,
                        "Recover",
                    )
                    .await
                    .map_err(RuntimeControlPlaneError::Internal)?;

                let (report, active_after_recover) = {
                    let mut drv = driver.lock().await;
                    let report = match drv.as_driver_mut().recover().await {
                        Ok(report) => report,
                        Err(err) => {
                            drop(drv);
                            self.restore_session_dsl_state(&session_id, previous_dsl_state)
                                .await;
                            return Err(RuntimeControlPlaneError::Internal(err.to_string()));
                        }
                    };
                    let active_after_recover = drv.as_driver().active_input_ids();
                    (report, active_after_recover)
                };

                {
                    let pending_after: HashSet<InputId> =
                        active_after_recover.into_iter().collect();
                    let mut comp = completions.lock().await;
                    comp.resolve_not_pending(
                        |input_id| pending_after.contains(input_id),
                        "recovered input no longer pending",
                    );
                }

                if let Some(ref tx) = wake_tx {
                    let _ = tx.try_send(());
                }
                Ok(MeerkatMachineCommandResult::RecoveryReport(report))
            }
            MeerkatMachineCommand::Destroy { runtime_id } => {
                let (session_id, driver, completions, _wake_tx) =
                    self.lookup_entry(&runtime_id).await?;

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                let state = self
                    .existing_session_runtime_state(&session_id)
                    .await
                    .unwrap_or(RuntimeState::Destroyed);
                if matches!(state, RuntimeState::Destroyed) {
                    return Err(RuntimeControlPlaneError::InvalidState { state });
                }

                // Guard: runtime_is_bound — reject Destroy if the runtime was
                // never bound via PrepareBindings. The driver's Initializing
                // state means Initialize hasn't even been called. Beyond that,
                // the ephemeral driver transitions through PrepareBindings ->
                // Attached, so Initializing is the only state where the runtime
                // is definitely unbound. (The schema field `active_runtime_id`
                // maps to "has PrepareBindings been called and not reset".)
                if matches!(state, RuntimeState::Initializing) {
                    return Err(RuntimeControlPlaneError::InvalidState { state });
                }
                let previous_dsl_state = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::Destroy,
                        "Destroy",
                    )
                    .await
                    .map_err(RuntimeControlPlaneError::Internal)?;

                let mut drv = driver.lock().await;
                let report = match machine_destroy(&mut drv).await {
                    Ok(report) => report,
                    Err(err) => {
                        drop(drv);
                        self.restore_session_dsl_state(&session_id, previous_dsl_state)
                            .await;
                        return Err(RuntimeControlPlaneError::Internal(err.to_string()));
                    }
                };
                drop(drv);

                let mut comp = completions.lock().await;
                comp.resolve_all_terminated("runtime destroyed");
                Ok(MeerkatMachineCommandResult::DestroyReport(report))
            }
            MeerkatMachineCommand::RuntimeState { runtime_id } => {
                let session_id = Self::resolve_session_id(&runtime_id)?;
                let state = self
                    .existing_session_runtime_state(&session_id)
                    .await
                    .ok_or(RuntimeControlPlaneError::NotFound(runtime_id))?;
                Ok(MeerkatMachineCommandResult::RuntimeState(state))
            }
            MeerkatMachineCommand::RuntimeRealtimeAttachmentStatus { session_id } => {
                let sessions = self.sessions.read().await;
                let entry = sessions.get(&session_id).ok_or_else(|| {
                    RuntimeControlPlaneError::NotFound(LogicalRuntimeId::new(
                        session_id.to_string(),
                    ))
                })?;
                let authority = entry
                    .dsl_authority
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let status = project_realtime_attachment_status(&authority.state);
                Ok(MeerkatMachineCommandResult::RealtimeAttachmentStatus(
                    status,
                ))
            }
            MeerkatMachineCommand::LoadBoundaryReceipt {
                runtime_id,
                run_id,
                sequence,
            } => {
                let receipt = match &self.store {
                    Some(store) => store
                        .load_boundary_receipt(&runtime_id, &run_id, sequence)
                        .await
                        .map_err(|e| RuntimeControlPlaneError::StoreError(e.to_string()))?,
                    None => None,
                };
                Ok(MeerkatMachineCommandResult::BoundaryReceipt(receipt))
            }
            _ => unreachable!("non-control command routed to control handler"),
        }
    }
}

/// Project the DSL state's realtime-binding fields onto the shell-facing
/// `RealtimeAttachmentStatus` enum. The DSL owns the canonical fact; this
/// projection is pure and maintains the dogma's "derived projections are
/// rebuildable, never authoritative" principle.
fn project_realtime_attachment_status(
    state: &super::dsl::MeerkatMachineState,
) -> crate::meerkat_machine_types::RealtimeAttachmentStatus {
    use crate::meerkat_machine_types::RealtimeAttachmentStatus;
    if state.realtime_reattach_required {
        return RealtimeAttachmentStatus::ReattachRequired;
    }
    match state.realtime_binding_state.as_str() {
        "Unbound" => {
            if state.realtime_intent_present {
                RealtimeAttachmentStatus::IntentPresentUnbound
            } else {
                RealtimeAttachmentStatus::Unattached
            }
        }
        "BindingNotReady" => RealtimeAttachmentStatus::BindingNotReady,
        "BindingReady" => RealtimeAttachmentStatus::BindingReady,
        "ReplacementPending" => RealtimeAttachmentStatus::ReplacementPending,
        _ => RealtimeAttachmentStatus::Unattached,
    }
}
