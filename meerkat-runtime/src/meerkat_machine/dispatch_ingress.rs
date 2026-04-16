use super::*;

impl MeerkatMachine {
    pub(super) async fn execute_meerkat_machine_ingress_command(
        &self,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        match command {
            MeerkatMachineCommand::AcceptWithCompletion { session_id, input } => {
                let (driver, completions, wake_tx, control_tx) = {
                    let sessions = self.sessions.read().await;
                    let entry = sessions
                        .get(&session_id)
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?;
                    (
                        entry.driver.clone(),
                        entry.completions.clone(),
                        entry.wake_sender(),
                        entry.control_sender(),
                    )
                };

                let state = self
                    .existing_session_runtime_state(&session_id)
                    .await
                    .unwrap_or(RuntimeState::Destroyed);
                if matches!(
                    state,
                    RuntimeState::Retired | RuntimeState::Stopped | RuntimeState::Destroyed
                ) {
                    return Err(match state {
                        RuntimeState::Destroyed => RuntimeDriverError::Destroyed,
                        RuntimeState::Retired | RuntimeState::Stopped => {
                            RuntimeDriverError::NotReady { state }
                        }
                        _ => unreachable!("guard only matches retired/stopped/destroyed"),
                    });
                }

                let request_immediate_processing =
                    crate::accept::requests_immediate_processing(&input);
                let (outcome, signal, handle) = {
                    let mut driver = driver.lock().await;
                    let result = driver
                        .as_driver_mut()
                        .accept_input(input)
                        .await
                        .map_err(Self::normalize_destroyed_error)?;
                    let signal =
                        Self::machine_owned_admission_signal(&result, request_immediate_processing);

                    match &result {
                        AcceptOutcome::Accepted { input_id, .. } => {
                            let is_terminal = driver
                                .as_driver()
                                .input_state(input_id)
                                .map(|state| state.current_state().is_terminal())
                                .unwrap_or(true);
                            let handle = if is_terminal {
                                None
                            } else {
                                Some({
                                    let mut completions = completions.lock().await;
                                    completions.register(input_id.clone())
                                })
                            };
                            (result, signal, handle)
                        }
                        AcceptOutcome::Deduplicated { existing_id, .. } => {
                            let existing_state = driver.as_driver().input_state(existing_id);
                            let is_terminal = existing_state
                                .map(|s| s.current_state().is_terminal())
                                .unwrap_or(true);

                            if is_terminal {
                                (result, signal, None)
                            } else {
                                let handle = {
                                    let mut completions = completions.lock().await;
                                    completions.register(existing_id.clone())
                                };
                                (result, signal, Some(handle))
                            }
                        }
                        AcceptOutcome::Rejected { reason } => {
                            return Err(RuntimeDriverError::ValidationFailed {
                                reason: reason.to_string(),
                            });
                        }
                    }
                };

                if signal.should_wake()
                    && let Some(ref wake_tx) = wake_tx
                {
                    let _ = wake_tx.try_send(());
                }
                if signal.should_interrupt_yielding()
                    && let Some(ref tx) = control_tx
                {
                    let _ = tx.try_send(
                        meerkat_core::lifecycle::run_control::RunControlCommand::InterruptYielding,
                    );
                }

                if let Err(err) = self.sync_session_dsl_projection(&session_id).await {
                    tracing::error!(
                        error = %err,
                        "failed to resync DSL projection after AcceptWithCompletion"
                    );
                }
                Ok(MeerkatMachineCommandResult::AcceptWithCompletion {
                    outcome,
                    handle,
                    admission_signal: signal,
                })
            }
            MeerkatMachineCommand::AcceptWithoutWake { session_id, input } => {
                let driver = {
                    let sessions = self.sessions.read().await;
                    let entry = sessions
                        .get(&session_id)
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?;
                    entry.driver.clone()
                };

                let state = self
                    .existing_session_runtime_state(&session_id)
                    .await
                    .unwrap_or(RuntimeState::Destroyed);
                if matches!(
                    state,
                    RuntimeState::Retired | RuntimeState::Stopped | RuntimeState::Destroyed
                ) {
                    return Err(match state {
                        RuntimeState::Destroyed => RuntimeDriverError::Destroyed,
                        RuntimeState::Retired | RuntimeState::Stopped => {
                            RuntimeDriverError::NotReady { state }
                        }
                        _ => unreachable!("guard only matches retired/stopped/destroyed"),
                    });
                }

                let request_immediate_processing =
                    crate::accept::requests_immediate_processing(&input);
                let outcome = {
                    let mut driver = driver.lock().await;
                    let result = driver
                        .as_driver_mut()
                        .accept_input(input)
                        .await
                        .map_err(Self::normalize_destroyed_error)?;
                    let signal =
                        Self::machine_owned_admission_signal(&result, request_immediate_processing);
                    debug_assert!(
                        !signal.should_process_immediately(),
                        "queue-only admission unexpectedly requested immediate processing"
                    );
                    result
                };

                if let Err(err) = self.sync_session_dsl_projection(&session_id).await {
                    tracing::error!(
                        error = %err,
                        "failed to resync DSL projection after AcceptWithoutWake"
                    );
                }

                Ok(MeerkatMachineCommandResult::AcceptOutcome(outcome))
            }
            _ => unreachable!("non-ingress command routed to ingress handler"),
        }
    }

