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

                let (resolved, outcome, handle, accepted_input_id, signal) = {
                    let mut driver = driver.lock().await;
                    let runtime_idle = state.is_idle_or_attached();
                    let resolved = driver.resolve_admission_for_runtime_idle(&input, runtime_idle);
                    self.preview_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::AcceptWithCompletion {
                            input_id: crate::meerkat_machine::dsl::InputId::from_domain(
                                &InputId::new(),
                            ),
                            request_immediate_processing: resolved
                                .coarse_flags
                                .request_immediate_processing,
                            interrupt_yielding: resolved.coarse_flags.interrupt_yielding,
                            wake_if_idle: resolved.coarse_flags.wake_if_idle,
                        },
                        "AcceptWithCompletion",
                    )
                    .await
                    .map_err(|reason| RuntimeDriverError::ValidationFailed {
                        reason: format!(
                            "{reason}; input_kind={}; immediate={}; interrupt_yielding={}; wake_if_idle={}",
                            input.kind(),
                            resolved.coarse_flags.request_immediate_processing,
                            resolved.coarse_flags.interrupt_yielding,
                            resolved.coarse_flags.wake_if_idle,
                        ),
                    })?;
                    let result = match driver
                        .accept_resolved_input(input, resolved.clone())
                        .await
                        .map_err(Self::normalize_destroyed_error)
                    {
                        Ok(r) => r,
                        Err(err) => return Err(err),
                    };

                    match &result {
                        AcceptOutcome::Accepted { input_id, .. } => {
                            let accepted_input_id = input_id.clone();
                            let is_terminal = driver
                                .as_driver()
                                .input_phase(&accepted_input_id)
                                .map(|phase| phase.is_terminal())
                                .unwrap_or(true);
                            let handle = if is_terminal {
                                None
                            } else {
                                Some({
                                    let mut completions = completions.lock().await;
                                    completions.register(accepted_input_id.clone())
                                })
                            };
                            (
                                resolved,
                                result,
                                handle,
                                Some(accepted_input_id),
                                crate::driver::ephemeral::PostAdmissionSignal::None,
                            )
                        }
                        AcceptOutcome::Deduplicated { existing_id, .. } => {
                            let is_terminal = driver
                                .as_driver()
                                .input_phase(existing_id)
                                .map(|phase| phase.is_terminal())
                                .unwrap_or(true);

                            if is_terminal {
                                (
                                    resolved,
                                    result,
                                    None,
                                    None,
                                    crate::driver::ephemeral::PostAdmissionSignal::None,
                                )
                            } else {
                                let handle = {
                                    let mut completions = completions.lock().await;
                                    completions.register(existing_id.clone())
                                };
                                (
                                    resolved,
                                    result,
                                    Some(handle),
                                    None,
                                    crate::driver::ephemeral::PostAdmissionSignal::None,
                                )
                            }
                        }
                        AcceptOutcome::Rejected { reason } => {
                            return Err(RuntimeDriverError::ValidationFailed {
                                reason: reason.to_string(),
                            });
                        }
                    }
                };
                let signal = if let Some(input_id) = accepted_input_id {
                    let (_, effects) = self
                        .apply_session_dsl_input(
                            &session_id,
                            crate::meerkat_machine::dsl::MeerkatMachineInput::AcceptWithCompletion {
                                input_id: crate::meerkat_machine::dsl::InputId::from_domain(
                                    &input_id,
                                ),
                                request_immediate_processing: resolved
                                    .coarse_flags
                                    .request_immediate_processing,
                                interrupt_yielding: resolved.coarse_flags.interrupt_yielding,
                                wake_if_idle: resolved.coarse_flags.wake_if_idle,
                            },
                            "AcceptWithCompletion",
                        )
                        .await
                        .map_err(|reason| {
                            RuntimeDriverError::Internal(format!(
                                "canonical AcceptWithCompletion apply failed after admission: {reason}"
                            ))
                        })?;
                    {
                        let mut driver = driver.lock().await;
                        driver.absorb_post_admission_effects(&effects);
                    }
                    Self::post_admission_signal_from_effects(&effects)
                } else {
                    signal
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

                let (outcome, accepted_input_id) = {
                    let mut driver = driver.lock().await;
                    let runtime_idle = state.is_idle_or_attached();
                    let resolved = driver.resolve_admission_for_runtime_idle(&input, runtime_idle);
                    self.preview_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::AcceptWithoutWake {
                            input_id: crate::meerkat_machine::dsl::InputId::from_domain(
                                &InputId::new(),
                            ),
                        },
                        "AcceptWithoutWake",
                    )
                    .await
                    .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                    let mut resolved = resolved;
                    resolved.policy.wake_mode = crate::policy::WakeMode::None;
                    let result = match driver
                        .accept_resolved_input(input, resolved)
                        .await
                        .map_err(Self::normalize_destroyed_error)
                    {
                        Ok(r) => r,
                        Err(err) => return Err(err),
                    };
                    if let AcceptOutcome::Rejected { reason } = &result {
                        return Err(RuntimeDriverError::ValidationFailed {
                            reason: reason.to_string(),
                        });
                    }
                    let accepted_input_id = match &result {
                        AcceptOutcome::Accepted { input_id, .. } => Some(input_id.clone()),
                        AcceptOutcome::Deduplicated { .. } => None,
                        AcceptOutcome::Rejected { .. } => unreachable!("handled above"),
                    };
                    (result, accepted_input_id)
                };
                if let Some(input_id) = accepted_input_id {
                    let (_, effects) = self
                        .apply_session_dsl_input(
                            &session_id,
                            crate::meerkat_machine::dsl::MeerkatMachineInput::AcceptWithoutWake {
                                input_id: crate::meerkat_machine::dsl::InputId::from_domain(
                                    &input_id,
                                ),
                            },
                            "AcceptWithoutWake",
                        )
                        .await
                        .map_err(|reason| {
                            RuntimeDriverError::Internal(format!(
                                "canonical AcceptWithoutWake apply failed after admission: {reason}"
                            ))
                        })?;
                    {
                        let mut driver = driver.lock().await;
                        driver.absorb_post_admission_effects(&effects);
                    }
                    let signal = Self::post_admission_signal_from_effects(&effects);
                    debug_assert!(
                        !signal.should_wake()
                            && !signal.should_interrupt_yielding()
                            && !signal.should_process_immediately(),
                        "AcceptWithoutWake unexpectedly emitted a post-admission signal"
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

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                {
                    let driver = driver.lock().await;
                    if !driver.is_idle_or_attached() {
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
                        let duplicate_active_input = input
                            .header()
                            .idempotency_key
                            .as_ref()
                            .and_then(|key| driver.input_id_for_idempotency_key(key));
                        if let Some(existing_id) = duplicate_active_input {
                            return Err(RuntimeDriverError::ValidationFailed {
                                reason: format!(
                                    "accept_input_and_run does not support deduplicated admission; existing input {existing_id} already owns execution"
                                ),
                            });
                        }
                        return Err(RuntimeDriverError::NotReady {
                            state: self
                                .existing_session_runtime_state(&session_id)
                                .await
                                .unwrap_or(RuntimeState::Destroyed),
                        });
                    }
                }

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
                        let _ = machine_apply_run_return_projection(
                            &mut driver,
                            &run_id,
                            crate::meerkat_machine::driver::RunReturnDisposition::Fail,
                            next_phase,
                        );
                        self.restore_session_dsl_state(&session_id, previous_dsl_state)
                            .await;
                        return Err(RuntimeDriverError::Internal(format!(
                            "failed to stage accepted input: {err}"
                        )));
                    }

                    MeerkatMachineRunPrepared {
                        input_id,
                        run_id,
                        primitive: crate::runtime_loop::input_to_primitive(
                            &dequeued_input,
                            dequeued_id,
                        ),
                    }
                };

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
                    let _ = previous_dsl_state;
                    // Driver already unwinds the shared canonical ingress state
                    // on commit failure. Only the coarse Fail transition remains
                    // to keep the top-level machine phase in sync.
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
