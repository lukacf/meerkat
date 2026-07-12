use super::*;
use meerkat_core::ToolName;

/// Machine-backed tool-visibility owner. Staged-revision tokens are minted
/// by the session's `MeerkatMachine` DSL authority via its
/// `next_staged_visibility_revision` counter (dogma round 4, wave 2b #12) —
/// the owner holds a handle to that authority and routes every
/// staging-path trait call through the DSL so there is a single
/// monotonic source of truth.
///
/// The former `next_revision: AtomicU64` counter is gone; the DSL's
/// counter is authoritative and trait-object callers see the same values
/// as dispatch-path callers without a second shell-side source.
#[derive(Default)]
pub struct MachineToolVisibilityOwner {
    /// Canonical visibility projection. Any operation that needs both this
    /// projection and the shared DSL authority must acquire this lock first,
    /// then the DSL mutex. Snapshot readers sample visibility facts directly
    /// from the DSL instead of nesting these locks in the opposite order.
    pub state: StdRwLock<SessionToolVisibilityState>,
    /// Handle to the per-session DSL authority — set by
    /// `MeerkatMachine::session_management` immediately after the owner is
    /// created. When present, staging-path trait calls mint their revision
    /// by firing the matching DSL input and reading back
    /// `staged_visibility_revision` from the authority state.
    ///
    /// Remains `None` only during construction / tests that never call a
    /// staging trait method — the owner refuses staging calls in that
    /// state rather than falling back to any shadow counter.
    dsl_authority: StdRwLock<Option<Arc<std::sync::Mutex<super::dsl::MeerkatMachineAuthority>>>>,
    #[cfg(test)]
    lock_order_probe: std::sync::OnceLock<Arc<VisibilityLockOrderProbe>>,
}

#[cfg(test)]
pub(super) struct VisibilityLockOrderProbe {
    snapshot_dsl_acquired: std::sync::Barrier,
    first_locks_acquired: std::sync::Barrier,
}

#[cfg(test)]
impl VisibilityLockOrderProbe {
    pub(super) fn new() -> Self {
        Self {
            snapshot_dsl_acquired: std::sync::Barrier::new(2),
            first_locks_acquired: std::sync::Barrier::new(2),
        }
    }

    fn snapshot_acquired_dsl(&self) {
        self.snapshot_dsl_acquired.wait();
        self.first_locks_acquired.wait();
    }

    pub(super) fn wait_for_snapshot_dsl(&self) {
        self.snapshot_dsl_acquired.wait();
    }

    fn commit_acquired_projection(&self) {
        self.first_locks_acquired.wait();
    }
}

impl std::fmt::Debug for MachineToolVisibilityOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MachineToolVisibilityOwner")
            .field("state", &"<StdRwLock<SessionToolVisibilityState>>")
            .field(
                "dsl_authority",
                &self
                    .dsl_authority
                    .read()
                    .ok()
                    .and_then(|slot| slot.as_ref().map(|_| "bound"))
                    .unwrap_or("unbound"),
            )
            .finish()
    }
}

impl MachineToolVisibilityOwner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach the per-session DSL authority. Called by the session
    /// management wiring immediately after the owner `Arc` is installed on
    /// the runtime session entry. Idempotent under re-binding during
    /// recovery.
    pub fn bind_dsl_authority(
        &self,
        authority: Arc<std::sync::Mutex<super::dsl::MeerkatMachineAuthority>>,
    ) {
        let mut slot = self
            .dsl_authority
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = Some(authority);
    }

    #[cfg(test)]
    pub(super) fn install_lock_order_probe(
        &self,
        probe: Arc<VisibilityLockOrderProbe>,
    ) -> Result<(), Arc<VisibilityLockOrderProbe>> {
        self.lock_order_probe.set(probe)
    }

    fn dsl_authority_for_stage(
        &self,
    ) -> Result<Arc<std::sync::Mutex<super::dsl::MeerkatMachineAuthority>>, ToolScopeStageError>
    {
        let slot = self
            .dsl_authority
            .read()
            .map_err(|_| ToolScopeStageError::Owner {
                message: "machine visibility DSL authority slot lock poisoned".to_string(),
            })?;
        let authority = slot
            .as_ref()
            .cloned()
            .ok_or_else(|| ToolScopeStageError::Owner {
                message:
                    "machine visibility DSL authority not bound — staging call before session wiring"
                        .to_string(),
            })?;
        Ok(authority)
    }

    fn dsl_authority_for_apply(
        &self,
    ) -> Result<Arc<std::sync::Mutex<super::dsl::MeerkatMachineAuthority>>, ToolScopeApplyError>
    {
        let slot = self
            .dsl_authority
            .read()
            .map_err(|_| ToolScopeApplyError::Owner {
                message: "machine visibility DSL authority slot lock poisoned".to_string(),
            })?;
        let authority = slot
            .as_ref()
            .cloned()
            .ok_or_else(|| ToolScopeApplyError::Owner {
                message:
                    "machine visibility DSL authority not bound — apply call before session wiring"
                        .to_string(),
            })?;
        Ok(authority)
    }
}

