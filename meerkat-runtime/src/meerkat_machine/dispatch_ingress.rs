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

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
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

                // DSL-first: validate the coarse AcceptWithCompletion transition
                // before the driver realizes the effect.
                let run_id_for_dsl = RunId::new();
                // We don't have the input_id yet (driver hasn't accepted), so use a
                // placeholder InputId. The DSL transition for AcceptWithCompletion
                // doesn't gate on input_id value — it gates on phase/state.
                let dsl_input_id =
                    crate::meerkat_machine::dsl::InputId::from_domain(&InputId::new());
                let previous_dsl_state = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::AcceptWithCompletion {
                            input_id: dsl_input_id,
                            request_immediate_processing,
                            // We don't know interrupt_yielding yet (depends on outcome),
                            // but the DSL transition doesn't gate on this field.
                            interrupt_yielding: false,
                            run_id: crate::meerkat_machine::dsl::RunId::from_domain(
                                &run_id_for_dsl,
                            ),
                        },
                        "AcceptWithCompletion",
                    )
                    .await
                    .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

                let (outcome, signal, handle) = {
                    let mut driver = driver.lock().await;
                    let result = match driver
                        .as_driver_mut()
                        .accept_input(input)
                        .await
                        .map_err(Self::normalize_destroyed_error)
                    {
                        Ok(r) => r,
                        Err(err) => {
                            self.restore_session_dsl_state(&session_id, previous_dsl_state)
                                .await;
                            return Err(err);
                        }
                    };
                    let signal =
                        Self::machine_owned_admission_signal(&result, request_immediate_processing);

                    match &result {
                        AcceptOutcome::Accepted { input_id, .. } => {
                            let is_terminal = driver
                                .as_driver()
                                .input_phase(input_id)
                                .map(|phase| phase.is_terminal())
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
                            let is_terminal = driver
                                .as_driver()
                                .input_phase(existing_id)
                                .map(|phase| phase.is_terminal())
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
                            self.restore_session_dsl_state(&session_id, previous_dsl_state)
                                .await;
                            return Err(RuntimeDriverError::ValidationFailed {
                                reason: reason.to_string(),
                            });
                        }
                    }
                };

                // Input lifecycle shadow: infer from outcome which transition fired
                let shadow_input_id = match &outcome {
                    AcceptOutcome::Accepted { input_id, .. } => input_id.clone(),
                    AcceptOutcome::Deduplicated { existing_id, .. } => existing_id.clone(),
                    AcceptOutcome::Rejected { .. } => {
                        unreachable!("rejected case is returned above")
                    }
                };
                {
                    let is_terminal = match &outcome {
                        AcceptOutcome::Accepted { input_id, .. } => {
                            let drv = driver.lock().await;
                            drv.as_driver()
                                .input_phase(input_id)
                                .map(|phase| phase.is_terminal())
                                .unwrap_or(true)
                        }
                        _ => false,
                    };
                    let shadow_input = if is_terminal {
                        Some(
                            crate::meerkat_machine::dsl::MeerkatMachineInput::ConsumeOnAccept {
                                input_id: shadow_input_id.to_string(),
                            },
                        )
                    } else if matches!(&outcome, AcceptOutcome::Accepted { .. }) {
                        Some(
                            crate::meerkat_machine::dsl::MeerkatMachineInput::QueueAccepted {
                                input_id: shadow_input_id.to_string(),
                            },
                        )
                    } else {
                        None // Deduplicated — no new input lifecycle transition
                    };
                    if let Some(input) = shadow_input
                        && let Err(err) = self
                            .stage_session_dsl_input(&session_id, input, "InputLifecycle(accept)")
                            .await
                    {
                        tracing::warn!(
                            session_id = %session_id,
                            error = %err,
                            "DSL/runtime DISAGREEMENT on input lifecycle after accept"
                        );
                    }
                }

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

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
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

                // DSL-first: validate AcceptWithoutWake before driver realizes the effect.
                let dsl_input_id =
                    crate::meerkat_machine::dsl::InputId::from_domain(&InputId::new());
                let previous_dsl_state = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::AcceptWithoutWake {
                            input_id: dsl_input_id,
                        },
                        "AcceptWithoutWake",
                    )
                    .await
                    .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

                let outcome = {
                    let mut driver = driver.lock().await;
                    let result = match driver
                        .as_driver_mut()
                        .accept_input(input)
                        .await
                        .map_err(Self::normalize_destroyed_error)
                    {
                        Ok(r) => r,
                        Err(err) => {
                            self.restore_session_dsl_state(&session_id, previous_dsl_state)
                                .await;
                            return Err(err);
                        }
                    };
                    let signal =
                        Self::machine_owned_admission_signal(&result, request_immediate_processing);
                    debug_assert!(
                        !signal.should_process_immediately(),
                        "queue-only admission unexpectedly requested immediate processing"
                    );
                    if let AcceptOutcome::Rejected { reason } = &result {
                        self.restore_session_dsl_state(&session_id, previous_dsl_state)
                            .await;
                        return Err(RuntimeDriverError::ValidationFailed {
                            reason: reason.to_string(),
                        });
                    }
                    result
                };
                let shadow_input_id = match &outcome {
                    AcceptOutcome::Accepted { input_id, .. } => input_id.clone(),
                    AcceptOutcome::Deduplicated { existing_id, .. } => existing_id.clone(),
                    AcceptOutcome::Rejected { .. } => {
                        unreachable!("rejected case handled above")
                    }
                };

                // Input lifecycle shadow: infer from outcome which transition fired
                {
                    let is_terminal = match &outcome {
                        AcceptOutcome::Accepted { input_id, .. } => {
                            let drv = driver.lock().await;
                            drv.as_driver()
                                .input_phase(input_id)
                                .map(|phase| phase.is_terminal())
                                .unwrap_or(true)
                        }
                        _ => false,
                    };
                    let shadow_input = if is_terminal {
                        Some(
                            crate::meerkat_machine::dsl::MeerkatMachineInput::ConsumeOnAccept {
                                input_id: shadow_input_id.to_string(),
                            },
                        )
                    } else if matches!(&outcome, AcceptOutcome::Accepted { .. }) {
                        Some(
                            crate::meerkat_machine::dsl::MeerkatMachineInput::QueueAccepted {
                                input_id: shadow_input_id.to_string(),
                            },
                        )
                    } else {
                        None // Deduplicated — no new input lifecycle transition
                    };
                    if let Some(input) = shadow_input
                        && let Err(err) = self
                            .stage_session_dsl_input(
                                &session_id,
                                input,
                                "InputLifecycle(accept_without_wake)",
                            )
                            .await
                    {
                        tracing::warn!(
                            session_id = %session_id,
                            error = %err,
                            "DSL/runtime DISAGREEMENT on input lifecycle after accept_without_wake"
                        );
                    }
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

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                // DSL-first: validate Prepare transition before driver realizes the effect.
                let run_id = RunId::new();
                let previous_dsl_state = match self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::Prepare {
                            session_id: crate::meerkat_machine::dsl::SessionId::from_domain(
                                &session_id,
                            ),
                            run_id: crate::meerkat_machine::dsl::RunId::from_domain(&run_id),
                        },
                        "Prepare",
                    )
                    .await
                {
                    Ok(state) => state,
                    Err(reason) => {
                        tracing::error!(
                            error = %reason,
                            session_id = %session_id,
                            "DSL rejected Prepare input"
                        );
                        let state = self
                            .existing_session_runtime_state(&session_id)
                            .await
                            .unwrap_or(RuntimeState::Destroyed);
                        return Err(Self::normalize_destroyed_error(
                            RuntimeDriverError::NotReady { state },
                        ));
                    }
                };

                let prepared = {
                    let mut driver = driver.lock().await;
                    if !driver.is_idle_or_attached() {
                        self.restore_session_dsl_state(&session_id, previous_dsl_state)
                            .await;
                        return Err(Self::normalize_destroyed_error(
                            RuntimeDriverError::NotReady {
                                state: self
                                    .existing_session_runtime_state(&session_id)
                                    .await
                                    .unwrap_or(RuntimeState::Destroyed),
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
                            self.restore_session_dsl_state(&session_id, previous_dsl_state)
                                .await;
                            return Err(RuntimeDriverError::ValidationFailed {
                                reason: format!(
                                    "accept_input_and_run does not support deduplicated admission; existing input {existing_id} already owns execution"
                                ),
                            });
                        }
                        self.restore_session_dsl_state(&session_id, previous_dsl_state)
                            .await;
                        return Err(RuntimeDriverError::NotReady {
                            state: self
                                .existing_session_runtime_state(&session_id)
                                .await
                                .unwrap_or(RuntimeState::Destroyed),
                        });
                    }

                    let outcome = match driver
                        .as_driver_mut()
                        .accept_input(input)
                        .await
                        .map_err(Self::normalize_destroyed_error)
                    {
                        Ok(o) => o,
                        Err(err) => {
                            self.restore_session_dsl_state(&session_id, previous_dsl_state)
                                .await;
                            return Err(err);
                        }
                    };
                    let input_id = match outcome {
                        AcceptOutcome::Accepted { input_id, .. } => input_id,
                        AcceptOutcome::Deduplicated { existing_id, .. } => {
                            self.restore_session_dsl_state(&session_id, previous_dsl_state)
                                .await;
                            return Err(RuntimeDriverError::ValidationFailed {
                                reason: format!(
                                    "accept_input_and_run does not support deduplicated admission; existing input {existing_id} already owns execution"
                                ),
                            });
                        }
                        AcceptOutcome::Rejected { reason } => {
                            self.restore_session_dsl_state(&session_id, previous_dsl_state)
                                .await;
                            return Err(RuntimeDriverError::ValidationFailed {
                                reason: reason.to_string(),
                            });
                        }
                    };

                    if !driver.is_idle_or_attached() {
                        self.restore_session_dsl_state(&session_id, previous_dsl_state)
                            .await;
                        return Err(Self::normalize_destroyed_error(
                            RuntimeDriverError::NotReady {
                                state: self
                                    .existing_session_runtime_state(&session_id)
                                    .await
                                    .unwrap_or(RuntimeState::Destroyed),
                            },
                        ));
                    }

                    let (dequeued_id, dequeued_input) = match driver.dequeue_next() {
                        Some(pair) => pair,
                        None => {
                            self.restore_session_dsl_state(&session_id, previous_dsl_state)
                                .await;
                            return Err(RuntimeDriverError::Internal(
                                "accepted input was not queued for execution".into(),
                            ));
                        }
                    };
                    if dequeued_id != input_id {
                        self.restore_session_dsl_state(&session_id, previous_dsl_state)
                            .await;
                        return Err(Self::normalize_destroyed_error(
                            RuntimeDriverError::NotReady {
                                state: self
                                    .existing_session_runtime_state(&session_id)
                                    .await
                                    .unwrap_or(RuntimeState::Destroyed),
                            },
                        ));
                    }

                    if let Err(err) = machine_begin_run(&mut driver, run_id.clone()) {
                        self.restore_session_dsl_state(&session_id, previous_dsl_state)
                            .await;
                        return Err(RuntimeDriverError::Internal(format!(
                            "failed to start runtime run: {err}"
                        )));
                    }
                    if let Err(err) = driver.stage_input(&dequeued_id, &run_id) {
                        let _ = driver.rollback_staged(std::slice::from_ref(&dequeued_id));
                        let next_phase = crate::runtime_state::run_return_phase_from_pre_run_phase(
                            driver.pre_run_phase(),
                        );
                        let _ =
                            machine_apply_run_return_projection(&mut driver, &run_id, next_phase);
                        self.restore_session_dsl_state(&session_id, previous_dsl_state)
                            .await;
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

                // Input lifecycle shadows: accept queued + stage for run
                for shadow in [
                    crate::meerkat_machine::dsl::MeerkatMachineInput::QueueAccepted {
                        input_id: prepared.input_id.to_string(),
                    },
                    crate::meerkat_machine::dsl::MeerkatMachineInput::StageForRun {
                        input_id: prepared.input_id.to_string(),
                        run_id: prepared.run_id.to_string(),
                    },
                    crate::meerkat_machine::dsl::MeerkatMachineInput::IncrementAttemptCount {
                        input_id: prepared.input_id.to_string(),
                    },
                ] {
                    if let Err(err) = self
                        .stage_session_dsl_input(&session_id, shadow, "InputLifecycle(prepare)")
                        .await
                    {
                        tracing::warn!(
                            session_id = %session_id,
                            error = %err,
                            "DSL/runtime DISAGREEMENT on input lifecycle during prepare"
                        );
                    }
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

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
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

                // DSL-first: stage ConsumeInput before the driver commit realizes it.
                if let Err(err) = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::ConsumeInput {
                            input_id: shadow_input_id.to_string(),
                        },
                        "InputLifecycle(commit)",
                    )
                    .await
                {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %err,
                        "DSL/runtime DISAGREEMENT on input lifecycle during commit"
                    );
                }

                let commit_run_id = run_id.clone();
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
                    // Driver unwinds Running → pre-run phase on commit failure.
                    // Apply DSL Fail to keep the machine phase in sync.
                    if let Err(dsl_err) = self
                        .stage_session_dsl_input(
                            &session_id,
                            crate::meerkat_machine::dsl::MeerkatMachineInput::Fail {
                                run_id: crate::meerkat_machine::dsl::RunId::from_domain(
                                    &commit_run_id,
                                ),
                            },
                            "Fail(commit_unwind)",
                        )
                        .await
                    {
                        tracing::warn!(
                            session_id = %session_id,
                            error = %dsl_err,
                            "DSL rejected Fail unwind after commit failure"
                        );
                    }
                    let should_unregister =
                        !err.to_string().contains("runtime boundary commit failed");
                    if should_unregister {
                        self.unregister_session_inner(&session_id).await;
                    }
                    return Err(RuntimeDriverError::Internal(format!(
                        "runtime commit failed: {err}"
                    )));
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

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
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

                Ok(MeerkatMachineCommandResult::Unit)
            }
            _ => unreachable!("non-legacy-run command routed to legacy-run handler"),
        }
    }
}
