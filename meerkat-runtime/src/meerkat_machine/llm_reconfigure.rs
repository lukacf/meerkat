use super::*;

pub(super) struct SessionLlmReconfigureApplyError {
    pub error: RuntimeDriverError,
    pub clear_generated_llm_state: bool,
}

pub(super) struct SessionLlmVisibilityPlanProposal {
    pub next_visibility_state: SessionToolVisibilityState,
    pub view_image_tool_available: bool,
    pub previous_view_image_visible: bool,
    pub next_view_image_visible: bool,
    pub previous_active_visibility_revision: u64,
    pub previous_staged_visibility_revision: u64,
    pub next_active_visibility_revision: u64,
    pub tool_visibility_delta: SessionToolVisibilityDelta,
}

pub(super) struct SessionLlmReconfigureAuthorityPlan {
    pub capability_delta: SessionLlmCapabilityDelta,
    pub tool_visibility_delta: SessionToolVisibilityDelta,
    pub next_capability_base_filter: meerkat_core::ToolFilter,
    pub next_active_visibility_revision: u64,
}

impl From<RuntimeDriverError> for SessionLlmReconfigureApplyError {
    fn from(error: RuntimeDriverError) -> Self {
        Self {
            error,
            clear_generated_llm_state: false,
        }
    }
}

