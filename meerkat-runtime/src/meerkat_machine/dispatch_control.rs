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

                let state = Self::driver_runtime_state(&driver).await;
                if matches!(state, RuntimeState::Destroyed | RuntimeState::Stopped) {
                    return Err(RuntimeControlPlaneError::InvalidState { state });
                }

                let mut drv = driver.lock().await;
                let mut report = machine_retire(&mut drv)
                    .await
                    .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
                drop(drv);

                if report.inputs_pending_drain > 0 {
                    if let Some(ref tx) = wake_tx
                        && tx.send(()).await.is_ok()
                    {
                        let result = MeerkatMachineCommandResult::RetireReport(report);
                        if let Some(entry) = self.sessions.write().await.get_mut(&session_id) {
                            // DSL shadow
                            use crate::meerkat_machine::dsl as mm_dsl;
                            if let Err(e) = mm_dsl::MeerkatMachineMutator::apply(
                                &mut entry.dsl_authority,
                                mm_dsl::MeerkatMachineInput::Retire,
                            ) {
                                tracing::error!(
                                    error = %e,
                                    "DSL/runtime DISAGREEMENT on Retire"
                                );
                            }
                        }
                        return Ok(result);
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

                let result = MeerkatMachineCommandResult::RetireReport(report);
                if let Some(entry) = self.sessions.write().await.get_mut(&session_id) {
                    // DSL shadow
                    use crate::meerkat_machine::dsl as mm_dsl;
                    if let Err(e) = mm_dsl::MeerkatMachineMutator::apply(
                        &mut entry.dsl_authority,
                        mm_dsl::MeerkatMachineInput::Retire,
                    ) {
                        tracing::error!(
                            error = %e,
                            "DSL/runtime DISAGREEMENT on Retire"
                        );
                    }
                }
                Ok(result)
            }
            MeerkatMachineCommand::Recycle { runtime_id } => {
                let (session_id, driver, completions, wake_tx) =
                    self.lookup_entry(&runtime_id).await?;

                let state = Self::driver_runtime_state(&driver).await;
                if matches!(state, RuntimeState::Destroyed | RuntimeState::Running) {
                    return Err(RuntimeControlPlaneError::InvalidState { state });
                }

                let (transferred, active_after_recycle) = {
                    let mut drv = driver.lock().await;
                    let transferred = machine_recycle_preserving_work(&mut drv)
                        .await
                        .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;

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

                let result = MeerkatMachineCommandResult::RecycleReport(RecycleReport {
                    inputs_transferred: transferred,
                });
                if let Some(entry) = self.sessions.write().await.get_mut(&session_id) {
                    // DSL shadow
                    use crate::meerkat_machine::dsl as mm_dsl;
                    if let Err(e) = mm_dsl::MeerkatMachineMutator::apply(
                        &mut entry.dsl_authority,
                        mm_dsl::MeerkatMachineInput::Recycle,
                    ) {
                        tracing::error!(
                            error = %e,
                            "DSL/runtime DISAGREEMENT on Recycle"
                        );
                    }
                }
                Ok(result)
            }
            MeerkatMachineCommand::Reset { runtime_id } => {
                let (session_id, driver, completions, _wake_tx) =
                    self.lookup_entry(&runtime_id).await?;

                let state = Self::driver_runtime_state(&driver).await;
                if matches!(state, RuntimeState::Destroyed | RuntimeState::Running) {
                    return Err(RuntimeControlPlaneError::InvalidState { state });
                }

                let mut drv = driver.lock().await;
                let report = machine_reset(&mut drv)
                    .await
                    .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
                drop(drv);

                let mut comp = completions.lock().await;
                comp.resolve_all_terminated("runtime reset");

                let result = MeerkatMachineCommandResult::ResetReport(report);
                if let Some(entry) = self.sessions.write().await.get_mut(&session_id) {
                    // DSL shadow
                    use crate::meerkat_machine::dsl as mm_dsl;
                    if let Err(e) = mm_dsl::MeerkatMachineMutator::apply(
                        &mut entry.dsl_authority,
                        mm_dsl::MeerkatMachineInput::Reset,
                    ) {
                        tracing::error!(
                            error = %e,
                            "DSL/runtime DISAGREEMENT on Reset"
                        );
                    }
                }
                Ok(result)
            }
            MeerkatMachineCommand::Recover { runtime_id } => {
                let (session_id, driver, completions, wake_tx) =
                    self.lookup_entry(&runtime_id).await?;

                let state = Self::driver_runtime_state(&driver).await;
                if matches!(state, RuntimeState::Destroyed | RuntimeState::Running) {
                    return Err(RuntimeControlPlaneError::InvalidState { state });
                }

                let (report, active_after_recover) = {
                    let mut drv = driver.lock().await;
                    let report = drv
                        .as_driver_mut()
                        .recover()
                        .await
                        .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
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

                let result = MeerkatMachineCommandResult::RecoveryReport(report);
                if let Some(entry) = self.sessions.write().await.get_mut(&session_id) {
                    // DSL shadow
                    use crate::meerkat_machine::dsl as mm_dsl;
                    if let Err(e) = mm_dsl::MeerkatMachineMutator::apply(
                        &mut entry.dsl_authority,
                        mm_dsl::MeerkatMachineInput::Recover,
                    ) {
                        tracing::error!(
                            error = %e,
                            "DSL/runtime DISAGREEMENT on Recover"
                        );
                    }
                }
                Ok(result)
            }
            MeerkatMachineCommand::Destroy { runtime_id } => {
                let (session_id, driver, completions, _wake_tx) =
                    self.lookup_entry(&runtime_id).await?;

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

                let mut drv = driver.lock().await;
                let report = machine_destroy(&mut drv)
                    .await
                    .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
                drop(drv);

                let mut comp = completions.lock().await;
                comp.resolve_all_terminated("runtime destroyed");

                let result = MeerkatMachineCommandResult::DestroyReport(report);
                if let Some(entry) = self.sessions.write().await.get_mut(&session_id) {
                    // DSL shadow
                    use crate::meerkat_machine::dsl as mm_dsl;
                    if let Err(e) = mm_dsl::MeerkatMachineMutator::apply(
                        &mut entry.dsl_authority,
                        mm_dsl::MeerkatMachineInput::Destroy,
                    ) {
                        tracing::error!(
                            error = %e,
                            "DSL/runtime DISAGREEMENT on Destroy"
                        );
                    }
                }
                Ok(result)
            }
            MeerkatMachineCommand::RuntimeState { runtime_id } => {
                let session_id = Self::resolve_session_id(&runtime_id)?;
                let sessions = self.sessions.read().await;
                let entry = sessions
                    .get(&session_id)
                    .ok_or(RuntimeControlPlaneError::NotFound(runtime_id.clone()))?;
                Ok(MeerkatMachineCommandResult::RuntimeState(
                    entry.control_snapshot().phase,
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
