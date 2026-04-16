use super::*;

impl MeerkatMachine {
    pub(super) async fn register_session_inner(&self, session_id: SessionId) {
        {
            let mut sessions = self.sessions.write().await;
            if let Some(existing) = sessions.get_mut(&session_id) {
                existing.clear_dead_attachment();
                return;
            }
        }

        let mut entry = self.make_driver(&session_id);
        if let Err(err) = entry.as_driver_mut().recover().await {
            tracing::error!(%session_id, error = %err, "failed to recover runtime driver during registration");
            return;
        }

        let (ops_lifecycle, epoch_id, cursor_state) =
            self.recover_or_create_ops_state(&session_id).await;
        let control_projection = entry.control_projection_handle();
        let session_entry = RuntimeSessionEntry {
            control_projection,
            driver: Arc::new(Mutex::new(entry)),
            ops_lifecycle,
            epoch_id,
            cursor_state,
            completions: Arc::new(Mutex::new(crate::completion::CompletionRegistry::new())),
            tool_visibility_owner: Arc::new(MachineToolVisibilityOwner::new()),
            current_llm_identity: None,
            current_capability_surface: None,
            capability_surface_status: SessionLlmCapabilitySurfaceStatus::Unresolved,
            phase: RegistrationPhase::Queuing,
            detached_wake: None,
            dsl_authority: super::dsl::MeerkatMachineAuthority::new(),
        };
        let mut sessions = self.sessions.write().await;
        if let Some(existing) = sessions.get_mut(&session_id) {
            existing.clear_dead_attachment();
        } else {
            sessions.insert(session_id, session_entry);
        }
    }

