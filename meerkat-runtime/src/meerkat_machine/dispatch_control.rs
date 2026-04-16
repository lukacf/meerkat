use super::*;

impl MeerkatMachine {
    pub(super) async fn execute_meerkat_machine_control_command(
        &self,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeControlPlaneError> {
        match command {
            MeerkatMachineCommand::Ingest { runtime_id, input } => {
                let (_session_id, driver, _completions, wake_tx, control_tx) = {
                    let (sid, d, c, w) = self.lookup_entry(&runtime_id).await?;
                    let ctrl = {
                        let sessions = self.sessions.read().await;
                        sessions
                            .get(&sid)
                            .and_then(RuntimeSessionEntry::control_sender)
                    };
                    (sid, d, c, w, ctrl)
                };

                // Guard: AdmitQueuedInput requires phase not in
                // {Retired, Stopped, Destroyed}. Steered inputs are more
                // permissive but the dispatch cannot distinguish here, so we
                // reject all three conservatively.
                let state = Self::driver_runtime_state(&driver).await;
                if matches!(
                    state,
                    RuntimeState::Retired | RuntimeState::Stopped | RuntimeState::Destroyed
                ) {
                    return Err(RuntimeControlPlaneError::InvalidState { state });
                }

                let request_immediate_processing =
                    crate::accept::requests_immediate_processing(&input);
                let (outcome, signal) = {
                    let mut drv = driver.lock().await;
                    let result = drv
                        .as_driver_mut()
                        .accept_input(input)
                        .await
                        .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
                    let signal =
                        Self::machine_owned_admission_signal(&result, request_immediate_processing);
                    (result, signal)
                };

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
                let (_session_id, driver, _completions, _wake_tx) =
                    self.lookup_entry(&runtime_id).await?;

                if matches!(
                    Self::driver_runtime_state(&driver).await,
                    RuntimeState::Destroyed
                ) {
                    return Err(RuntimeControlPlaneError::InvalidState {
                        state: RuntimeState::Destroyed,
                    });
                }

                let mut drv = driver.lock().await;
                drv.as_driver_mut()
                    .on_runtime_event(event)
                    .await
                    .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
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

                let state = Self::driver_runtime_state(&driver).await;
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
                        if let Err(err) = self.sync_session_dsl_projection(&session_id).await {
                            self.restore_session_dsl_state(&session_id, previous_dsl_state)
                                .await;
                            return Err(RuntimeControlPlaneError::Internal(err.to_string()));
                        }
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

                if let Err(err) = self.sync_session_dsl_projection(&session_id).await {
                    self.restore_session_dsl_state(&session_id, previous_dsl_state)
                        .await;
                    return Err(RuntimeControlPlaneError::Internal(err.to_string()));
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

                let state = Self::driver_runtime_state(&driver).await;
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

                if let Err(err) = self.sync_session_dsl_projection(&session_id).await {
                    self.restore_session_dsl_state(&session_id, previous_dsl_state)
                        .await;
                    return Err(RuntimeControlPlaneError::Internal(err.to_string()));
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

                let state = Self::driver_runtime_state(&driver).await;
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

                if let Err(err) = self.sync_session_dsl_projection(&session_id).await {
                    self.restore_session_dsl_state(&session_id, previous_dsl_state)
                        .await;
                    return Err(RuntimeControlPlaneError::Internal(err.to_string()));
                }
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

                let state = Self::driver_runtime_state(&driver).await;
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

                if let Err(err) = self.sync_session_dsl_projection(&session_id).await {
                    self.restore_session_dsl_state(&session_id, previous_dsl_state)
                        .await;
                    return Err(RuntimeControlPlaneError::Internal(err.to_string()));
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

                let state = Self::driver_runtime_state(&driver).await;
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

                if let Err(err) = self.sync_session_dsl_projection(&session_id).await {
                    self.restore_session_dsl_state(&session_id, previous_dsl_state)
                        .await;
                    return Err(RuntimeControlPlaneError::Internal(err.to_string()));
                }
                Ok(MeerkatMachineCommandResult::DestroyReport(report))
            }
            MeerkatMachineCommand::RuntimeState { runtime_id } => {
                let session_id = Self::resolve_session_id(&runtime_id)?;
                self.sync_session_dsl_projection(&session_id)
                    .await
                    .map_err(|err| RuntimeControlPlaneError::Internal(err.to_string()))?;
                let sessions = self.sessions.read().await;
                let entry = sessions
                    .get(&session_id)
                    .ok_or(RuntimeControlPlaneError::NotFound(runtime_id.clone()))?;
                Ok(MeerkatMachineCommandResult::RuntimeState(
                    crate::meerkat_machine::dsl_authority::write_back_phase(
                        entry.dsl_authority.state.lifecycle_phase,
                    ),
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
