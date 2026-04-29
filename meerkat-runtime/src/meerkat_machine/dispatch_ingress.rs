use super::*;

impl MeerkatMachine {
    fn classify_ingress_dsl_rejection(state: RuntimeState, reason: String) -> RuntimeDriverError {
        match state {
            RuntimeState::Destroyed => RuntimeDriverError::Destroyed,
            RuntimeState::Retired | RuntimeState::Stopped => RuntimeDriverError::NotReady { state },
            _ => RuntimeDriverError::ValidationFailed { reason },
        }
    }

    fn reject_visible_terminal_ingress(state: RuntimeState) -> Result<(), RuntimeDriverError> {
        match state {
            RuntimeState::Destroyed => Err(RuntimeDriverError::Destroyed),
            RuntimeState::Retired | RuntimeState::Stopped => {
                Err(RuntimeDriverError::NotReady { state })
            }
            _ => Ok(()),
        }
    }

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
                let visible_state = self
                    .existing_session_visible_runtime_state(&session_id)
                    .await
                    .unwrap_or(RuntimeState::Destroyed);
                Self::reject_visible_terminal_ingress(visible_state)?;

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
                    .map_err(|reason| {
                        let reason = format!(
                            "{reason}; input_kind={}; immediate={}; interrupt_yielding={}; wake_if_idle={}",
                            input.kind(),
                            resolved.coarse_flags.request_immediate_processing,
                            resolved.coarse_flags.interrupt_yielding,
                            resolved.coarse_flags.wake_if_idle,
                        );
                        Self::classify_ingress_dsl_rejection(state, reason)
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
                let visible_state = self
                    .existing_session_visible_runtime_state(&session_id)
                    .await
                    .unwrap_or(RuntimeState::Destroyed);
                Self::reject_visible_terminal_ingress(visible_state)?;

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
                    .map_err(|reason| Self::classify_ingress_dsl_rejection(state, reason))?;
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

                let visible_state = self
                    .existing_session_visible_runtime_state(&session_id)
                    .await
                    .unwrap_or(RuntimeState::Destroyed);
                Self::reject_visible_terminal_ingress(visible_state)?;

                let prepare_precheck_error = {
                    let driver = driver.lock().await;
                    let state = driver.runtime_state();
                    if !driver.is_idle_or_attached() {
                        Some(Self::normalize_destroyed_error(
                            RuntimeDriverError::NotReady { state },
                        ))
                    } else if !driver.as_driver().active_input_ids().is_empty() {
                        let duplicate_active_input = input
                            .header()
                            .idempotency_key
                            .as_ref()
                            .and_then(|key| driver.input_id_for_idempotency_key(key));
                        if let Some(existing_id) = duplicate_active_input {
                            Some(RuntimeDriverError::ValidationFailed {
                                reason: format!(
                                    "accept_input_and_run does not support deduplicated admission; existing input {existing_id} already owns execution"
                                ),
                            })
                        } else {
                            Some(RuntimeDriverError::NotReady { state })
                        }
                    } else {
                        None
                    }
                };
                if let Some(err) = prepare_precheck_error {
                    return Err(err);
                }

                let run_id = RunId::new();
                self.preview_session_dsl_input(
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
                .map_err(|reason| {
                    tracing::error!(
                        error = %reason,
                        session_id = %session_id,
                        "DSL rejected Prepare input"
                    );
                    let state = visible_state;
                    Self::normalize_destroyed_error(RuntimeDriverError::NotReady { state })
                })?;

                let prepared = {
                    let mut driver = driver.lock().await;
                    let outcome = match driver
                        .as_driver_mut()
                        .accept_input(input)
                        .await
                        .map_err(Self::normalize_destroyed_error)
                    {
                        Ok(o) => o,
                        Err(err) => return Err(err),
                    };
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

                    let (dequeued_id, dequeued_input) = match driver.dequeue_next() {
                        Some(pair) => pair,
                        None => {
                            return Err(RuntimeDriverError::Internal(
                                "accepted input was not queued for execution".into(),
                            ));
                        }
                    };
                    if dequeued_id != input_id {
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
                        return Err(RuntimeDriverError::Internal(format!(
                            "failed to stage accepted input: {err}"
                        )));
                    }

                    let primitive =
                        crate::runtime_loop::input_to_primitive(&dequeued_input, dequeued_id)
                            .map_err(|err| {
                                RuntimeDriverError::Internal(format!(
                                    "failed to build accepted input primitive: {err}"
                                ))
                            })?;

                    MeerkatMachineRunPrepared {
                        input_id,
                        run_id,
                        primitive,
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

                if let Err(err) = commit_runtime_loop_run(
                    &driver,
                    run_id,
                    vec![input_id],
                    output.receipt,
                    output.session_snapshot,
                )
                .await
                {
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

                if let Err(run_err) = fail_runtime_loop_run(&driver, run_id, error).await {
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
