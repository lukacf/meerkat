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
                            origin: crate::meerkat_machine::dsl::WorkOrigin::Ingest,
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

                let (outcome, signal) = {
                    let mut drv = driver.lock().await;
                    let runtime_idle = self
                        .existing_session_runtime_state(&session_id)
                        .await
                        .unwrap_or(RuntimeState::Destroyed)
                        .is_idle_or_attached();
                    let resolved = drv.resolve_admission_for_runtime_idle(&input, runtime_idle);
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
                        "AcceptWithCompletion(Ingest)",
                    )
                    .await
                    .map_err(RuntimeControlPlaneError::Internal)?;
                    let result = match drv.accept_resolved_input(input, resolved.clone()).await {
                        Ok(result) => result,
                        Err(err) => {
                            drop(drv);
                            self.restore_session_dsl_state(&session_id, previous_dsl_state)
                                .await;
                            return Err(RuntimeControlPlaneError::Internal(err.to_string()));
                        }
                    };
                    let signal = match &result {
                        AcceptOutcome::Accepted { input_id, .. } => {
                            let (_, effects) = self
                                .apply_session_dsl_input(
                                    &session_id,
                                    crate::meerkat_machine::dsl::MeerkatMachineInput::AcceptWithCompletion {
                                        input_id: crate::meerkat_machine::dsl::InputId::from_domain(
                                            input_id,
                                        ),
                                        request_immediate_processing: resolved
                                            .coarse_flags
                                            .request_immediate_processing,
                                        interrupt_yielding: resolved.coarse_flags.interrupt_yielding,
                                        wake_if_idle: resolved.coarse_flags.wake_if_idle,
                                    },
                                    "AcceptWithCompletion(Ingest)",
                                )
                                .await
                                .map_err(RuntimeControlPlaneError::Internal)?;
                            drv.absorb_post_admission_effects(&effects);
                            Self::post_admission_signal_from_effects(&effects)
                        }
                        AcceptOutcome::Deduplicated { .. } | AcceptOutcome::Rejected { .. } => {
                            crate::driver::ephemeral::PostAdmissionSignal::None
                        }
                    };
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
                let previous_dsl_state = if state == RuntimeState::Retired {
                    // The DSL Retire transition is intentionally not
                    // self-looping. Command-level idempotence belongs here,
                    // after reading the DSL-authoritative phase, while the
                    // driver still owns retire realization and reporting.
                    None
                } else {
                    Some(
                        self.stage_session_dsl_input(
                            &session_id,
                            crate::meerkat_machine::dsl::MeerkatMachineInput::Retire {
                                session_id: crate::meerkat_machine::dsl::SessionId::from_domain(
                                    &session_id,
                                ),
                            },
                            "Retire",
                        )
                        .await
                        .map_err(RuntimeControlPlaneError::Internal)?,
                    )
                };

                let mut drv = driver.lock().await;
                let mut report = match machine_retire(&mut drv).await {
                    Ok(report) => report,
                    Err(err) => {
                        drop(drv);
                        if let Some(previous_dsl_state) = previous_dsl_state {
                            self.restore_session_dsl_state(&session_id, previous_dsl_state)
                                .await;
                        }
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
                let destroy_input = crate::meerkat_machine::dsl::MeerkatMachineInput::Destroy {
                    session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&session_id),
                };
                self.preview_session_dsl_input(&session_id, destroy_input.clone(), "Destroy")
                    .await
                    .map_err(RuntimeControlPlaneError::Internal)?;

                let mut drv = driver.lock().await;
                let report = match machine_destroy(&mut drv).await {
                    Ok(report) => report,
                    Err(err) => return Err(RuntimeControlPlaneError::Internal(err.to_string())),
                };
                drop(drv);

                self.apply_session_dsl_input(&session_id, destroy_input, "Destroy")
                    .await
                    .map_err(RuntimeControlPlaneError::Internal)?;
                driver
                    .lock()
                    .await
                    .sync_control_projection_from_dsl_authority();

                let mut comp = completions.lock().await;
                comp.resolve_all_terminated("runtime destroyed");
                Ok(MeerkatMachineCommandResult::DestroyReport(report))
            }
            MeerkatMachineCommand::RuntimeState { runtime_id } => {
                let session_id = Self::resolve_session_id(&runtime_id)?;
                let state = self
                    .existing_session_visible_runtime_state(&session_id)
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
            MeerkatMachineCommand::RuntimeRealtimeChannelStatus { session_id } => {
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
                let reconnect = project_realtime_reconnect_progress(&authority.state);
                let channel_status = status.to_channel_status(reconnect.as_ref());
                Ok(MeerkatMachineCommandResult::RealtimeChannelStatus(
                    channel_status,
                ))
            }
            MeerkatMachineCommand::ConfigureModelRoutingBaseline {
                session_id,
                baseline_model,
                realtime_capable,
            } => {
                self.apply_session_dsl_input(
                    &session_id,
                    crate::meerkat_machine::dsl::MeerkatMachineInput::SetModelRoutingBaseline {
                        baseline_model: baseline_model.to_string(),
                        realtime_capable,
                    },
                    "SetModelRoutingBaseline",
                )
                .await
                .map_err(RuntimeControlPlaneError::Internal)?;
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::SessionModelRoutingStatus { session_id } => {
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
                Ok(MeerkatMachineCommandResult::SessionModelRoutingStatus(
                    project_model_routing_status(&authority.state),
                ))
            }
            MeerkatMachineCommand::RequestSwitchTurn {
                session_id,
                request,
            } => {
                let request = *request;
                let request_key = switch_request_key(request.request_id);
                match &request.intent.duration {
                    meerkat_core::image_generation::SwitchTurnDuration::Finite { duration } => {
                        self.apply_session_dsl_input(
                            &session_id,
                            crate::meerkat_machine::dsl::MeerkatMachineInput::RequestFiniteSwitchTurn {
                                request_id: request_key.clone(),
                                target_model: request.intent.target_model.to_string(),
                                turns: finite_turn_count(*duration),
                                target_realtime_capable: request.target_realtime.target_realtime_capable,
                                requires_approval: !matches!(
                                    request.approval,
                                    crate::meerkat_machine_types::ModelRoutingApprovalDisposition::NotRequired
                                ),
                                approval_available: !matches!(
                                    request.approval,
                                    crate::meerkat_machine_types::ModelRoutingApprovalDisposition::RequiredButUnavailable
                                ),
                                approval_denied: matches!(
                                    request.approval,
                                    crate::meerkat_machine_types::ModelRoutingApprovalDisposition::DeniedByUser
                                ),
                                realtime_detach_allowed: request.target_realtime.allow_realtime_detach,
                            },
                            "RequestSwitchTurn",
                        )
                        .await
                        .map_err(RuntimeControlPlaneError::Internal)?;
                    }
                    meerkat_core::image_generation::SwitchTurnDuration::UntilChanged => {
                        let previous_dsl_state = self
                            .stage_session_dsl_input(
                                &session_id,
                                crate::meerkat_machine::dsl::MeerkatMachineInput::RequestUntilChangedSwitchTurn {
                                    request_id: request_key.clone(),
                                    target_model: request.intent.target_model.to_string(),
                                    target_realtime_capable: request.target_realtime.target_realtime_capable,
                                    requires_approval: !matches!(
                                        request.approval,
                                        crate::meerkat_machine_types::ModelRoutingApprovalDisposition::NotRequired
                                    ),
                                    approval_available: !matches!(
                                        request.approval,
                                        crate::meerkat_machine_types::ModelRoutingApprovalDisposition::RequiredButUnavailable
                                    ),
                                    approval_denied: matches!(
                                        request.approval,
                                        crate::meerkat_machine_types::ModelRoutingApprovalDisposition::DeniedByUser
                                    ),
                                    realtime_detach_allowed: request.target_realtime.allow_realtime_detach,
                                },
                                "RequestSwitchTurn",
                            )
                            .await
                            .map_err(RuntimeControlPlaneError::Internal)?;
                        let status_state = self.session_dsl_state(&session_id).await?;
                        if !status_state
                            .model_routing_switch_denials
                            .contains_key(&request_key)
                        {
                            let reconfigure = match self
                                .prepare_reconfigure_session_llm_command(
                                    &session_id,
                                    crate::meerkat_machine_types::SessionLlmReconfigureRequest {
                                        model: Some(request.intent.target_model.to_string()),
                                        provider: None,
                                        provider_params: None,
                                        connection_ref: None,
                                    },
                                )
                                .await
                            {
                                Ok(command) => command,
                                Err(err) => {
                                    self.restore_session_dsl_state(&session_id, previous_dsl_state)
                                        .await;
                                    return Err(RuntimeControlPlaneError::Internal(
                                        err.to_string(),
                                    ));
                                }
                            };
                            if let Err(err) = self
                                .execute_meerkat_machine_session_command(reconfigure)
                                .await
                            {
                                self.restore_session_dsl_state(&session_id, previous_dsl_state)
                                    .await;
                                return Err(RuntimeControlPlaneError::Internal(err.to_string()));
                            }
                            self.apply_session_dsl_input(
                                &session_id,
                                crate::meerkat_machine::dsl::MeerkatMachineInput::SetModelRoutingBaseline {
                                    baseline_model: request.intent.target_model.to_string(),
                                    realtime_capable: request.target_realtime.target_realtime_capable,
                                },
                                "SetModelRoutingBaseline",
                            )
                            .await
                            .map_err(RuntimeControlPlaneError::Internal)?;
                            self.apply_session_dsl_input(
                                &session_id,
                                crate::meerkat_machine::dsl::MeerkatMachineInput::CompleteUntilChangedSwitchTurnReconfigure {
                                    request_id: request_key.clone(),
                                },
                                "CompleteUntilChangedSwitchTurnReconfigure",
                            )
                            .await
                            .map_err(RuntimeControlPlaneError::Internal)?;
                        }
                    }
                }

                let status_state = self.session_dsl_state(&session_id).await?;
                if let Some(reason) = status_state.model_routing_switch_denials.get(&request_key) {
                    return Ok(MeerkatMachineCommandResult::SwitchTurnControlResult(
                        meerkat_core::image_generation::SwitchTurnControlResult::Denied {
                            request_id: request.request_id,
                            reason: switch_denial_from_routing(*reason, request.approval_reason),
                        },
                    ));
                }

                Ok(MeerkatMachineCommandResult::SwitchTurnControlResult(
                    meerkat_core::image_generation::SwitchTurnControlResult::Applied {
                        request_id: request.request_id,
                        target_model: request.intent.target_model,
                        duration: request.intent.duration,
                    },
                ))
            }
            MeerkatMachineCommand::AdmitModelRoutingAssistantTurn { session_id } => {
                self.apply_session_dsl_input(
                    &session_id,
                    crate::meerkat_machine::dsl::MeerkatMachineInput::AdmitModelRoutingAssistantTurn,
                    "AdmitModelRoutingAssistantTurn",
                )
                .await
                .map_err(RuntimeControlPlaneError::Internal)?;
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::BeginImageOperation {
                session_id,
                request,
            } => {
                let request = *request;
                let operation_key = image_operation_key(request.operation_id);
                self.apply_session_dsl_input(
                    &session_id,
                    crate::meerkat_machine::dsl::MeerkatMachineInput::BeginImageOperation {
                        operation_id: operation_key.clone(),
                        target_model: request.target_model.to_string(),
                        target_realtime_capable: request.target_realtime.target_realtime_capable,
                        requires_approval: !matches!(
                            request.approval,
                            crate::meerkat_machine_types::ModelRoutingApprovalDisposition::NotRequired
                        ),
                        approval_available: !matches!(
                            request.approval,
                            crate::meerkat_machine_types::ModelRoutingApprovalDisposition::RequiredButUnavailable
                        ),
                        approval_denied: matches!(
                            request.approval,
                            crate::meerkat_machine_types::ModelRoutingApprovalDisposition::DeniedByUser
                        ),
                        realtime_detach_allowed: request.target_realtime.allow_realtime_detach,
                        requires_scoped_override: request.requires_scoped_override,
                    },
                    "BeginImageOperation",
                )
                .await
                .map_err(RuntimeControlPlaneError::Internal)?;
                let status_state = self.session_dsl_state(&session_id).await?;
                if let Some(reason) = status_state
                    .model_routing_image_denials
                    .get(&operation_key)
                    .copied()
                {
                    return Ok(MeerkatMachineCommandResult::ImageOperationRoutingResult(
                        crate::meerkat_machine_types::ImageOperationRoutingResult::Denied {
                            operation_id: request.operation_id,
                            reason: image_denial_from_routing(reason, request.approval_reason),
                        },
                    ));
                }
                Ok(MeerkatMachineCommandResult::ImageOperationRoutingResult(
                    crate::meerkat_machine_types::ImageOperationRoutingResult::Accepted {
                        operation_id: request.operation_id,
                        phase: meerkat_core::image_generation::ImageOperationPhase::PlanResolved,
                    },
                ))
            }
            MeerkatMachineCommand::ActivateImageOperationOverride {
                session_id,
                operation_id,
            } => {
                let operation_key = image_operation_key(operation_id);
                let state = self.session_dsl_state(&session_id).await?;
                let target_model = state
                    .model_routing_image_operation_target_models
                    .get(&operation_key)
                    .cloned()
                    .ok_or_else(|| {
                        RuntimeControlPlaneError::Internal(format!(
                            "image operation {operation_key} is not plan-resolved"
                        ))
                    })?;
                let target_realtime_capable = state
                    .model_routing_image_operation_realtime
                    .get(&operation_key)
                    .copied()
                    .ok_or_else(|| {
                        RuntimeControlPlaneError::Internal(format!(
                            "image operation {operation_key} is missing realtime routing facts"
                        ))
                    })?;
                self.apply_session_dsl_input(
                    &session_id,
                    crate::meerkat_machine::dsl::MeerkatMachineInput::ActivateImageOperationOverride {
                        operation_id: operation_key,
                        target_model,
                        target_realtime_capable,
                    },
                    "ActivateImageOperationOverride",
                )
                .await
                .map_err(RuntimeControlPlaneError::Internal)?;
                Ok(MeerkatMachineCommandResult::ImageOperationPhase(
                    meerkat_core::image_generation::ImageOperationPhase::ScopedOverrideActive,
                ))
            }
            MeerkatMachineCommand::CompleteImageOperation {
                session_id,
                operation_id,
                terminal,
            } => {
                let operation_key = image_operation_key(operation_id);
                self.apply_session_dsl_input(
                    &session_id,
                    crate::meerkat_machine::dsl::MeerkatMachineInput::CompleteImageOperation {
                        operation_id: operation_key.clone(),
                        terminal: routing_image_terminal(terminal.clone()),
                        terminal_payload: serde_json::to_string(&terminal).map_err(|err| {
                            RuntimeControlPlaneError::Internal(format!(
                                "failed to serialize image operation terminal: {err}"
                            ))
                        })?,
                    },
                    "CompleteImageOperation",
                )
                .await
                .map_err(RuntimeControlPlaneError::Internal)?;
                let state = self.session_dsl_state(&session_id).await?;
                let phase = if state
                    .model_routing_image_operation_phases
                    .get(&operation_key)
                    .is_some_and(|phase| {
                        matches!(
                            phase,
                            crate::meerkat_machine::dsl::RoutingImageOperationPhase::Terminal
                        )
                    }) {
                    meerkat_core::image_generation::ImageOperationPhase::Terminal { terminal }
                } else {
                    meerkat_core::image_generation::ImageOperationPhase::RestoringScopedOverride
                };
                Ok(MeerkatMachineCommandResult::ImageOperationPhase(phase))
            }
            MeerkatMachineCommand::RestoreImageOperationOverride {
                session_id,
                operation_id,
            } => {
                let operation_key = image_operation_key(operation_id);
                let state = self.session_dsl_state(&session_id).await?;
                let terminal = state
                    .model_routing_image_terminal_payloads
                    .get(&operation_key)
                    .and_then(|payload| serde_json::from_str(payload).ok())
                    .or_else(|| {
                        state
                            .model_routing_image_terminals
                            .get(&operation_key)
                            .copied()
                            .map(|terminal| {
                                image_terminal_from_routing(
                                    terminal,
                                    state
                                        .model_routing_image_denials
                                        .get(&operation_key)
                                        .copied()
                                        .map(|reason| image_denial_from_routing(reason, None)),
                                )
                            })
                    })
                    .unwrap_or(meerkat_core::image_generation::ImageOperationTerminalClass::Failed);
                self.apply_session_dsl_input(
                    &session_id,
                    crate::meerkat_machine::dsl::MeerkatMachineInput::RestoreImageOperationOverride {
                        operation_id: operation_key,
                    },
                    "RestoreImageOperationOverride",
                )
                .await
                .map_err(RuntimeControlPlaneError::Internal)?;
                Ok(MeerkatMachineCommandResult::ImageOperationPhase(
                    meerkat_core::image_generation::ImageOperationPhase::Terminal { terminal },
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

fn switch_request_key(id: meerkat_core::image_generation::SwitchTurnRequestId) -> String {
    id.0.to_string()
}

fn image_operation_key(id: meerkat_core::image_generation::ImageOperationId) -> String {
    id.0.to_string()
}

fn finite_turn_count(duration: meerkat_core::image_generation::FiniteScopedTurnDuration) -> u64 {
    match duration {
        meerkat_core::image_generation::FiniteScopedTurnDuration::OneTurn => 1,
        meerkat_core::image_generation::FiniteScopedTurnDuration::Turns { turns } => {
            u64::from(turns.get())
        }
    }
}

fn uuid_from_key(key: &str) -> Option<uuid::Uuid> {
    uuid::Uuid::parse_str(key).ok()
}

fn switch_denial_from_routing(
    reason: super::dsl::RoutingDenialReason,
    approval_reason: Option<meerkat_core::image_generation::SwitchTurnApprovalReason>,
) -> meerkat_core::image_generation::SwitchTurnDenialReason {
    use meerkat_core::image_generation::{SwitchTurnApprovalReason, SwitchTurnDenialReason};
    match reason {
        super::dsl::RoutingDenialReason::ApprovalRequiredButUnavailable => {
            SwitchTurnDenialReason::ApprovalRequiredButUnavailable
        }
        super::dsl::RoutingDenialReason::DeniedDuringApproval => {
            SwitchTurnDenialReason::DeniedDuringApproval {
                approvable: approval_reason.unwrap_or(SwitchTurnApprovalReason::CrossProvider),
            }
        }
        super::dsl::RoutingDenialReason::ScopedOverrideConflict => {
            SwitchTurnDenialReason::ScopedOverrideConflict
        }
        super::dsl::RoutingDenialReason::RealtimeTransportConflict => {
            SwitchTurnDenialReason::RealtimeTransportConflict
        }
        super::dsl::RoutingDenialReason::CapabilityPolicy => {
            SwitchTurnDenialReason::CapabilityPolicy
        }
    }
}

fn image_denial_from_routing(
    reason: super::dsl::RoutingDenialReason,
    approval_reason: Option<meerkat_core::image_generation::ImageOperationApprovalReason>,
) -> meerkat_core::image_generation::ImageOperationDenialReason {
    use meerkat_core::image_generation::{
        ImageOperationApprovalReason, ImageOperationDenialReason,
    };
    match reason {
        super::dsl::RoutingDenialReason::ApprovalRequiredButUnavailable => {
            ImageOperationDenialReason::ApprovalRequiredButUnavailable
        }
        super::dsl::RoutingDenialReason::DeniedDuringApproval => {
            ImageOperationDenialReason::DeniedDuringApproval {
                approvable: approval_reason.unwrap_or(ImageOperationApprovalReason::CrossProvider),
            }
        }
        super::dsl::RoutingDenialReason::ScopedOverrideConflict => {
            ImageOperationDenialReason::ScopedOverrideConflict
        }
        super::dsl::RoutingDenialReason::RealtimeTransportConflict => {
            ImageOperationDenialReason::RealtimeTransportConflict
        }
        super::dsl::RoutingDenialReason::CapabilityPolicy => {
            ImageOperationDenialReason::CapabilityPolicy
        }
    }
}

fn routing_image_terminal(
    terminal: meerkat_core::image_generation::ImageOperationTerminalClass,
) -> super::dsl::RoutingImageTerminal {
    use meerkat_core::image_generation::ImageOperationTerminalClass;
    match terminal {
        ImageOperationTerminalClass::Generated => super::dsl::RoutingImageTerminal::Generated,
        ImageOperationTerminalClass::EmptyResult { .. } => {
            super::dsl::RoutingImageTerminal::EmptyResult
        }
        ImageOperationTerminalClass::Denied { .. } => super::dsl::RoutingImageTerminal::Denied,
        ImageOperationTerminalClass::RefusedByProvider => {
            super::dsl::RoutingImageTerminal::RefusedByProvider
        }
        ImageOperationTerminalClass::SafetyFiltered => {
            super::dsl::RoutingImageTerminal::SafetyFiltered
        }
        ImageOperationTerminalClass::Failed => super::dsl::RoutingImageTerminal::Failed,
        ImageOperationTerminalClass::Cancelled => super::dsl::RoutingImageTerminal::Cancelled,
        ImageOperationTerminalClass::Timeout => super::dsl::RoutingImageTerminal::Timeout,
        ImageOperationTerminalClass::ScopedRestoreFailed { .. } => {
            super::dsl::RoutingImageTerminal::ScopedRestoreFailed
        }
    }
}

fn image_terminal_from_routing(
    terminal: super::dsl::RoutingImageTerminal,
    denial_reason: Option<meerkat_core::image_generation::ImageOperationDenialReason>,
) -> meerkat_core::image_generation::ImageOperationTerminalClass {
    use meerkat_core::image_generation::{ImageOperationTerminalClass, ProviderTextDisposition};
    match terminal {
        super::dsl::RoutingImageTerminal::Generated => ImageOperationTerminalClass::Generated,
        super::dsl::RoutingImageTerminal::EmptyResult => ImageOperationTerminalClass::EmptyResult {
            provider_text: ProviderTextDisposition::NotEmitted,
        },
        super::dsl::RoutingImageTerminal::Denied => ImageOperationTerminalClass::Denied {
            reason: denial_reason.unwrap_or(
                meerkat_core::image_generation::ImageOperationDenialReason::CapabilityPolicy,
            ),
        },
        super::dsl::RoutingImageTerminal::RefusedByProvider => {
            ImageOperationTerminalClass::RefusedByProvider
        }
        super::dsl::RoutingImageTerminal::SafetyFiltered => {
            ImageOperationTerminalClass::SafetyFiltered
        }
        super::dsl::RoutingImageTerminal::Failed => ImageOperationTerminalClass::Failed,
        super::dsl::RoutingImageTerminal::Cancelled => ImageOperationTerminalClass::Cancelled,
        super::dsl::RoutingImageTerminal::Timeout => ImageOperationTerminalClass::Timeout,
        super::dsl::RoutingImageTerminal::ScopedRestoreFailed => {
            ImageOperationTerminalClass::ScopedRestoreFailed {
                trigger: meerkat_core::image_generation::PostActivationImageTerminal::Failed,
            }
        }
    }
}

fn project_model_routing_status(
    state: &super::dsl::MeerkatMachineState,
) -> meerkat_core::image_generation::SessionModelRoutingStatus {
    use meerkat_core::image_generation::{
        FiniteScopedTurnDuration, ScopedModelOverrideId, ScopedModelOverrideKind,
        ScopedModelOverrideSummary, SessionModelRoutingStatus, SwitchTurnDuration, SwitchTurnPhase,
        SwitchTurnRequestId, SwitchTurnRequestSummary, TopologyEpoch,
    };
    use meerkat_core::lifecycle::run_primitive::ModelId;

    let baseline = ModelId::new(
        state
            .model_routing_baseline_model
            .clone()
            .unwrap_or_default(),
    );
    let topology_epoch = TopologyEpoch(state.model_routing_topology_epoch);
    let active_turn_override = state
        .model_routing_turn_override_id
        .as_ref()
        .and_then(|id| {
            let override_id = uuid_from_key(id)?;
            let request_id =
                uuid_from_key(state.model_routing_turn_request_id.as_deref().unwrap_or(id))?;
            Some(ScopedModelOverrideSummary {
                id: ScopedModelOverrideId::new(override_id),
                kind: ScopedModelOverrideKind::FiniteSwitchTurn {
                    request_id: SwitchTurnRequestId::new(request_id),
                    duration: FiniteScopedTurnDuration::Turns {
                        turns: std::num::NonZeroU32::new(
                            state
                                .model_routing_turn_remaining_turns
                                .unwrap_or(1)
                                .try_into()
                                .unwrap_or(1),
                        )
                        .unwrap_or(std::num::NonZeroU32::MIN),
                    },
                },
                target_model: ModelId::new(
                    state
                        .model_routing_turn_target_model
                        .clone()
                        .unwrap_or_default(),
                ),
                topology_epoch,
            })
        });

    let active_operation_override =
        state
            .model_routing_operation_override_id
            .as_ref()
            .and_then(|id| {
                let operation_id = uuid_from_key(id)?;
                Some(ScopedModelOverrideSummary {
                    id: ScopedModelOverrideId::new(operation_id),
                    kind: ScopedModelOverrideKind::ImageOperation {
                        operation_id: meerkat_core::image_generation::ImageOperationId::new(
                            operation_id,
                        ),
                    },
                    target_model: ModelId::new(
                        state
                            .model_routing_operation_target_model
                            .clone()
                            .unwrap_or_default(),
                    ),
                    topology_epoch,
                })
            });

    let pending_switch_turn = state
        .model_routing_pending_switch_request_id
        .as_ref()
        .and_then(|id| {
            let request_id = uuid_from_key(id)?;
            Some(SwitchTurnRequestSummary {
                request_id: SwitchTurnRequestId::new(request_id),
                target_model: ModelId::new(
                    state
                        .model_routing_pending_switch_target_model
                        .clone()
                        .unwrap_or_default(),
                ),
                duration: state
                    .model_routing_pending_switch_turns
                    .map(|turns| SwitchTurnDuration::Finite {
                        duration: if turns == 1 {
                            FiniteScopedTurnDuration::OneTurn
                        } else {
                            FiniteScopedTurnDuration::Turns {
                                turns: std::num::NonZeroU32::new(
                                    turns.try_into().unwrap_or(u32::MAX),
                                )
                                .unwrap_or(std::num::NonZeroU32::MIN),
                            }
                        },
                    })
                    .unwrap_or(SwitchTurnDuration::UntilChanged),
                phase: SwitchTurnPhase::PendingForBoundary,
            })
        });

    SessionModelRoutingStatus::new(
        baseline,
        active_turn_override,
        active_operation_override,
        pending_switch_turn,
    )
}

/// Project the DSL state's realtime-binding fields onto the shell-facing
/// `RealtimeAttachmentStatus` enum. The DSL owns the canonical fact; this
/// projection is pure and maintains the dogma's "derived projections are
/// rebuildable, never authoritative" principle.
fn project_realtime_attachment_status(
    state: &super::dsl::MeerkatMachineState,
) -> crate::meerkat_machine_types::RealtimeAttachmentStatus {
    use super::dsl::RealtimeBindingState;
    use crate::meerkat_machine_types::RealtimeAttachmentStatus;
    if state.realtime_reattach_required {
        return RealtimeAttachmentStatus::ReattachRequired;
    }
    match state.realtime_binding_state {
        RealtimeBindingState::Unbound => {
            if state.realtime_intent_present {
                RealtimeAttachmentStatus::IntentPresentUnbound
            } else {
                RealtimeAttachmentStatus::Unattached
            }
        }
        RealtimeBindingState::BindingNotReady => RealtimeAttachmentStatus::BindingNotReady,
        RealtimeBindingState::BindingReady => RealtimeAttachmentStatus::BindingReady,
        RealtimeBindingState::ReplacementPending => RealtimeAttachmentStatus::ReplacementPending,
    }
}

/// Wave-c C-9c R4: project the DSL's reconnect-progress fields onto the
/// shell-facing `ReconnectProgress` struct. Returns `None` when the
/// overlay has cleared (`attempt_count == 0` and no pending retry) —
/// the default state for a binding that isn't actively reconnecting.
fn project_realtime_reconnect_progress(
    state: &super::dsl::MeerkatMachineState,
) -> Option<crate::meerkat_machine_types::ReconnectProgress> {
    use crate::meerkat_machine_types::ReconnectProgress;
    use chrono::{DateTime, Utc};

    let attempt_count_u64 = state.realtime_reconnect_attempt_count;
    let next_retry_at_ms = state.realtime_reconnect_next_retry_at_ms;
    let deadline_at_ms = state.realtime_reconnect_deadline_at_ms;

    if attempt_count_u64 == 0 && next_retry_at_ms.is_none() && deadline_at_ms.is_none() {
        return None;
    }

    fn ms_to_utc(ms: Option<u64>) -> Option<DateTime<Utc>> {
        ms.and_then(|ms| {
            let secs = i64::try_from(ms / 1_000).ok()?;
            let nanos = u32::try_from((ms % 1_000) * 1_000_000).ok()?;
            DateTime::<Utc>::from_timestamp(secs, nanos)
        })
    }

    // Surface clamp: `RealtimeChannelStatus.attempt_count` is u32 on the wire;
    // any DSL-side overflow pegs at u32::MAX rather than wrapping.
    let attempt_count = u32::try_from(attempt_count_u64).unwrap_or(u32::MAX);

    Some(ReconnectProgress::new(
        attempt_count,
        ms_to_utc(next_retry_at_ms),
        ms_to_utc(deadline_at_ms),
    ))
}