    pub(super) async fn execute_meerkat_machine_legacy_run_command(
        &self,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        match command {
            MeerkatMachineCommand::Prepare { session_id, input } => {
                let driver = {
                    let sessions = self.sessions.read().await;
                    sessions
                        .get(&session_id)
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?
                        .driver
                        .clone()
                };

                let prepared = {
                    let mut driver = driver.lock().await;
                    if !driver.is_idle_or_attached() {
                        return Err(Self::normalize_destroyed_error(
                            RuntimeDriverError::NotReady {
                                state: driver.as_driver().runtime_state(),
                            },
                        ));
                    }

                    let active_input_ids = driver.as_driver().active_input_ids();
                    if !active_input_ids.is_empty() {
                        let duplicate_active_input =
                            input.header().idempotency_key.as_ref().and_then(|key| {
                                active_input_ids.iter().find(|active_id| {
                                    driver
                                        .as_driver()
                                        .input_state(active_id)
                                        .and_then(|state| state.idempotency_key.as_ref())
                                        == Some(key)
                                })
                            });
                        if let Some(existing_id) = duplicate_active_input {
                            return Err(RuntimeDriverError::ValidationFailed {
                                reason: format!(
                                    "accept_input_and_run does not support deduplicated admission; existing input {existing_id} already owns execution"
                                ),
                            });
                        }
                        return Err(RuntimeDriverError::NotReady {
                            state: driver.as_driver().runtime_state(),
                        });
                    }

                    let outcome = driver
                        .as_driver_mut()
                        .accept_input(input)
                        .await
                        .map_err(Self::normalize_destroyed_error)?;
                    let input_id = match outcome {
                        AcceptOutcome::Accepted { input_id, .. } => input_id,
                        AcceptOutcome::Deduplicated { existing_id, .. } => {
                            return Err(RuntimeDriverError::ValidationFailed {
                                reason: format!(
                                    "accept_input_and_run does not support deduplicated admission; existing input {existing_id} already owns execution"
                                ),
                            });
                        }
                        AcceptOutcome::Rejected { reason } => {
                            return Err(RuntimeDriverError::ValidationFailed {
                                reason: reason.to_string(),
                            });
                        }
                    };

                    if !driver.is_idle_or_attached() {
                        return Err(Self::normalize_destroyed_error(
                            RuntimeDriverError::NotReady {
                                state: driver.as_driver().runtime_state(),
                            },
                        ));
                    }

                    let (dequeued_id, dequeued_input) = driver.dequeue_next().ok_or_else(|| {
                        RuntimeDriverError::Internal(
                            "accepted input was not queued for execution".into(),
                        )
                    })?;
                    if dequeued_id != input_id {
                        return Err(Self::normalize_destroyed_error(
                            RuntimeDriverError::NotReady {
                                state: driver.as_driver().runtime_state(),
                            },
                        ));
                    }

                    let run_id = RunId::new();
                    machine_begin_run(&mut driver, run_id.clone()).map_err(|err| {
                        RuntimeDriverError::Internal(format!("failed to start runtime run: {err}"))
                    })?;
                    if let Err(err) = driver.stage_input(&dequeued_id, &run_id) {
                        let _ = driver.rollback_staged(std::slice::from_ref(&dequeued_id));
                        let next_phase = crate::runtime_state::run_return_phase_from_pre_run_phase(
                            driver.pre_run_phase(),
                        );
                        let _ =
                            machine_apply_run_return_projection(&mut driver, &run_id, next_phase);
                        return Err(RuntimeDriverError::Internal(format!(
                            "failed to stage accepted input: {err}"
                        )));
                    }

                    MeerkatMachineLegacyRunPrepared {
                        input_id,
                        run_id,
                        primitive: crate::runtime_loop::input_to_primitive(
                            &dequeued_input,
                            dequeued_id,
                        ),
                    }
                };

                if let Err(err) = self.sync_session_dsl_projection(&session_id).await {
                    tracing::error!(error = %err, "failed to resync DSL projection after Prepare");
                }

                Ok(MeerkatMachineCommandResult::Prepared(prepared))
            }
            MeerkatMachineCommand::Commit {
                session_id,
                input_id,
                run_id,
                output,
            } => {
                let shadow_input_id = input_id.clone();
                let shadow_run_id = run_id.clone();
                let driver = {
                    let sessions = self.sessions.read().await;
                    sessions
                        .get(&session_id)
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?
                        .driver
                        .clone()
                };

                let previous_dsl_state = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::Commit {
                            input_id: crate::meerkat_machine::dsl::InputId::from_domain(
                                &shadow_input_id,
                            ),
                            run_id: crate::meerkat_machine::dsl::RunId::from_domain(&shadow_run_id),
                        },
                        "Commit",
                    )
                    .await
                    .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

                if let Err(err) = commit_runtime_loop_run(
                    &driver,
                    run_id,
                    vec![input_id],
                    output.receipt,
                    output.session_snapshot,
                )
                .await
                {
                    self.restore_session_dsl_state(&session_id, previous_dsl_state)
                        .await;
                    let should_unregister =
                        !err.to_string().contains("runtime boundary commit failed");
                    if should_unregister {
                        self.unregister_session_inner(&session_id).await;
                    }
                    return Err(RuntimeDriverError::Internal(format!(
                        "runtime commit failed: {err}"
                    )));
                }

                if let Err(err) = self.sync_session_dsl_projection(&session_id).await {
                    self.restore_session_dsl_state(&session_id, previous_dsl_state)
                        .await;
                    return Err(err);
                }

                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::Fail {
                session_id,
                run_id,
                error,
            } => {
                let shadow_fail_run_id = run_id.clone();
                let driver = {
                    let sessions = self.sessions.read().await;
                    sessions
                        .get(&session_id)
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?
                        .driver
                        .clone()
                };

                let previous_dsl_state = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::Fail {
                            run_id: crate::meerkat_machine::dsl::RunId::from_domain(
                                &shadow_fail_run_id,
                            ),
                        },
                        "Fail",
                    )
                    .await
                    .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

                if let Err(run_err) = fail_runtime_loop_run(&driver, run_id, error).await {
                    self.restore_session_dsl_state(&session_id, previous_dsl_state)
                        .await;
                    self.unregister_session_inner(&session_id).await;
                    return Err(RuntimeDriverError::Internal(format!(
                        "failed to persist runtime failure snapshot: {run_err}"
                    )));
                }

                if let Err(err) = self.sync_session_dsl_projection(&session_id).await {
                    self.restore_session_dsl_state(&session_id, previous_dsl_state)
                        .await;
                    return Err(err);
                }

                Ok(MeerkatMachineCommandResult::Unit)
            }
            _ => unreachable!("non-legacy-run command routed to legacy-run handler"),
        }
    }
}
