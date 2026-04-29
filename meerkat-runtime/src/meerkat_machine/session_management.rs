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
    fn replay_recovered_runtime_phase_through_dsl_authority(
        session_id: &SessionId,
        dsl_authority: &Arc<std::sync::Mutex<super::dsl::MeerkatMachineAuthority>>,
        recovered_phase: RuntimeState,
    ) {
        // Cold-restart: when `recover()` realizes a stored terminal runtime
        // state on the driver, replay that fact through the DSL authority
        // before publishing or attaching the session entry. The shell
        // projection remains a persistence witness; it never directly seeds
        // `authority.state`.
        let authority_phase = {
            let authority = dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            super::dsl_authority::runtime_phase_from_authority(&authority)
        };
        if recovered_phase == RuntimeState::Idle || recovered_phase == authority_phase {
            return;
        }

        let input = match recovered_phase {
            RuntimeState::Retired => Some(super::dsl::MeerkatMachineInput::Retire {
                session_id: super::dsl::SessionId::from_domain(session_id),
            }),
            RuntimeState::Stopped => Some(super::dsl::MeerkatMachineInput::StopRuntimeExecutor),
            RuntimeState::Destroyed => Some(super::dsl::MeerkatMachineInput::Destroy {
                session_id: super::dsl::SessionId::from_domain(session_id),
            }),
            RuntimeState::Attached | RuntimeState::Running | RuntimeState::Initializing => None,
            RuntimeState::Idle => None,
        };
        let Some(input) = input else {
            return;
        };

        let mut authority = dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Err(err) = super::dsl::MeerkatMachineMutator::apply(&mut *authority, input) {
            tracing::error!(
                %session_id,
                ?recovered_phase,
                error = ?err,
                "failed to replay recovered runtime phase through DSL authority"
            );
        }
    }

    pub(super) async fn register_session_inner(&self, session_id: SessionId) {
        {
            let mut sessions = self.sessions.write().await;
            if let Some(existing) = sessions.get_mut(&session_id) {
                existing.clear_dead_attachment();
                return;
            }
        }

        let dsl_authority = Arc::new(std::sync::Mutex::new(
            super::dsl::MeerkatMachineAuthority::from_state(super::dsl_authority::project_state(
                &session_id,
                RuntimeState::Idle,
                None,
                None,
                None,
                std::collections::BTreeSet::new(),
                None,
            )),
        ));
        let mut entry = self.make_driver(&session_id, Arc::clone(&dsl_authority));
        if let Err(err) = entry.as_driver_mut().recover().await {
            tracing::error!(%session_id, error = %err, "failed to recover runtime driver during registration");
            return;
        }
        Self::replay_recovered_runtime_phase_through_dsl_authority(
            &session_id,
            &dsl_authority,
            entry.as_driver().runtime_state(),
        );
        let control_projection = entry.control_projection_handle();

        let (ops_lifecycle, epoch_id, cursor_state) =
            self.recover_or_create_ops_state(&session_id).await;

        let tool_visibility_owner = Arc::new(MachineToolVisibilityOwner::new());
        // Bind the DSL authority into the visibility owner so its staging
        // trait calls route through the canonical DSL counter
        // `next_staged_visibility_revision` (dogma round 4, wave 2b #12).
        tool_visibility_owner.bind_dsl_authority(Arc::clone(&dsl_authority));
        let session_entry = RuntimeSessionEntry {
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
            dsl_authority,
            drain_slot: CommsDrainSlot::new(),
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
    /// `CoreExecutor`. Takes `self: &Arc<Self>` so the spawned runtime loop
    /// can hold a `Weak<Self>` to fire `RuntimeExecutorExited` on async stop
    /// realisation.
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

    pub(super) async fn ensure_session_with_executor_inner(
        self: &Arc<Self>,
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
                let dsl_authority = Arc::new(std::sync::Mutex::new(
                    super::dsl::MeerkatMachineAuthority::from_state(
                        super::dsl_authority::project_state(
                            &session_id,
                            RuntimeState::Idle,
                            None,
                            None,
                            None,
                            std::collections::BTreeSet::new(),
                            None,
                        ),
                    ),
                ));
                let mut recovered_entry = self.make_driver(&session_id, Arc::clone(&dsl_authority));
                if let Err(err) = recovered_entry.as_driver_mut().recover().await {
                    tracing::error!(
                        %session_id,
                        error = %err,
                        "failed to recover runtime driver during registration"
                    );
                    return;
                }
                Self::replay_recovered_runtime_phase_through_dsl_authority(
                    &session_id,
                    &dsl_authority,
                    recovered_entry.as_driver().runtime_state(),
                );

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
                    let tool_visibility_owner = Arc::new(MachineToolVisibilityOwner::new());
                    // Bind the DSL authority before the entry is inserted — any
                    // subsequent staging trait call must see the bound authority.
                    tool_visibility_owner.bind_dsl_authority(Arc::clone(&dsl_authority));
                    sessions.insert(
                        session_id.clone(),
                        RuntimeSessionEntry {
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

        // Wire persistence channel if a durable store is available.
        if let Some(ref store) = self.store {
            let (persist_tx, persist_rx) = crate::tokio::sync::mpsc::unbounded_channel::<
                crate::ops_lifecycle::OpsLifecyclePersistenceRequest,
            >();
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
            let runtime_id = crate::identifiers::LogicalRuntimeId::new(session_id.to_string());
            spawn_ops_lifecycle_persistence_worker(Arc::clone(store), runtime_id, persist_rx);
            ops_lifecycle.set_persistence_channel(persist_tx, entry_epoch_id, entry_cursor);
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
                                entry.attach_runtime_loop(wake_tx.clone(), control_tx, loop_handle);
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

        // Capability-driven realtime transport: after the session is attached,
        // consult the resolved model's ModelCapabilities.realtime and auto-
        // attach (or detach) the realtime transport. Closes the P2 session-init
        // seam — without this the capability is declared but no caller ever
        // invokes it at init time (dogma #19).
        //
        // Errors from this are logged, not propagated: session attach already
        // succeeded; a realtime-capability hiccup should not fail the attach.
        // The session's first turn or explicit reconfigure will retry.
        match self
            .apply_capability_driven_realtime_transport(&session_id)
            .await
        {
            Ok(_authority) => {}
            Err(err) => {
                tracing::debug!(
                    %session_id,
                    error = %err,
                    "capability-driven realtime transport at session init yielded no attachment; \
                     session will proceed without realtime binding"
                );
            }
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

    // =====================================================================
    // Realtime-attachment public API
    //
    // Each method applies the corresponding DSL input; the shell holds no
    // realtime-binding state. Authority epochs, reattach flags, and the
    // binding-state transitions all live in the MeerkatMachine DSL, and the
    // shell's job is pure dispatch + projecting the DSL's returned state
    // back into the token/status types the shell-facing API exposes.
    // =====================================================================

    /// Project durable live-attachment intent onto the session's DSL authority.
    pub async fn project_realtime_attachment_intent(
        &self,
        session_id: &SessionId,
        intent_present: bool,
    ) -> Result<(), RuntimeDriverError> {
        self.stage_session_dsl_input(
            session_id,
            dsl::MeerkatMachineInput::ProjectRealtimeIntent {
                present: intent_present,
            },
            "ProjectRealtimeIntent",
        )
        .await
        .map(|_| ())
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })
    }

    /// Wave-c C-9c R4: project the realtime-WS retry-machine progress
    /// (attempt count, next retry deadline) into DSL state so RPC/MCP
    /// `realtime/status` queries surface real retry-budget data instead of
    /// hard-coded `0`/`1` defaults.
    ///
    /// `next_retry_at_ms` / `deadline_at_ms` are milliseconds since the
    /// Unix epoch (shell converts from `chrono::DateTime<Utc>` at the
    /// emission site; the DSL stays chrono-free).
    pub async fn project_realtime_reconnect_progress(
        &self,
        session_id: &SessionId,
        attempt_count: u64,
        next_retry_at_ms: Option<u64>,
        deadline_at_ms: Option<u64>,
    ) -> Result<(), RuntimeDriverError> {
        self.stage_session_dsl_input(
            session_id,
            dsl::MeerkatMachineInput::ProjectRealtimeReconnectProgress {
                attempt_count,
                next_retry_at_ms,
                deadline_at_ms,
            },
            "ProjectRealtimeReconnectProgress",
        )
        .await
        .map(|_| ())
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })
    }

    /// Wave-c C-9c R4: clear the reconnect-progress fields. Shell fires
    /// this when the retry cycle ends — successful reconnect (DSL
    /// moves back to `BindingReady`) or operator detach.
    pub async fn clear_realtime_reconnect_progress(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        self.stage_session_dsl_input(
            session_id,
            dsl::MeerkatMachineInput::ClearRealtimeReconnectProgress,
            "ClearRealtimeReconnectProgress",
        )
        .await
        .map(|_| ())
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })
    }

    /// Begin a live attachment and return the authority token minted by the
    /// DSL transition. Subsequent provider callbacks present this token and
    /// the DSL validates their `authority_epoch` before mutating state.
    ///
    /// Shell precondition: the session must have a live executor binding
    /// (runtime state == Attached or Running). Callers hitting Idle sessions
    /// get `RuntimeDriverError::NotReady`, matching the pre-DSL contract used
    /// by the realtime attachment host.
    pub(crate) async fn attach_live(
        &self,
        session_id: &SessionId,
    ) -> Result<crate::meerkat_machine_types::RealtimeAttachmentSignalAuthority, RuntimeDriverError>
    {
        self.require_live_executor_for_realtime(session_id).await?;
        self.require_realtime_capable_model(session_id, "attach_live")
            .await?;
        self.stage_session_dsl_input(
            session_id,
            dsl::MeerkatMachineInput::BeginRealtimeBinding,
            "BeginRealtimeBinding",
        )
        .await
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        self.read_session_realtime_authority(session_id, "attach_live")
            .await
    }

    /// Replace the current live binding and mint a fresh authority token.
    pub async fn replace_realtime_attachment(
        &self,
        session_id: &SessionId,
    ) -> Result<crate::meerkat_machine_types::RealtimeAttachmentSignalAuthority, RuntimeDriverError>
    {
        self.require_live_executor_for_realtime(session_id).await?;
        self.require_realtime_capable_model(session_id, "replace_realtime_attachment")
            .await?;
        self.stage_session_dsl_input(
            session_id,
            dsl::MeerkatMachineInput::ReplaceRealtimeBinding,
            "ReplaceRealtimeBinding",
        )
        .await
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        self.read_session_realtime_authority(session_id, "replace_realtime_attachment")
            .await
    }

    /// Read the current runtime-owned realtime binding authority without
    /// changing attachment lifecycle state. Provider hosts use this after the
    /// capability-driven transport policy has already decided that a binding
    /// should exist.
    pub async fn current_realtime_attachment_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<crate::meerkat_machine_types::RealtimeAttachmentSignalAuthority, RuntimeDriverError>
    {
        self.read_session_realtime_authority(session_id, "current_realtime_attachment_authority")
            .await
    }

    async fn require_live_executor_for_realtime(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        if !entry.attachment_is_live() {
            return Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Idle,
            });
        }
        Ok(())
    }

    /// Reject realtime attach/replace for sessions whose current model does
    /// not carry `ModelCapabilities.realtime = true`. Session and realtime
    /// share one conversation; opening realtime on a non-realtime model
    /// would require a silent model substitution (dogma #1/#5 violation).
    ///
    /// Callers must `session/reconfigure_llm` to a realtime-capable model
    /// (e.g. `gpt-realtime`) before attach.
    async fn require_realtime_capable_model(
        &self,
        session_id: &SessionId,
        op: &'static str,
    ) -> Result<(), RuntimeDriverError> {
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        match &entry.current_capability_surface {
            Some(surface) if surface.realtime => Ok(()),
            Some(_) => {
                let model_name = entry
                    .current_llm_identity
                    .as_ref()
                    .map(|id| id.model.as_str())
                    .unwrap_or("<unknown>");
                Err(RuntimeDriverError::ValidationFailed {
                    reason: format!(
                        "{op}: session model '{model_name}' does not support realtime; reconfigure to a realtime-capable model (e.g. gpt-realtime) before attach"
                    ),
                })
            }
            // Unresolved capability surface: defer to the hydrate path. In
            // production, `apply_capability_driven_realtime_transport` (called
            // from session attach) hydrates before any realtime decision, so a
            // None here means the session has not yet reached the cap-resolve
            // seam — either this is a low-level test or an unusual timing
            // window. Both cases prefer a permissive attach over hard-failing;
            // the downstream provider request will still fail loudly if the
            // session model genuinely cannot open a realtime endpoint.
            None => Ok(()),
        }
    }

    /// Detach the runtime-owned binding. Preserves the durable intent bit so
    /// `realtime_attachment_status` can still project `IntentPresentUnbound`.
    pub(crate) async fn detach_live(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        self.stage_session_dsl_input(
            session_id,
            dsl::MeerkatMachineInput::DetachRealtimeBinding,
            "DetachRealtimeBinding",
        )
        .await
        .map(|_| ())
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })
    }

    /// Require the session to reattach live voice, discarding any outstanding
    /// authority token.
    pub async fn require_realtime_attachment_reattach(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        self.stage_session_dsl_input(
            session_id,
            dsl::MeerkatMachineInput::RequireRealtimeReattach,
            "RequireRealtimeReattach",
        )
        .await
        .map(|_| ())
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })
    }

    /// Require reattach only if the caller still holds the current authority.
    /// A stale authority surfaces as a DSL guard rejection (wrong epoch) and
    /// returns `ValidationFailed` without mutating state.
    pub async fn require_realtime_attachment_reattach_for_authority(
        &self,
        authority: crate::meerkat_machine_types::RealtimeAttachmentSignalAuthority,
    ) -> Result<(), RuntimeDriverError> {
        self.stage_session_dsl_input(
            &authority.session_id,
            dsl::MeerkatMachineInput::RequireRealtimeReattachForAuthority {
                authority_epoch: authority.authority_epoch,
            },
            "RequireRealtimeReattachForAuthority",
        )
        .await
        .map(|_| ())
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })
    }

    /// Apply a provider-callback realtime signal through the DSL's authority-
    /// epoch guard. The DSL rejects stale tokens, reconfigures-in-progress,
    /// and projection-only statuses.
    pub async fn publish_realtime_attachment_signal(
        &self,
        authority: crate::meerkat_machine_types::RealtimeAttachmentSignalAuthority,
        status: crate::meerkat_machine_types::RealtimeAttachmentStatus,
    ) -> Result<(), RuntimeDriverError> {
        let next_binding_state = match status {
            crate::meerkat_machine_types::RealtimeAttachmentStatus::BindingNotReady => {
                dsl::RealtimeBindingState::BindingNotReady
            }
            crate::meerkat_machine_types::RealtimeAttachmentStatus::BindingReady => {
                dsl::RealtimeBindingState::BindingReady
            }
            crate::meerkat_machine_types::RealtimeAttachmentStatus::ReplacementPending => {
                dsl::RealtimeBindingState::ReplacementPending
            }
            other => {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason: format!(
                        "realtime signal cannot publish projection-only status {other:?}"
                    ),
                });
            }
        };
        self.stage_session_dsl_input(
            &authority.session_id,
            dsl::MeerkatMachineInput::PublishRealtimeSignal {
                authority_epoch: authority.authority_epoch,
                next_binding_state,
            },
            "PublishRealtimeSignal",
        )
        .await
        .map(|_| ())
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })
    }

    // ---- Projection helpers (read DSL state) ----

    async fn read_session_realtime_authority(
        &self,
        session_id: &SessionId,
        context: &'static str,
    ) -> Result<crate::meerkat_machine_types::RealtimeAttachmentSignalAuthority, RuntimeDriverError>
    {
        self.read_session_realtime_authority_if_any(session_id)
            .await?
            .ok_or_else(|| RuntimeDriverError::Internal(format!(
                "DSL did not surface a binding authority epoch after {context}; guard regression"
            )))
    }

    async fn read_session_realtime_authority_if_any(
        &self,
        session_id: &SessionId,
    ) -> Result<
        Option<crate::meerkat_machine_types::RealtimeAttachmentSignalAuthority>,
        RuntimeDriverError,
    > {
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        let authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(authority
            .state
            .realtime_binding_authority_epoch
            .map(
                |epoch| crate::meerkat_machine_types::RealtimeAttachmentSignalAuthority {
                    session_id: session_id.clone(),
                    authority_epoch: epoch,
                },
            ))
    }
}
