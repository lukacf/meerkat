use super::*;

impl MeerkatMachine {
    fn classify_ingress_dsl_rejection(state: RuntimeState, reason: String) -> RuntimeDriverError {
        match crate::meerkat_machine::classify_runtime_lifecycle_state(state) {
            Ok(facts) => match facts.ingress_admission {
                crate::meerkat_machine::dsl::RuntimeIngressAdmission::Destroyed => {
                    RuntimeDriverError::Destroyed
                }
                crate::meerkat_machine::dsl::RuntimeIngressAdmission::NotReady => {
                    RuntimeDriverError::NotReady { state }
                }
                crate::meerkat_machine::dsl::RuntimeIngressAdmission::Open => {
                    RuntimeDriverError::ValidationFailed { reason }
                }
            },
            Err(error) => RuntimeDriverError::Internal(error),
        }
    }

    fn reject_visible_terminal_ingress(state: RuntimeState) -> Result<(), RuntimeDriverError> {
        let facts =
            crate::meerkat_machine::classify_runtime_lifecycle_state(state).map_err(|reason| {
                RuntimeDriverError::Internal(format!(
                    "generated runtime ingress admission classification failed for {state}: {reason}"
                ))
            })?;
        match facts.ingress_admission {
            crate::meerkat_machine::dsl::RuntimeIngressAdmission::Destroyed => {
                Err(RuntimeDriverError::Destroyed)
            }
            crate::meerkat_machine::dsl::RuntimeIngressAdmission::NotReady => {
                Err(RuntimeDriverError::NotReady { state })
            }
            crate::meerkat_machine::dsl::RuntimeIngressAdmission::Open => Ok(()),
        }
    }

    async fn reject_unregistration_drain_ingress(
        &self,
        session_id: &SessionId,
        state: RuntimeState,
    ) -> Result<(), RuntimeDriverError> {
        let dsl_state = self.session_dsl_state(session_id).await.map_err(|reason| {
            RuntimeDriverError::Internal(format!(
                "failed to read generated registration phase for ingress admission: {reason}"
            ))
        })?;
        if dsl_state.registration_phase == crate::meerkat_machine::dsl::RegistrationPhase::Draining
        {
            return Err(RuntimeDriverError::NotReady { state });
        }
        Ok(())
    }

    pub(super) async fn observe_active_turn_boundary_available(
        session_id: &SessionId,
        boundary_handle: Option<
            &std::sync::Arc<dyn meerkat_core::lifecycle::CoreExecutorBoundaryHandle>,
        >,
    ) -> Result<bool, RuntimeDriverError> {
        if let Some(boundary_handle) = boundary_handle {
            boundary_handle
                .active_turn_boundary_available()
                .await
                .map_err(|error| {
                    tracing::debug!(
                        session_id = %session_id,
                        error = %error,
                        "active turn boundary availability check failed"
                    );
                    RuntimeDriverError::Internal(format!(
                        "active turn boundary availability check failed: {error}"
                    ))
                })
        } else {
            Ok(false)
        }
    }

    /// Preview generated admission feedback for shell capacity mechanics.
    ///
    /// The caller does not classify policy/defaults itself; it only observes
    /// whether generated `ResolveAdmissionPlan` would ask the runtime to wake,
    /// interrupt, or process immediately.
    pub async fn input_requires_active_pre_admission(
        &self,
        session_id: &SessionId,
        input: &Input,
    ) -> Result<bool, RuntimeDriverError> {
        self.input_requires_active_pre_admission_with_wake_policy(session_id, input, false)
            .await
    }

    /// Preview generated admission feedback for no-wake shell capacity mechanics.
    ///
    /// This mirrors `accept_input_without_wake`: the shell supplies only the
    /// command mode, while generated `ResolveAdmissionPlan` owns the semantic
    /// pre-admission answer.
    pub async fn input_requires_active_pre_admission_without_wake(
        &self,
        session_id: &SessionId,
        input: &Input,
    ) -> Result<bool, RuntimeDriverError> {
        self.input_requires_active_pre_admission_with_wake_policy(session_id, input, true)
            .await
    }

    async fn input_requires_active_pre_admission_with_wake_policy(
        &self,
        session_id: &SessionId,
        input: &Input,
        without_wake: bool,
    ) -> Result<bool, RuntimeDriverError> {
        let (driver, boundary_handle) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            (entry.driver.clone(), entry.boundary_handle())
        };

        let gate = self.session_mutation_gate(session_id).await;
        let _gate_guard = match gate {
            Some(ref g) => Some(g.lock().await),
            None => None,
        };

