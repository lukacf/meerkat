use super::*;

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

    pub(super) fn capability_delta(
        previous: Option<SessionLlmCapabilitySurface>,
        current: Option<SessionLlmCapabilitySurface>,
    ) -> SessionLlmCapabilityDelta {
        SessionLlmCapabilityDelta {
            changed: previous != current,
            previous,
            current,
        }
    }

    pub(super) fn committed_visibility_allows(
        base_tool_names: &std::collections::BTreeSet<String>,
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

    pub(super) fn derive_reconfigured_visibility_state(
        current: &SessionToolVisibilityState,
        target_capability_surface: &SessionLlmCapabilitySurface,
        base_tool_names: &std::collections::BTreeSet<String>,
    ) -> (SessionToolVisibilityState, SessionToolVisibilityDelta) {
        let previous_capability_base_filter = current.capability_base_filter.clone();
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

        (
            next.clone(),
            SessionToolVisibilityDelta {
                previous_capability_base_filter,
                current_capability_base_filter: next.capability_base_filter,
                committed_visible_set_changed,
                revision_bumped,
            },
        )
    }

    pub(super) async fn set_cached_session_llm_state(
        &self,
        session_id: &SessionId,
        current_identity: Option<meerkat_core::SessionLlmIdentity>,
        current_capability_surface: Option<SessionLlmCapabilitySurface>,
        capability_surface_status: SessionLlmCapabilitySurfaceStatus,
    ) {
        let mut sessions = self.sessions.write().await;
        if let Some(entry) = sessions.get_mut(session_id) {
            entry.current_llm_identity = current_identity;
            entry.current_capability_surface = current_capability_surface;
            entry.capability_surface_status = capability_surface_status;
        }
    }

    pub(super) async fn cache_hydrated_session_llm_state(
        &self,
        session_id: &SessionId,
        hydrated: &HydratedSessionLlmState,
    ) -> Result<(), RuntimeDriverError> {
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        entry
            .tool_visibility_owner
            .replace_visibility_state(hydrated.current_visibility_state.clone())
            .map_err(|err| RuntimeDriverError::Internal(err.to_string()))?;
        entry.current_llm_identity = Some(hydrated.current_identity.clone());
        entry.current_capability_surface = hydrated.current_capability_surface.clone();
        entry.capability_surface_status = hydrated.capability_surface_status;
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
        previous_capability_surface: Option<SessionLlmCapabilitySurface>,
        previous_capability_surface_status: SessionLlmCapabilitySurfaceStatus,
        original_error: RuntimeDriverError,
    ) -> Result<SessionLlmReconfigureReport, RuntimeDriverError> {
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
            Ok(()) => {
                self.set_cached_session_llm_state(
                    session_id,
                    Some(previous_identity.clone()),
                    previous_capability_surface,
                    previous_capability_surface_status,
                )
                .await;
                Err(original_error)
            }
            Err(rollback_error) => {
                let _ = host.discard_live_session(session_id).await;
                self.set_cached_session_llm_state(
                    session_id,
                    None,
                    None,
                    SessionLlmCapabilitySurfaceStatus::Unresolved,
                )
                .await;
                Err(RuntimeDriverError::Internal(format!(
                    "failed to rollback live llm reconfiguration after error ({original_error}): {rollback_error}"
                )))
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
            RuntimeState::Attached | RuntimeState::Running
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

        let (next_visibility_state, tool_visibility_delta) =
            Self::derive_reconfigured_visibility_state(
                &hydrated.current_visibility_state,
                &resolved.target_capability_surface,
                &hydrated.base_tool_names,
            );
        let next_capability_base_filter = next_visibility_state.capability_base_filter.clone();
        let next_active_visibility_revision = next_visibility_state.active_revision;

        Ok(MeerkatMachineCommand::ReconfigureSessionLlmIdentity {
            session_id: session_id.clone(),
            previous_identity: Box::new(hydrated.current_identity),
            previous_visibility_state: Box::new(hydrated.current_visibility_state),
            previous_capability_surface: hydrated.current_capability_surface,
            previous_capability_surface_status: hydrated.capability_surface_status,
            target_identity: Box::new(resolved.target_identity),
            target_capability_surface: Box::new(resolved.target_capability_surface),
            next_visibility_state: Box::new(next_visibility_state),
            next_capability_base_filter,
            next_active_visibility_revision,
            tool_visibility_delta: Box::new(tool_visibility_delta),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn reconfigure_session_llm_identity_inner(
        &self,
        session_id: &SessionId,
        previous_identity: meerkat_core::SessionLlmIdentity,
        previous_visibility_state: SessionToolVisibilityState,
        previous_capability_surface: Option<SessionLlmCapabilitySurface>,
        previous_capability_surface_status: SessionLlmCapabilitySurfaceStatus,
        target_identity: meerkat_core::SessionLlmIdentity,
        target_capability_surface: SessionLlmCapabilitySurface,
        next_visibility_state: SessionToolVisibilityState,
        tool_visibility_delta: SessionToolVisibilityDelta,
    ) -> Result<SessionLlmReconfigureReport, RuntimeDriverError> {
        let host = self.llm_reconfigure_host()?;
        let runtime_state = self
            .existing_session_runtime_state(session_id)
            .await
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        if !matches!(
            runtime_state,
            RuntimeState::Attached | RuntimeState::Running
        ) {
            return Err(RuntimeDriverError::NotReady {
                state: runtime_state,
            });
        }

        let capability_delta = Self::capability_delta(
            previous_capability_surface.clone(),
            Some(target_capability_surface.clone()),
        );

        host.apply_live_session_llm_identity(session_id, &target_identity)
            .await?;
        if let Err(error) = host
            .apply_live_session_tool_visibility_state(
                session_id,
                Some(next_visibility_state.clone()),
            )
            .await
        {
            return self
                .rollback_reconfigure_failure(
                    &host,
                    session_id,
                    &previous_identity,
                    &previous_visibility_state,
                    previous_capability_surface.clone(),
                    previous_capability_surface_status,
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
                    previous_capability_surface.clone(),
                    previous_capability_surface_status,
                    error,
                )
                .await;
        }

        self.replace_machine_visibility_state(session_id, next_visibility_state)
            .await?;
        self.set_cached_session_llm_state(
            session_id,
            Some(target_identity.clone()),
            Some(target_capability_surface.clone()),
            SessionLlmCapabilitySurfaceStatus::Resolved,
        )
        .await;

        Ok(SessionLlmReconfigureReport {
            previous_identity,
            new_identity: target_identity,
            capability_delta,
            tool_visibility_delta,
            rollback_occurred: false,
        })
    }
}
