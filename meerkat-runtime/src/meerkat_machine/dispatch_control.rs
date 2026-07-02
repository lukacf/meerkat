use super::*;

impl MeerkatMachine {
    fn accept_outcome_matches_preview(preview: &AcceptOutcome, committed: &AcceptOutcome) -> bool {
        match (preview, committed) {
            (
                AcceptOutcome::Accepted {
                    input_id: preview_id,
                    ..
                },
                AcceptOutcome::Accepted {
                    input_id: committed_id,
                    ..
                },
            ) => preview_id == committed_id,
            (
                AcceptOutcome::Deduplicated {
                    input_id: preview_input,
                    existing_id: preview_existing,
                },
                AcceptOutcome::Deduplicated {
                    input_id: committed_input,
                    existing_id: committed_existing,
                },
            ) => preview_input == committed_input && preview_existing == committed_existing,
            (
                AcceptOutcome::Rejected {
                    reason: preview_reason,
                },
                AcceptOutcome::Rejected {
                    reason: committed_reason,
                },
            ) => preview_reason == committed_reason,
            _ => false,
        }
    }

    pub(super) async fn execute_meerkat_machine_control_command(
        &self,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeControlPlaneError> {
        match command {
            MeerkatMachineCommand::Ingest { runtime_id, input } => {
                let (
                    session_id,
                    driver,
                    _completions,
                    wake_tx,
                    effect_tx,
                    boundary_handle,
                    active_fence_token,
                    active_runtime_generation,
                    active_runtime_epoch_id,
                ) = {
                    let (sid, d, c, w) = self.lookup_entry(&runtime_id).await?;
                    let (effect, boundary_handle, fence, generation, epoch) = {
                        let sessions = self.sessions.read().await;
                        match sessions.get(&sid) {
                            Some(entry) => {
                                let authority = entry
                                    .dsl_authority
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                let state = authority.state();
                                (
                                    entry.effect_sender(),
                                    entry.boundary_handle(),
                                    state.active_fence_token,
                                    state.active_runtime_generation,
                                    state.active_runtime_epoch_id.clone(),
                                )
                            }
                            None => (None, None, None, None, None),
                        }
                    };
                    (
                        sid,
                        d,
                        c,
                        w,
                        effect,
                        boundary_handle,
                        fence,
                        generation,
                        epoch,
                    )
                };

                let _gate_guard = self
                    .lock_current_session_mutation_gate(&session_id)
                    .await
                    .ok_or(RuntimeControlPlaneError::InvalidState {
                        state: RuntimeState::Destroyed,
                    })?;

                // Use the canonical admission input id (the id the `Input`
                // already carries, which admission reports back verbatim as
                // `AcceptOutcome::Accepted { input_id }`) rather than minting a
                // disconnected provisional UUID. This keeps the work id the DSL
                // observes identical to the committed admission id.
                let ingest_input = crate::meerkat_machine::dsl::MeerkatMachineInput::Ingest {
                    session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&session_id),
                    runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(
                        &runtime_id,
                    ),
                    fence_token: active_fence_token.unwrap_or_default(),
                    generation: active_runtime_generation,
                    runtime_epoch_id: active_runtime_epoch_id,
                    work_id: crate::meerkat_machine::dsl::WorkId::from_domain(input.id()),
                    origin: crate::meerkat_machine::dsl::WorkOrigin::Ingest,
                };
                // On a machine-rejected Ingest, surface the TYPED lifecycle
                // state when the session has a known projected runtime state
                // (e.g. Retired/Stopped) — `InvalidState { state }` is the most
                // typed cause. Only when there is NO projected state (an unbound
                // session) do we surface the machine-owned DSL rejection reason
                // verbatim, rather than fabricating `InvalidState { Destroyed }`
                // from a defaulted projection (#56: do not re-author the cause as
                // a wrong state, but do not degrade a known typed state either).
                if let Err(reason) = self
                    .preview_session_dsl_input(&session_id, ingest_input.clone(), "Ingest")
                    .await
                {
                    return Err(
                        match self.existing_session_runtime_state(&session_id).await {
                            Some(state) => RuntimeControlPlaneError::InvalidState { state },
                            None => RuntimeControlPlaneError::Internal(reason),
                        },
                    );
                }
                self.apply_session_dsl_input_with_dispatch_failure(
                    &session_id,
                    ingest_input,
                    "Ingest",
                    CommittedEffectDispatchFailure::PreserveCommittedDslState,
                )
                .await
                .map_err(RuntimeControlPlaneError::Internal)?;

                let active_turn_boundary_available = Self::observe_active_turn_boundary_available(
                    &session_id,
                    boundary_handle.as_ref(),
                )
                .await
                .map_err(|err| RuntimeControlPlaneError::Internal(err.to_string()))?;

                let (outcome, signal, runtime_effect) = {
                    let resolved = {
                        let drv = driver.lock().await;
                        drv.resolve_admission_with_active_turn_boundary(
                            &input,
                            active_turn_boundary_available,
                        )
                        .map_err(|err| RuntimeControlPlaneError::Internal(err.to_string()))?
                    };
                    let flags = resolved.coarse_flags();
                    let preview_result = {
                        let drv = driver.lock().await;
                        drv.preview_accept_resolved_input(input.clone(), &resolved)
                            .await
                            .map_err(|err| RuntimeControlPlaneError::Internal(err.to_string()))?
                    };

                    let (signal, runtime_effect, accepted_effects) = match &preview_result {
                        AcceptOutcome::Accepted { input_id, .. } => {
                            let (_, effects) = self
                                .apply_session_dsl_input(
                                    &session_id,
                                    crate::meerkat_machine::dsl::MeerkatMachineInput::AcceptWithCompletion {
                                        input_id: crate::meerkat_machine::dsl::InputId::from_domain(
                                            input_id,
                                        ),
                                        request_immediate_processing: flags.request_immediate_processing,
                                        interrupt_yielding: flags.interrupt_yielding,
                                        wake_if_idle: flags.wake_if_idle,
                                    },
                                    "AcceptWithCompletion(Ingest)",
                                )
                                .await
                                .map_err(RuntimeControlPlaneError::Internal)?;
                            let signal = Self::post_admission_signal_from_effects(&effects);
                            let runtime_effect =
                                crate::effect::runtime_effect_projection_optional_from_dsl_effects(
                                    &effects,
                                )
                                .map_err(RuntimeControlPlaneError::Internal)?;
                            if signal.should_wake() && wake_tx.is_none() {
                                return Err(RuntimeControlPlaneError::InvalidState {
                                    state: RuntimeState::Destroyed,
                                });
                            }
                            (signal, runtime_effect, Some(effects))
                        }
                        AcceptOutcome::Deduplicated { .. } | AcceptOutcome::Rejected { .. } => (
                            crate::driver::ephemeral::PostAdmissionSignal::None,
                            None,
                            None,
                        ),
                    };

                    let result = {
                        let mut drv = driver.lock().await;
                        let result = drv
                            .accept_resolved_input(input, resolved)
                            .await
                            .map_err(|err| RuntimeControlPlaneError::Internal(err.to_string()))?;
                        if !Self::accept_outcome_matches_preview(&preview_result, &result) {
                            return Err(RuntimeControlPlaneError::Internal(format!(
                                "direct ingest admission preview diverged from committed outcome: preview={preview_result:?}, committed={result:?}"
                            )));
                        }
                        if let Some(effects) = accepted_effects.as_ref() {
                            drv.absorb_post_admission_effects(effects);
                        }
                        result
                    };
                    (result, signal, runtime_effect)
                };

                if signal.should_wake()
                    && let Some(ref tx) = wake_tx
                {
                    let _ = tx.try_send(());
                }
                if let Some(projected_effect) = runtime_effect
                    && let Err(err) = self
                        .dispatch_cancel_after_boundary_runtime_effect(
                            &session_id,
                            effect_tx,
                            boundary_handle,
                            projected_effect,
                            "AcceptWithCompletion(Ingest)",
                        )
                        .await
                {
                    return Err(RuntimeControlPlaneError::Internal(err.to_string()));
                }

                Ok(MeerkatMachineCommandResult::AcceptOutcome(outcome))
            }
            MeerkatMachineCommand::PublishEvent { event } => {
                let runtime_id = event.runtime_id.clone();
                let (session_id, driver, _completions, _wake_tx) =
                    self.lookup_entry(&runtime_id).await?;

                // DSL-first: stage PublishEvent before driver mutation.
                // Classify the event into the typed RuntimeEventKind discriminant
                // before consuming the event — the DSL carries the closed enum, not
                // a Debug-derived discriminant string. Exhaustive match (RuntimeEvent
                // is #[non_exhaustive] but defined in this crate, so a new variant is
                // a compile error here rather than a silent fallthrough).
                use crate::runtime_event::RuntimeEvent;
                let event_kind = match &event.event {
                    RuntimeEvent::InputLifecycle(_) => {
                        crate::meerkat_machine::dsl::RuntimeEventKind::InputLifecycle
                    }
                    RuntimeEvent::RunLifecycle(_) => {
                        crate::meerkat_machine::dsl::RuntimeEventKind::RunLifecycle
                    }
                    RuntimeEvent::RuntimeStateChange(_) => {
                        crate::meerkat_machine::dsl::RuntimeEventKind::RuntimeStateChange
                    }
                    RuntimeEvent::Topology(_) => {
                        crate::meerkat_machine::dsl::RuntimeEventKind::Topology
                    }
                    RuntimeEvent::Projection(_) => {
                        crate::meerkat_machine::dsl::RuntimeEventKind::Projection
                    }
                };
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
                tracing::info!(
                    runtime_id = %runtime_id,
                    "MeerkatMachine::Retire command start"
                );
                let (session_id, driver, completions, wake_tx) =
                    self.lookup_entry(&runtime_id).await?;
                tracing::info!(
                    runtime_id = %runtime_id,
                    session_id = %session_id,
                    "MeerkatMachine::Retire command looked up entry"
                );

                // Acquire the per-session mutation gate to serialize the
                // full DSL-stage → driver-mutate → DSL-sync span.
                let gate = self.session_mutation_gate(&session_id).await;
                tracing::info!(
                    runtime_id = %runtime_id,
                    session_id = %session_id,
                    "MeerkatMachine::Retire command locking mutation gate"
                );
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };
                tracing::info!(
                    runtime_id = %runtime_id,
                    session_id = %session_id,
                    "MeerkatMachine::Retire command locked mutation gate"
                );

                tracing::info!(
                    runtime_id = %runtime_id,
                    session_id = %session_id,
                    "MeerkatMachine::Retire command staging DSL"
                );
                let staged_dsl = self
                    .stage_session_dsl_transition(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::Retire {
                            session_id: crate::meerkat_machine::dsl::SessionId::from_domain(
                                &session_id,
                            ),
                        },
                        "Retire",
                    )
                    .await
                    .map_err(RuntimeControlPlaneError::Internal)?;
                tracing::info!(
                    runtime_id = %runtime_id,
                    session_id = %session_id,
                    "MeerkatMachine::Retire command staged DSL"
                );

                tracing::info!(
                    runtime_id = %runtime_id,
                    session_id = %session_id,
                    "MeerkatMachine::Retire command locking driver"
                );
                let mut drv = driver.lock().await;
                tracing::info!(
                    runtime_id = %runtime_id,
                    session_id = %session_id,
                    "MeerkatMachine::Retire command locked driver"
                );
                let mut report = match machine_retire(&mut drv).await {
                    Ok(report) => report,
                    Err(err) => {
                        drv.sync_control_projection_from_dsl_authority();
                        return Err(RuntimeControlPlaneError::Internal(err.to_string()));
                    }
                };
                drop(drv);
                tracing::info!(
                    runtime_id = %runtime_id,
                    session_id = %session_id,
                    inputs_pending_drain = report.inputs_pending_drain,
                    "MeerkatMachine::Retire command retired driver"
                );

                let mut commit_error = None;
                tracing::info!(
                    runtime_id = %runtime_id,
                    session_id = %session_id,
                    "MeerkatMachine::Retire command committing DSL"
                );
                if let Err(reason) = self
                    .commit_session_dsl_transition_preserving_committed_state(
                        &session_id,
                        staged_dsl,
                        "Retire",
                    )
                    .await
                {
                    driver
                        .lock()
                        .await
                        .sync_control_projection_from_dsl_authority();
                    commit_error = Some(reason);
                }
                tracing::info!(
                    runtime_id = %runtime_id,
                    session_id = %session_id,
                    "MeerkatMachine::Retire command committed DSL"
                );

                if report.inputs_pending_drain > 0 {
                    if let Some(ref tx) = wake_tx
                        && tx.send(()).await.is_ok()
                    {
                        if let Some(reason) = commit_error {
                            return Err(RuntimeControlPlaneError::Internal(reason));
                        }
                        return Ok(MeerkatMachineCommandResult::RetireReport(report));
                    }

                    let mut drv = driver.lock().await;
                    let abandoned = drv
                        .abandon_pending_inputs(crate::input_state::InputAbandonReason::Retired)
                        .await
                        .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
                    drop(drv);
                    let result_class =
                        crate::meerkat_machine::driver::machine_resolve_runtime_terminated_completion_result(
                            &driver,
                        )
                        .await
                        .map_err(|err| RuntimeControlPlaneError::Internal(err.to_string()))?;
                    let mut comp = completions.lock().await;
                    comp.resolve_all_runtime_terminated(
                        "retired without runtime loop",
                        result_class,
                    );
                    report.inputs_abandoned += abandoned;
                    report.inputs_pending_drain = 0;
                }
                if let Some(reason) = commit_error {
                    return Err(RuntimeControlPlaneError::Internal(reason));
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

                self.apply_session_dsl_input(
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
                            drv.sync_control_projection_from_dsl_authority();
                            return Err(RuntimeControlPlaneError::Internal(err.to_string()));
                        }
                    };

                    let active_after_recycle = drv.as_driver().active_input_ids();
                    (transferred, active_after_recycle)
                };

                {
                    let pending_after: HashSet<InputId> =
                        active_after_recycle.into_iter().collect();
                    let result_class =
                        crate::meerkat_machine::driver::machine_resolve_runtime_terminated_completion_result(
                            &driver,
                        )
                        .await
                        .map_err(|err| RuntimeControlPlaneError::Internal(err.to_string()))?;
                    let mut comp = completions.lock().await;
                    comp.resolve_not_pending_runtime_terminated(
                        |input_id| pending_after.contains(input_id),
                        "recycled input no longer pending",
                        result_class,
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

                self.apply_session_dsl_input(
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
                        drv.sync_control_projection_from_dsl_authority();
                        return Err(RuntimeControlPlaneError::Internal(err.to_string()));
                    }
                };
                drop(drv);

                let result_class =
                    crate::meerkat_machine::driver::machine_resolve_runtime_terminated_completion_result(
                        &driver,
                    )
                    .await
                    .map_err(|err| RuntimeControlPlaneError::Internal(err.to_string()))?;
                let mut comp = completions.lock().await;
                comp.resolve_all_runtime_terminated("runtime reset", result_class);
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

                self.apply_session_dsl_input(
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
                            drv.sync_control_projection_from_dsl_authority();
                            return Err(RuntimeControlPlaneError::Internal(err.to_string()));
                        }
                    };
                    let active_after_recover = drv.as_driver().active_input_ids();
                    (report, active_after_recover)
                };