impl MeerkatMachine {
    pub(super) fn llm_reconfigure_host(
        &self,
    ) -> Result<Arc<dyn SessionLlmReconfigureHost>, RuntimeDriverError> {
        self.llm_reconfigure_host
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| {
                RuntimeDriverError::Internal(
                    "session llm reconfigure host is not configured".to_string(),
                )
            })
    }

    pub(super) fn committed_visibility_allows(
        base_tool_names: &std::collections::BTreeSet<meerkat_core::ToolName>,
        visibility_state: &SessionToolVisibilityState,
        tool_name: &str,
    ) -> bool {
        if !base_tool_names.contains(tool_name) {
            return false;
        }

        meerkat_core::ToolScope::compose(&[
            visibility_state.capability_base_filter.clone(),
            visibility_state.inherited_base_filter.clone(),
            visibility_state.active_filter.clone(),
        ])
        .allows(tool_name)
    }

    /// Build the shell's proposed visibility witness for generated authority.
    ///
    /// The generated `ReconfigureSessionLlmIdentity*` transition validates
    /// this proposal and emits the authoritative report facts; callers must
    /// not publish these proposed deltas directly.
    pub(super) fn propose_reconfigured_visibility_plan(
        current: &SessionToolVisibilityState,
        target_capability_surface: &SessionLlmCapabilitySurface,
        base_tool_names: &std::collections::BTreeSet<meerkat_core::ToolName>,
    ) -> SessionLlmVisibilityPlanProposal {
        let previous_capability_base_filter = current.capability_base_filter.clone();
        let view_image_tool_available =
            base_tool_names.contains(meerkat_core::VIEW_IMAGE_TOOL_NAME);
        let current_view_image_visible = Self::committed_visibility_allows(
            base_tool_names,
            current,
            meerkat_core::VIEW_IMAGE_TOOL_NAME,
        );

        let mut next = current.clone();
        next.capability_base_filter = meerkat_core::capability_base_filter_for_image_tool_results(
            target_capability_surface.image_tool_results,
        );

        let next_view_image_visible = Self::committed_visibility_allows(
            base_tool_names,
            &next,
            meerkat_core::VIEW_IMAGE_TOOL_NAME,
        );
        let committed_visible_set_changed = current_view_image_visible != next_view_image_visible;
        let revision_bumped = committed_visible_set_changed;
        if revision_bumped {
            next.active_revision = current.active_revision.max(current.staged_revision) + 1;
        }

        SessionLlmVisibilityPlanProposal {
            next_active_visibility_revision: next.active_revision,
            next_visibility_state: next.clone(),
            view_image_tool_available,
            previous_view_image_visible: current_view_image_visible,
            next_view_image_visible,
            previous_active_visibility_revision: current.active_revision,
            previous_staged_visibility_revision: current.staged_revision,
            tool_visibility_delta: SessionToolVisibilityDelta {
                previous_capability_base_filter,
                current_capability_base_filter: next.capability_base_filter,
                committed_visible_set_changed,
                revision_bumped,
            },
        }
    }

    pub(super) fn session_llm_reconfigure_authority_plan(
        effects: &DslTransitionEffects,
    ) -> Result<SessionLlmReconfigureAuthorityPlan, RuntimeDriverError> {
        let mut resolved = None;
        for effect in effects.as_slice() {
            let dsl::MeerkatMachineEffect::SessionLlmReconfigurePlanResolved {
                previous_capability_surface,
                current_capability_surface,
                capability_changed,
                previous_capability_base_filter,
                current_capability_base_filter,
                committed_visible_set_changed,
                revision_bumped,
                active_visibility_revision,
            } = effect
            else {
                continue;
            };

            let capability_delta = SessionLlmCapabilityDelta {
                previous: (*previous_capability_surface).map(Into::into),
                current: (*current_capability_surface).map(Into::into),
                changed: *capability_changed,
            };
            let previous_capability_base_filter: meerkat_core::ToolFilter =
                previous_capability_base_filter.clone().into();
            let current_capability_base_filter: meerkat_core::ToolFilter =
                current_capability_base_filter.clone().into();
            let tool_visibility_delta = SessionToolVisibilityDelta {
                previous_capability_base_filter,
                current_capability_base_filter: current_capability_base_filter.clone(),
                committed_visible_set_changed: *committed_visible_set_changed,
                revision_bumped: *revision_bumped,
            };
            let plan = SessionLlmReconfigureAuthorityPlan {
                capability_delta,
                tool_visibility_delta,
                next_capability_base_filter: current_capability_base_filter,
                next_active_visibility_revision: *active_visibility_revision,
            };
            if resolved.replace(plan).is_some() {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason:
                        "generated SessionLlmReconfigurePlanResolved effect emitted more than once"
                            .to_string(),
                });
            }
        }
        resolved.ok_or_else(|| RuntimeDriverError::ValidationFailed {
            reason: "generated SessionLlmReconfigurePlanResolved effect was absent".to_string(),
        })
    }

    pub(super) async fn cache_hydrated_session_llm_state(
        &self,
        session_id: &SessionId,
        hydrated: &HydratedSessionLlmState,
    ) -> Result<(), RuntimeDriverError> {
        use crate::meerkat_machine::dsl as mm_dsl;
        let previous_dsl_state = self
            .stage_session_dsl_input(
                session_id,
                mm_dsl::MeerkatMachineInput::HydrateSessionLlmState {
                    current_identity: mm_dsl::SessionLlmIdentity::from_domain(
                        &hydrated.current_identity,
                    ),
                    current_capability_surface: hydrated
                        .current_capability_surface
                        .as_ref()
                        .map(mm_dsl::SessionLlmCapabilitySurface::from_domain),
                    current_capability_surface_status:
                        mm_dsl::SessionLlmCapabilitySurfaceStatus::from_domain(
                            &hydrated.capability_surface_status,
                        ),
                    current_capability_base_filter: mm_dsl::ToolFilter::from_domain(
                        &hydrated.current_visibility_state.capability_base_filter,
                    ),
                },
                "HydrateSessionLlmState",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        let mut sessions = self.sessions.write().await;
        let Some(entry) = sessions.get_mut(session_id) else {
            drop(sessions);
            self.restore_session_dsl_state(session_id, previous_dsl_state)
                .await;
            return Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            });
        };
        if let Err(err) = entry
            .tool_visibility_owner
            .replace_visibility_state(hydrated.current_visibility_state.clone())
        {
            drop(sessions);
            self.restore_session_dsl_state(session_id, previous_dsl_state)
                .await;
            return Err(RuntimeDriverError::Internal(err.to_string()));
        }
        Ok(())
    }

    pub(super) async fn replace_machine_visibility_state(
        &self,
        session_id: &SessionId,
        visibility_state: SessionToolVisibilityState,
    ) -> Result<(), RuntimeDriverError> {
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        entry
            .tool_visibility_owner
            .replace_visibility_state(visibility_state)
            .map_err(|err| RuntimeDriverError::Internal(err.to_string()))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn rollback_reconfigure_failure(
        &self,
        host: &Arc<dyn SessionLlmReconfigureHost>,
        session_id: &SessionId,
        previous_identity: &meerkat_core::SessionLlmIdentity,
        previous_visibility_state: &SessionToolVisibilityState,
        original_error: RuntimeDriverError,
    ) -> Result<SessionLlmReconfigureReport, SessionLlmReconfigureApplyError> {
        let rollback_result = async {
            host.apply_live_session_llm_identity(session_id, previous_identity)
                .await?;
            host.apply_live_session_tool_visibility_state(
                session_id,
                Some(previous_visibility_state.clone()),
            )
            .await?;
            Ok::<(), RuntimeDriverError>(())
        }
        .await;

        match rollback_result {
            Ok(()) => Err(SessionLlmReconfigureApplyError {
                error: original_error,
                clear_generated_llm_state: false,
            }),
            Err(rollback_error) => {
                let _ = host.discard_live_session(session_id).await;
                Err(SessionLlmReconfigureApplyError {
                    error: RuntimeDriverError::Internal(format!(
                        "failed to rollback live llm reconfiguration after error ({original_error}): {rollback_error}"
                    )),
                    clear_generated_llm_state: true,
                })
            }
        }
    }

    pub(super) async fn prepare_reconfigure_session_llm_command(
        &self,
        session_id: &SessionId,
        request: SessionLlmReconfigureRequest,
    ) -> Result<MeerkatMachineCommand, RuntimeDriverError> {
        let host = self.llm_reconfigure_host()?;
        let runtime_state = self
            .existing_session_runtime_state(session_id)
            .await
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        if !matches!(
            runtime_state,
            RuntimeState::Idle | RuntimeState::Attached | RuntimeState::Running
        ) {
            return Err(RuntimeDriverError::NotReady {
                state: runtime_state,
            });
        }

        let hydrated = host.hydrate_session_llm_state(session_id).await?;
        self.cache_hydrated_session_llm_state(session_id, &hydrated)
            .await?;

        let resolved = host
            .resolve_target_session_llm_identity(&request, &hydrated.current_identity)
            .await?;

        let visibility_plan = Self::propose_reconfigured_visibility_plan(
            &hydrated.current_visibility_state,
            &resolved.target_capability_surface,
            &hydrated.base_tool_names,
        );
        let next_capability_base_filter = visibility_plan
            .next_visibility_state
            .capability_base_filter
            .clone();
        let next_active_visibility_revision = visibility_plan.next_active_visibility_revision;

        Ok(MeerkatMachineCommand::ReconfigureSessionLlmIdentity {
            session_id: session_id.clone(),
            previous_identity: Box::new(hydrated.current_identity),
            previous_visibility_state: Box::new(hydrated.current_visibility_state),
            previous_capability_surface: hydrated.current_capability_surface,
            previous_capability_surface_status: hydrated.capability_surface_status,
            view_image_tool_available: visibility_plan.view_image_tool_available,
            previous_view_image_visible: visibility_plan.previous_view_image_visible,
            next_view_image_visible: visibility_plan.next_view_image_visible,
            previous_active_visibility_revision: visibility_plan
                .previous_active_visibility_revision,
            previous_staged_visibility_revision: visibility_plan
                .previous_staged_visibility_revision,
            target_identity: Box::new(resolved.target_identity),
            target_capability_surface: Box::new(resolved.target_capability_surface),
            next_visibility_state: Box::new(visibility_plan.next_visibility_state),
            next_capability_base_filter,
            next_active_visibility_revision,
            tool_visibility_delta: Box::new(visibility_plan.tool_visibility_delta),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn reconfigure_session_llm_identity_inner(
        &self,
        session_id: &SessionId,
        previous_identity: meerkat_core::SessionLlmIdentity,
        previous_visibility_state: SessionToolVisibilityState,
        target_identity: meerkat_core::SessionLlmIdentity,
        next_visibility_state: SessionToolVisibilityState,
        authority_plan: SessionLlmReconfigureAuthorityPlan,
    ) -> Result<SessionLlmReconfigureReport, SessionLlmReconfigureApplyError> {
        let host = self.llm_reconfigure_host()?;
        let runtime_state = self
            .existing_session_runtime_state(session_id)
            .await
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        if !matches!(
            runtime_state,
            RuntimeState::Idle | RuntimeState::Attached | RuntimeState::Running
        ) {
            return Err(RuntimeDriverError::NotReady {
                state: runtime_state,
            }
            .into());
        }

        let mut authorized_next_visibility_state = next_visibility_state;
        authorized_next_visibility_state.capability_base_filter =
            authority_plan.next_capability_base_filter.clone();
        authorized_next_visibility_state.active_revision =
            authority_plan.next_active_visibility_revision;

        host.apply_live_session_llm_identity(session_id, &target_identity)
            .await?;
        if let Err(error) = host
            .apply_live_session_tool_visibility_state(
                session_id,
                Some(authorized_next_visibility_state.clone()),
            )
            .await
        {
            return self
                .rollback_reconfigure_failure(
                    &host,
                    session_id,
                    &previous_identity,
                    &previous_visibility_state,
                    error,
                )
                .await;
        }

        if let Err(error) = host.persist_live_session(session_id).await {
            return self
                .rollback_reconfigure_failure(
                    &host,
                    session_id,
                    &previous_identity,
                    &previous_visibility_state,
                    error,
                )
                .await;
        }

        self.replace_machine_visibility_state(session_id, authorized_next_visibility_state)
            .await?;

        Ok(SessionLlmReconfigureReport {
            previous_identity,
            new_identity: target_identity,
            capability_delta: authority_plan.capability_delta,
            tool_visibility_delta: authority_plan.tool_visibility_delta,
            rollback_occurred: false,
        })
    }

    // NOTE: reconfigure_live_topology, fail_live_topology_past_detach,
    // apply_capability_driven_realtime_transport, and realtime_bootstrap_eligibility
    // were removed as part of the realtime/live-topology DSL plane deletion.
    // Provider session lifecycle now lives outside MeerkatMachine (live-adapter MVP).
}
