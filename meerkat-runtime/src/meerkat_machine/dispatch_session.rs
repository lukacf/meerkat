use super::*;

impl MeerkatMachine {
    pub(super) async fn execute_meerkat_machine_session_command(
        &self,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        match command {
            MeerkatMachineCommand::RegisterSession { session_id } => {
                // Guard: DestroyedShapeInvariant — a destroyed binding must
                // never be resurrected.
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                let sid = session_id.clone();
                self.register_session_inner(session_id).await;
                if let Some(entry) = self.sessions.write().await.get_mut(&sid) {
                    // DSL shadow
                    use crate::meerkat_machine::dsl as mm_dsl;
                    if let Err(e) = mm_dsl::MeerkatMachineMutator::apply(
                        &mut entry.dsl_authority,
                        mm_dsl::MeerkatMachineInput::RegisterSession {
                            session_id: mm_dsl::SessionId::from(sid.to_string()),
                        },
                    ) {
                        tracing::error!(
                            error = %e,
                            "DSL/runtime DISAGREEMENT on RegisterSession"
                        );
                    }
                }
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::UnregisterSession { session_id } => {
                // Guard: session must exist before it can be unregistered.
                if !self.sessions.read().await.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                self.unregister_session_inner(&session_id).await;
                if let Some(entry) = self.sessions.write().await.get_mut(&session_id) {
                    // DSL shadow
                    use crate::meerkat_machine::dsl as mm_dsl;
                    if let Err(e) = mm_dsl::MeerkatMachineMutator::apply(
                        &mut entry.dsl_authority,
                        mm_dsl::MeerkatMachineInput::UnregisterSession {
                            session_id: mm_dsl::SessionId::from(session_id.to_string()),
                        },
                    ) {
                        tracing::error!(
                            error = %e,
                            "DSL/runtime DISAGREEMENT on UnregisterSession"
                        );
                    }
                }
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::EnsureSessionWithExecutor {
                session_id,
                executor,
            } => {
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                self.ensure_session_with_executor_inner(session_id, executor)
                    .await;
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::SetSilentIntents {
                session_id,
                intents,
            } => {
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                self.set_session_silent_intents_inner(&session_id, intents)
                    .await;
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::InterruptCurrentRun { session_id } => {
                // Guard: DestroyedShapeInvariant — no mutation on destroyed sessions.
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                self.interrupt_current_run_inner(&session_id)
                    .await
                    .map(|()| MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::CancelAfterBoundary { session_id } => {
                // Guard: DestroyedShapeInvariant — no mutation on destroyed sessions.
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                self.cancel_after_boundary_inner(&session_id)
                    .await
                    .map(|()| MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::StopRuntimeExecutor {
                session_id,
                command,
            } => {
                // Guard: DestroyedShapeInvariant — no mutation on destroyed sessions.
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                self.stop_runtime_executor_inner(&session_id, command)
                    .await?;
                if let Some(entry) = self.sessions.write().await.get_mut(&session_id) {
                    // DSL shadow
                    use crate::meerkat_machine::dsl as mm_dsl;
                    if let Err(e) = mm_dsl::MeerkatMachineMutator::apply(
                        &mut entry.dsl_authority,
                        mm_dsl::MeerkatMachineInput::StopRuntimeExecutor,
                    ) {
                        tracing::error!(
                            error = %e,
                            "DSL/runtime DISAGREEMENT on StopRuntimeExecutor"
                        );
                    }
                }
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::ContainsSession { session_id } => {
                Ok(MeerkatMachineCommandResult::Bool(
                    self.sessions.read().await.contains_key(&session_id),
                ))
            }
            MeerkatMachineCommand::SessionHasExecutor { session_id } => {
                let sessions = self.sessions.read().await;
                Ok(MeerkatMachineCommandResult::Bool(
                    sessions
                        .get(&session_id)
                        .map(RuntimeSessionEntry::has_attachment_or_attaching)
                        .unwrap_or(false),
                ))
            }
            MeerkatMachineCommand::SessionHasComms { session_id } => {
                let slots = self.comms_drain_slots.read().await;
                Ok(MeerkatMachineCommandResult::Bool(
                    slots.contains_key(&session_id),
                ))
            }
            MeerkatMachineCommand::OpsLifecycleRegistry { session_id } => {
                let sessions = self.sessions.read().await;
                Ok(MeerkatMachineCommandResult::OpsLifecycleRegistry(
                    sessions
                        .get(&session_id)
                        .map(|e| Arc::clone(&e.ops_lifecycle)),
                ))
            }
            MeerkatMachineCommand::PrepareBindings { session_id } => {
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                self.register_session_inner(session_id.clone()).await;
                let sessions = self.sessions.read().await;
                let entry = sessions
                    .get(&session_id)
                    .ok_or(RuntimeDriverError::Internal(format!(
                        "session {session_id} missing after register_session_inner"
                    )))?;
                let mut driver = entry.driver.lock().await;
                machine_prepare_bindings_projection(&mut driver)?;
                drop(driver);
                // TODO: DSL shadow needs field construction
                Ok(MeerkatMachineCommandResult::Bindings(
                    meerkat_core::SessionRuntimeBindings {
                        session_id,
                        epoch_id: entry.epoch_id.clone(),
                        ops_lifecycle: Arc::clone(&entry.ops_lifecycle)
                            as Arc<dyn meerkat_core::OpsLifecycleRegistry>,
                        cursor_state: Arc::clone(&entry.cursor_state),
                        tool_visibility_owner: Arc::clone(&entry.tool_visibility_owner)
                            as Arc<dyn meerkat_core::ToolVisibilityOwner>,
                    },
                ))
            }
            MeerkatMachineCommand::InputState {
                session_id,
                input_id,
            } => {
                let driver = {
                    let sessions = self.sessions.read().await;
                    let entry = sessions
                        .get(&session_id)
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?;
                    entry.driver.clone()
                };
                let driver = driver.lock().await;
                Ok(MeerkatMachineCommandResult::InputState(
                    driver.as_driver().input_state(&input_id).cloned(),
                ))
            }
            MeerkatMachineCommand::ListActiveInputs { session_id } => {
                let driver = {
                    let sessions = self.sessions.read().await;
                    let entry = sessions
                        .get(&session_id)
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?;
                    entry.driver.clone()
                };
                let driver = driver.lock().await;
                Ok(MeerkatMachineCommandResult::ActiveInputs(
                    driver.as_driver().active_input_ids(),
                ))
            }
            MeerkatMachineCommand::ReconfigureSessionLlmIdentity {
                session_id,
                previous_identity,
                previous_visibility_state,
                previous_capability_surface,
                previous_capability_surface_status,
                target_identity,
                target_capability_surface,
                next_visibility_state,
                next_capability_base_filter,
                next_active_visibility_revision,
                tool_visibility_delta,
            } => {
                use crate::meerkat_machine::dsl as mm_dsl;
                let dsl_previous_identity =
                    mm_dsl::SessionLlmIdentity::from_domain(previous_identity.as_ref());
                let dsl_previous_visibility_state = mm_dsl::SessionToolVisibilityState::from_domain(
                    previous_visibility_state.as_ref(),
                );
                let dsl_previous_capability_surface = previous_capability_surface
                    .as_ref()
                    .map(mm_dsl::SessionLlmCapabilitySurface::from_domain);
                let dsl_previous_capability_surface_status =
                    mm_dsl::SessionLlmCapabilitySurfaceStatus::from_domain(
                        &previous_capability_surface_status,
                    );
                let dsl_target_identity =
                    mm_dsl::SessionLlmIdentity::from_domain(target_identity.as_ref());
                let dsl_target_capability_surface =
                    mm_dsl::SessionLlmCapabilitySurface::from_domain(&target_capability_surface);
                let dsl_next_visibility_state =
                    mm_dsl::SessionToolVisibilityState::from_domain(next_visibility_state.as_ref());
                let dsl_next_capability_base_filter =
                    mm_dsl::ToolFilter::from_domain(&next_capability_base_filter);
                let dsl_tool_visibility_delta =
                    mm_dsl::SessionToolVisibilityDelta::from_domain(tool_visibility_delta.as_ref());

                let report = self
                    .reconfigure_session_llm_identity_inner(
                        &session_id,
                        *previous_identity,
                        *previous_visibility_state,
                        previous_capability_surface,
                        previous_capability_surface_status,
                        *target_identity,
                        *target_capability_surface,
                        *next_visibility_state,
                        *tool_visibility_delta,
                    )
                    .await?;
                if let Some(entry) = self.sessions.write().await.get_mut(&session_id) {
                    // DSL shadow
                    use crate::meerkat_machine::dsl as mm_dsl;
                    if let Err(e) = mm_dsl::MeerkatMachineMutator::apply(
                        &mut entry.dsl_authority,
                        mm_dsl::MeerkatMachineInput::ReconfigureSessionLlmIdentity {
                            previous_identity: dsl_previous_identity,
                            previous_visibility_state: dsl_previous_visibility_state,
                            previous_capability_surface: dsl_previous_capability_surface,
                            previous_capability_surface_status:
                                dsl_previous_capability_surface_status,
                            target_identity: dsl_target_identity,
                            target_capability_surface: dsl_target_capability_surface,
                            next_visibility_state: dsl_next_visibility_state,
                            next_capability_base_filter: dsl_next_capability_base_filter,
                            next_active_visibility_revision,
                            tool_visibility_delta: dsl_tool_visibility_delta,
                        },
                    ) {
                        tracing::error!(
                            error = %e,
                            "DSL/runtime DISAGREEMENT on ReconfigureSessionLlmIdentity"
                        );
                    }
                }
                Ok(MeerkatMachineCommandResult::LlmReconfigured(report))
            }
            MeerkatMachineCommand::StagePersistentFilter {
                session_id,
                filter,
                witnesses,
            } => {
                if !self.sessions.read().await.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                let owner = {
                    let sessions = self.sessions.read().await;
                    Arc::clone(
                        &sessions
                            .get(&session_id)
                            .ok_or(RuntimeDriverError::NotReady {
                                state: RuntimeState::Destroyed,
                            })?
                            .tool_visibility_owner,
                    )
                };
                let revision = owner
                    .stage_persistent_filter(filter, witnesses)
                    .map_err(|err| RuntimeDriverError::Internal(err.to_string()))?;
                Ok(MeerkatMachineCommandResult::VisibilityRevision(revision))
            }
            MeerkatMachineCommand::RequestDeferredTools {
                session_id,
                names,
                witnesses,
            } => {
                if !self.sessions.read().await.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                let owner = {
                    let sessions = self.sessions.read().await;
                    Arc::clone(
                        &sessions
                            .get(&session_id)
                            .ok_or(RuntimeDriverError::NotReady {
                                state: RuntimeState::Destroyed,
                            })?
                            .tool_visibility_owner,
                    )
                };
                let revision = owner
                    .request_deferred_tools(names, witnesses)
                    .map_err(|err| RuntimeDriverError::Internal(err.to_string()))?;
                Ok(MeerkatMachineCommandResult::VisibilityRevision(revision))
            }
            MeerkatMachineCommand::PublishCommittedVisibleSet {
                session_id,
                visibility_state,
            } => {
                // Guard: session must exist — publishing to an unknown session
                // has no target.
                let sessions = self.sessions.read().await;
                if !sessions.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                drop(sessions);

                // Guard: DestroyedShapeInvariant — no mutation on destroyed sessions.
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }

                // Guard: VisibleSurfacesMatchAppliedStateInvariant —
                // the committed (active) revision must not lag behind the staged
                // revision. A lagging active revision means the visible set has
                // not caught up with staged mutations, violating the TLA+
                // invariant that visible_surfaces == {s : base_state[s] # None}.
                if visibility_state.active_revision < visibility_state.staged_revision {
                    return Err(RuntimeDriverError::ValidationFailed {
                        reason: format!(
                            "VisibleSurfacesMatchAppliedStateInvariant violated: \
                             active_revision ({}) < staged_revision ({})",
                            visibility_state.active_revision, visibility_state.staged_revision,
                        ),
                    });
                }

                if visibility_state.active_revision == visibility_state.staged_revision
                    && (visibility_state.active_filter != visibility_state.staged_filter
                        || visibility_state.active_requested_deferred_names
                            != visibility_state.staged_requested_deferred_names)
                {
                    return Err(RuntimeDriverError::ValidationFailed {
                        reason: "VisibleSurfacesMatchAppliedStateInvariant violated: equal revisions require equal active and staged visibility state".to_string(),
                    });
                }

                if !visibility_state
                    .active_requested_deferred_names
                    .is_subset(&visibility_state.staged_requested_deferred_names)
                {
                    return Err(RuntimeDriverError::ValidationFailed {
                        reason: "VisibleSurfacesMatchAppliedStateInvariant violated: active requested deferred names must remain a subset of staged requested deferred names".to_string(),
                    });
                }

                {
                    let sessions = self.sessions.read().await;
                    let owner = Arc::clone(
                        &sessions
                            .get(&session_id)
                            .ok_or(RuntimeDriverError::NotReady {
                                state: RuntimeState::Destroyed,
                            })?
                            .tool_visibility_owner,
                    );
                    owner
                        .replace_visibility_state(*visibility_state.clone())
                        .map_err(|err| RuntimeDriverError::Internal(err.to_string()))?;
                }

                Ok(MeerkatMachineCommandResult::VisibilityPublished(
                    *visibility_state,
                ))
            }
            _ => unreachable!("non-session command routed to session handler"),
        }
    }
}