pub fn formal_projection_value<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|err| format!("\"<serialization error: {err}>\""))
}

fn authority_witnesses_for_names(
    names: &std::collections::BTreeSet<ToolName>,
    witnesses: &std::collections::BTreeMap<ToolName, ToolVisibilityWitness>,
) -> std::collections::BTreeMap<ToolName, ToolVisibilityWitness> {
    names
        .iter()
        .filter_map(|name| {
            witnesses
                .get(name)
                .map(|witness| (name.clone(), witness.clone()))
        })
        .collect()
}

fn deferred_load_authority_map(
    authorities: &[DeferredToolLoadAuthority],
) -> Result<std::collections::BTreeMap<ToolName, ToolVisibilityWitness>, ToolScopeStageError> {
    let mut by_name = std::collections::BTreeMap::new();
    let mut invalid = Vec::new();

    for authority in authorities {
        match by_name.insert(authority.name.clone(), authority.witness.clone()) {
            Some(existing) if existing != authority.witness => invalid.push(authority.name.clone()),
            _ => {}
        }
    }

    if invalid.is_empty() {
        return Ok(by_name);
    }

    invalid.sort_unstable();
    invalid.dedup();
    Err(ToolScopeStageError::InvalidWitnesses { names: invalid })
}

fn dsl_witnesses(
    witnesses: &std::collections::BTreeMap<ToolName, ToolVisibilityWitness>,
) -> std::collections::BTreeMap<ToolName, super::dsl::ToolVisibilityWitness> {
    witnesses
        .iter()
        .map(|(name, witness)| {
            (
                name.clone(),
                super::dsl::ToolVisibilityWitness::from(witness),
            )
        })
        .collect()
}

fn core_tool_source_kind(kind: super::dsl::ToolSourceKind) -> meerkat_core::types::ToolSourceKind {
    match kind {
        super::dsl::ToolSourceKind::Builtin => meerkat_core::types::ToolSourceKind::Builtin,
        super::dsl::ToolSourceKind::Shell => meerkat_core::types::ToolSourceKind::Shell,
        super::dsl::ToolSourceKind::Comms => meerkat_core::types::ToolSourceKind::Comms,
        super::dsl::ToolSourceKind::Memory => meerkat_core::types::ToolSourceKind::Memory,
        super::dsl::ToolSourceKind::Schedule => meerkat_core::types::ToolSourceKind::Schedule,
        super::dsl::ToolSourceKind::WorkGraph => meerkat_core::types::ToolSourceKind::WorkGraph,
        super::dsl::ToolSourceKind::Mob => meerkat_core::types::ToolSourceKind::Mob,
        super::dsl::ToolSourceKind::Callback => meerkat_core::types::ToolSourceKind::Callback,
        super::dsl::ToolSourceKind::Mcp => meerkat_core::types::ToolSourceKind::Mcp,
        super::dsl::ToolSourceKind::RustBundle => meerkat_core::types::ToolSourceKind::RustBundle,
    }
}

fn core_tool_visibility_witness(
    witness: &super::dsl::ToolVisibilityWitness,
) -> ToolVisibilityWitness {
    ToolVisibilityWitness {
        last_seen_provenance: witness.last_seen_provenance.as_ref().map(|provenance| {
            meerkat_core::types::ToolProvenance {
                kind: core_tool_source_kind(provenance.kind),
                source_id: meerkat_core::types::ToolSourceId::new(provenance.source_id.clone()),
            }
        }),
    }
}

fn core_witnesses(
    witnesses: &std::collections::BTreeMap<ToolName, super::dsl::ToolVisibilityWitness>,
) -> std::collections::BTreeMap<ToolName, ToolVisibilityWitness> {
    witnesses
        .iter()
        .map(|(name, witness)| (name.clone(), core_tool_visibility_witness(witness)))
        .collect()
}

fn mirror_visibility_projection_from_authority(
    projection: &mut SessionToolVisibilityState,
    authority: &super::dsl::MeerkatMachineAuthority,
) {
    mirror_visibility_projection_from_state(projection, authority.state());
}