                {
                    let pending_after: HashSet<InputId> =
                        active_after_recover.into_iter().collect();
                    let result_class =
                        crate::meerkat_machine::driver::machine_resolve_runtime_terminated_completion_result(
                            &driver,
                        )
                        .await
                        .map_err(|err| RuntimeControlPlaneError::Internal(err.to_string()))?;
                    let mut comp = completions.lock().await;
                    comp.resolve_not_pending_runtime_terminated(
                        |input_id| pending_after.contains(input_id),
                        "recovered input no longer pending",
                        result_class,
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

                let destroy_input = crate::meerkat_machine::dsl::MeerkatMachineInput::Destroy {
                    session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&session_id),
                };
                self.preview_session_dsl_input(&session_id, destroy_input.clone(), "Destroy")
                    .await
                    .map_err(RuntimeControlPlaneError::Internal)?;

                let mut drv = driver.lock().await;
                let prepared_destroy = match machine_prepare_destroy(&mut drv) {
                    Ok(prepared) => prepared,
                    Err(err) => return Err(RuntimeControlPlaneError::Internal(err.to_string())),
                };
                let staged_dsl = Self::stage_dsl_transition_on_authority(
                    &drv.shared_dsl_authority(),
                    destroy_input,
                    "Destroy",
                );
                let staged_dsl = match staged_dsl {
                    Ok(staged) => staged,
                    Err(reason) => {
                        drv.rollback_prepared_destroy_lifecycle(prepared_destroy.lifecycle);
                        drv.sync_control_projection_from_dsl_authority();
                        return Err(RuntimeControlPlaneError::Internal(reason));
                    }
                };
                let report = prepared_destroy.report;
                match Box::pin(machine_commit_prepared_destroy(
                    &mut drv,
                    prepared_destroy.lifecycle,
                ))
                .await
                {
                    Ok(()) => {}
                    Err(err) => {
                        drv.sync_control_projection_from_dsl_authority();
                        return Err(RuntimeControlPlaneError::Internal(err.to_string()));
                    }
                }
                drop(drv);

                // The durable destroy is already committed above, so a
                // completion-classification failure must NOT early-return and
                // skip the session DSL commit + waiter terminalization — that
                // would leave staged driver-side state behind a committed
                // terminal. Finish the commit/terminalize legs, then surface
                // the typed fault.
                let result_class =
                    crate::meerkat_machine::driver::machine_resolve_runtime_terminated_completion_result(
                        &driver,
                    )
                    .await;
                let apply_result = self
                    .commit_session_dsl_transition_preserving_committed_state(
                        &session_id,
                        staged_dsl,
                        "Destroy",
                    )
                    .await;
                driver
                    .lock()
                    .await
                    .sync_control_projection_from_dsl_authority();

                let mut comp = completions.lock().await;
                let result_class_err = match result_class {
                    Ok(result_class) => {
                        comp.resolve_all_runtime_terminated("runtime destroyed", result_class);
                        None
                    }
                    Err(err) => {
                        comp.fail_all_waiters(
                            crate::completion::CompletionWaitError::AuthorityUnavailable(format!(
                                "runtime destroyed without completion authority: {err}"
                            )),
                        );
                        Some(RuntimeControlPlaneError::Internal(err.to_string()))
                    }
                };
                drop(comp);
                if let Err(reason) = apply_result {
                    return Err(RuntimeControlPlaneError::Internal(reason));
                }
                if let Some(err) = result_class_err {
                    return Err(err);
                }
                Ok(MeerkatMachineCommandResult::DestroyReport(report))
            }
            MeerkatMachineCommand::RuntimeState { runtime_id } => {
                let session_id = self.resolve_session_id(&runtime_id).await?;
                let state = self
                    .existing_session_visible_runtime_state(&session_id)
                    .await
                    .ok_or(RuntimeControlPlaneError::NotFound(runtime_id))?;
                Ok(MeerkatMachineCommandResult::RuntimeState(state))
            }
            MeerkatMachineCommand::ResolvedSessionLlmCapabilities { session_id } => {
                let sessions = self.sessions.read().await;
                let entry = sessions.get(&session_id).ok_or_else(|| {
                    RuntimeControlPlaneError::NotFound(Self::logical_runtime_id(&session_id))
                })?;
                let (status, surface) = {
                    let authority = entry
                        .dsl_authority
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let state = authority.state();
                    (
                        state.current_session_capability_surface_status,
                        state.current_session_capability_surface,
                    )
                };
                let capabilities = match status {
                    crate::meerkat_machine::dsl::SessionLlmCapabilitySurfaceStatus::Resolved => {
                        surface.map(SessionLlmCapabilitySurface::from)
                    }
                    crate::meerkat_machine::dsl::SessionLlmCapabilitySurfaceStatus::Unresolved => {
                        None
                    }
                };
                Ok(MeerkatMachineCommandResult::ResolvedSessionLlmCapabilities(
                    capabilities,
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
                    RuntimeControlPlaneError::NotFound(Self::logical_runtime_id(&session_id))
                })?;
                let authority = entry
                    .dsl_authority
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                Ok(MeerkatMachineCommandResult::SessionModelRoutingStatus(
                    project_model_routing_status(authority.state()),
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
                                approval_reason: request
                                    .approval_reason
                                    .map(routing_switch_approval_reason),
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
                                    approval_reason: request
                                        .approval_reason
                                        .map(routing_switch_approval_reason),
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
                                        auth_binding: None,
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
                            if let Err(err) =
                                Box::pin(self.execute_meerkat_machine_session_command(reconfigure))
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
                    let reason = switch_denial_from_routing(
                        *reason,
                        status_state
                            .model_routing_switch_approval_reasons
                            .get(&request_key)
                            .copied(),
                    )
                    .map_err(RuntimeControlPlaneError::Internal)?;
                    return Ok(MeerkatMachineCommandResult::SwitchTurnControlResult(
                        meerkat_core::image_generation::SwitchTurnControlResult::Denied {
                            request_id: request.request_id,
                            reason,
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
                        approval_reason: request.approval_reason.map(routing_image_approval_reason),
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
                    let reason = image_denial_from_routing(
                        reason,
                        status_state
                            .model_routing_image_approval_reasons
                            .get(&operation_key)
                            .copied(),
                    )
                    .map_err(RuntimeControlPlaneError::Internal)?;
                    return Ok(MeerkatMachineCommandResult::ImageOperationRoutingResult(
                        crate::meerkat_machine_types::ImageOperationRoutingResult::Denied {
                            operation_id: request.operation_id,
                            reason,
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
            MeerkatMachineCommand::DenyImageOperationPlan {
                session_id,
                operation_id,
                reason,
            } => {
                let operation_key = image_operation_key(operation_id);
                let expected_reason = routing_image_plan_denial(&reason);
                let terminal =
                    meerkat_core::image_generation::ImageOperationTerminalClass::Denied {
                        reason: reason.clone(),
                    };
                self.apply_session_dsl_input(
                    &session_id,
                    crate::meerkat_machine::dsl::MeerkatMachineInput::DenyImageOperationPlan {
                        operation_id: operation_key.clone(),
                        reason: expected_reason,
                        terminal_payload: serde_json::to_string(&terminal).map_err(|err| {
                            RuntimeControlPlaneError::Internal(format!(
                                "failed to serialize image operation planner denial: {err}"
                            ))
                        })?,
                    },
                    "DenyImageOperationPlan",
                )
                .await
                .map_err(RuntimeControlPlaneError::Internal)?;
                let state = self.session_dsl_state(&session_id).await?;
                let machine_phase = state
                    .model_routing_image_operation_phases
                    .get(&operation_key)
                    .copied()
                    .ok_or_else(|| {
                        RuntimeControlPlaneError::Internal(format!(
                            "image operation planner denial missing machine phase for {operation_key}"
                        ))
                    })?;
                if machine_phase != super::dsl::RoutingImageOperationPhase::Terminal {
                    return Err(RuntimeControlPlaneError::Internal(format!(
                        "image operation planner denial did not terminalize {operation_key}: {machine_phase:?}"
                    )));
                }
                let machine_terminal = state
                    .model_routing_image_terminals
                    .get(&operation_key)
                    .copied()
                    .ok_or_else(|| {
                        RuntimeControlPlaneError::Internal(format!(
                            "image operation planner denial missing machine terminal for {operation_key}"
                        ))
                    })?;
                if machine_terminal != super::dsl::RoutingImageTerminal::Denied {
                    return Err(RuntimeControlPlaneError::Internal(format!(
                        "image operation planner denial recorded non-denied terminal for {operation_key}: {machine_terminal:?}"
                    )));
                }
                let machine_reason = state
                    .model_routing_image_plan_denials
                    .get(&operation_key)
                    .copied()
                    .ok_or_else(|| {
                        RuntimeControlPlaneError::Internal(format!(
                            "image operation planner denial missing machine denial reason for {operation_key}"
                        ))
                    })?;
                if machine_reason != expected_reason {
                    return Err(RuntimeControlPlaneError::Internal(format!(
                        "image operation planner denial reason drift for {operation_key}: {machine_reason:?}"
                    )));
                }
                let terminal_payload = state
                    .model_routing_image_terminal_payloads
                    .get(&operation_key)
                    .ok_or_else(|| {
                        RuntimeControlPlaneError::Internal(format!(
                            "image operation planner denial missing machine terminal payload for {operation_key}"
                        ))
                    })?;
                let terminal = serde_json::from_str(terminal_payload).map_err(|err| {
                    RuntimeControlPlaneError::Internal(format!(
                        "image operation planner denial machine terminal payload is invalid for {operation_key}: {err}"
                    ))
                })?;
                Ok(MeerkatMachineCommandResult::ImageOperationPhase(
                    meerkat_core::image_generation::ImageOperationPhase::Terminal { terminal },
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
            MeerkatMachineCommand::ClassifyImageOperationTerminal {
                session_id,
                operation_id,
                observation,
                provider_text,
            } => {
                let operation_key = image_operation_key(operation_id);
                let (observation, http_status_code, error_code) =
                    routing_image_terminal_observation(&observation);
                let provider_text_disposition = routing_provider_text_disposition(&provider_text)
                    .map_err(RuntimeControlPlaneError::Internal)?;
                let (_, effects) = self
                    .apply_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::ClassifyImageOperationTerminal {
                            operation_id: operation_key.clone(),
                            observation,
                            http_status_code,
                            error_code,
                            provider_text: provider_text_disposition,
                        },
                        "ClassifyImageOperationTerminal",
                    )
                    .await
                    .map_err(RuntimeControlPlaneError::Internal)?;
                let (terminal, effect_provider_text) = effects
                    .as_slice()
                    .iter()
                    .find_map(|effect| match effect {
                        crate::meerkat_machine::dsl::MeerkatMachineEffect::ImageOperationTerminalClassified {
                            operation_id: effect_operation_id,
                            terminal,
                            provider_text,
                        } if effect_operation_id == &operation_key => Some((*terminal, *provider_text)),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        RuntimeControlPlaneError::Internal(format!(
                            "image operation terminal classification emitted no authority effect for {operation_key}"
                        ))
                    })?;
                if effect_provider_text != provider_text_disposition {
                    return Err(RuntimeControlPlaneError::Internal(format!(
                        "image operation terminal classification provider-text drift for {operation_key}: input={provider_text_disposition:?}, effect={effect_provider_text:?}"
                    )));
                }
                let terminal = image_terminal_from_classification(terminal, &provider_text)
                    .map_err(RuntimeControlPlaneError::Internal)?;
                Ok(MeerkatMachineCommandResult::ImageOperationTerminalClass(
                    terminal,
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
                let operation_phase = state
                    .model_routing_image_operation_phases
                    .get(&operation_key)
                    .copied()
                    .ok_or_else(|| {
                        RuntimeControlPlaneError::Internal(format!(
                            "image operation restore missing machine phase for {operation_key}"
                        ))
                    })?;
                if operation_phase
                    != super::dsl::RoutingImageOperationPhase::RestoringScopedOverride
                {
                    return Err(RuntimeControlPlaneError::Internal(format!(
                        "image operation restore requires machine restoring phase for {operation_key}: {operation_phase:?}"
                    )));
                }
                let machine_terminal = state
                    .model_routing_image_terminals
                    .get(&operation_key)
                    .copied()
                    .ok_or_else(|| {
                        RuntimeControlPlaneError::Internal(format!(
                            "image operation restore missing machine terminal for {operation_key}"
                        ))
                    })?;
                let terminal_payload = state
                    .model_routing_image_terminal_payloads
                    .get(&operation_key)
                    .ok_or_else(|| {
                        RuntimeControlPlaneError::Internal(format!(
                            "image operation restore missing machine terminal payload for {operation_key}"
                        ))
                    })?;
                let terminal: meerkat_core::image_generation::ImageOperationTerminalClass =
                    serde_json::from_str(terminal_payload).map_err(|err| {
                        RuntimeControlPlaneError::Internal(format!(
                            "image operation restore machine terminal payload is invalid for {operation_key}: {err}"
                        ))
                    })?;
                let payload_terminal = routing_image_terminal(terminal.clone());
                if payload_terminal != machine_terminal {
                    return Err(RuntimeControlPlaneError::Internal(format!(
                        "image operation restore terminal payload disagrees with machine terminal for {operation_key}: payload={payload_terminal:?}, machine={machine_terminal:?}"
                    )));
                }
                self.apply_session_dsl_input(
                    &session_id,
                    crate::meerkat_machine::dsl::MeerkatMachineInput::RestoreImageOperationOverride {
                        operation_id: operation_key.clone(),
                    },
                    "RestoreImageOperationOverride",
                )
                .await
                .map_err(RuntimeControlPlaneError::Internal)?;
                let restored_state = self.session_dsl_state(&session_id).await?;
                let restored_phase = restored_state
                    .model_routing_image_operation_phases
                    .get(&operation_key)
                    .copied()
                    .ok_or_else(|| {
                        RuntimeControlPlaneError::Internal(format!(
                            "image operation restore missing terminal machine phase for {operation_key}"
                        ))
                    })?;
                if restored_phase != super::dsl::RoutingImageOperationPhase::Terminal {
                    return Err(RuntimeControlPlaneError::Internal(format!(
                        "image operation restore did not terminalize {operation_key}: {restored_phase:?}"
                    )));
                }
                Ok(MeerkatMachineCommandResult::ImageOperationPhase(
                    meerkat_core::image_generation::ImageOperationPhase::Terminal { terminal },
                ))
            }
            MeerkatMachineCommand::LoadBoundaryReceipt {
                runtime_id,
                run_id,
                sequence,
            } => {
                let _session_id = self.resolve_session_id(&runtime_id).await?;
                let receipt = match &self.store {
                    Some(store) => super::driver::load_boundary_receipt_for_runtime(
                        store.as_ref(),
                        &runtime_id,
                        &run_id,
                        sequence,
                    )
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
    approval_reason: Option<super::dsl::RoutingSwitchApprovalReason>,
) -> Result<meerkat_core::image_generation::SwitchTurnDenialReason, String> {
    use meerkat_core::image_generation::SwitchTurnDenialReason;
    match reason {
        super::dsl::RoutingDenialReason::ApprovalRequiredButUnavailable => {
            Ok(SwitchTurnDenialReason::ApprovalRequiredButUnavailable)
        }
        super::dsl::RoutingDenialReason::DeniedDuringApproval => {
            let approval_reason = approval_reason.ok_or_else(|| {
                "generated switch-turn denial is missing approval reason".to_string()
            })?;
            Ok(SwitchTurnDenialReason::DeniedDuringApproval {
                approvable: switch_approval_reason_from_routing(approval_reason),
            })
        }
        super::dsl::RoutingDenialReason::ScopedOverrideConflict => {
            Ok(SwitchTurnDenialReason::ScopedOverrideConflict)
        }
        super::dsl::RoutingDenialReason::RealtimeTransportConflict => {
            Ok(SwitchTurnDenialReason::RealtimeTransportConflict)
        }
        super::dsl::RoutingDenialReason::CapabilityPolicy => {
            Ok(SwitchTurnDenialReason::CapabilityPolicy)
        }
    }
}

fn image_denial_from_routing(
    reason: super::dsl::RoutingDenialReason,
    approval_reason: Option<super::dsl::RoutingImageApprovalReason>,
) -> Result<meerkat_core::image_generation::ImageOperationDenialReason, String> {
    use meerkat_core::image_generation::ImageOperationDenialReason;
    match reason {
        super::dsl::RoutingDenialReason::ApprovalRequiredButUnavailable => {
            Ok(ImageOperationDenialReason::ApprovalRequiredButUnavailable)
        }
        super::dsl::RoutingDenialReason::DeniedDuringApproval => {
            let approval_reason = approval_reason.ok_or_else(|| {
                "generated image-operation denial is missing approval reason".to_string()
            })?;
            Ok(ImageOperationDenialReason::DeniedDuringApproval {
                approvable: image_approval_reason_from_routing(approval_reason),
            })
        }
        super::dsl::RoutingDenialReason::ScopedOverrideConflict => {
            Ok(ImageOperationDenialReason::ScopedOverrideConflict)
        }
        super::dsl::RoutingDenialReason::RealtimeTransportConflict => {
            Ok(ImageOperationDenialReason::RealtimeTransportConflict)
        }
        super::dsl::RoutingDenialReason::CapabilityPolicy => {
            Ok(ImageOperationDenialReason::CapabilityPolicy)
        }
    }
}

fn routing_switch_approval_reason(
    reason: meerkat_core::image_generation::SwitchTurnApprovalReason,
) -> super::dsl::RoutingSwitchApprovalReason {
    use meerkat_core::image_generation::SwitchTurnApprovalReason;
    match reason {
        SwitchTurnApprovalReason::CrossProvider => {
            super::dsl::RoutingSwitchApprovalReason::CrossProvider
        }
        SwitchTurnApprovalReason::CostExceedsThreshold => {
            super::dsl::RoutingSwitchApprovalReason::CostExceedsThreshold
        }
        SwitchTurnApprovalReason::SafetyHold => super::dsl::RoutingSwitchApprovalReason::SafetyHold,
        SwitchTurnApprovalReason::UntilChangedFromModelOrigin => {
            super::dsl::RoutingSwitchApprovalReason::UntilChangedFromModelOrigin
        }
        SwitchTurnApprovalReason::RealtimeDetachRequired => {
            super::dsl::RoutingSwitchApprovalReason::RealtimeDetachRequired
        }
    }
}

fn switch_approval_reason_from_routing(
    reason: super::dsl::RoutingSwitchApprovalReason,
) -> meerkat_core::image_generation::SwitchTurnApprovalReason {
    use meerkat_core::image_generation::SwitchTurnApprovalReason;
    match reason {
        super::dsl::RoutingSwitchApprovalReason::CrossProvider => {
            SwitchTurnApprovalReason::CrossProvider
        }
        super::dsl::RoutingSwitchApprovalReason::CostExceedsThreshold => {
            SwitchTurnApprovalReason::CostExceedsThreshold
        }
        super::dsl::RoutingSwitchApprovalReason::SafetyHold => SwitchTurnApprovalReason::SafetyHold,
        super::dsl::RoutingSwitchApprovalReason::UntilChangedFromModelOrigin => {
            SwitchTurnApprovalReason::UntilChangedFromModelOrigin
        }
        super::dsl::RoutingSwitchApprovalReason::RealtimeDetachRequired => {
            SwitchTurnApprovalReason::RealtimeDetachRequired
        }
    }
}

fn routing_image_approval_reason(
    reason: meerkat_core::image_generation::ImageOperationApprovalReason,
) -> super::dsl::RoutingImageApprovalReason {
    use meerkat_core::image_generation::ImageOperationApprovalReason;
    match reason {
        ImageOperationApprovalReason::CrossProvider => {
            super::dsl::RoutingImageApprovalReason::CrossProvider
        }
        ImageOperationApprovalReason::CostExceedsThreshold => {
            super::dsl::RoutingImageApprovalReason::CostExceedsThreshold
        }
        ImageOperationApprovalReason::SafetyHold => {
            super::dsl::RoutingImageApprovalReason::SafetyHold
        }
        ImageOperationApprovalReason::RealtimeDetachRequired => {
            super::dsl::RoutingImageApprovalReason::RealtimeDetachRequired
        }
    }
}

fn image_approval_reason_from_routing(
    reason: super::dsl::RoutingImageApprovalReason,
) -> meerkat_core::image_generation::ImageOperationApprovalReason {
    use meerkat_core::image_generation::ImageOperationApprovalReason;
    match reason {
        super::dsl::RoutingImageApprovalReason::CrossProvider => {
            ImageOperationApprovalReason::CrossProvider
        }
        super::dsl::RoutingImageApprovalReason::CostExceedsThreshold => {
            ImageOperationApprovalReason::CostExceedsThreshold
        }
        super::dsl::RoutingImageApprovalReason::SafetyHold => {
            ImageOperationApprovalReason::SafetyHold
        }
        super::dsl::RoutingImageApprovalReason::RealtimeDetachRequired => {
            ImageOperationApprovalReason::RealtimeDetachRequired
        }
    }
}

fn routing_image_plan_denial(
    reason: &meerkat_core::image_generation::ImageOperationDenialReason,
) -> super::dsl::RoutingImagePlanDenialReason {
    use meerkat_core::image_generation::ImageOperationDenialReason;
    match reason {
        ImageOperationDenialReason::UnsupportedTarget => {
            super::dsl::RoutingImagePlanDenialReason::UnsupportedTarget
        }
        ImageOperationDenialReason::UnsupportedCount => {
            super::dsl::RoutingImagePlanDenialReason::UnsupportedCount
        }
        ImageOperationDenialReason::CapabilityPolicy => {
            super::dsl::RoutingImagePlanDenialReason::CapabilityPolicy
        }
        ImageOperationDenialReason::CostPolicy => {
            super::dsl::RoutingImagePlanDenialReason::CostPolicy
        }
        ImageOperationDenialReason::SafetyPolicy => {
            super::dsl::RoutingImagePlanDenialReason::SafetyPolicy
        }
        ImageOperationDenialReason::ApprovalRequiredButUnavailable => {
            super::dsl::RoutingImagePlanDenialReason::ApprovalRequiredButUnavailable
        }
        ImageOperationDenialReason::DeniedDuringApproval { .. } => {
            super::dsl::RoutingImagePlanDenialReason::DeniedDuringApproval
        }
        ImageOperationDenialReason::ScopedOverrideConflict => {
            super::dsl::RoutingImagePlanDenialReason::ScopedOverrideConflict
        }
        ImageOperationDenialReason::RealtimeTransportConflict => {
            super::dsl::RoutingImagePlanDenialReason::RealtimeTransportConflict
        }
        ImageOperationDenialReason::ProjectionUnsupported => {
            super::dsl::RoutingImagePlanDenialReason::ProjectionUnsupported
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

fn routing_image_terminal_observation(
    observation: &meerkat_core::image_generation::ImageProviderTerminalObservation,
) -> (
    super::dsl::RoutingImageTerminalObservation,
    Option<u64>,
    super::dsl::RoutingImageProviderErrorCode,
) {
    use meerkat_core::image_generation::ImageProviderTerminalObservation;
    match observation {
        ImageProviderTerminalObservation::Generated => (
            super::dsl::RoutingImageTerminalObservation::Generated,
            None,
            super::dsl::RoutingImageProviderErrorCode::Unknown,
        ),
        ImageProviderTerminalObservation::EmptyResult => (
            super::dsl::RoutingImageTerminalObservation::EmptyResult,
            None,
            super::dsl::RoutingImageProviderErrorCode::Unknown,
        ),
        ImageProviderTerminalObservation::ProviderHttpError { status_code, code } => (
            super::dsl::RoutingImageTerminalObservation::ProviderHttpError,
            status_code.map(u64::from),
            routing_image_provider_error_code(*code),
        ),
        ImageProviderTerminalObservation::ProviderNativeError { code } => (
            super::dsl::RoutingImageTerminalObservation::ProviderNativeError,
            None,
            routing_image_provider_error_code(*code),
        ),
        ImageProviderTerminalObservation::ExecutionFailed => (
            super::dsl::RoutingImageTerminalObservation::ExecutionFailed,
            None,
            super::dsl::RoutingImageProviderErrorCode::Unknown,
        ),
        ImageProviderTerminalObservation::BlobCommitFailed => (
            super::dsl::RoutingImageTerminalObservation::BlobCommitFailed,
            None,
            super::dsl::RoutingImageProviderErrorCode::Unknown,
        ),
    }
}

fn routing_image_provider_error_code(
    code: meerkat_core::image_generation::ImageProviderErrorCode,
) -> super::dsl::RoutingImageProviderErrorCode {
    use meerkat_core::image_generation::ImageProviderErrorCode;
    match code {
        ImageProviderErrorCode::Unknown => super::dsl::RoutingImageProviderErrorCode::Unknown,
        ImageProviderErrorCode::OpenAiContentFilter => {
            super::dsl::RoutingImageProviderErrorCode::OpenAiContentFilter
        }
        ImageProviderErrorCode::OpenAiModelRefusal => {
            super::dsl::RoutingImageProviderErrorCode::OpenAiModelRefusal
        }
        ImageProviderErrorCode::GeminiSafety => {
            super::dsl::RoutingImageProviderErrorCode::GeminiSafety
        }
        ImageProviderErrorCode::GeminiModelRefusal => {
            super::dsl::RoutingImageProviderErrorCode::GeminiModelRefusal
        }
        ImageProviderErrorCode::GeminiDeadlineExceeded => {
            super::dsl::RoutingImageProviderErrorCode::GeminiDeadlineExceeded
        }
    }
}

fn routing_provider_text_disposition(
    provider_text: &meerkat_core::image_generation::ProviderTextDisposition,
) -> Result<super::dsl::RoutingProviderTextDisposition, String> {
    use meerkat_core::image_generation::ProviderTextDisposition;
    match provider_text {
        ProviderTextDisposition::NotEmitted => {
            Ok(super::dsl::RoutingProviderTextDisposition::NotEmitted)
        }
        ProviderTextDisposition::Captured { .. } => {
            Ok(super::dsl::RoutingProviderTextDisposition::Captured)
        }
        ProviderTextDisposition::EmittedButNotStored => {
            Ok(super::dsl::RoutingProviderTextDisposition::EmittedButNotStored)
        }
        ProviderTextDisposition::UnsupportedByBackend => {
            Err("image operation terminal classification does not accept unsupported provider text disposition".into())
        }
    }
}

fn image_terminal_from_classification(
    terminal: super::dsl::RoutingImageTerminal,
    provider_text: &meerkat_core::image_generation::ProviderTextDisposition,
) -> Result<meerkat_core::image_generation::ImageOperationTerminalClass, String> {
    use meerkat_core::image_generation::ImageOperationTerminalClass;
    match terminal {
        super::dsl::RoutingImageTerminal::Generated => Ok(ImageOperationTerminalClass::Generated),
        super::dsl::RoutingImageTerminal::EmptyResult => {
            Ok(ImageOperationTerminalClass::EmptyResult {
                provider_text: provider_text.clone(),
            })
        }
        super::dsl::RoutingImageTerminal::RefusedByProvider => {
            Ok(ImageOperationTerminalClass::RefusedByProvider)
        }
        super::dsl::RoutingImageTerminal::SafetyFiltered => {
            Ok(ImageOperationTerminalClass::SafetyFiltered)
        }
        super::dsl::RoutingImageTerminal::Failed => Ok(ImageOperationTerminalClass::Failed),
        super::dsl::RoutingImageTerminal::Cancelled => Ok(ImageOperationTerminalClass::Cancelled),
        super::dsl::RoutingImageTerminal::Timeout => Ok(ImageOperationTerminalClass::Timeout),
        super::dsl::RoutingImageTerminal::Denied
        | super::dsl::RoutingImageTerminal::ScopedRestoreFailed => Err(format!(
            "generated image terminal classification returned invalid provider terminal {terminal:?}"
        )),
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
    // The session's typed provider identity is owned by the machine's
    // `current_session_llm_identity` (hydrated at construction, recommitted
    // by the generated live-reconfigure transition). Project it as-is —
    // `None` faithfully reports "not hydrated"; downstream consumers must
    // not re-derive a provider from any model string.
    let session_provider = state
        .current_session_llm_identity
        .as_ref()
        .map(|identity| meerkat_core::Provider::from(identity.provider));
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
    .with_session_provider(session_provider)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn make_prompt(text: &str) -> Input {
        Input::Prompt(crate::input::PromptInput {
            injected_context: Vec::new(),
            header: crate::input::InputHeader {
                id: InputId::new(),
                timestamp: chrono::Utc::now(),
                source: crate::input::InputOrigin::Operator,
                durability: crate::input::InputDurability::Durable,
                visibility: crate::input::InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            content: text.into(),
            typed_turn_appends: Vec::new(),
            turn_metadata: None,
        })
    }

    /// Row #56 gate: a machine-rejected direct `Ingest` must surface the
    /// session's REAL typed lifecycle state, NOT a re-derived
    /// `InvalidState { state: Destroyed }` fabricated from a defaulted
    /// projection. A freshly registered session has no runtime binding (no
    /// `PrepareBindings`), so the DSL preview rejects the `Ingest` transition;
    /// the dispatcher surfaces the actual (non-`Destroyed`) state rather than
    /// laundering the cause into a fabricated state.
    #[tokio::test]
    async fn rejected_direct_ingest_surfaces_machine_reason_not_invalid_state_destroyed() {
        let machine = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        machine
            .register_session(session_id.clone())
            .await
            .expect("register session");

        let runtime_id = MeerkatMachine::logical_runtime_id(&session_id);
        let err = machine
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Ingest {
                    runtime_id,
                    input: make_prompt("ingest before binding"),
                },
            )
            .await
            .map_err(MeerkatMachine::control_plane_error_from_command_error)
            .expect_err("ingest on an unbound session must be rejected by the DSL preview");

        // OLD behavior laundered the rejection into a re-derived
        // `InvalidState { state: Destroyed }` from a defaulted projection. The
        // fix surfaces the session's REAL typed lifecycle state when it is known
        // (here, the unbound pre-binding state), which must never be the
        // fabricated `Destroyed`. A typed `InvalidState { state }` is strictly
        // more informative than an `Internal(reason)` string.
        match err {
            RuntimeControlPlaneError::InvalidState { state } => {
                assert_ne!(
                    state,
                    RuntimeState::Destroyed,
                    "ingest rejection must surface the real pre-binding state, \
                     not a fabricated InvalidState{{Destroyed}}"
                );
            }
            other => panic!(
                "expected a typed InvalidState{{state}} for the unbound session, got {other:?}"
            ),
        }
    }

    /// The model-routing status projection must carry the session's typed
    /// provider identity from the machine-owned `current_session_llm_identity`
    /// — `None` before hydration, the hydrated provider after, and the
    /// SWAPPED provider after the identity is recommitted (the same machine
    /// fact the live `ReconfigureSessionLlmIdentity` transition writes).
    /// Consumers (the image auto-planner) read this instead of re-deriving a
    /// provider from the effective model string through the built-in catalog,
    /// which has no row for `ModelRegistry`-owned custom models.
    #[test]
    fn model_routing_status_projects_session_provider_from_generated_identity() {
        use crate::meerkat_machine::dsl as mm_dsl;

        let mut authority = crate::meerkat_machine::dsl_authority::new_initialized_authority(
            "initialize projection-test authority",
        );
        mm_dsl::MeerkatMachineMutator::apply(
            &mut authority,
            mm_dsl::MeerkatMachineInput::RegisterSession {
                session_id: mm_dsl::SessionId::from("projection-session"),
            },
        )
        .expect("register session");

        // Pre-hydration: the machine holds no identity, so the projection
        // reports None rather than minting a provider from the model string.
        assert_eq!(
            project_model_routing_status(authority.state()).session_provider,
            None
        );

        let hydrate =
            |provider: mm_dsl::Provider| mm_dsl::MeerkatMachineInput::HydrateSessionLlmState {
                current_identity: mm_dsl::SessionLlmIdentity {
                    // Deliberately NOT a built-in catalog model: the projected
                    // provider must come from the typed identity, never from
                    // model-name inference.
                    model: "my-custom-model".to_string(),
                    provider,
                    self_hosted_server_id: None,
                    provider_params_repr: None,
                    auth_binding: None,
                },
                current_capability_surface: None,
                current_capability_surface_status:
                    mm_dsl::SessionLlmCapabilitySurfaceStatus::Unresolved,
                current_capability_base_filter: mm_dsl::ToolFilter::All,
            };

        mm_dsl::MeerkatMachineMutator::apply(&mut authority, hydrate(mm_dsl::Provider::Gemini))
            .expect("hydrate session llm identity");
        assert_eq!(
            project_model_routing_status(authority.state()).session_provider,
            Some(meerkat_core::Provider::Gemini)
        );

        // Identity swap: once the machine recommits a new current identity,
        // the projection follows the swapped provider.
        mm_dsl::MeerkatMachineMutator::apply(&mut authority, hydrate(mm_dsl::Provider::OpenAI))
            .expect("re-hydrate swapped session llm identity");
        assert_eq!(
            project_model_routing_status(authority.state()).session_provider,
            Some(meerkat_core::Provider::OpenAI)
        );
    }
}