    /// Set the silent comms intents for a session's runtime driver.
    ///
    /// Peer requests whose intent matches one of these strings will be accepted
    /// without triggering an LLM turn (ApplyMode::Ignore, WakeMode::None).
    pub async fn set_session_silent_intents(&self, session_id: &SessionId, intents: Vec<String>) {
        let _ = self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::SetSilentIntents {
                    session_id: session_id.clone(),
                    intents,
                },
            )
            .await;
    }

    pub(super) async fn set_session_silent_intents_inner(
        &self,
        session_id: &SessionId,
        intents: Vec<String>,
    ) {
        let sessions = self.sessions.read().await;
        if let Some(entry) = sessions.get(session_id) {
            let mut driver = entry.driver.lock().await;
            driver.set_silent_comms_intents(intents);
        }
    }

    /// Register a runtime driver for a session WITH a RuntimeLoop backed by a
    /// `CoreExecutor`. When `accept_input()` queues an input and requests wake,
    /// the loop dequeues it and calls `executor.apply()` (which triggers
    /// `SessionService::start_turn()`).
    pub async fn register_session_with_executor(
        &self,
        session_id: SessionId,
        executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    ) {
        let _ = self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::EnsureSessionWithExecutor {
                    session_id,
                    executor,
                },
            )
            .await;
    }

    /// Ensure a runtime driver with executor exists for the session.
    ///
    /// If a session was already registered without a loop, upgrade the
    /// existing driver in place so queued inputs remain attached to the same
    /// runtime ledger and can start draining immediately.
    pub async fn ensure_session_with_executor(
        &self,
        session_id: SessionId,
        executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    ) {
        let _ = self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::EnsureSessionWithExecutor {
                    session_id,
                    executor,
                },
            )
            .await;
    }

    pub(super) async fn ensure_session_with_executor_inner(
        &self,
        session_id: SessionId,
        executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    ) {
        let existing = {
            let mut sessions = self.sessions.write().await;
            sessions.get_mut(&session_id).map(|entry| {
                entry.clear_dead_attachment();
                let occupied = entry.has_attachment_or_attaching();
                if !occupied {
                    // Claim the attachment slot so concurrent callers see
                    // Attaching and return early instead of racing a second
                    // loop spawn (which would cross-wire detached-wake state).
                    entry.phase = RegistrationPhase::Attaching;
                }
                (
                    occupied,
                    entry.driver.clone(),
                    entry.completions.clone(),
                    entry.ops_lifecycle.clone(),
                )
            })
        };

        let (driver, completions, ops_lifecycle) =
            if let Some((has_attachment, driver, completions, ops_lifecycle)) = existing {
                if has_attachment {
                    return;
                }
                (driver, completions, ops_lifecycle)
            } else {
                let mut recovered_entry = self.make_driver(&session_id);
                if let Err(err) = recovered_entry.as_driver_mut().recover().await {
                    tracing::error!(
                        %session_id,
                        error = %err,
                        "failed to recover runtime driver during registration"
                    );
                    return;
                }

                // Recover ops state OUTSIDE the sessions lock to avoid blocking
                // other adapter operations behind potentially slow disk I/O.
                let (recovered_ops, recovered_epoch, recovered_cursors) =
                    self.recover_or_create_ops_state(&session_id).await;

                // Double-check under the lock — another task may have inserted
                // the entry while we were rebuilding runtime state.
                let mut sessions = self.sessions.write().await;
                if let Some(entry) = sessions.get_mut(&session_id) {
                    entry.clear_dead_attachment();
                    if entry.has_attachment_or_attaching() {
                        return;
                    }
                    entry.phase = RegistrationPhase::Attaching;
                    (
                        entry.driver.clone(),
                        entry.completions.clone(),
                        entry.ops_lifecycle.clone(),
                    )
                } else {
                    let control_projection = recovered_entry.control_projection_handle();
                    let driver = Arc::new(Mutex::new(recovered_entry));
                    let completions =
                        Arc::new(Mutex::new(crate::completion::CompletionRegistry::new()));
                    sessions.insert(
                        session_id.clone(),
                        RuntimeSessionEntry {
                            control_projection,
                            driver: driver.clone(),
                            ops_lifecycle: recovered_ops.clone(),
                            epoch_id: recovered_epoch,
                            cursor_state: recovered_cursors,
                            completions: completions.clone(),
                            tool_visibility_owner: Arc::new(MachineToolVisibilityOwner::new()),
                            current_llm_identity: None,
                            current_capability_surface: None,
                            capability_surface_status:
                                SessionLlmCapabilitySurfaceStatus::Unresolved,
                            phase: RegistrationPhase::Queuing,
                            detached_wake: None,
                            dsl_authority: super::dsl::MeerkatMachineAuthority::new(),
                        },
                    );
                    (driver, completions, recovered_ops)
                }
            };

        let should_wake = {
            let mut driver_guard = driver.lock().await;
            match machine_executor_attach_projection(&mut driver_guard) {
                Ok(true) => {}
                Ok(false) => {
                    tracing::warn!(
                        %session_id,
                        "runtime driver remained attached without a live published loop; republishing attachment"
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        %session_id,
                        error = %error,
                        "failed to attach runtime driver before publishing loop attachment"
                    );
                    self.revert_attaching(&session_id).await;
                    return;
                }
            }
            !driver_guard.as_driver().active_input_ids().is_empty()
        };

        // Wire detached-op wake state: the ops lifecycle registry will set
        // `pending = true` and fire `notify` when a BackgroundToolOp reaches
        // terminal. The waker task (spawned after attachment below) then injects
        // a continuation through the canonical ingress seam.
        let detached_wake_state = Arc::new(crate::detached_wake::DetachedWakeState::new());
        ops_lifecycle.set_detached_wake(Arc::clone(&detached_wake_state));

        // Wire persistence channel if a durable store is available.
        if let Some(ref store) = self.store {
            let (persist_tx, mut persist_rx) =
                crate::tokio::sync::mpsc::channel::<crate::ops_lifecycle::PersistedOpsSnapshot>(16);
            let entry_epoch_id = {
                let sessions = self.sessions.read().await;
                sessions
                    .get(&session_id)
                    .map(|e| e.epoch_id.clone())
                    .unwrap_or_else(meerkat_core::RuntimeEpochId::new)
            };
            let entry_cursor = {
                let sessions = self.sessions.read().await;
                sessions
                    .get(&session_id)
                    .map(|e| Arc::clone(&e.cursor_state))
                    .unwrap_or_else(|| Arc::new(meerkat_core::EpochCursorState::new()))
            };
            ops_lifecycle.set_persistence_channel(persist_tx, entry_epoch_id, entry_cursor);

            // Spawn persistence task
            let store_clone = Arc::clone(store);
            let runtime_id = crate::identifiers::LogicalRuntimeId::new(session_id.to_string());
            crate::tokio::spawn(async move {
                while let Some(snapshot) = persist_rx.recv().await {
                    if let Err(e) = store_clone
                        .persist_ops_lifecycle(&runtime_id, &snapshot)
                        .await
                    {
                        tracing::warn!(
                            error = %e,
                            "failed to persist ops lifecycle snapshot"
                        );
                    }
                }
            });
        }

        // Get the completion feed from the registry for feed-based idle wake.
        let completion_feed = ops_lifecycle.completion_feed_handle();

        let (wake_tx, wake_rx) = mpsc::channel(16);
        let (control_tx, control_rx) = mpsc::channel(16);
        let entry_cursor_state = {
            let sessions = self.sessions.read().await;
            sessions
                .get(&session_id)
                .map(|e| Arc::clone(&e.cursor_state))
        };
        let mut pending_loop_handle =
            Some(crate::runtime_loop::spawn_runtime_loop_with_completions(
                driver.clone(),
                executor,
                wake_rx,
                control_rx,
                Some(completions.clone()),
                Some(Arc::clone(&detached_wake_state)),
                Some(completion_feed),
                entry_cursor_state,
            ));

        let (published, detach_after_abort) = {
            let mut sessions = self.sessions.write().await;
            match sessions.get_mut(&session_id) {
                None => (false, true),
                Some(entry) => {
                    entry.clear_dead_attachment();
                    if entry.has_live_attachment() {
                        (false, false)
                    } else if !Arc::ptr_eq(&entry.driver, &driver)
                        || !Arc::ptr_eq(&entry.completions, &completions)
                    {
                        tracing::warn!(
                            %session_id,
                            "runtime session entry changed while wiring executor; aborting stale loop attachment"
                        );
                        (false, true)
                    } else {
                        match pending_loop_handle.take() {
                            Some(loop_handle) => {
                                entry.attach_runtime_loop(wake_tx.clone(), control_tx, loop_handle);
                                entry.detached_wake = Some(Arc::clone(&detached_wake_state));
                                (true, false)
                            }
                            None => {
                                tracing::error!(
                                    %session_id,
                                    "runtime loop handle missing during attachment publish"
                                );
                                (false, true)
                            }
                        }
                    }
                }
            }
        };

        if !published {
            if let Some(loop_handle) = pending_loop_handle.take() {
                loop_handle.abort();
            }
            if detach_after_abort {
                let mut driver_guard = driver.lock().await;
                machine_unregister_session_projection(&mut driver_guard);
            }
            self.revert_attaching(&session_id).await;
            return;
        }

        if should_wake {
            let _ = wake_tx.try_send(());
        }
    }

    /// Revert `Attaching → Queuing` if attachment failed. This unblocks
    /// future `ensure_session_with_executor` callers that would otherwise
    /// see `Attaching` forever and return early.
    pub(super) async fn revert_attaching(&self, session_id: &SessionId) {
        let mut sessions = self.sessions.write().await;
        if let Some(entry) = sessions.get_mut(session_id)
            && matches!(entry.phase, RegistrationPhase::Attaching)
        {
            entry.phase = RegistrationPhase::Queuing;
        }
    }

    /// Unregister a session's runtime driver.
    ///
    /// Detaches the executor (Attached → Idle) before removal, then drops
    /// the wake channel sender, which causes the RuntimeLoop to exit.
    pub async fn unregister_session(&self, session_id: &SessionId) {
        let _ = self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::UnregisterSession {
                    session_id: session_id.clone(),
                },
            )
            .await;
    }

    pub(super) async fn unregister_session_inner(&self, session_id: &SessionId) {
        let entry = {
            let mut sessions = self.sessions.write().await;
            let mut slots = self.comms_drain_slots.write().await;
            // Remove + abort drain slot before dropping session binding so
            // slot keys remain a subset of registered-session keys.
            if let Some(mut slot) = slots.remove(session_id) {
                abort_slot(&mut slot);
            }
            sessions.remove(session_id)
        };

        if let Some(entry) = entry {
            let mut driver = entry.driver.lock().await;
            machine_unregister_session_projection(&mut driver);
            drop(driver);

            let mut completions = entry.completions.lock().await;
            completions.resolve_all_terminated("runtime session unregistered");
        }
    }

    /// Check whether a runtime driver is already registered for a session.
    pub async fn contains_session(&self, session_id: &SessionId) -> bool {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::ContainsSession {
                    session_id: session_id.clone(),
                },
            )
            .await
        {
            Ok(MeerkatMachineCommandResult::Bool(present)) => present,
            Ok(_) => {
                tracing::error!("contains_session: unexpected command result variant");
                false
            }
            Err(_) => false,
        }
    }

    /// Check whether a session has an active RuntimeLoop or attachment in
    /// progress. Returns `false` only for `Queuing` sessions (registered via
    /// `prepare_bindings()` with no executor) and unknown sessions.
    pub async fn session_has_executor(&self, session_id: &SessionId) -> bool {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::SessionHasExecutor {
                    session_id: session_id.clone(),
                },
            )
            .await
        {
            Ok(MeerkatMachineCommandResult::Bool(present)) => present,
            Ok(_) => {
                tracing::error!("session_has_executor: unexpected command result variant");
                false
            }
            Err(_) => false,
        }
    }

    /// Check whether a session already has a comms runtime configured.
    ///
    /// Returns `true` if `update_peer_ingress_context` was previously called
    /// with a non-None comms runtime for this session (e.g., via
    /// `SessionRuntime::enable_comms_drain`).
    pub async fn session_has_comms(&self, session_id: &SessionId) -> bool {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::SessionHasComms {
                    session_id: session_id.clone(),
                },
            )
            .await
        {
            Ok(MeerkatMachineCommandResult::Bool(present)) => present,
            Ok(_) => {
                tracing::error!("session_has_comms: unexpected command result variant");
                false
            }
            Err(_) => false,
        }
    }

    /// Cancel the currently-running turn for a registered session.
    pub async fn interrupt_current_run(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        self.execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::InterruptCurrentRun {
                session_id: session_id.clone(),
            },
        )
        .await
        .map_err(MeerkatMachine::driver_error_from_command_error)
        .map(|_| ())
    }

    /// Request cancellation at the next safe boundary for the currently-running turn.
    pub async fn cancel_after_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        self.execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::CancelAfterBoundary {
                session_id: session_id.clone(),
            },
        )
        .await
        .map_err(MeerkatMachine::driver_error_from_command_error)
        .map(|_| ())
    }

    /// Stage a durable session visibility filter through the machine-owned visibility state.
    pub async fn stage_persistent_filter(
        &self,
        session_id: &SessionId,
        filter: meerkat_core::ToolFilter,
        witnesses: std::collections::BTreeMap<String, meerkat_core::ToolVisibilityWitness>,
    ) -> Result<meerkat_core::ToolScopeRevision, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::StagePersistentFilter {
                    session_id: session_id.clone(),
                    filter,
                    witnesses,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::VisibilityRevision(revision) => Ok(revision),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for stage_persistent_filter: {other:?}"
            ))),
        }
    }

    /// Record durable deferred-tool visibility intent through the machine seam.
    pub async fn request_deferred_tools(
        &self,
        session_id: &SessionId,
        names: std::collections::BTreeSet<String>,
        witnesses: std::collections::BTreeMap<String, meerkat_core::ToolVisibilityWitness>,
    ) -> Result<meerkat_core::ToolScopeRevision, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::RequestDeferredTools {
                    session_id: session_id.clone(),
                    names,
                    witnesses,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::VisibilityRevision(revision) => Ok(revision),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for request_deferred_tools: {other:?}"
            ))),
        }
    }

    /// Publish the committed visible tool set through the machine dispatch.
    ///
    /// Routes the visibility publication through the canonical command path,
    /// enforcing session-existence and Destroyed guards per the TLA+
    /// `VisibleSurfacesMatchAppliedStateInvariant`.
    ///
    /// Returns the validated visibility state on success.
    pub async fn publish_committed_visible_set(
        &self,
        session_id: &SessionId,
        visibility_state: meerkat_core::SessionToolVisibilityState,
    ) -> Result<meerkat_core::SessionToolVisibilityState, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::PublishCommittedVisibleSet {
                    session_id: session_id.clone(),
                    visibility_state: Box::new(visibility_state),
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::VisibilityPublished(state) => Ok(state),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for publish_committed_visible_set: {other:?}"
            ))),
        }
    }

    /// Install the runtime-owned shell seam for live LLM reconfiguration.
    pub fn set_session_llm_reconfigure_host(&self, host: Arc<dyn SessionLlmReconfigureHost>) {
        *self
            .llm_reconfigure_host
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(host);
    }
}