pub(super) fn mirror_visibility_projection_from_state(
    projection: &mut SessionToolVisibilityState,
    authority_state: &super::dsl::MeerkatMachineState,
) {
    projection.capability_base_filter = meerkat_core::ToolFilter::from(
        authority_state
            .current_session_capability_base_filter
            .clone(),
    );
    projection.inherited_base_filter =
        meerkat_core::ToolFilter::from(authority_state.inherited_base_filter.clone());
    projection.active_filter =
        meerkat_core::ToolFilter::from(authority_state.active_filter.clone());
    projection.staged_filter =
        meerkat_core::ToolFilter::from(authority_state.staged_filter.clone());
    projection.active_requested_deferred_names = authority_state.active_deferred_names.clone();
    projection.staged_requested_deferred_names = authority_state.staged_deferred_names.clone();
    projection.active_revision = authority_state.active_visibility_revision;
    projection.staged_revision = authority_state.staged_visibility_revision;
    projection.requested_witnesses =
        core_witnesses(&authority_state.requested_visibility_witnesses);
    projection.filter_witnesses = core_witnesses(&authority_state.filter_visibility_witnesses);
}

impl MachineToolVisibilityOwner {
    /// Consume a preauthorized sticky-fallback transition only if the exact
    /// generated parent sampled during staging is still current, mirroring the
    /// committed visibility projection in the same critical section.
    pub(crate) fn commit_previewed_sticky_model_fallback(
        &self,
        dsl: &crate::handles::HandleDslAuthority,
        expected: &super::dsl::MeerkatMachineAuthoritySnapshot,
        input: super::dsl::MeerkatMachineInput,
    ) -> Result<(), meerkat_core::handles::DslTransitionError> {
        let mut projection = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        #[cfg(test)]
        if let Some(probe) = self.lock_order_probe.get() {
            probe.commit_acquired_projection();
        }
        dsl.apply_previewed_input_and_sample_state(
            expected,
            input,
            "ModelRoutingHandle::stage_sticky_model_fallback/commit",
            |authority_state| {
                mirror_visibility_projection_from_state(&mut projection, authority_state);
            },
        )
    }
}

impl ToolVisibilityOwner for MachineToolVisibilityOwner {
    fn visibility_state(&self) -> Result<SessionToolVisibilityState, ToolScopeApplyError> {
        self.state
            .read()
            .map(|state| state.clone())
            .map_err(|_| ToolScopeApplyError::Owner {
                message: "machine visibility state lock poisoned".to_string(),
            })
    }

    fn replace_visibility_state(
        &self,
        visibility_state: SessionToolVisibilityState,
    ) -> Result<(), ToolScopeApplyError> {
        let active_deferred_authorities = dsl_witnesses(&authority_witnesses_for_names(
            &visibility_state.active_requested_deferred_names,
            &visibility_state.requested_witnesses,
        ));
        let staged_deferred_authorities = dsl_witnesses(&authority_witnesses_for_names(
            &visibility_state.staged_requested_deferred_names,
            &visibility_state.requested_witnesses,
        ));
        let dsl_visibility_state =
            super::dsl::SessionToolVisibilityState::from_domain(&visibility_state);
        let authority = self.dsl_authority_for_apply()?;
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut guard = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        super::dsl::MeerkatMachineMutator::apply(
            &mut *guard,
            super::dsl::MeerkatMachineInput::ReplaceVisibilityState {
                capability_base_filter: super::dsl::ToolFilter::from(
                    &visibility_state.capability_base_filter,
                ),
                inherited_base_filter: super::dsl::ToolFilter::from(
                    &visibility_state.inherited_base_filter,
                ),
                active_filter: super::dsl::ToolFilter::from(&visibility_state.active_filter),
                staged_filter: super::dsl::ToolFilter::from(&visibility_state.staged_filter),
                requested_witnesses: dsl_visibility_state.requested_witnesses.clone(),
                filter_witnesses: dsl_visibility_state.filter_witnesses,
                active_deferred_names: visibility_state.active_requested_deferred_names.clone(),
                staged_deferred_names: visibility_state.staged_requested_deferred_names.clone(),
                active_deferred_authorities,
                staged_deferred_authorities,
                active_revision: visibility_state.active_revision,
                staged_revision: visibility_state.staged_revision,
            },
        )
        .map_err(|err| ToolScopeApplyError::Owner {
            message: super::dsl_authority::map_error(err, "ReplaceVisibilityState"),
        })?;
        mirror_visibility_projection_from_authority(&mut state, &guard);
        Ok(())
    }

