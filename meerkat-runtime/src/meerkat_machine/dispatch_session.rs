use super::*;

#[path = "../user_interrupt.rs"]
mod user_interrupt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionBindingPreparation {
    /// The runtime binding itself is the semantic event: apply
    /// `PrepareBindings` to MeerkatMachine and publish any routed seam signals.
    AuthoritativeRuntimeBinding,
    /// Only create the session-local handle bundle. A separate owner will
    /// route the authoritative binding later.
    LocalSessionResources,
}

fn visibility_authorities_for_names(
    names: &std::collections::BTreeSet<String>,
    witnesses: &std::collections::BTreeMap<String, meerkat_core::ToolVisibilityWitness>,
) -> std::collections::BTreeMap<String, crate::meerkat_machine::dsl::ToolVisibilityWitness> {
    names
        .iter()
        .filter_map(|name| {
            witnesses.get(name).map(|witness| {
                (
                    name.clone(),
                    crate::meerkat_machine::dsl::ToolVisibilityWitness::from(witness),
                )
            })
        })
        .collect()
}

impl MeerkatMachine {
    async fn dispatch_user_interrupt(
        &self,
        session_id: &SessionId,
        reason: String,
    ) -> Result<(), RuntimeDriverError> {
        // Guard: DestroyedShapeInvariant — no mutation on destroyed sessions.
        if matches!(
            self.existing_session_runtime_state(session_id).await,
            Some(RuntimeState::Destroyed)
        ) {
            return Err(RuntimeDriverError::Destroyed);
        }

        let gate = self.session_mutation_gate(session_id).await;
        let _gate_guard = match gate {
            Some(ref g) => Some(g.lock().await),
            None => None,
        };

        let previous_dsl_state = match self
            .stage_session_runtime_internal_dsl_input(
                session_id,
                crate::meerkat_machine_types::MeerkatMachineFieldlessRuntimeInternalInput::InterruptCurrentRun,
            )
            .await
        {
            Ok(state) => state,
            Err(_) => {
                let state = self
                    .existing_session_runtime_state(session_id)
                    .await
                    .unwrap_or(RuntimeState::Destroyed);
                return Err(RuntimeDriverError::NotReady { state });
            }
        };

        if let Err(err) = self
            .apply_user_interrupt_live_cancel(session_id, reason)
            .await
        {
            self.restore_session_dsl_state(session_id, previous_dsl_state)
                .await;
            return Err(err);
        }

        Ok(())
    }

