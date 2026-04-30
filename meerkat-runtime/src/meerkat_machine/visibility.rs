use super::*;

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
    pub state: StdRwLock<SessionToolVisibilityState>,
    deferred_authority_catalog:
        StdRwLock<std::collections::BTreeMap<String, ToolVisibilityWitness>>,
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
            .field(
                "deferred_authority_catalog",
                &self
                    .deferred_authority_catalog
                    .read()
                    .map(|catalog| catalog.len())
                    .unwrap_or_default(),
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

    fn mint_revision_via_dsl(
        &self,
        input: super::dsl::MeerkatMachineInput,
        context: &'static str,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
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
        drop(slot);
        let mut guard = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        super::dsl::MeerkatMachineMutator::apply(&mut *guard, input).map_err(|err| {
            ToolScopeStageError::Owner {
                message: super::dsl_authority::map_error(err, context),
            }
        })?;
        Ok(ToolScopeRevision(guard.state.staged_visibility_revision))
    }

    fn apply_visibility_dsl_input(
        &self,
        input: super::dsl::MeerkatMachineInput,
        context: &'static str,
    ) -> Result<(), ToolScopeApplyError> {
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
        drop(slot);
        let mut guard = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        super::dsl::MeerkatMachineMutator::apply(&mut *guard, input).map_err(|err| {
            ToolScopeApplyError::Owner {
                message: super::dsl_authority::map_error(err, context),
            }
        })?;
        Ok(())
    }

    pub(super) fn canonical_deferred_authorities_for_visibility_state(
        &self,
        visibility_state: &SessionToolVisibilityState,
    ) -> Result<std::collections::BTreeMap<String, ToolVisibilityWitness>, ToolScopeStageError>
    {
        let names = deferred_authority_names_for_visibility_state(visibility_state);
        if names.is_empty() {
            return Ok(Default::default());
        }
        let authority_catalog =
            self.deferred_authority_catalog
                .read()
                .map_err(|_| ToolScopeStageError::Owner {
                    message: "machine visibility deferred authority catalog lock poisoned"
                        .to_string(),
                })?;
        canonical_deferred_authorities_for_names(
            &names,
            &visibility_state.requested_witnesses,
            &authority_catalog,
        )
    }
}

pub fn formal_projection_value<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|err| format!("\"<serialization error: {err}>\""))
}

fn missing_visibility_witness_names(
    names: &std::collections::BTreeSet<String>,
    witnesses: &std::collections::BTreeMap<String, ToolVisibilityWitness>,
) -> Vec<String> {
    names
        .iter()
        .filter(|name| {
            witnesses
                .get(name.as_str())
                .is_none_or(|witness| !witness.has_provenance_identity_witness())
        })
        .cloned()
        .collect()
}

fn authority_witnesses_for_names(
    names: &std::collections::BTreeSet<String>,
    witnesses: &std::collections::BTreeMap<String, ToolVisibilityWitness>,
) -> std::collections::BTreeMap<String, ToolVisibilityWitness> {
    names
        .iter()
        .filter_map(|name| {
            witnesses
                .get(name)
                .map(|witness| (name.clone(), witness.clone()))
        })
        .collect()
}

fn deferred_authority_names_for_visibility_state(
    visibility_state: &SessionToolVisibilityState,
) -> std::collections::BTreeSet<String> {
    visibility_state
        .active_requested_deferred_names
        .union(&visibility_state.staged_requested_deferred_names)
        .cloned()
        .collect()
}