    fn stage_persistent_filter(
        &self,
        filter: ToolFilter,
        witnesses: std::collections::BTreeMap<ToolName, ToolVisibilityWitness>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        let authority = self.dsl_authority_for_stage()?;
        let mut state = self.state.write().map_err(|_| ToolScopeStageError::Owner {
            message: "machine visibility state lock poisoned".to_string(),
        })?;
        let mut guard = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut next_filter_witnesses = guard.state().filter_visibility_witnesses.clone();
        next_filter_witnesses.extend(dsl_witnesses(&witnesses));
        super::dsl::MeerkatMachineMutator::apply(
            &mut *guard,
            super::dsl::MeerkatMachineInput::StageVisibilityFilter {
                filter: super::dsl::ToolFilter::from(&filter),
                witnesses: next_filter_witnesses,
            },
        )
        .map_err(|err| ToolScopeStageError::Owner {
            message: super::dsl_authority::map_error(err, "StageVisibilityFilter"),
        })?;
        let revision = ToolScopeRevision(guard.state().staged_visibility_revision);
        mirror_visibility_projection_from_authority(&mut state, &guard);
        Ok(revision)
    }

    fn stage_requested_deferred_names(
        &self,
        names: std::collections::BTreeSet<ToolName>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        if !names.is_empty() {
            return Err(ToolScopeStageError::MissingWitnesses {
                names: names.into_iter().collect(),
            });
        }
        let authority = self.dsl_authority_for_stage()?;
        let mut state = self.state.write().map_err(|_| ToolScopeStageError::Owner {
            message: "machine visibility state lock poisoned".to_string(),
        })?;
        let mut guard = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        super::dsl::MeerkatMachineMutator::apply(
            &mut *guard,
            super::dsl::MeerkatMachineInput::StageDeferredNames { names },
        )
        .map_err(|err| ToolScopeStageError::Owner {
            message: super::dsl_authority::map_error(err, "StageDeferredNames"),
        })?;
        let revision = ToolScopeRevision(guard.state().staged_visibility_revision);
        mirror_visibility_projection_from_authority(&mut state, &guard);
        Ok(revision)
    }

    fn request_deferred_tools(
        &self,
        authorities: Vec<DeferredToolLoadAuthority>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        let authorities = deferred_load_authority_map(&authorities)?;
        if authorities.is_empty() {
            return Err(ToolScopeStageError::Owner {
                message: "deferred tool request requires at least one authority".to_string(),
            });
        }
        let authority = self.dsl_authority_for_stage()?;
        let mut state = self.state.write().map_err(|_| ToolScopeStageError::Owner {
            message: "machine visibility state lock poisoned".to_string(),
        })?;
        let mut guard = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut target_authorities = guard.state().staged_deferred_authorities.clone();
        target_authorities.extend(dsl_witnesses(&authorities));
        super::dsl::MeerkatMachineMutator::apply(
            &mut *guard,
            super::dsl::MeerkatMachineInput::RequestDeferredTools {
                authorities: target_authorities,
            },
        )
        .map_err(|err| ToolScopeStageError::Owner {
            message: super::dsl_authority::map_error(err, "RequestDeferredTools"),
        })?;
        let revision = ToolScopeRevision(guard.state().staged_visibility_revision);
        mirror_visibility_projection_from_authority(&mut state, &guard);
        Ok(revision)
    }

    fn replace_deferred_tool_authority_catalog(
        &self,
        catalog: std::collections::BTreeMap<ToolName, ToolVisibilityWitness>,
    ) -> Result<(), ToolScopeApplyError> {
        let authority = self.dsl_authority_for_apply()?;
        let mut state = self.state.write().map_err(|_| ToolScopeApplyError::Owner {
            message: "machine visibility state lock poisoned".to_string(),
        })?;
        let mut guard = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        super::dsl::MeerkatMachineMutator::apply(
            &mut *guard,
            super::dsl::MeerkatMachineInput::ReplaceDeferredToolAuthorityCatalog {
                catalog: dsl_witnesses(&catalog),
            },
        )
        .map_err(|err| ToolScopeApplyError::Owner {
            message: super::dsl_authority::map_error(err, "ReplaceDeferredToolAuthorityCatalog"),
        })?;
        mirror_visibility_projection_from_authority(&mut state, &guard);
        Ok(())
    }

    fn replace_filter_tool_authority_catalog(
        &self,
        catalog: std::collections::BTreeMap<ToolName, ToolVisibilityWitness>,
    ) -> Result<(), ToolScopeApplyError> {
        let authority = self.dsl_authority_for_apply()?;
        let mut state = self.state.write().map_err(|_| ToolScopeApplyError::Owner {
            message: "machine visibility state lock poisoned".to_string(),
        })?;
        let mut guard = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        super::dsl::MeerkatMachineMutator::apply(
            &mut *guard,
            super::dsl::MeerkatMachineInput::ReplaceFilterToolAuthorityCatalog {
                catalog: dsl_witnesses(&catalog),
            },
        )
        .map_err(|err| ToolScopeApplyError::Owner {
            message: super::dsl_authority::map_error(err, "ReplaceFilterToolAuthorityCatalog"),
        })?;
        mirror_visibility_projection_from_authority(&mut state, &guard);
        Ok(())
    }

