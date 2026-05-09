use super::*;

type OpsLifecyclePersistenceReceiver = crate::tokio::sync::mpsc::UnboundedReceiver<
    crate::ops_lifecycle::OpsLifecyclePersistenceRequest,
>;

async fn persist_ops_lifecycle_request(
    store: &Arc<dyn RuntimeStore>,
    runtime_id: &LogicalRuntimeId,
    request: crate::ops_lifecycle::OpsLifecyclePersistenceRequest,
) {
    let result = store
        .persist_ops_lifecycle(runtime_id, request.snapshot())
        .await
        .map_err(|error| {
            meerkat_core::ops_lifecycle::OpsLifecycleError::Internal(format!(
                "failed to persist ops lifecycle snapshot: {error}"
            ))
        });
    if let Err(error) = &result {
        tracing::warn!(
            %runtime_id,
            error = %error,
            "failed to persist ops lifecycle snapshot"
        );
    }
    request.complete(result);
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_ops_lifecycle_persistence_worker(
    store: Arc<dyn RuntimeStore>,
    runtime_id: LogicalRuntimeId,
    mut persist_rx: OpsLifecyclePersistenceReceiver,
) {
    let thread_name = format!("ops-lifecycle-persist-{runtime_id}");
    let worker_runtime_id = runtime_id.clone();
    let spawn_result = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let runtime = match crate::tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    tracing::error!(
                        %worker_runtime_id,
                        error = %error,
                        "failed to start ops lifecycle persistence worker runtime"
                    );
                    return;
                }
            };
            runtime.block_on(async move {
                while let Some(request) = persist_rx.recv().await {
                    persist_ops_lifecycle_request(&store, &worker_runtime_id, request).await;
                }
            });
        });
    if let Err(error) = spawn_result {
        tracing::error!(
            %runtime_id,
            error = %error,
            "failed to spawn ops lifecycle persistence worker"
        );
    }
}

#[cfg(target_arch = "wasm32")]
fn spawn_ops_lifecycle_persistence_worker(
    store: Arc<dyn RuntimeStore>,
    runtime_id: LogicalRuntimeId,
    mut persist_rx: OpsLifecyclePersistenceReceiver,
) {
    crate::tokio::spawn(async move {
        while let Some(request) = persist_rx.recv().await {
            persist_ops_lifecycle_request(&store, &runtime_id, request).await;
        }
    });
}