fn invalid_deferred_authority_names(
    names: &std::collections::BTreeSet<String>,
    witnesses: &std::collections::BTreeMap<String, ToolVisibilityWitness>,
    authority_catalog: &std::collections::BTreeMap<String, ToolVisibilityWitness>,
) -> Vec<String> {
    let mut invalid = names
        .iter()
        .filter(|name| {
            let witness = witnesses.get(name.as_str());
            let expected = authority_catalog.get(name.as_str());
            !matches!(
                (witness, expected),
                (Some(witness), Some(expected))
                    if witness.stable_owner_key == expected.stable_owner_key
                        && witness.last_seen_provenance == expected.last_seen_provenance
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    invalid.sort_unstable();
    invalid.dedup();
    invalid
}

fn canonical_deferred_authorities_for_names(
    names: &std::collections::BTreeSet<String>,
    witnesses: &std::collections::BTreeMap<String, ToolVisibilityWitness>,
    authority_catalog: &std::collections::BTreeMap<String, ToolVisibilityWitness>,
) -> Result<std::collections::BTreeMap<String, ToolVisibilityWitness>, ToolScopeStageError> {
    let missing = missing_visibility_witness_names(names, witnesses);
    if !missing.is_empty() {
        return Err(ToolScopeStageError::MissingWitnesses { names: missing });
    }
    let invalid = invalid_deferred_authority_names(names, witnesses, authority_catalog);
    if !invalid.is_empty() {
        return Err(ToolScopeStageError::InvalidWitnesses { names: invalid });
    }
    let mut authorities = std::collections::BTreeMap::new();
    for name in names {
        let Some(witness) = authority_catalog.get(name.as_str()) else {
            return Err(ToolScopeStageError::InvalidWitnesses {
                names: vec![name.clone()],
            });
        };
        authorities.insert(name.clone(), witness.clone());
    }
    Ok(authorities)
}

fn dsl_witnesses(
    witnesses: &std::collections::BTreeMap<String, ToolVisibilityWitness>,
) -> std::collections::BTreeMap<String, super::dsl::ToolVisibilityWitness> {
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
        mut visibility_state: SessionToolVisibilityState,
    ) -> Result<(), ToolScopeApplyError> {
        let deferred_authorities = self
            .canonical_deferred_authorities_for_visibility_state(&visibility_state)
            .map_err(|err| ToolScopeApplyError::Owner {
                message: format!("invalid deferred visibility authority: {err}"),
            })?;
        visibility_state
            .requested_witnesses
            .extend(deferred_authorities.clone());
        let active_deferred_authorities = dsl_witnesses(&authority_witnesses_for_names(
            &visibility_state.active_requested_deferred_names,
            &deferred_authorities,
        ));
        let staged_deferred_authorities = dsl_witnesses(&authority_witnesses_for_names(
            &visibility_state.staged_requested_deferred_names,
            &deferred_authorities,
        ));
        // Sync the DSL monotonic counter up to the externally-installed
        // revisions before we overwrite the owner-held state. The DSL
        // owns `next_staged_visibility_revision`; without this sync,
        // subsequent `stage_*` calls on a session whose durable state
        // was just replaced (recovery, LLM hot-swap, shell-side
        // projection re-install) would mint revisions starting from 0
        // and regress behind the durable state just installed (dogma
        // round 4, wave 2b #12 — single monotonic source, honest across
        // recovery / hot-swap boundaries).
        //
        // The DSL guard on `SyncVisibilityRevisions` makes the call
        // idempotent: when the installed revisions are at or below the
        // counter the transition is guard-rejected (no-op), which the
        // DSL classifies for us. Firing unconditionally is correct —
        // the guard is the single place that decides whether the sync
        // advances state, not a shell-side pre-check on the input
        // fields.
        let slot = self
            .dsl_authority
            .read()
            .map_err(|_| ToolScopeApplyError::Owner {
                message: "machine visibility DSL authority slot lock poisoned".to_string(),
            })?;
        if let Some(authority) = slot.as_ref().cloned() {
            drop(slot);
            let mut guard = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match super::dsl::MeerkatMachineMutator::apply(
                &mut *guard,
                super::dsl::MeerkatMachineInput::SyncVisibilityRevisions {
                    active_revision: visibility_state.active_revision,
                    staged_revision: visibility_state.staged_revision,
                    active_deferred_names: visibility_state.active_requested_deferred_names.clone(),
                    staged_deferred_names: visibility_state.staged_requested_deferred_names.clone(),
                    active_deferred_authorities: active_deferred_authorities.clone(),
                    staged_deferred_authorities: staged_deferred_authorities.clone(),
                },
            ) {
                Ok(_) => {}
                // Typed guard rejection is the idempotent no-op case:
                // neither revision advances the counter and the installed
                // deferred authority projection already matches the DSL.
                Err(err @ super::dsl::MeerkatMachineTransitionError::GuardRejected { .. }) => {
                    let counter_covers_install = visibility_state.active_revision
                        <= guard.state.next_staged_visibility_revision
                        && visibility_state.staged_revision
                            <= guard.state.next_staged_visibility_revision;
                    let deferred_authority_matches = guard.state.active_deferred_names
                        == visibility_state.active_requested_deferred_names
                        && guard.state.staged_deferred_names
                            == visibility_state.staged_requested_deferred_names
                        && guard.state.active_deferred_authorities == active_deferred_authorities
                        && guard.state.staged_deferred_authorities == staged_deferred_authorities;
                    if !(counter_covers_install && deferred_authority_matches) {
                        return Err(ToolScopeApplyError::Owner {
                            message: super::dsl_authority::map_error(
                                err,
                                "SyncVisibilityRevisions",
                            ),
                        });
                    }
                }
                Err(err) => {
                    return Err(ToolScopeApplyError::Owner {
                        message: super::dsl_authority::map_error(err, "SyncVisibilityRevisions"),
                    });
                }
            }
        }
        let mut state = self.state.write().map_err(|_| ToolScopeApplyError::Owner {
            message: "machine visibility state lock poisoned".to_string(),
        })?;
        *state = visibility_state;
        Ok(())
    }

    fn stage_persistent_filter(
        &self,
        filter: ToolFilter,
        witnesses: std::collections::BTreeMap<String, ToolVisibilityWitness>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        // DSL is the single monotonic source — mint first, then apply
        // the projection to the owner-held state under the projection
        // lock. The DSL input's `update {}` increments
        // `next_staged_visibility_revision` and stamps
        // `staged_visibility_revision` atomically inside the DSL
        // authority lock.
        let revision = self.mint_revision_via_dsl(
            super::dsl::MeerkatMachineInput::StageVisibilityFilter {
                filter: super::dsl::ToolFilter::from(&filter),
            },
            "StageVisibilityFilter",
        )?;
        let mut state = self.state.write().map_err(|_| ToolScopeStageError::Owner {
            message: "machine visibility state lock poisoned".to_string(),
        })?;
        state.staged_filter = filter;
        state.filter_witnesses.extend(witnesses);
        state.staged_revision = revision.0;
        Ok(revision)
    }

    fn stage_requested_deferred_names(
        &self,
        names: std::collections::BTreeSet<String>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        if !names.is_empty() {
            return Err(ToolScopeStageError::MissingWitnesses {
                names: names.into_iter().collect(),
            });
        }
        let revision = self.mint_revision_via_dsl(
            super::dsl::MeerkatMachineInput::StageDeferredNames {
                names: names.clone(),
            },
            "StageDeferredNames",
        )?;
        let mut state = self.state.write().map_err(|_| ToolScopeStageError::Owner {
            message: "machine visibility state lock poisoned".to_string(),
        })?;
        state.staged_requested_deferred_names = names;
        state.staged_revision = revision.0;
        Ok(revision)
    }

    fn request_deferred_tools(
        &self,
        authorities: std::collections::BTreeMap<String, ToolVisibilityWitness>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        // `request_deferred_tools` extends the staged set and carries the
        // authority map through the DSL input so deferred admission is
        // authority-visible, not a shell-only name projection.
        let (extended, mut combined_witnesses): (
            std::collections::BTreeSet<String>,
            std::collections::BTreeMap<String, ToolVisibilityWitness>,
        ) = {
            let state = self.state.read().map_err(|_| ToolScopeStageError::Owner {
                message: "machine visibility state lock poisoned".to_string(),
            })?;
            let names = authorities.keys().cloned().collect();
            let extended = state
                .staged_requested_deferred_names
                .union(&names)
                .cloned()
                .collect();
            let mut combined_witnesses = state.requested_witnesses.clone();
            combined_witnesses.extend(authorities);
            (extended, combined_witnesses)
        };
        let missing = missing_visibility_witness_names(&extended, &combined_witnesses);
        if !missing.is_empty() {
            return Err(ToolScopeStageError::MissingWitnesses { names: missing });
        }
        let staged_authorities = {
            let authority_catalog =
                self.deferred_authority_catalog
                    .read()
                    .map_err(|_| ToolScopeStageError::Owner {
                        message: "machine visibility deferred authority catalog lock poisoned"
                            .to_string(),
                    })?;
            canonical_deferred_authorities_for_names(
                &extended,
                &combined_witnesses,
                &authority_catalog,
            )?
        };
        combined_witnesses.extend(staged_authorities.clone());
        let dsl_witnesses = dsl_witnesses(&staged_authorities);
        let revision = self.mint_revision_via_dsl(
            super::dsl::MeerkatMachineInput::RequestDeferredTools {
                authorities: dsl_witnesses,
            },
            "RequestDeferredTools",
        )?;
        let mut state = self.state.write().map_err(|_| ToolScopeStageError::Owner {
            message: "machine visibility state lock poisoned".to_string(),
        })?;
        state.staged_requested_deferred_names = extended;
        state.requested_witnesses = combined_witnesses;
        state.staged_revision = revision.0;
        Ok(revision)
    }

    fn replace_deferred_tool_authority_catalog(
        &self,
        catalog: std::collections::BTreeMap<String, ToolVisibilityWitness>,
    ) {
        let mut guard = self
            .deferred_authority_catalog
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = catalog;
    }

    fn boundary_applied(&self) -> Result<SessionToolVisibilityState, ToolScopeApplyError> {
        let (names, authorities) = {
            let state = self.state.read().map_err(|_| ToolScopeApplyError::Owner {
                message: "machine visibility state lock poisoned".to_string(),
            })?;
            let names = state.staged_requested_deferred_names.clone();
            let authorities = authority_witnesses_for_names(&names, &state.requested_witnesses);
            (names, authorities)
        };
        self.apply_visibility_dsl_input(
            super::dsl::MeerkatMachineInput::CommitDeferredNames {
                authorities: dsl_witnesses(&authorities),
            },
            "CommitDeferredNames",
        )?;
        let mut state = self.state.write().map_err(|_| ToolScopeApplyError::Owner {
            message: "machine visibility state lock poisoned".to_string(),
        })?;
        state.active_filter = state.staged_filter.clone();
        state.active_requested_deferred_names = names;
        state.active_revision = state.staged_revision;
        Ok(state.clone())
    }
}

impl MeerkatMachine {
    pub async fn meerkat_machine_spine_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Option<MeerkatMachineSpineSnapshot> {
        let (
            driver_handle,
            control_snapshot,
            completions_handle,
            ops_lifecycle,
            cursor_state,
            completions_present,
            ops_registry_present,
            attachment_live,
            epoch_id,
            _visibility_state,
            formal_pre_run_phase,
        ) =
            {
                let sessions = self.sessions.read().await;
                let entry = sessions.get(session_id)?;
                // W6 Class B (`e5c5ecaf3`): the shell-side `control_projection`
                // is no longer written by the finalize-* paths in the driver
                // (`machine_retire` / `machine_destroy` / `machine_stop_runtime`
                // / `machine_reset` used to call `set_control_projection(...)`
                // pre-finalize, deleted by the shadow-truth cleanup). DSL is
                // the canonical source for `lifecycle_phase` AND `current_run_id`
                // (both fields were updated together by the deleted
                // `set_control_projection` call; both are tracked on the DSL
                // side at `dsl.rs::state.lifecycle_phase` + `state.current_run_id`).
                // Project both from the DSL authority so `spine_snapshot` matches
                // the DSL's visible control contract post-retire/destroy/reset.
                // A retired drain uses an internal Running/pre_run_phase=Retired
                // pair to execute preserved work, but remains externally Retired.
                // `pre_run_phase` is also DSL-owned now, so the spine projects the
                // whole lifecycle/run tuple from one authority.
                // Mirrors `existing_session_runtime_state`.
                let mut snapshot = entry.control_snapshot();
                let (phase, current_run_id, pre_run_phase, formal_pre_run_phase) =
                    {
                        let authority = entry
                            .dsl_authority
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        (
                    crate::meerkat_machine::dsl_authority::visible_runtime_phase_from_authority(
                        &authority,
                    ),
                    crate::meerkat_machine::dsl_authority::current_run_id_from_authority(
                        &authority,
                    ),
                    crate::meerkat_machine::dsl_authority::pre_run_phase_from_authority(&authority),
                    authority.state.pre_run_phase.map(|phase| format!("{phase:?}")),
                )
                    };
                snapshot.phase = phase;
                snapshot.current_run_id = current_run_id;
                snapshot.pre_run_phase = pre_run_phase;
                (
                    Arc::clone(&entry.driver),
                    snapshot,
                    Arc::clone(&entry.completions),
                    Arc::clone(&entry.ops_lifecycle),
                    Arc::clone(&entry.cursor_state),
                    true,
                    true,
                    entry.attachment_is_live(),
                    entry.epoch_id.clone(),
                    entry.tool_visibility_owner.visibility_state().ok()?,
                    formal_pre_run_phase,
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
        let drain = {
            let sessions = self.sessions.read().await;
            if let Some(entry) = sessions.get(session_id) {
                let slot = &entry.drain_slot;
                // Wave-c C-H2 (37cc46a44) collapsed the separate
                // `comms_drain_slots` map into `RuntimeSessionEntry`, so
                // the slot is structurally always present once the
                // session is registered. `slot_present` must keep its
                // pre-collapse semantics: "has this session been
                // drain-spawned?" — i.e. true once the slot has moved
                // past `Inactive` with no bindings. Inactive + no
                // bindings + no handle means the slot has never run.
                let activated = slot.phase != crate::meerkat_machine::CommsDrainPhase::Inactive
                    || slot.mode.is_some()
                    || slot.handle.is_some()
                    || slot.bound_runtime.is_some();
                if activated {
                    MeerkatDrainSnapshot {
                        slot_present: true,
                        phase: Some(slot.phase),
                        mode: slot.mode,
                        handle_present: slot.handle.is_some(),
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
        let driver = driver_handle.lock().await;
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
            attachment_live,
            epoch_id,
            cursor_state: {
                let cursor_state = cursor_state.snapshot();
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
            .iter()
            .cloned()
            .map(|input_id| MeerkatAdmittedInputSnapshot {
                content_shape: ingress.content_shape(&input_id),
                request_id: ingress.request_id(&input_id),
                reservation_key: ingress.reservation_key(&input_id),
                handling_mode: ingress.handling_mode(&input_id),
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
                let lifecycle = driver
                    .input_phase(input_id)
                    .unwrap_or(InputLifecycleState::Accepted);
                if !lifecycle.is_terminal() {
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
        let ops_snapshot = ops_lifecycle.diagnostic_snapshot();
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