    fn set_turn_overlay(
        &self,
        allow: Option<std::collections::BTreeSet<ToolName>>,
        deny: std::collections::BTreeSet<ToolName>,
    ) -> Result<ToolScopeTurnOverlay, ToolScopeStageError> {
        let authority = self.dsl_authority_for_stage()?;
        let allow_active = allow.is_some();
        let allow_names = allow.unwrap_or_default();
        let mut guard = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        super::dsl::MeerkatMachineMutator::apply(
            &mut *guard,
            super::dsl::MeerkatMachineInput::SetTurnToolOverlay {
                allow_active,
                allow_names,
                deny_names: deny,
            },
        )
        .map_err(|err| ToolScopeStageError::Owner {
            message: super::dsl_authority::map_error(err, "SetTurnToolOverlay"),
        })?;
        let state = guard.state();
        Ok(ToolScopeTurnOverlay::from_string_sets(
            state
                .turn_tool_overlay_allow_active
                .then(|| state.turn_tool_overlay_allow_names.clone()),
            state.turn_tool_overlay_deny_names.clone(),
        ))
    }

    fn clear_turn_overlay(&self) -> Result<ToolScopeTurnOverlay, ToolScopeStageError> {
        let authority = self.dsl_authority_for_stage()?;
        let mut guard = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        super::dsl::MeerkatMachineMutator::apply(
            &mut *guard,
            super::dsl::MeerkatMachineInput::ClearTurnToolOverlay,
        )
        .map_err(|err| ToolScopeStageError::Owner {
            message: super::dsl_authority::map_error(err, "ClearTurnToolOverlay"),
        })?;
        Ok(ToolScopeTurnOverlay::cleared())
    }

    fn requires_filter_witnesses(&self) -> bool {
        true
    }

    fn boundary_applied(&self) -> Result<SessionToolVisibilityState, ToolScopeApplyError> {
        let authority = self.dsl_authority_for_apply()?;
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut guard = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let staged_filter = guard.state().staged_filter.clone();
        let staged_revision = guard.state().staged_visibility_revision;
        let staged_deferred_authorities = guard.state().staged_deferred_authorities.clone();
        super::dsl::MeerkatMachineMutator::apply(
            &mut *guard,
            super::dsl::MeerkatMachineInput::CommitVisibilityFilter {
                filter: staged_filter,
                revision: staged_revision,
            },
        )
        .map_err(|err| ToolScopeApplyError::Owner {
            message: super::dsl_authority::map_error(err, "CommitVisibilityFilter"),
        })?;
        super::dsl::MeerkatMachineMutator::apply(
            &mut *guard,
            super::dsl::MeerkatMachineInput::CommitDeferredNames {
                authorities: staged_deferred_authorities,
            },
        )
        .map_err(|err| ToolScopeApplyError::Owner {
            message: super::dsl_authority::map_error(err, "CommitDeferredNames"),
        })?;

        mirror_visibility_projection_from_authority(&mut state, &guard);
        Ok(state.clone())
    }
}