    async fn prepare_session_runtime_bindings(
        &self,
        session_id: SessionId,
        preparation: SessionBindingPreparation,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
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
        if preparation == SessionBindingPreparation::AuthoritativeRuntimeBinding {
            let runtime_id = {
                let driver = driver_handle.lock().await;
                driver.runtime_id().clone()
            };
            let dsl_input = crate::meerkat_machine::dsl::MeerkatMachineInput::PrepareBindings {
                agent_runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(
                    &runtime_id,
                ),
                fence_token: crate::meerkat_machine::dsl::FenceToken::from(0),
                generation: crate::meerkat_machine::dsl::Generation::from(0),
                session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&session_id),
            };
            let staged = self
                .stage_session_dsl_transition(&session_id, dsl_input, "PrepareBindings")
                .await
                .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
            {
                let mut driver = driver_handle.lock().await;
                if let Err(err) = machine_prepare_bindings_projection(&mut driver) {
                    drop(driver);
                    self.restore_session_dsl_state(&session_id, staged.previous_state)
                        .await;
                    return Err(err);
                }
            }
            if let Err(reason) = self
                .commit_session_dsl_transition(&session_id, staged, "PrepareBindings")
                .await
            {
                driver_handle
                    .lock()
                    .await
                    .sync_control_projection_from_dsl_authority();
                return Err(RuntimeDriverError::Internal(reason));
            }
        }
        // Share ONE HandleDslAuthority across all 5 handles so their
        // transitions land on the session's real DSL state (same Arc
        // as RuntimeSessionEntry.dsl_authority). Phase 5F/1-5 callsites
        // rely on this — parallel private authorities would silently
        // diverge from the session DSL.
        let shared_handle_authority = Arc::new(crate::handles::HandleDslAuthority::from_shared(
            dsl_authority_shared,
        ));
        let auth_lease = self.auth_lease_handle();
        Ok(MeerkatMachineCommandResult::Bindings(
            meerkat_core::SessionRuntimeBindings {
                session_id,
                epoch_id,
                ops_lifecycle: ops_lifecycle as Arc<dyn meerkat_core::OpsLifecycleRegistry>,
                cursor_state,
                tool_visibility_owner: tool_visibility_owner
                    as Arc<dyn meerkat_core::ToolVisibilityOwner>,
                turn_state: Arc::new(crate::handles::RuntimeTurnStateHandle::new(Arc::clone(
                    &shared_handle_authority,
                ))),
                comms_drain: Arc::new(crate::handles::RuntimeCommsDrainHandle::new(Arc::clone(
                    &shared_handle_authority,
                ))),
                external_tool_surface: Arc::new(
                    crate::handles::RuntimeExternalToolSurfaceHandle::new(Arc::clone(
                        &shared_handle_authority,
                    )),
                ),
                peer_comms: Arc::new(crate::handles::RuntimePeerCommsHandle::new(Arc::clone(
                    &shared_handle_authority,
                ))),
                session_admission: Arc::new(crate::handles::RuntimeSessionAdmissionHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                model_routing: Arc::new(crate::handles::RuntimeModelRoutingHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                auth_lease,
                mcp_server_lifecycle: Arc::new(
                    crate::handles::RuntimeMcpServerLifecycleHandle::new(Arc::clone(
                        &shared_handle_authority,
                    )),
                ),
                peer_interaction: Arc::new(crate::handles::RuntimePeerInteractionHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                session_context: Arc::new(crate::handles::RuntimeSessionContextHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                session_claim_handle: self.session_claim_handle(),
                interaction_stream: Arc::new(crate::handles::RuntimeInteractionStreamHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                realtime_product_turn: Arc::new(
                    crate::handles::RuntimeRealtimeProductTurnHandle::new(Arc::clone(
                        &shared_handle_authority,
                    )),
                ),
            },
        ))
    }

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
                if previous_dsl_state.lifecycle_phase
                    != crate::meerkat_machine::dsl::MeerkatPhase::Stopped
                {
                    self.set_session_silent_intents_inner(&session_id, intents)
                        .await;
                }
                // set_session_silent_intents_inner is infallible — no rollback needed.
                let _ = previous_dsl_state;
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

                self.cancel_after_boundary_inner(&session_id).await?;
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::StopRuntimeExecutor { session_id, reason } => {
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

                self.stop_runtime_executor_inner(&session_id, reason)
                    .await?;
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
                // Wave-c C-H2: pre-collapse, "slot present in sibling
                // map" was true iff `spawn_comms_drain_if_needed` (or the
                // test setup helper) had actually inserted a slot for
                // this session. Post-collapse every registered session
                // carries a `CommsDrainSlot` in its `drain_slot` field,
                // initialised `phase = Inactive` with no bound runtime.
                // Preserve the pre-collapse predicate by checking "drain
                // has ever been engaged" (phase != Inactive or a comms
                // runtime is bound), which is the observable meaning
                // callers depend on.
                let sessions = self.sessions.read().await;
                let engaged = sessions.get(&session_id).is_some_and(|entry| {
                    entry.drain_slot.phase != crate::meerkat_machine::CommsDrainPhase::Inactive
                        || entry.drain_slot.bound_runtime.is_some()
                });
                Ok(MeerkatMachineCommandResult::Bool(engaged))
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
                self.prepare_session_runtime_bindings(
                    session_id,
                    SessionBindingPreparation::AuthoritativeRuntimeBinding,
                )
                .await
            }
            MeerkatMachineCommand::PrepareLocalSessionBindings { session_id } => {
                self.prepare_session_runtime_bindings(
                    session_id,
                    SessionBindingPreparation::LocalSessionResources,
                )
                .await
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
                // Delegate to the owner — the `MachineToolVisibilityOwner`
                // trait impl fires the `StageVisibilityFilter` DSL input
                // internally (dogma round 4, wave 2b #12: DSL owns the
                // `next_staged_visibility_revision` monotonic). The DSL
                // input's `update {}` increments and stamps the revision
                // under the authority lock; the owner reads the minted
                // value back and projects it onto its own state.
                let revision = owner
                    .stage_persistent_filter(filter, witnesses)
                    .map_err(|err| RuntimeDriverError::Internal(err.to_string()))?;
                Ok(MeerkatMachineCommandResult::VisibilityRevision(revision))
            }
            MeerkatMachineCommand::RequestDeferredTools {
                session_id,
                authorities,
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
                // Delegate to the owner — `request_deferred_tools` fires
                // the authority-bearing `RequestDeferredTools` DSL input (with
                // the extended authority set) to mint the revision and then
                // projects onto owner state.
                let revision = owner
                    .request_deferred_tools(authorities)
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

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                // Deferred-tool authority is owned by the visibility
                // catalog, so publish must validate caller-supplied state
                // before projecting names/witnesses into the DSL state.
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
                let deferred_authorities = owner
                    .canonical_deferred_authorities_for_visibility_state(&visibility_state)
                    .map_err(|err| RuntimeDriverError::ValidationFailed {
                        reason: err.to_string(),
                    })?;

                // DSL-first: fire the canonical typed `PublishCommittedVisibleSet`
                // input. The per-phase transitions at `dsl::PublishCommittedVisibleSet*`
                // own the `VisibleSurfacesMatchAppliedStateInvariant`:
                //
                //   * `active_not_behind_staged`
                //   * `equal_revision_requires_equal_active_and_staged_input`
                //   * `active_requested_subset_of_staged_requested`
                //
                // Guard rejections surface as `RuntimeDriverError::ValidationFailed`
                // via `stage_session_dsl_input`, so the hand-written shell
                // pre-checks that previously duplicated these invariants have
                // been deleted — the DSL guard is the single source of truth.
                let previous_dsl_state = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::PublishCommittedVisibleSet {
                            active_filter: crate::meerkat_machine::dsl::ToolFilter::from(
                                &visibility_state.active_filter,
                            ),
                            staged_filter: crate::meerkat_machine::dsl::ToolFilter::from(
                                &visibility_state.staged_filter,
                            ),
                            active_requested_deferred_names: visibility_state
                                .active_requested_deferred_names
                                .clone(),
                            staged_requested_deferred_names: visibility_state
                                .staged_requested_deferred_names
                                .clone(),
                            active_deferred_authorities: visibility_authorities_for_names(
                                &visibility_state.active_requested_deferred_names,
                                &deferred_authorities,
                            ),
                            staged_deferred_authorities: visibility_authorities_for_names(
                                &visibility_state.staged_requested_deferred_names,
                                &deferred_authorities,
                            ),
                            active_visibility_revision: visibility_state.active_revision,
                            staged_visibility_revision: visibility_state.staged_revision,
                        },
                        "PublishCommittedVisibleSet",
                    )
                    .await
                    .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

                if let Err(err) = owner.replace_visibility_state(*visibility_state.clone()) {
                    self.restore_session_dsl_state(&session_id, previous_dsl_state)
                        .await;
                    return Err(RuntimeDriverError::Internal(err.to_string()));
                }

                Ok(MeerkatMachineCommandResult::VisibilityPublished(
                    *visibility_state,
                ))
            }
            _ => unreachable!("non-session command routed to session handler"),
        }
    }

    /// Arc-requiring session dispatch: handles commands that spawn runtime-owned
    /// background tasks.
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
                // driver, attaches the executor, and spawns the runtime loop.
                self.ensure_session_with_executor_inner(session_id, executor)
                    .await;
                Ok(MeerkatMachineCommandResult::Unit)
            }
            _ => unreachable!("non-ensure-session command routed to arc session handler"),
        }
    }
}
