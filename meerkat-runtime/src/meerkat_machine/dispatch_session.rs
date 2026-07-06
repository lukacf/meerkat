use super::*;
use meerkat_core::ToolName;

#[path = "../user_interrupt.rs"]
mod user_interrupt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionBindingPreparation {
    /// The runtime binding itself is the semantic event: apply
    /// `PrepareBindings` to MeerkatMachine and publish any routed seam signals.
    AuthoritativeRuntimeBinding,
    /// Only create the session-local handle bundle. A separate owner will
    /// route the authoritative binding later.
    LocalSessionResources,
}

fn visibility_authorities_for_names(
    names: &std::collections::BTreeSet<ToolName>,
    witnesses: &std::collections::BTreeMap<ToolName, meerkat_core::ToolVisibilityWitness>,
) -> std::collections::BTreeMap<ToolName, crate::meerkat_machine::dsl::ToolVisibilityWitness> {
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
        let gate = self.session_mutation_gate(session_id).await;
        let _gate_guard = match gate {
            Some(g) => match Arc::clone(&g).try_lock_owned() {
                Ok(guard) => Some(guard),
                Err(_) if self.generated_stop_deferred(session_id).await => None,
                Err(_) => Some(g.lock_owned().await),
            },
            None => None,
        };

        let staged_interrupt = match self
            .stage_session_runtime_internal_dsl_transition(
                session_id,
                crate::meerkat_machine_types::MeerkatMachineFieldlessRuntimeInternalInput::InterruptCurrentRun,
            )
            .await
        {
            Ok(state) => state,
            Err(_) => {
                // The generated machine rejected `InterruptCurrentRun` for the
                // current phase. Surface the terminal `Destroyed` truth as its
                // own typed variant (DestroyedShapeInvariant) so callers that
                // distinguish a destroyed binding from a merely not-ready one
                // still observe it; every other rejected phase is `NotReady`.
                let state = self
                    .existing_session_runtime_state(session_id)
                    .await
                    .unwrap_or(RuntimeState::Destroyed);
                if state == RuntimeState::Destroyed {
                    return Err(RuntimeDriverError::Destroyed);
                }
                return Err(RuntimeDriverError::NotReady { state });
            }
        };

        if let Err(err) = self
            .apply_user_interrupt_live_cancel(session_id, reason)
            .await
        {
            self.restore_session_dsl_state_if_current(
                session_id,
                staged_interrupt.committed_snapshot,
                staged_interrupt.previous_snapshot,
            )
            .await;
            return Err(err);
        }

        Ok(())
    }

    /// Classify a generated-machine rejection of a session lifecycle input.
    ///
    /// The machine already made the legality decision (stage-first shape, same
    /// as `dispatch_user_interrupt`); this only maps the rejection onto the
    /// typed wire error: a `Destroyed` binding surfaces as the terminal
    /// [`RuntimeDriverError::Destroyed`], every other rejection keeps its
    /// reason as `ValidationFailed`. Reading the runtime state here is a
    /// post-verdict projection read for classification, never a guard.
    pub(super) async fn classify_session_dsl_rejection(
        &self,
        session_id: &SessionId,
        reason: String,
    ) -> RuntimeDriverError {
        if matches!(
            self.existing_session_runtime_state(session_id).await,
            Some(RuntimeState::Destroyed)
        ) {
            return RuntimeDriverError::Destroyed;
        }
        RuntimeDriverError::ValidationFailed { reason }
    }

    /// Same stage-first classification for lifecycle errors that already carry
    /// a typed [`RuntimeDriverError`]: a rejection observed on a `Destroyed`
    /// binding is surfaced as the terminal `Destroyed` truth; everything else
    /// propagates unchanged.
    pub(super) async fn classify_session_driver_rejection(
        &self,
        session_id: &SessionId,
        err: RuntimeDriverError,
    ) -> RuntimeDriverError {
        if matches!(
            self.existing_session_runtime_state(session_id).await,
            Some(RuntimeState::Destroyed)
        ) {
            return RuntimeDriverError::Destroyed;
        }
        err
    }

    async fn generated_stop_deferred(&self, session_id: &SessionId) -> bool {
        let Ok(authority) = self.session_dsl_authority(session_id).await else {
            return false;
        };
        authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .state()
            .runtime_stop_deferred
    }

    pub(super) async fn prepare_session_runtime_bindings(
        &self,
        session_id: SessionId,
        preparation: SessionBindingPreparation,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        tracing::debug!(
            %session_id,
            ?preparation,
            "MeerkatMachine::prepare_session_runtime_bindings start"
        );
        tracing::debug!(
            %session_id,
            ?preparation,
            "MeerkatMachine::prepare_session_runtime_bindings registering session"
        );
        #[cfg(target_arch = "wasm32")]
        let inserted_by_call = if self.store.is_none() {
            {
                tracing::debug!(%session_id, "MeerkatMachine::prepare_session_runtime_bindings attempting storeless existing check lock");
                let mut sessions = self.sessions.try_write().map_err(|_| {
                    tracing::warn!(
                        %session_id,
                        "storeless session map busy while checking existing registration"
                    );
                    RuntimeDriverError::Internal(format!(
                        "storeless session map busy while registering {session_id}"
                    ))
                })?;
                tracing::debug!(%session_id, "MeerkatMachine::prepare_session_runtime_bindings locked storeless existing check");
                if let Some(existing) = sessions.get_mut(&session_id) {
                    tracing::debug!(
                        %session_id,
                        "MeerkatMachine::prepare_session_runtime_bindings found existing session"
                    );
                    if existing.clear_dead_attachment() {
                        existing.stage_generated_executor_exit_observation().map_err(|reason| {
                            RuntimeDriverError::Internal(format!(
                                "generated MeerkatMachine rejected executor-exit observation: {reason}"
                            ))
                        })?;
                    }
                    false
                } else {
                    drop(sessions);
                    self.register_storeless_session_inner_sync_build_step(session_id.clone())?
                }
            }
        } else {
            Box::pin(self.register_session_inner(session_id.clone())).await?
        };
        #[cfg(not(target_arch = "wasm32"))]
        let inserted_by_call = Box::pin(self.register_session_inner(session_id.clone())).await?;
        tracing::debug!(
            %session_id,
            inserted_by_call,
            ?preparation,
            "MeerkatMachine::prepare_session_runtime_bindings registered session"
        );
        let (
            driver_handle,
            epoch_id,
            ops_lifecycle,
            cursor_state,
            tool_visibility_owner,
            dsl_authority_shared,
            handle_teardown_gate,
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
                Arc::clone(&entry.handle_teardown_gate),
            )
        };
        let dsl_session_id = crate::meerkat_machine::dsl::SessionId::from_domain(&session_id);
        // Stage RegisterSession unconditionally: the generated machine owns
        // both the idempotence verdict (`RegisterSessionIdempotent` no-ops a
        // same-binding re-registration) and the Destroyed rejection
        // (RegisterSession is not declared from Destroyed). No shell probe of
        // the authority state precedes the staging.
        if let Err(reason) = self
            .stage_session_dsl_input(
                &session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
                    session_id: dsl_session_id,
                },
                "RegisterSession",
            )
            .await
        {
            let err = self
                .classify_session_dsl_rejection(&session_id, reason)
                .await;
            if inserted_by_call {
                self.unregister_session_inner_if_epoch(&session_id, &epoch_id)
                    .await;
            }
            return Err(err);
        }
        tracing::debug!(
            %session_id,
            ?preparation,
            "MeerkatMachine::prepare_session_runtime_bindings prepared generated registration"
        );
        if preparation == SessionBindingPreparation::AuthoritativeRuntimeBinding {
            let runtime_id = {
                tracing::debug!(
                    %session_id,
                    ?preparation,
                    "MeerkatMachine::prepare_session_runtime_bindings locking driver for runtime id"
                );
                let driver = driver_handle.lock().await;
                driver.runtime_id().clone()
            };
            tracing::debug!(
                %session_id,
                ?preparation,
                "MeerkatMachine::prepare_session_runtime_bindings locked driver for runtime id"
            );
            let agent_runtime_id =
                crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(&runtime_id);
            let fence_token = crate::meerkat_machine::dsl::FenceToken::from(0);
            let runtime_epoch_id =
                crate::meerkat_machine::dsl::RuntimeEpochId::from_domain(&epoch_id);
            let dsl_input = crate::meerkat_machine::dsl::MeerkatMachineInput::PrepareBindings {
                agent_runtime_id,
                fence_token,
                generation: None,
                runtime_epoch_id: Some(runtime_epoch_id),
                session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&session_id),
            };
            let staged = match self
                .stage_session_dsl_transition(&session_id, dsl_input, "PrepareBindings")
                .await
            {
                Ok(staged) => staged,
                Err(reason) => {
                    if inserted_by_call {
                        self.unregister_session_inner_if_epoch(&session_id, &epoch_id)
                            .await;
                    }
                    return Err(RuntimeDriverError::ValidationFailed { reason });
                }
            };
            {
                tracing::debug!(
                    %session_id,
                    ?preparation,
                    "MeerkatMachine::prepare_session_runtime_bindings locking driver for authoritative projection"
                );
                let mut driver = driver_handle.lock().await;
                machine_prepare_bindings_projection(&mut driver);
            }
            tracing::debug!(
                %session_id,
                ?preparation,
                "MeerkatMachine::prepare_session_runtime_bindings applied authoritative projection"
            );
            if let Err(reason) = self
                .commit_session_dsl_transition(&session_id, staged, "PrepareBindings")
                .await
            {
                driver_handle
                    .lock()
                    .await
                    .sync_control_projection_from_dsl_authority();
                if inserted_by_call {
                    self.unregister_session_inner_if_epoch(&session_id, &epoch_id)
                        .await;
                }
                return Err(RuntimeDriverError::Internal(reason));
            }
        } else {
            {
                tracing::debug!(
                    %session_id,
                    ?preparation,
                    "MeerkatMachine::prepare_session_runtime_bindings locking driver for local projection"
                );
                let mut driver = driver_handle.lock().await;
                machine_prepare_bindings_projection(&mut driver);
            }
            tracing::debug!(
                %session_id,
                ?preparation,
                "MeerkatMachine::prepare_session_runtime_bindings applied local projection"
            );
        }
        // Share ONE HandleDslAuthority across all 5 handles so their
        // transitions land on the session's real DSL state (same Arc
        // as RuntimeSessionEntry.dsl_authority). Phase 5F/1-5 callsites
        // rely on this — parallel private authorities would silently
        // diverge from the session DSL.
        let shared_handle_authority = Arc::new(
            crate::handles::HandleDslAuthority::from_shared_with_teardown_gate(
                dsl_authority_shared,
                handle_teardown_gate,
            ),
        );
        let auth_lease = self.generated_auth_lease_handle();
        let runtime_authority = match preparation {
            SessionBindingPreparation::AuthoritativeRuntimeBinding => {
                crate::session_runtime_bindings_authority()
            }
            SessionBindingPreparation::LocalSessionResources => {
                crate::local_session_runtime_bindings_authority()
            }
        };
        let peer_comms_install = crate::handles::RuntimePeerCommsHandle::generated_install_factory(
            Arc::clone(&shared_handle_authority),
        )
        .map_err(RuntimeDriverError::Internal)?;

        tracing::debug!(
            %session_id,
            ?preparation,
            "MeerkatMachine::prepare_session_runtime_bindings assembling bindings"
        );
        Ok(MeerkatMachineCommandResult::Bindings(
            meerkat_core::SessionRuntimeBindings::__from_runtime_authority(
                session_id,
                epoch_id,
                ops_lifecycle as Arc<dyn meerkat_core::OpsLifecycleRegistry>,
                cursor_state,
                generated_tool_visibility_owner(
                    tool_visibility_owner as Arc<dyn meerkat_core::ToolVisibilityOwner>,
                )
                .map_err(RuntimeDriverError::Internal)?,
                Arc::new(crate::handles::RuntimeTurnStateHandle::new(Arc::clone(
                    &shared_handle_authority,
                ))),
                Arc::new(crate::handles::RuntimeCommsDrainHandle::new(Arc::clone(
                    &shared_handle_authority,
                ))),
                Arc::new(crate::handles::RuntimeExternalToolSurfaceHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                peer_comms_install,
                Arc::new(crate::handles::RuntimeSessionAdmissionHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                Arc::new(crate::handles::RuntimeModelRoutingHandle::new(Arc::clone(
                    &shared_handle_authority,
                ))),
                auth_lease,
                Arc::new(crate::handles::RuntimeMcpServerLifecycleHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                Arc::new(crate::handles::RuntimePeerInteractionHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                Arc::new(crate::handles::RuntimeSessionContextHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                self.session_claim_handle(),
                Arc::new(crate::handles::RuntimeInteractionStreamHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                runtime_authority,
            ),
        ))
    }

    pub(super) async fn execute_meerkat_machine_session_command(
        &self,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        match command {
            MeerkatMachineCommand::RegisterSession { session_id } => {
                let sid = session_id.clone();
                self.register_session_inner(session_id).await?;
                // Stage-first: the generated machine owns the legality verdict.
                // RegisterSession is not declared from Destroyed (it is a
                // resurrection input the DestroyedShapeInvariant forbids), so a
                // resident OR cold-recovered Destroyed binding is rejected by
                // the machine and classified as the terminal `Destroyed` truth
                // — never silently skipped, never preflighted in the shell. A
                // same-binding re-registration is the machine-owned
                // `RegisterSessionIdempotent` no-op.
                if let Err(reason) = self
                    .stage_session_dsl_input(
                        &sid,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
                            session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&sid),
                        },
                        "RegisterSession",
                    )
                    .await
                {
                    return Err(self.classify_session_dsl_rejection(&sid, reason).await);
                }
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::UnregisterSession { session_id } => {
                let Some(gate_guard) = self.lock_current_session_mutation_gate(&session_id).await
                else {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                };
                self.unregister_session_inner_locked_authorized(&session_id, gate_guard)
                    .await?;
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::SetSilentIntents {
                session_id,
                intents,
            } => {
                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                // Stage-first: SetSilentIntents is not declared from Destroyed,
                // so the machine rejects it there and the rejection is
                // classified as the terminal `Destroyed` truth.
                if let Err(reason) = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::SetSilentIntents {
                            session_id: crate::meerkat_machine::dsl::SessionId::from_domain(
                                &session_id,
                            ),
                            intents: intents.into_iter().collect(),
                        },
                        "SetSilentIntents",
                    )
                    .await
                {
                    return Err(self
                        .classify_session_dsl_rejection(&session_id, reason)
                        .await);
                }
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::CancelAfterBoundary { session_id } => {
                // Stage-first: `cancel_after_boundary_inner` stages the
                // CancelAfterBoundary DSL input; the machine rejects it on a
                // Destroyed binding and the inner classification surfaces the
                // terminal `Destroyed` truth.
                self.cancel_after_boundary_inner(&session_id).await?;
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::StopRuntimeExecutor { session_id, reason } => {
                // Stage-first: `stop_runtime_executor_inner` stages the
                // StopRuntimeExecutor DSL input; the machine rejects it on a
                // Destroyed binding and the inner classification surfaces the
                // terminal `Destroyed` truth.
                self.stop_runtime_executor_inner(&session_id, reason)
                    .await?;
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::CommitServiceTurnTerminalReceipt { session_id } => {
                // Direct SessionService turns share the runtime turn-state
                // handle, but their durable commit occurs inside
                // `SessionService::start_turn`, not the runtime loop. After
                // that call returns successfully, close the run binding here
                // through the same machine-owned lifecycle authority.
                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };
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
                let receipt_result = {
                    let mut driver = driver.lock().await;
                    machine_commit_service_turn_terminal_receipt(&mut driver).await
                };
                // The driver-level receipt requires a Running machine-owned
                // lifecycle (it rejects every other phase, including
                // Destroyed); classify a rejection observed on a Destroyed
                // binding as the terminal `Destroyed` truth.
                if let Err(err) = receipt_result {
                    return Err(self
                        .classify_session_driver_rejection(&session_id, err)
                        .await);
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
                        .map(RuntimeSessionEntry::generated_executor_registration_active)
                        .unwrap_or(false),
                ))
            }
            MeerkatMachineCommand::SessionHasComms { session_id } => {
                let engaged = self
                    .drain_authority_state(&session_id)
                    .await
                    .is_some_and(|state| {
                        state.peer_owner_kind
                            != crate::meerkat_machine::dsl::PeerIngressOwnerKind::Unattached
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
                Box::pin(self.prepare_session_runtime_bindings(
                    session_id,
                    SessionBindingPreparation::AuthoritativeRuntimeBinding,
                ))
                .await
            }
            MeerkatMachineCommand::PrepareLocalSessionBindings { session_id } => {
                Box::pin(self.prepare_session_runtime_bindings(
                    session_id,
                    SessionBindingPreparation::LocalSessionResources,
                ))
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
            MeerkatMachineCommand::InputStateByIdempotencyKey {
                session_id,
                idempotency_key,
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
                let driver = driver.as_driver();
                Ok(MeerkatMachineCommandResult::InputState(
                    driver
                        .input_id_for_idempotency_key(&idempotency_key)
                        .and_then(|input_id| driver.stored_input_state(&input_id)),
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
                view_image_tool_available,
                previous_view_image_visible,
                next_view_image_visible,
                previous_active_visibility_revision,
                previous_staged_visibility_revision,
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
                let dsl_previous_capability_base_filter = mm_dsl::ToolFilter::from_domain(
                    &previous_visibility_state.capability_base_filter,
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

                let staged_dsl_input = self
                    .stage_session_dsl_transition(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::ReconfigureSessionLlmIdentity {
                            previous_identity: dsl_previous_identity,
                            previous_visibility_state: dsl_previous_visibility_state,
                            previous_capability_surface: dsl_previous_capability_surface,
                            previous_capability_surface_status:
                                dsl_previous_capability_surface_status,
                            previous_capability_base_filter: dsl_previous_capability_base_filter,
                            view_image_tool_available,
                            previous_view_image_visible,
                            next_view_image_visible,
                            previous_active_visibility_revision,
                            previous_staged_visibility_revision,
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
                let authority_plan =
                    Self::session_llm_reconfigure_authority_plan(&staged_dsl_input.effects)?;
                let report = match self
                    .reconfigure_session_llm_identity_inner(
                        &session_id,
                        *previous_identity,
                        *previous_visibility_state,
                        *target_identity,
                        *next_visibility_state,
                        authority_plan,
                    )
                    .await
                {
                    Ok(report) => report,
                    Err(err) => {
                        self.restore_session_dsl_state(
                            &session_id,
                            staged_dsl_input.previous_snapshot,
                        )
                        .await;
                        if err.clear_generated_llm_state {
                            self.stage_session_dsl_input(
                                &session_id,
                                crate::meerkat_machine::dsl::MeerkatMachineInput::ClearSessionLlmState,
                                "ClearSessionLlmState",
                            )
                            .await
                            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                        }
                        return Err(err.error);
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
                // Stage-first: the owner fires the StageVisibilityFilter DSL
                // input, which is not declared from Destroyed — classify a
                // rejection on a Destroyed binding as the terminal truth.
                let revision = match owner.stage_persistent_filter(filter, witnesses) {
                    Ok(revision) => revision,
                    Err(err) => {
                        return Err(self
                            .classify_session_driver_rejection(
                                &session_id,
                                RuntimeDriverError::Internal(err.to_string()),
                            )
                            .await);
                    }
                };
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
                // Delegate to the owner: `request_deferred_tools` applies one
                // generated authority-bearing batch input and then mirrors the
                // accepted machine state into the owner projection. Stage-first:
                // the input is not declared from Destroyed — classify a
                // rejection on a Destroyed binding as the terminal truth.
                let revision = match owner.request_deferred_tools(authorities) {
                    Ok(revision) => revision,
                    Err(err) => {
                        return Err(self
                            .classify_session_driver_rejection(
                                &session_id,
                                RuntimeDriverError::Internal(err.to_string()),
                            )
                            .await);
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
                let previous_dsl_state = match self
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
                                &visibility_state.requested_witnesses,
                            ),
                            staged_deferred_authorities: visibility_authorities_for_names(
                                &visibility_state.staged_requested_deferred_names,
                                &visibility_state.requested_witnesses,
                            ),
                            active_visibility_revision: visibility_state.active_revision,
                            staged_visibility_revision: visibility_state.staged_revision,
                        },
                        "PublishCommittedVisibleSet",
                    )
                    .await
                {
                    Ok(previous) => previous,
                    Err(reason) => {
                        // Stage-first: PublishCommittedVisibleSet is declared
                        // per non-Destroyed phase only — classify a rejection
                        // on a Destroyed binding as the terminal truth.
                        return Err(self
                            .classify_session_dsl_rejection(&session_id, reason)
                            .await);
                    }
                };

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
                // Stage-first: `inner` stages the generated executor
                // registration claim; the machine rejects it on a Destroyed
                // binding and the inner classification surfaces the terminal
                // `Destroyed` truth. `inner` creates the session entry (if
                // new), holds the per-session mutation gate across the
                // generated registration claim and shell publication, attaches
                // the executor, and spawns the runtime loop.
                self.ensure_session_with_executor_inner(session_id, executor)
                    .await?;
                Ok(MeerkatMachineCommandResult::Unit)
            }
            _ => unreachable!("non-ensure-session command routed to arc session handler"),
        }
    }
}
