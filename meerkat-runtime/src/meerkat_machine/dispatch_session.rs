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
                let _ = self
                    .stage_session_dsl_input(
                        &sid,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
                            session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&sid),
                        },
                        "RegisterSession",
                    )
                    .await
                    .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::UnregisterSession { session_id } => {
                // Guard: session must exist before it can be unregistered.
                if !self.sessions.read().await.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                let _ = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::UnregisterSession {
                            session_id: crate::meerkat_machine::dsl::SessionId::from_domain(
                                &session_id,
                            ),
                        },
                        "UnregisterSession",
                    )
                    .await
                    .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                self.unregister_session_inner(&session_id).await;
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

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                let previous_dsl_state = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::SetSilentIntents {
                            session_id: crate::meerkat_machine::dsl::SessionId::from_domain(
                                &session_id,
                            ),
                            intents: intents.clone().into_iter().collect(),
                        },
                        "SetSilentIntents",
                    )
                    .await
                    .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                self.set_session_silent_intents_inner(&session_id, intents)
                    .await;
                // set_session_silent_intents_inner is infallible — no rollback needed.
                let _ = previous_dsl_state;
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

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                let previous_dsl_state = match self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::InterruptCurrentRun,
                        "InterruptCurrentRun",
                    )
                    .await
                {
                    Ok(state) => state,
                    Err(_) => {
                        let state = self
                            .existing_session_runtime_state(&session_id)
                            .await
                            .unwrap_or(RuntimeState::Destroyed);
                        return Err(RuntimeDriverError::NotReady { state });
                    }
                };
                if let Err(err) = self.interrupt_current_run_inner(&session_id).await {
                    self.restore_session_dsl_state(&session_id, previous_dsl_state)
                        .await;
                    return Err(err);
                }
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::CancelAfterBoundary { session_id } => {
                // Guard: DestroyedShapeInvariant — no mutation on destroyed sessions.
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                let previous_dsl_state = match self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::CancelAfterBoundary,
                        "CancelAfterBoundary",
                    )
                    .await
                {
                    Ok(state) => state,
                    Err(_) => {
                        let state = self
                            .existing_session_runtime_state(&session_id)
                            .await
                            .unwrap_or(RuntimeState::Destroyed);
                        return Err(RuntimeDriverError::NotReady { state });
                    }
                };
                if let Err(err) = self.cancel_after_boundary_inner(&session_id).await {
                    self.restore_session_dsl_state(&session_id, previous_dsl_state)
                        .await;
                    return Err(err);
                }
                Ok(MeerkatMachineCommandResult::Unit)
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

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                let previous_dsl_state = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::StopRuntimeExecutor,
                        "StopRuntimeExecutor",
                    )
                    .await
                    .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                if let Err(err) = self.stop_runtime_executor_inner(&session_id, command).await {
                    self.restore_session_dsl_state(&session_id, previous_dsl_state)
                        .await;
                    return Err(err);
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
                let (
                    driver_handle,
                    epoch_id,
                    ops_lifecycle,
                    cursor_state,
                    tool_visibility_owner,
                    dsl_authority_shared,
                ) = {
                    let sessions = self.sessions.read().await;
                    let entry = sessions
                        .get(&session_id)
                        .ok_or(RuntimeDriverError::Internal(format!(
                            "session {session_id} missing after register_session_inner"
                        )))?;
                    (
                        Arc::clone(&entry.driver),
                        entry.epoch_id.clone(),
                        Arc::clone(&entry.ops_lifecycle),
                        Arc::clone(&entry.cursor_state),
                        Arc::clone(&entry.tool_visibility_owner),
                        Arc::clone(&entry.dsl_authority),
                    )
                };
                let mut driver = driver_handle.lock().await;
                let dsl_input = crate::meerkat_machine::dsl::MeerkatMachineInput::PrepareBindings {
                    agent_runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(
                        driver.runtime_id(),
                    ),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from(0),
                    generation: crate::meerkat_machine::dsl::Generation::from(0),
                };
                machine_prepare_bindings_projection(&mut driver)?;
                drop(driver);
                let _ = self
                    .stage_session_dsl_input(&session_id, dsl_input, "PrepareBindings")
                    .await
                    .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                // Share ONE HandleDslAuthority across all 5 handles so their
                // transitions land on the session's real DSL state (same Arc
                // as RuntimeSessionEntry.dsl_authority). Phase 5F/1-5 callsites
                // rely on this — parallel private authorities would silently
                // diverge from the session DSL.
                let shared_handle_authority = Arc::new(
                    crate::handles::HandleDslAuthority::from_shared(dsl_authority_shared),
                );
                Ok(MeerkatMachineCommandResult::Bindings(
                    meerkat_core::SessionRuntimeBindings {
                        session_id,
                        epoch_id,
                        ops_lifecycle: ops_lifecycle as Arc<dyn meerkat_core::OpsLifecycleRegistry>,
                        cursor_state,
                        tool_visibility_owner: tool_visibility_owner
                            as Arc<dyn meerkat_core::ToolVisibilityOwner>,
                        turn_state: Arc::new(crate::handles::RuntimeTurnStateHandle::new(
                            Arc::clone(&shared_handle_authority),
                        )),
                        comms_drain: Arc::new(crate::handles::RuntimeCommsDrainHandle::new(
                            Arc::clone(&shared_handle_authority),
                        )),
                        external_tool_surface: Arc::new(
                            crate::handles::RuntimeExternalToolSurfaceHandle::new(Arc::clone(
                                &shared_handle_authority,
                            )),
                        ),
                        peer_comms: Arc::new(crate::handles::RuntimePeerCommsHandle::new(
                            Arc::clone(&shared_handle_authority),
                        )),
                        session_admission: Arc::new(
                            crate::handles::RuntimeSessionAdmissionHandle::new(Arc::clone(
                                &shared_handle_authority,
                            )),
                        ),
                        auth_lease: Arc::new(crate::handles::RuntimeAuthLeaseHandle::new()),
                        mcp_server_lifecycle: Arc::new(
                            crate::handles::RuntimeMcpServerLifecycleHandle::new(Arc::clone(
                                &shared_handle_authority,
                            )),
                        ),
                        peer_interaction: Some(Arc::new(
                            crate::handles::RuntimePeerInteractionHandle::new(Arc::clone(
                                &shared_handle_authority,
                            )),
                        )),
                        session_context: Arc::new(
                            crate::handles::RuntimeSessionContextHandle::new(Arc::clone(
                                &shared_handle_authority,
                            )),
                        ),
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
                    driver.as_driver().stored_input_state(&input_id),
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
                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

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

                let previous_dsl_state = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::ReconfigureSessionLlmIdentity {
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
                        "ReconfigureSessionLlmIdentity",
                    )
                    .await
                    .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                let report = match self
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
                    .await
                {
                    Ok(report) => report,
                    Err(err) => {
                        self.restore_session_dsl_state(&session_id, previous_dsl_state)
                            .await;
                        return Err(err);
                    }
                };
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

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

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
                let filter_str = serde_json::to_string(&filter).unwrap_or_default();
                // DSL-first: stage visibility filter before mutation
                let previous_dsl_state = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::StageVisibilityFilter {
                            filter: filter_str,
                            // Use a placeholder revision — the real revision comes from the
                            // owner after mutation. DSL validates the transition shape, not
                            // the revision value.
                            revision: 0,
                        },
                        "StageVisibilityFilter",
                    )
                    .await
                    .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                let revision = match owner.stage_persistent_filter(filter, witnesses) {
                    Ok(rev) => rev,
                    Err(err) => {
                        self.restore_session_dsl_state(&session_id, previous_dsl_state)
                            .await;
                        return Err(RuntimeDriverError::Internal(err.to_string()));
                    }
                };
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

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

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
                // Read current accumulated deferred names, union with new names
                // (request_deferred_tools extends, DSL replaces — pass the full set)
                let current_names = owner
                    .visibility_state()
                    .map(|s| s.staged_requested_deferred_names)
                    .map_err(|err| RuntimeDriverError::Internal(err.to_string()))?;
                let accumulated_names: BTreeSet<String> =
                    current_names.union(&names).cloned().collect();
                // DSL-first: stage deferred names before mutation
                let previous_dsl_state = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::StageDeferredNames {
                            names: accumulated_names,
                        },
                        "StageDeferredNames",
                    )
                    .await
                    .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                let revision = match owner.request_deferred_tools(names, witnesses) {
                    Ok(rev) => rev,
                    Err(err) => {
                        self.restore_session_dsl_state(&session_id, previous_dsl_state)
                            .await;
                        return Err(RuntimeDriverError::Internal(err.to_string()));
                    }
                };
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

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                // DSL-first: stage CommitVisibilityFilter + CommitDeferredNames before mutation
                let previous_dsl_state = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::CommitVisibilityFilter {
                            filter: serde_json::to_string(&visibility_state.active_filter)
                                .unwrap_or_default(),
                            revision: visibility_state.active_revision,
                        },
                        "CommitVisibilityFilter",
                    )
                    .await
                    .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                if let Err(reason) = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::CommitDeferredNames {
                            names: visibility_state.active_requested_deferred_names.clone(),
                        },
                        "CommitDeferredNames",
                    )
                    .await
                {
                    self.restore_session_dsl_state(&session_id, previous_dsl_state)
                        .await;
                    return Err(RuntimeDriverError::ValidationFailed { reason });
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
                    if let Err(err) = owner.replace_visibility_state(*visibility_state.clone()) {
                        self.restore_session_dsl_state(&session_id, previous_dsl_state)
                            .await;
                        return Err(RuntimeDriverError::Internal(err.to_string()));
                    }
                }

                Ok(MeerkatMachineCommandResult::VisibilityPublished(
                    *visibility_state,
                ))
            }
            _ => unreachable!("non-session command routed to session handler"),
        }
    }

    /// Arc-requiring session dispatch: handles commands that spawn background
    /// tasks holding `Weak<Self>` for async DSL transitions (currently
    /// `EnsureSessionWithExecutor` for runtime-loop `RuntimeExecutorExited`
    /// wiring).
    pub(super) async fn execute_meerkat_machine_ensure_session_command(
        self: &Arc<Self>,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        match command {
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
                // `inner` creates the session entry (if new), stages the DSL
                // EnsureSessionWithExecutor transition BEFORE mutating the
                // driver, attaches the executor, and spawns the runtime loop
                // with `Weak<Self>` so the loop can fire
                // `RuntimeExecutorExited` on async stop completion.
                self.ensure_session_with_executor_inner(session_id, executor)
                    .await;
                Ok(MeerkatMachineCommandResult::Unit)
            }
            _ => unreachable!("non-ensure-session command routed to arc session handler"),
        }
    }
}