impl MeerkatMachine {
    pub async fn meerkat_machine_archive_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Option<MeerkatArchiveSnapshot> {
        let (driver_handle, control_snapshot, completions_handle) = {
            let sessions = self.sessions.read().await;
            let entry = sessions.get(session_id)?;
            let control = entry.control_snapshot();
            let mut snapshot = control.clone();
            let authority = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let dsl_phase =
                crate::meerkat_machine::dsl_authority::runtime_phase_from_authority(&authority);
            let dsl_current_run_id =
                crate::meerkat_machine::dsl_authority::current_run_id_from_authority(&authority);
            let dsl_pre_run_phase =
                crate::meerkat_machine::dsl_authority::pre_run_phase_from_authority(&authority);
            // Mirror the machine-owned visible-phase arbitration verdict: the
            // machine decides `publish_control` (terminal precedence) and the
            // visibility rewrite. When the control projection supersedes the DSL
            // we keep the raw control snapshot; otherwise we expose the visible
            // DSL phase plus the live DSL run binding.
            let plan = crate::meerkat_machine::resolve_visible_runtime_phase(
                dsl_phase,
                dsl_pre_run_phase,
                control.phase,
                control.pre_run_phase,
                self.has_runtime_persistence(),
            )
            .unwrap_or(crate::meerkat_machine::VisibleRuntimePhasePlan {
                publish_control: true,
                selected_raw_phase: control.phase,
                visible_phase: control.phase,
            });
            if !plan.publish_control {
                snapshot.phase = plan.visible_phase;
                snapshot.current_run_id = dsl_current_run_id;
                snapshot.pre_run_phase = dsl_pre_run_phase;
            }
            (
                Arc::clone(&entry.driver),
                snapshot,
                Arc::clone(&entry.completions),
            )
        };

        let completion_waiters = {
            let completions = completions_handle.lock().await;
            let snapshot = completions.diagnostic_snapshot();
            MeerkatCompletionWaitersSnapshot {
                input_count: snapshot.input_count,
                waiter_count: snapshot.waiter_count,
                waiting_inputs: snapshot
                    .waiting_inputs
                    .into_iter()
                    .map(|entry| MeerkatCompletionWaiterSnapshot {
                        input_id: entry.input_id,
                        waiter_count: entry.waiter_count,
                    })
                    .collect(),
            }
        };

        let (queue, steer_queue) = {
            let driver = driver_handle.lock().await;
            let ingress = driver.driver_ingress();
            (ingress.queue(), ingress.steer_queue())
        };

        Some(MeerkatArchiveSnapshot {
            control: MeerkatControlSnapshot {
                phase: control_snapshot.phase,
                current_run_id: control_snapshot.current_run_id,
                pre_run_phase: control_snapshot.pre_run_phase,
            },
            queue,
            steer_queue,
            completion_waiters,
        })
    }