impl MeerkatMachine {
    async fn durable_runtime_state_for_registration(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<RuntimeState>, RuntimeDriverError> {
        let Some(store) = self.store.as_ref() else {
            return Ok(None);
        };
        store
            .load_runtime_state(runtime_id)
            .await
            .map_err(|err| RuntimeDriverError::Internal(err.to_string()))
    }

    pub(super) async fn register_session_inner(
        &self,
        session_id: SessionId,
    ) -> Result<bool, RuntimeDriverError> {
        {
            let mut sessions = self.sessions.write().await;
            if let Some(existing) = sessions.get_mut(&session_id) {
                existing.clear_dead_attachment();
                return Ok(false);
            }
        }

        let runtime_id = Self::logical_runtime_id(&session_id);
        let durable_runtime_state = self
            .durable_runtime_state_for_registration(&runtime_id)
            .await?;
        let dsl_authority = Arc::new(std::sync::Mutex::new(
            super::dsl::MeerkatMachineAuthority::from_state(super::dsl_authority::project_state(
                &session_id,
                durable_runtime_state.unwrap_or(RuntimeState::Idle),
                None,
                None,
                None,
                std::collections::BTreeSet::new(),
                None,
            )),
        ));
        let initial_runtime_state = durable_runtime_state.unwrap_or(RuntimeState::Idle);
        let mut entry = self.make_driver(
            runtime_id.clone(),
            Arc::clone(&dsl_authority),
            initial_runtime_state,
        );
        if let Err(err) = entry.as_driver_mut().recover().await {
            tracing::error!(%session_id, error = %err, "failed to recover runtime driver during registration");
            return Err(err);
        }
        let control_projection = entry.control_projection_handle();

        let (ops_lifecycle, epoch_id, cursor_state) = self
            .recover_or_create_ops_state(&session_id, &runtime_id)
            .await;

        let tool_visibility_owner = Arc::new(MachineToolVisibilityOwner::new());
        // Bind the DSL authority into the visibility owner so its staging
        // trait calls route through the canonical DSL counter
        // `next_staged_visibility_revision` (dogma round 4, wave 2b #12).
        tool_visibility_owner.bind_dsl_authority(Arc::clone(&dsl_authority));
        let session_entry = RuntimeSessionEntry {
            runtime_id,
            mutation_gate: Arc::new(Mutex::new(())),
            control_projection,
            driver: Arc::new(Mutex::new(entry)),
            ops_lifecycle,
            epoch_id,
            cursor_state,
            completions: Arc::new(Mutex::new(crate::completion::CompletionRegistry::new())),
            tool_visibility_owner,
            current_llm_identity: None,
            current_capability_surface: None,
            capability_surface_status: SessionLlmCapabilitySurfaceStatus::Unresolved,
            phase: RegistrationPhase::Queuing,
            provisional_interrupt_handle: None,
            dsl_authority,
            drain_slot: CommsDrainSlot::new(),
        };
        let mut sessions = self.sessions.write().await;
        if let Some(existing) = sessions.get_mut(&session_id) {
            existing.clear_dead_attachment();
            Ok(false)
        } else {
            sessions.insert(session_id, session_entry);
            Ok(true)
        }
    }

    pub(super) async fn unregister_session_inner_if_epoch(
        &self,
        session_id: &SessionId,
        epoch_id: &meerkat_core::RuntimeEpochId,
    ) {
        let entry = {
            let mut sessions = self.sessions.write().await;
            let should_unregister = sessions
                .get(session_id)
                .is_some_and(|entry| &entry.epoch_id == epoch_id);
            if should_unregister {
                if let Some(entry) = sessions.get_mut(session_id) {
                    abort_slot(&mut entry.drain_slot);
                }
                sessions.remove(session_id)
            } else {
                None
            }
        };

        if let Some(entry) = entry {
            self.finalize_unregistered_session(entry).await;
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

    pub async fn commit_service_turn_terminal_receipt(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::CommitServiceTurnTerminalReceipt {
                    session_id: session_id.clone(),
                },
            )
            .await
            .map_err(|err| match err {
                MeerkatMachineCommandError::Driver(err) => err,
                MeerkatMachineCommandError::Control(err) => {
                    RuntimeDriverError::Internal(err.to_string())
                }
            })? {
            MeerkatMachineCommandResult::Unit => Ok(()),
            _ => Err(RuntimeDriverError::Internal(
                "commit_service_turn_terminal_receipt: unexpected command result variant".into(),
            )),
        }
    }

    /// Register a runtime driver for a session WITH a RuntimeLoop backed by a
    /// `CoreExecutor`. Takes `self: &Arc<Self>` because executor attachment is
    /// routed through the Arc-backed command path that owns runtime-loop spawn.
    pub async fn register_session_with_executor(
        self: &Arc<Self>,
        session_id: SessionId,
        executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    ) {
        let _ = self
            .execute_meerkat_machine_command(
                Some(Arc::clone(self)),
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
    /// runtime ledger and can start draining immediately. See
    /// `register_session_with_executor` for why this takes `self: &Arc<Self>`.
    pub async fn ensure_session_with_executor(
        self: &Arc<Self>,
        session_id: SessionId,
        executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    ) {
        let _ = self
            .execute_meerkat_machine_command(
                Some(Arc::clone(self)),
                MeerkatMachineCommand::EnsureSessionWithExecutor {
                    session_id,
                    executor,
                },
            )
            .await;
    }

    /// Install a temporary live interrupt handle for a prepared session before
    /// its runtime loop executor is attached.
    ///
    /// Runtime-backed surfaces use this during eager session materialization:
    /// the session service owns the first turn until `create_session` returns,
    /// but explicit user interrupts must still route through
    /// `MeerkatMachine::hard_cancel_current_run`.
    pub async fn install_prepared_session_interrupt_handle(
        &self,
        session_id: &SessionId,
        handle: Arc<dyn meerkat_core::lifecycle::CoreExecutorInterruptHandle>,
    ) -> Result<(), RuntimeDriverError> {
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        entry.clear_dead_attachment();
        entry.install_provisional_interrupt_handle(handle);
        Ok(())
    }

    pub(super) async fn ensure_session_with_executor_inner(
        self: &Arc<Self>,
        session_id: SessionId,
        executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    ) {
        let mut repaired_dead_attachment = false;
        let existing = {
            let mut sessions = self.sessions.write().await;
            sessions.get_mut(&session_id).map(|entry| {
                repaired_dead_attachment = entry.clear_dead_attachment();
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
                let runtime_id = Self::logical_runtime_id(&session_id);
                let durable_runtime_state = match self
                    .durable_runtime_state_for_registration(&runtime_id)
                    .await
                {
                    Ok(state) => state,
                    Err(err) => {
                        tracing::error!(
                            %session_id,
                            error = %err,
                            "failed to load durable runtime state during executor registration"
                        );
                        return;
                    }
                };
                let initial_runtime_state = durable_runtime_state.unwrap_or(RuntimeState::Idle);
                let dsl_authority = Arc::new(std::sync::Mutex::new(
                    super::dsl::MeerkatMachineAuthority::from_state(
                        super::dsl_authority::project_state(
                            &session_id,
                            initial_runtime_state,
                            None,
                            None,
                            None,
                            std::collections::BTreeSet::new(),
                            None,
                        ),
                    ),
                ));
                let mut recovered_entry = self.make_driver(
                    runtime_id.clone(),
                    Arc::clone(&dsl_authority),
                    initial_runtime_state,
                );
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
                let (recovered_ops, recovered_epoch, recovered_cursors) = self
                    .recover_or_create_ops_state(&session_id, &runtime_id)
                    .await;

                // Double-check under the lock — another task may have inserted
                // the entry while we were rebuilding runtime state.
                let mut sessions = self.sessions.write().await;
                if let Some(entry) = sessions.get_mut(&session_id) {
                    repaired_dead_attachment = entry.clear_dead_attachment();
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
                    let tool_visibility_owner = Arc::new(MachineToolVisibilityOwner::new());
                    // Bind the DSL authority before the entry is inserted — any
                    // subsequent staging trait call must see the bound authority.
                    tool_visibility_owner.bind_dsl_authority(Arc::clone(&dsl_authority));
                    sessions.insert(
                        session_id.clone(),
                        RuntimeSessionEntry {
                            runtime_id,
                            mutation_gate: Arc::new(Mutex::new(())),
                            control_projection,
                            driver: driver.clone(),
                            ops_lifecycle: recovered_ops.clone(),
                            epoch_id: recovered_epoch,
                            cursor_state: recovered_cursors,
                            completions: completions.clone(),
                            tool_visibility_owner,
                            current_llm_identity: None,
                            current_capability_surface: None,
                            capability_surface_status:
                                SessionLlmCapabilitySurfaceStatus::Unresolved,
                            phase: RegistrationPhase::Queuing,
                            provisional_interrupt_handle: None,
                            dsl_authority,
                            drain_slot: CommsDrainSlot::new(),
                        },
                    );
                    (driver, completions, recovered_ops)
                }
            };

        // Stage the DSL EnsureSessionWithExecutor transition BEFORE mutating
        // the driver, so a DSL rejection never leaves shell and DSL disagreeing.
        // Session entry exists by this point (inner created it above).
        if let Err(reason) = self
            .stage_session_dsl_input(
                &session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::EnsureSessionWithExecutor {
                    session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&session_id),
                },
                "EnsureSessionWithExecutor",
            )
            .await
        {
            tracing::warn!(
                %session_id,
                error = %reason,
                "DSL rejected EnsureSessionWithExecutor; aborting attach"
            );
            self.revert_attaching(&session_id).await;
            return;
        }

        let should_wake = {
            let mut driver_guard = driver.lock().await;
            match machine_executor_attach_projection(&mut driver_guard) {
                Ok(true) => {}
                Ok(false) => {
                    if repaired_dead_attachment {
                        tracing::warn!(
                            %session_id,
                            "runtime driver remained attached without a live published loop; republishing attachment"
                        );
                    } else {
                        tracing::debug!(
                            %session_id,
                            "runtime driver already attached before live loop publication; publishing attachment"
                        );
                    }
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

        // Wire persistence channel if a durable store is available.
        if let Some(ref store) = self.store {
            let (persist_tx, persist_rx) = crate::tokio::sync::mpsc::unbounded_channel::<
                crate::ops_lifecycle::OpsLifecyclePersistenceRequest,
            >();
            let (entry_epoch_id, entry_cursor, runtime_id) = {
                let sessions = self.sessions.read().await;
                sessions.get(&session_id).map_or_else(
                    || {
                        (
                            meerkat_core::RuntimeEpochId::new(),
                            Arc::new(meerkat_core::EpochCursorState::new()),
                            Self::logical_runtime_id(&session_id),
                        )
                    },
                    |entry| {
                        (
                            entry.epoch_id.clone(),
                            Arc::clone(&entry.cursor_state),
                            entry.runtime_id.clone(),
                        )
                    },
                )
            };
            spawn_ops_lifecycle_persistence_worker(Arc::clone(store), runtime_id, persist_rx);
            ops_lifecycle.set_persistence_channel(persist_tx, entry_epoch_id, entry_cursor);
        }

        // Get the completion feed from the registry for feed-based idle wake.
        let completion_feed = ops_lifecycle.completion_feed_handle();

        let boundary_handle = executor.boundary_handle();
        let interrupt_handle = executor.interrupt_handle();
        let (wake_tx, wake_rx) = mpsc::channel(16);
        let (effect_tx, effect_rx) = mpsc::channel(16);
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
                effect_rx,
                Some(completions.clone()),
                Some(completion_feed),
                entry_cursor_state,
                Arc::downgrade(self),
                session_id.clone(),
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
                                entry.attach_runtime_loop(
                                    wake_tx.clone(),
                                    effect_tx,
                                    boundary_handle,
                                    interrupt_handle,
                                    loop_handle,
                                );
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

    async fn finalize_unregistered_session(&self, entry: RuntimeSessionEntry) {
        let mut driver = entry.driver.lock().await;
        machine_unregister_session_projection(&mut driver);
        drop(driver);

        let mut completions = entry.completions.lock().await;
        completions.resolve_all_terminated("runtime session unregistered");
    }

    pub(super) async fn unregister_session_inner(&self, session_id: &SessionId) {
        let entry = {
            let mut sessions = self.sessions.write().await;
            // Abort the drain slot inline before removing the entry — the
            // slot is now owned by the entry itself (wave-c C-H2), so the
            // "slot keys are a subset of registered-session keys" invariant
            // is structural rather than enforced by ordering.
            if let Some(entry) = sessions.get_mut(session_id) {
                abort_slot(&mut entry.drain_slot);
            }
            sessions.remove(session_id)
        };

        if let Some(entry) = entry {
            self.finalize_unregistered_session(entry).await;
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

    /// Wake the attached runtime loop when machine-owned input truth already
    /// contains active work. This does not mutate lifecycle state; it only
    /// replays the mechanical wake effect for callers that observe queued work
    /// at a boundary where user input must wait for canonical runtime work to
    /// drain.
    pub async fn wake_runtime_if_active_inputs(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, RuntimeDriverError> {
        let (driver, wake_tx) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            (entry.driver.clone(), entry.wake_sender())
        };

        let has_active_inputs = {
            let driver = driver.lock().await;
            !driver.as_driver().active_input_ids().is_empty()
        };
        if !has_active_inputs {
            return Ok(false);
        }

        let Some(wake_tx) = wake_tx else {
            return Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Idle,
            });
        };

        match wake_tx.try_send(()) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(())) => Ok(true),
            Err(mpsc::error::TrySendError::Closed(())) => Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Idle,
            }),
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
        authorities: Vec<meerkat_core::DeferredToolLoadAuthority>,
    ) -> Result<meerkat_core::ToolScopeRevision, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::RequestDeferredTools {
                    session_id: session_id.clone(),
                    authorities,
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

    // NOTE: Realtime-attachment public API was removed as part of
    // the realtime/live-topology DSL plane deletion.
    // Provider session lifecycle now lives outside MeerkatMachine (live-adapter MVP).
}