        let visible_state = self
            .existing_session_visible_runtime_state(session_id)
            .await
            .unwrap_or(RuntimeState::Destroyed);
        Self::reject_visible_terminal_ingress(visible_state)?;
        let active_turn_boundary_available =
            Self::observe_active_turn_boundary_available(session_id, boundary_handle.as_ref())
                .await?;

        let driver = driver.lock().await;
        let resolved = if without_wake {
            driver.resolve_admission_without_wake_with_active_turn_boundary(
                input,
                active_turn_boundary_available,
            )?
        } else {
            driver.resolve_admission_with_active_turn_boundary(
                input,
                active_turn_boundary_available,
            )?
        };
        Ok(resolved.requires_active_runtime_pre_admission())
    }

    pub(super) async fn execute_meerkat_machine_ingress_command(
        &self,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        match command {
            MeerkatMachineCommand::AcceptWithCompletion {
                session_id,
                input,
                register_completion,
            } => {
                tracing::debug!(
                    session_id = %session_id,
                    input_id = %input.id(),
                    register_completion,
                    "MeerkatMachine::AcceptWithCompletion loading session entry"
                );
                let (driver, completions, wake_tx, effect_tx, boundary_handle) = {
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
                        entry.effect_sender(),
                        entry.boundary_handle(),
                    )
                };
                tracing::debug!(
                    session_id = %session_id,
                    input_id = %input.id(),
                    "MeerkatMachine::AcceptWithCompletion loaded session entry"
                );

                tracing::debug!(
                    session_id = %session_id,
                    input_id = %input.id(),
                    "MeerkatMachine::AcceptWithCompletion resolving mutation gate"
                );
                let gate = self.session_mutation_gate(&session_id).await;
                tracing::debug!(
                    session_id = %session_id,
                    input_id = %input.id(),
                    has_gate = gate.is_some(),
                    "MeerkatMachine::AcceptWithCompletion resolved mutation gate"
                );
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };
                tracing::debug!(
                    session_id = %session_id,
                    input_id = %input.id(),
                    "MeerkatMachine::AcceptWithCompletion acquired mutation gate"
                );

                tracing::debug!(
                    session_id = %session_id,
                    input_id = %input.id(),
                    "MeerkatMachine::AcceptWithCompletion reading runtime state"
                );
                let state = self
                    .existing_session_runtime_state(&session_id)
                    .await
                    .unwrap_or(RuntimeState::Destroyed);
                let visible_state = self
                    .existing_session_visible_runtime_state(&session_id)
                    .await
                    .unwrap_or(RuntimeState::Destroyed);
                tracing::debug!(
                    session_id = %session_id,
                    input_id = %input.id(),
                    runtime_state = ?state,
                    visible_state = ?visible_state,
                    "MeerkatMachine::AcceptWithCompletion read runtime state"
                );
                Self::reject_visible_terminal_ingress(visible_state)?;
                self.reject_unregistration_drain_ingress(&session_id, state)
                    .await?;

                let active_turn_boundary_available = Self::observe_active_turn_boundary_available(
                    &session_id,
                    boundary_handle.as_ref(),
                )
                .await?;
                if active_turn_boundary_available {
                    tracing::debug!(
                        session_id = %session_id,
                        runtime_state = ?state,
                        "active turn boundary available during ingress admission"
                    );
                }

                let (flags, stages_run_boundary, outcome, handle, accepted_input_id, signal) = {
                    let mut driver = driver.lock().await;
                    // origin/main observability: surface the idle/running
                    // disposition (idle, attached non-steer, or attached peer)
                    // for ingress-admission diagnostics, including the
                    // remote-comms / paired-TCP peer admission path. The
                    // semantic decision itself is NOT taken here: it flows
                    // through the canonical machine seam
                    // (resolve_admission_with_active_turn_boundary), which
                    // derives idle vs running internally from the recovered
                    // authority plus active_turn_boundary_available (P0
                    // Invariant 1 — no handwritten admission resolution).
                    let input_kind = input.kind();
                    let runtime_idle = !active_turn_boundary_available
                        && (state == RuntimeState::Idle
                            || (state == RuntimeState::Attached
                                && !matches!(
                                    input.handling_mode(),
                                    Some(meerkat_core::types::HandlingMode::Steer)
                                ))
                            || (state == RuntimeState::Attached
                                && matches!(input, Input::Peer(_))));
                    tracing::debug!(
                        session_id = %session_id,
                        input_kind = ?input_kind,
                        runtime_state = ?state,
                        visible_state = ?visible_state,
                        runtime_idle,
                        active_turn_boundary_available,
                        "resolving runtime ingress admission via canonical machine seam"
                    );
                    let resolved = driver.resolve_admission_with_active_turn_boundary(
                        &input,
                        active_turn_boundary_available,
                    )?;
                    let flags = resolved.coarse_flags();
                    let stages_run_boundary = resolved.stages_run_boundary();
                    self.preview_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::AcceptWithCompletion {
                            input_id: crate::meerkat_machine::dsl::InputId::from_domain(
                                &InputId::new(),
                            ),
                            request_immediate_processing: flags.request_immediate_processing,
                            interrupt_yielding: flags.interrupt_yielding,
                            wake_if_idle: flags.wake_if_idle,
                        },
                        "AcceptWithCompletion",
                    )
                    .await
                    .map_err(|reason| {
                        let reason = format!(
                            "{reason}; input_kind={}; immediate={}; interrupt_yielding={}; wake_if_idle={}",
                            input.kind(),
                            flags.request_immediate_processing,
                            flags.interrupt_yielding,
                            flags.wake_if_idle,
                        );
                        Self::classify_ingress_dsl_rejection(state, reason)
                    })?;
                    let result = match driver
                        .accept_resolved_input(input, resolved)
                        .await
                        .map_err(Self::normalize_destroyed_error)
                    {
                        Ok(r) => r,
                        Err(err) => return Err(err),
                    };

                    match &result {
                        AcceptOutcome::Accepted { input_id, .. } => {
                            let accepted_input_id = input_id.clone();
                            let is_terminal =
                                driver.input_is_terminal_by_authority(&accepted_input_id)?;
                            let handle = if is_terminal || !register_completion {
                                None
                            } else {
                                Some({
                                    let mut completions = completions.lock().await;
                                    completions.register(accepted_input_id.clone())
                                })
                            };
                            (
                                flags,
                                stages_run_boundary,
                                result,
                                handle,
                                Some(accepted_input_id),
                                crate::driver::ephemeral::PostAdmissionSignal::None,
                            )
                        }
                        AcceptOutcome::Deduplicated { existing_id, .. } => {
                            let is_terminal = driver.input_is_terminal_by_authority(existing_id)?;

                            if is_terminal || !register_completion {
                                (
                                    flags,
                                    stages_run_boundary,
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
                                    flags,
                                    stages_run_boundary,
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
                let accepted_input_id_for_live_boundary = accepted_input_id.clone();
                let (signal, runtime_effect, effect_previous_dsl_state) = if let Some(input_id) =
                    accepted_input_id.clone()
                {
                    let (previous_dsl_state, effects) = self
                        .apply_session_dsl_input(
                            &session_id,
                            crate::meerkat_machine::dsl::MeerkatMachineInput::AcceptWithCompletion {
                                input_id: crate::meerkat_machine::dsl::InputId::from_domain(
                                    &input_id,
                                ),
                                request_immediate_processing: flags.request_immediate_processing,
                                interrupt_yielding: flags.interrupt_yielding,
                                wake_if_idle: flags.wake_if_idle,
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
                    let signal = Self::post_admission_signal_from_effects(&effects);
                    let runtime_effect =
                        crate::effect::runtime_effect_projection_optional_from_dsl_effects(
                            &effects,
                        )
                        .map_err(|reason| {
                            RuntimeDriverError::Internal(format!(
                                "canonical AcceptWithCompletion emitted invalid runtime effect facts: {reason}"
                            ))
                        })?;
                    (signal, runtime_effect, Some(previous_dsl_state))
                } else {
                    (signal, None, None)
                };

                if signal.should_wake()
                    && let Some(ref wake_tx) = wake_tx
                {
                    let _ = wake_tx.try_send(());
                }
                if let Some(projected_effect) = runtime_effect
                    && let Err(err) = self
                        .dispatch_cancel_after_boundary_runtime_effect(
                            &session_id,
                            effect_tx,
                            boundary_handle.clone(),
                            projected_effect,
                            "AcceptWithCompletion",
                        )
                        .await
                {
                    if let Some(previous_dsl_state) = effect_previous_dsl_state {
                        self.restore_session_dsl_state(&session_id, previous_dsl_state)
                            .await;
                    }
                    return Err(err);
                }

                let has_live_boundary_input = accepted_input_id_for_live_boundary.is_some();
                let has_boundary_handle = boundary_handle.is_some();
                if active_turn_boundary_available
                    && signal.should_interrupt_yielding()
                    && stages_run_boundary
                    && let (Some(input_id), Some(boundary_handle)) =
                        (accepted_input_id_for_live_boundary, boundary_handle)
                {
                    let live_boundary_plan = {
                        let driver = driver.lock().await;
                        let run_id = driver.current_run_id();
                        let projection = driver.driver_ingress().primitive_projection(&input_id);
                        run_id.and_then(|run_id| {
                            let projection = projection?;
                            let appends =
                                crate::input::projection_to_pending_system_context_appends(
                                    &input_id,
                                    &projection,
                                );
                            if appends.is_empty() {
                                return None;
                            }
                            Some((run_id, appends))
                        })
                    };

                    if let Some((run_id, appends)) = live_boundary_plan {
                        let rollback_keys = appends
                            .iter()
                            .filter_map(|append| append.idempotency_key.clone())
                            .collect::<Vec<_>>();
                        tracing::debug!(
                            session_id = %session_id,
                            run_id = %run_id,
                            input_id = %input_id,
                            append_count = appends.len(),
                            "staging live boundary context for accepted steer input"
                        );
                        match boundary_handle
                            .stage_system_context_at_boundary(&run_id, appends)
                            .await
                        {
                            Ok(stage_output) => {
                                let commit_result = {
                                    let mut driver = driver.lock().await;
                                    driver
                                        .machine_realize_live_boundary_context_injected(
                                            &run_id,
                                            std::slice::from_ref(&input_id),
                                            stage_output.session_snapshot,
                                        )
                                        .await
                                };
                                if let Err(error) = commit_result {
                                    let rollback_result = if rollback_keys.is_empty() {
                                        Ok(())
                                    } else {
                                        boundary_handle
                                            .discard_staged_system_context_at_boundary(
                                                &run_id,
                                                rollback_keys,
                                            )
                                            .await
                                    };
                                    match rollback_result {
                                        Ok(()) => {
                                            tracing::warn!(
                                                session_id = %session_id,
                                                run_id = %run_id,
                                                input_id = %input_id,
                                                error = %error,
                                                "live boundary runtime commit failed; rolled back staged session context"
                                            );
                                        }
                                        Err(rollback_error) => {
                                            tracing::error!(
                                                session_id = %session_id,
                                                run_id = %run_id,
                                                input_id = %input_id,
                                                error = %error,
                                                rollback_error = %rollback_error,
                                                "live boundary runtime commit failed and staged session context rollback failed"
                                            );
                                        }
                                    }
                                    return Err(error);
                                }
                                let result_class =
                                    crate::meerkat_machine::driver::machine_resolve_runtime_completed_without_result(
                                        &driver,
                                        &run_id,
                                    )
                                    .await?;
                                let mut completions = completions.lock().await;
                                completions
                                    .resolve_without_result_authorized(&input_id, result_class);
                            }
                            Err(error) => {
                                tracing::warn!(
                                    session_id = %session_id,
                                    run_id = %run_id,
                                    input_id = %input_id,
                                    error = %error,
                                    "live boundary context staging failed; leaving steer input queued for ordinary post-turn drain"
                                );
                            }
                        }
                    } else {
                        tracing::debug!(
                            session_id = %session_id,
                            input_id = %input_id,
                            runtime_state = ?state,
                            active_turn_boundary_available,
                            "accepted steer input had no live boundary plan; leaving input queued for ordinary post-turn drain"
                        );
                    }
                } else if signal.should_interrupt_yielding() && stages_run_boundary {
                    tracing::debug!(
                        session_id = %session_id,
                        runtime_state = ?state,
                        active_turn_boundary_available,
                        has_boundary_handle,
                        has_input_id = has_live_boundary_input,
                        "accepted steer input did not meet live boundary staging preconditions"
                    );
                }

                Ok(MeerkatMachineCommandResult::AcceptWithCompletion {
                    outcome,
                    handle,
                    admission_signal: signal,
                })
            }
            MeerkatMachineCommand::AcceptWithoutWake { session_id, input } => {
                let (driver, boundary_handle, has_live_attachment) = {
                    let sessions = self.sessions.read().await;
                    let entry = sessions
                        .get(&session_id)
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?;
                    (
                        entry.driver.clone(),
                        entry.boundary_handle(),
                        entry.has_live_attachment(),
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
                self.reject_unregistration_drain_ingress(&session_id, state)
                    .await?;
                if !has_live_attachment {
                    return Err(RuntimeDriverError::NotReady { state });
                }
                let active_turn_boundary_available = Self::observe_active_turn_boundary_available(
                    &session_id,
                    boundary_handle.as_ref(),
                )
                .await?;

                let (outcome, accepted_input_id) = {
                    let mut driver = driver.lock().await;
                    let resolved = driver
                        .resolve_admission_without_wake_with_active_turn_boundary(
                            &input,
                            active_turn_boundary_available,
                        )?;
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
}