    pub async fn meerkat_machine_spine_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Option<MeerkatMachineSpineSnapshot> {
        tracing::info!(%session_id, "meerkat_machine_spine_snapshot start");
        let (
            driver_handle,
            control_snapshot,
            completions_handle,
            ops_lifecycle,
            completions_present,
            ops_registry_present,
            epoch_id,
            formal_pre_run_phase,
            formal_visibility_authority_catalogs,
        ) = {
            tracing::info!(%session_id, "meerkat_machine_spine_snapshot reading session entry");
            let sessions = self.sessions.read().await;
            let entry = sessions.get(session_id)?;
            tracing::info!(%session_id, "meerkat_machine_spine_snapshot locking dsl authority");
            // DSL is the transition authority for live states. Persistent
            // sessions hold `control_projection` as the machine-published
            // lifecycle barrier for run-return and terminal boundaries until
            // the durable receipt succeeds, so diagnostics do not expose
            // in-flight terminal proposals.
            let control = entry.control_snapshot();
            let mut snapshot = control.clone();
            let authority = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            #[cfg(test)]
            if let Some(probe) = entry.tool_visibility_owner.lock_order_probe.get() {
                probe.snapshot_acquired_dsl();
            }
            tracing::info!(%session_id, "meerkat_machine_spine_snapshot locked dsl authority");
            let dsl_phase =
                crate::meerkat_machine::dsl_authority::runtime_phase_from_authority(&authority);
            let dsl_current_run_id =
                crate::meerkat_machine::dsl_authority::current_run_id_from_authority(&authority);
            let dsl_pre_run_phase =
                crate::meerkat_machine::dsl_authority::pre_run_phase_from_authority(&authority);
            // Mirror the machine-owned visible-phase arbitration verdict (see
            // the archive snapshot path above for the full contract).
            let plan = crate::meerkat_machine::resolve_visible_runtime_phase(
                dsl_phase,
                dsl_pre_run_phase,
                control.phase,
                control.pre_run_phase,
                self.has_runtime_persistence(),
            )
            .unwrap_or(crate::meerkat_machine::VisibleRuntimePhasePlan {
                publish_control: true,
                selected_raw_phase: control.phase,
                visible_phase: control.phase,
            });
            if !plan.publish_control {
                snapshot.phase = plan.visible_phase;
                snapshot.current_run_id = dsl_current_run_id;
                snapshot.pre_run_phase = dsl_pre_run_phase;
            }
            let formal_pre_run_phase = snapshot
                .pre_run_phase
                .and_then(crate::meerkat_machine::dsl_authority::pre_run_phase_from_runtime_state)
                .map(|phase| format!("{phase:?}"));
            let formal_visibility_authority_catalogs = {
                let state = authority.state();
                (
                    core_witnesses(&state.deferred_visibility_authority_catalog),
                    core_witnesses(&state.filter_visibility_authority_catalog),
                )
            };
            (
                Arc::clone(&entry.driver),
                snapshot,
                Arc::clone(&entry.completions),
                Arc::clone(&entry.ops_lifecycle),
                true,
                true,
                entry.epoch_id.clone(),
                formal_pre_run_phase,
                formal_visibility_authority_catalogs,
            )
        };
        tracing::info!(%session_id, "meerkat_machine_spine_snapshot locking completions");
        let completion_waiters = {
            let completions = completions_handle.lock().await;
            tracing::info!(%session_id, "meerkat_machine_spine_snapshot locked completions");
            let snapshot = completions.diagnostic_snapshot();
            MeerkatCompletionWaitersSnapshot {
                input_count: snapshot.input_count,
                waiter_count: snapshot.waiter_count,
                waiting_inputs: snapshot
                    .waiting_inputs
                    .into_iter()
                    .map(|entry| MeerkatCompletionWaiterSnapshot {
                        input_id: entry.input_id,
                        waiter_count: entry.waiter_count,
                    })
                    .collect(),
            }
        };
        tracing::info!(%session_id, "meerkat_machine_spine_snapshot reading drain");
        let drain = {
            let sessions = self.sessions.read().await;
            if let Some(entry) = sessions.get(session_id) {
                let authority = entry
                    .dsl_authority
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let state = authority.state();
                let phase = crate::meerkat_machine::CommsDrainPhase::from(state.drain_phase);
                let mode = state
                    .drain_mode
                    .map(crate::meerkat_machine::CommsDrainMode::from);
                let handle_present = entry.drain_slot.handle_present();
                let activated = phase != crate::meerkat_machine::CommsDrainPhase::Inactive
                    || mode.is_some()
                    || handle_present;
                if activated {
                    MeerkatDrainSnapshot {
                        slot_present: true,
                        phase: Some(phase),
                        mode,
                        handle_present,
                    }
                } else {
                    MeerkatDrainSnapshot {
                        slot_present: false,
                        phase: None,
                        mode: None,
                        handle_present: false,
                    }
                }
            } else {
                MeerkatDrainSnapshot {
                    slot_present: false,
                    phase: None,
                    mode: None,
                    handle_present: false,
                }
            }
        };
        tracing::info!(%session_id, "meerkat_machine_spine_snapshot locking driver");
        let driver = driver_handle.lock().await;
        tracing::info!(%session_id, "meerkat_machine_spine_snapshot locked driver");
        let driver_kind = match &*driver {
            DriverEntry::Ephemeral(_) => MeerkatDriverKind::Ephemeral,
            DriverEntry::Persistent(_) => MeerkatDriverKind::Persistent,
        };
        let ingress = driver.driver_ingress();

        let binding = MeerkatBindingSnapshot {
            session_id: session_id.clone(),
            runtime_id: driver.runtime_id().clone(),
            driver_kind,
            driver_present: true,
            completions_present,
            ops_registry_present,
            epoch_id,
            cursor_state: {
                let cursor_state = ops_lifecycle.completion_cursor_snapshot();
                MeerkatCursorSnapshot {
                    agent_applied_cursor: cursor_state.agent_applied_cursor,
                    runtime_observed_seq: cursor_state.runtime_observed_seq,
                    runtime_last_injected_seq: cursor_state.runtime_last_injected_seq,
                }
            },
        };

        let control = MeerkatControlSnapshot {
            phase: control_snapshot.phase,
            current_run_id: control_snapshot.current_run_id,
            pre_run_phase: control_snapshot.pre_run_phase,
        };

        let admission_order: Vec<MeerkatAdmittedInputSnapshot> = ingress
            .admission_order()
            .into_iter()
            .map(|input_id| MeerkatAdmittedInputSnapshot {
                content_shape: ingress.content_shape(&input_id),
                request_id: ingress.request_id(&input_id),
                reservation_key: ingress.reservation_key(&input_id),
                handling_mode: ingress.handling_mode(&input_id),
                live_interrupt_required: ingress.live_interrupt_required(&input_id),
                lifecycle: driver.input_phase(&input_id),
                terminal_outcome: driver.input_terminal_outcome(&input_id),
                last_run_id: driver.input_last_run_id(&input_id),
                last_boundary_sequence: driver.input_last_boundary_sequence(&input_id),
                is_prompt: ingress.is_prompt(&input_id),
                input_id,
            })
            .collect();

        let current_run_contributors = if let Some(control_run_id) = &control.current_run_id {
            admission_order
                .iter()
                .filter(|snapshot| {
                    snapshot.last_run_id.as_ref() == Some(control_run_id)
                        && matches!(
                            snapshot.lifecycle,
                            Some(
                                crate::input_state::InputLifecycleState::Staged
                                    | crate::input_state::InputLifecycleState::Applied
                                    | crate::input_state::InputLifecycleState::AppliedPendingConsumption
                            )
                        )
                })
                .map(|snapshot| snapshot.input_id.clone())
                .collect()
        } else {
            Vec::new()
        };

        let inputs = MeerkatInputsSnapshot {
            admission_order,
            queue: ingress.queue(),
            steer_queue: ingress.steer_queue(),
            current_run_id: control.current_run_id.clone(),
            current_run_contributors,
            post_admission_signal: format!("{:?}", driver.post_admission_signal()),
            silent_intent_overrides: driver.silent_comms_intents().into_iter().collect(),
        };
        let ledger = {
            let mut snapshot = MeerkatLedgerSnapshot {
                input_count: 0,
                non_terminal_count: 0,
                accepted_count: 0,
                queued_count: 0,
                staged_count: 0,
                applied_count: 0,
                applied_pending_consumption_count: 0,
                consumed_count: 0,
                superseded_count: 0,
                coalesced_count: 0,
                abandoned_count: 0,
            };

            for (input_id, _state) in driver.ledger().iter() {
                snapshot.input_count += 1;
                let Some(lifecycle) = driver.input_phase(input_id) else {
                    tracing::error!(
                        input_id = %input_id,
                        "missing generated input lifecycle authority for ledger snapshot"
                    );
                    continue;
                };
                let terminal = crate::meerkat_machine::input_phase_behavioral_terminality_via_authority(
                    input_id,
                    lifecycle,
                    driver.input_terminal_outcome(input_id),
                )
                .unwrap_or_else(|err| {
                    tracing::error!(
                        input_id = %input_id,
                        error = %err,
                        "generated input terminality authority rejected ledger snapshot classification"
                    );
                    true
                });
                if !terminal {
                    snapshot.non_terminal_count += 1;
                }
                match lifecycle {
                    InputLifecycleState::Accepted => snapshot.accepted_count += 1,
                    InputLifecycleState::Queued => snapshot.queued_count += 1,
                    InputLifecycleState::Staged => snapshot.staged_count += 1,
                    InputLifecycleState::Applied => snapshot.applied_count += 1,
                    InputLifecycleState::AppliedPendingConsumption => {
                        snapshot.applied_pending_consumption_count += 1;
                    }
                    InputLifecycleState::Consumed => snapshot.consumed_count += 1,
                    InputLifecycleState::Superseded => snapshot.superseded_count += 1,
                    InputLifecycleState::Coalesced => snapshot.coalesced_count += 1,
                    InputLifecycleState::Abandoned => snapshot.abandoned_count += 1,
                }
            }

            snapshot
        };
        let ops_snapshot = match ops_lifecycle.diagnostic_snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                tracing::error!(
                    session_id = %session_id,
                    error = %error,
                    "generated ops lifecycle authority rejected spine projection"
                );
                return None;
            }
        };
        let ops = MeerkatOpsSnapshot {
            operation_count: ops_snapshot.operation_count,
            active_count: ops_snapshot.active_count,
            wait_request_id: ops_snapshot.wait_request_id,
            pending_wait_present: ops_snapshot.pending_wait_present,
            pending_wait_request_id: ops_snapshot.pending_wait_request_id,
            wait_operation_ids: ops_snapshot.wait_operation_ids,
            operations: ops_snapshot.operations,
        };
        let formal_state = {
            let mut available_fields = std::collections::BTreeMap::new();
            available_fields.insert(
                "session_id".into(),
                formal_projection_value(&Some(session_id.to_string())),
            );
            available_fields.insert(
                "active_runtime_id".into(),
                formal_projection_value(&Some(driver.runtime_id().to_string())),
            );
            available_fields.insert(
                "current_run_id".into(),
                formal_projection_value(&control.current_run_id.as_ref().map(ToString::to_string)),
            );
            available_fields.insert(
                "pre_run_phase".into(),
                formal_projection_value(&formal_pre_run_phase),
            );
            available_fields.insert(
                "silent_intent_overrides".into(),
                formal_projection_value(
                    &driver
                        .silent_comms_intents()
                        .into_iter()
                        .collect::<BTreeSet<_>>(),
                ),
            );
            available_fields.insert(
                "deferred_visibility_authority_catalog".into(),
                formal_projection_value(&formal_visibility_authority_catalogs.0),
            );
            available_fields.insert(
                "filter_visibility_authority_catalog".into(),
                formal_projection_value(&formal_visibility_authority_catalogs.1),
            );
            MeerkatFormalStateProjection {
                available_fields,
                unavailable_fields: vec!["active_fence_token".into()],
            }
        };

        Some(MeerkatMachineSpineSnapshot {
            binding,
            control,
            inputs,
            ledger,
            completion_waiters,
            ops,
            drain,
            formal_state,
        })
    }
}
