use super::*;

#[derive(Debug, Default)]
pub struct MachineToolVisibilityOwner {
    pub state: StdRwLock<SessionToolVisibilityState>,
    pub next_revision: AtomicU64,
}

impl MachineToolVisibilityOwner {
    pub fn new() -> Self {
        Self::default()
    }
}

pub fn formal_projection_value<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|err| format!("\"<serialization error: {err}>\""))
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
        let mut state = self.state.write().map_err(|_| ToolScopeApplyError::Owner {
            message: "machine visibility state lock poisoned".to_string(),
        })?;
        let next_revision = visibility_state
            .active_revision
            .max(visibility_state.staged_revision);
        *state = visibility_state;
        self.next_revision.store(next_revision, Ordering::SeqCst);
        Ok(())
    }

    fn stage_persistent_filter(
        &self,
        filter: ToolFilter,
        witnesses: std::collections::BTreeMap<String, ToolVisibilityWitness>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        let mut state = self.state.write().map_err(|_| ToolScopeStageError::Owner {
            message: "machine visibility state lock poisoned".to_string(),
        })?;
        let revision = ToolScopeRevision(self.next_revision.fetch_add(1, Ordering::SeqCst) + 1);
        state.staged_filter = filter;
        state.filter_witnesses.extend(witnesses);
        state.staged_revision = revision.0;
        Ok(revision)
    }

    fn stage_requested_deferred_names(
        &self,
        names: std::collections::BTreeSet<String>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        let mut state = self.state.write().map_err(|_| ToolScopeStageError::Owner {
            message: "machine visibility state lock poisoned".to_string(),
        })?;
        let revision = ToolScopeRevision(self.next_revision.fetch_add(1, Ordering::SeqCst) + 1);
        state.staged_requested_deferred_names = names;
        state.staged_revision = revision.0;
        Ok(revision)
    }

    fn request_deferred_tools(
        &self,
        names: std::collections::BTreeSet<String>,
        witnesses: std::collections::BTreeMap<String, ToolVisibilityWitness>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        let mut state = self.state.write().map_err(|_| ToolScopeStageError::Owner {
            message: "machine visibility state lock poisoned".to_string(),
        })?;
        let revision = ToolScopeRevision(self.next_revision.fetch_add(1, Ordering::SeqCst) + 1);
        state.staged_requested_deferred_names.extend(names);
        state.requested_witnesses.extend(witnesses);
        state.staged_revision = revision.0;
        Ok(revision)
    }

    fn boundary_applied(&self) -> Result<SessionToolVisibilityState, ToolScopeApplyError> {
        let mut state = self.state.write().map_err(|_| ToolScopeApplyError::Owner {
            message: "machine visibility state lock poisoned".to_string(),
        })?;
        state.active_filter = state.staged_filter.clone();
        state.active_requested_deferred_names = state.staged_requested_deferred_names.clone();
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
            detached_wake_pending,
            detached_wake_signaled,
            epoch_id,
            _visibility_state,
        ) = {
            let sessions = self.sessions.read().await;
            let entry = sessions.get(session_id)?;
            let (detached_wake_pending, detached_wake_signaled) = entry
                .detached_wake
                .as_ref()
                .map(|wake| {
                    (
                        wake.pending.load(Ordering::Acquire),
                        wake.signaled.load(Ordering::Acquire),
                    )
                })
                .map_or((None, None), |(pending, signaled)| {
                    (Some(pending), Some(signaled))
                });
            (
                Arc::clone(&entry.driver),
                entry.control_snapshot(),
                Arc::clone(&entry.completions),
                Arc::clone(&entry.ops_lifecycle),
                Arc::clone(&entry.cursor_state),
                true,
                true,
                entry.attachment_is_live(),
                detached_wake_pending,
                detached_wake_signaled,
                entry.epoch_id.clone(),
                entry.tool_visibility_owner.visibility_state().ok()?,
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
            let slots = self.comms_drain_slots.read().await;
            if let Some(slot) = slots.get(session_id) {
                MeerkatDrainSnapshot {
                    slot_present: true,
                    phase: Some(slot.authority.phase()),
                    mode: slot.authority.mode(),
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
        };
        let driver = driver_handle.lock().await;
        let driver_kind = match &*driver {
            DriverEntry::Ephemeral(_) => MeerkatDriverKind::Ephemeral,
            DriverEntry::Persistent(_) => MeerkatDriverKind::Persistent,
        };
        let ingress = driver.ingress();

        let binding = MeerkatBindingSnapshot {
            session_id: session_id.clone(),
            runtime_id: driver.runtime_id().clone(),
            driver_kind,
            driver_present: true,
            completions_present,
            ops_registry_present,
            attachment_live,
            detached_wake_present: detached_wake_pending.is_some(),
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
            .map(|input_id| {
                let ledger_state = driver.ledger().get(&input_id);
                MeerkatAdmittedInputSnapshot {
                    content_shape: ingress.content_shape(&input_id),
                    request_id: ingress.request_id(&input_id),
                    reservation_key: ingress.reservation_key(&input_id),
                    handling_mode: ingress.handling_mode(&input_id),
                    lifecycle: ledger_state.map(crate::input_state::InputState::current_state),
                    terminal_outcome: ledger_state
                        .and_then(|state| state.terminal_outcome().cloned()),
                    last_run_id: ledger_state.and_then(|state| state.last_run_id().cloned()),
                    last_boundary_sequence: ledger_state
                        .and_then(crate::input_state::InputState::last_boundary_sequence),
                    is_prompt: ingress.is_prompt(&input_id),
                    input_id,
                }
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
            queue: ingress.queue().to_vec(),
            steer_queue: ingress.steer_queue().to_vec(),
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

            for (_input_id, state) in driver.ledger().iter() {
                snapshot.input_count += 1;
                let lifecycle = state.current_state();
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
            detached_wake_pending,
            detached_wake_signaled,
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
                formal_projection_value(&control.pre_run_phase.map(|phase| phase.to_string())),
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
