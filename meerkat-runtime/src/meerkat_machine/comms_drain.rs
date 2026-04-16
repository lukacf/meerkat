use super::*;

pub struct CommsDrainSlot {
    pub authority: CommsDrainLifecycleAuthority,
    pub handle: Option<tokio::task::JoinHandle<()>>,
}

impl CommsDrainSlot {
    pub fn new() -> Self {
        Self {
            authority: CommsDrainLifecycleAuthority::new(),
            handle: None,
        }
    }
}

pub fn apply_runtime_drain_effects(
    slot: &mut CommsDrainSlot,
    effects: &[CommsDrainLifecycleEffect],
) {
    for effect in effects {
        if let CommsDrainLifecycleEffect::AbortDrainTask = effect
            && let Some(handle) = slot.handle.take()
        {
            handle.abort();
        }
    }
}

pub fn abort_slot(slot: &mut CommsDrainSlot) {
    match protocol_comms_drain_abort::execute_stop_requested(&mut slot.authority) {
        Ok(result) => {
            apply_runtime_drain_effects(slot, &result.effects);
            // Under TerminalClosure policy, the abort obligation is implicitly
            // satisfied when the machine reaches Stopped phase. Drop it.
            let _ = result.obligation;
        }
        Err(_) => {
            // Already stopped or inactive — just clean up the handle
            if let Some(handle) = slot.handle.take() {
                handle.abort();
            }
        }
    }
}

impl MeerkatMachine {
    pub async fn update_peer_ingress_context(
        self: &Arc<Self>,
        session_id: &SessionId,
        keep_alive: bool,
        comms_runtime: Option<Arc<dyn meerkat_core::agent::CommsRuntime>>,
    ) -> bool {
        match self
            .execute_meerkat_machine_command(
                Some(Arc::clone(self)),
                MeerkatMachineCommand::SetPeerIngressContext {
                    session_id: session_id.clone(),
                    keep_alive,
                    comms_runtime,
                },
            )
            .await
        {
            Ok(MeerkatMachineCommandResult::Spawned(spawned)) => spawned,
            _ => false,
        }
    }

    /// Manage the comms drain lifecycle for a session based on keep_alive intent.
    ///
    /// When `keep_alive` is true, spawns a drain if one is not already running.
    /// When `keep_alive` is false, aborts any running drain for the session.
    /// Returns `true` if a new drain was spawned.
    ///
    /// All state transitions go through `CommsDrainLifecycleAuthority`.
    pub async fn maybe_spawn_comms_drain(
        self: &Arc<Self>,
        session_id: &SessionId,
        keep_alive: bool,
        comms_runtime: Option<Arc<dyn meerkat_core::agent::CommsRuntime>>,
    ) -> bool {
        match self
            .execute_meerkat_machine_command(
                Some(Arc::clone(self)),
                MeerkatMachineCommand::SetPeerIngressContext {
                    session_id: session_id.clone(),
                    keep_alive,
                    comms_runtime,
                },
            )
            .await
        {
            Ok(MeerkatMachineCommandResult::Spawned(spawned)) => spawned,
            _ => false,
        }
    }

    pub(super) async fn update_peer_ingress_context_inner(
        self: &Arc<Self>,
        session_id: &SessionId,
        keep_alive: bool,
        comms_runtime: Option<Arc<dyn meerkat_core::agent::CommsRuntime>>,
    ) -> bool {
        if !keep_alive {
            // Explicit disable: stop any running drain for this session.
            let _ = self
                .execute_meerkat_machine_drain_local_command(MeerkatMachineCommand::Abort {
                    session_id: session_id.clone(),
                })
                .await;
            return false;
        }

        let mode = CommsDrainMode::PersistentHost;

        let comms = match comms_runtime {
            Some(c) => c,
            None => return false,
        };

        let sessions = self.sessions.read().await;
        if !sessions.contains_key(session_id) {
            tracing::warn!(
                %session_id,
                "refusing to spawn comms drain for unregistered session"
            );
            return false;
        }
        // Keep the session read guard while mutating drain slots so unregister
        // cannot race between registration check and slot publication.
        let mut slots = self.comms_drain_slots.write().await;
        let slot = slots
            .entry(session_id.clone())
            .or_insert_with(CommsDrainSlot::new);

        let result =
            match protocol_comms_drain_spawn::execute_ensure_running(&mut slot.authority, mode) {
                Ok(r) => r,
                Err(e) => {
                    tracing::trace!(error = %e, "comms drain authority rejected EnsureRunning");
                    return false;
                }
            };

        // Execute effects from the transition
        for effect in &result.effects {
            match effect {
                CommsDrainLifecycleEffect::SpawnDrainTask { mode: spawn_mode } => {
                    let idle_timeout = match spawn_mode {
                        CommsDrainMode::PersistentHost => Some(std::time::Duration::MAX),
                        CommsDrainMode::Timed | CommsDrainMode::AttachedSession => None,
                    };
                    let handle = crate::comms_drain::spawn_comms_drain(
                        Arc::clone(self),
                        session_id.clone(),
                        comms.clone(),
                        idle_timeout,
                    );
                    slot.handle = Some(handle);
                }
                CommsDrainLifecycleEffect::AbortDrainTask => {
                    if let Some(handle) = slot.handle.take() {
                        handle.abort();
                    }
                }
            }
        }

        let Some(obligation) = result.obligation else {
            tracing::warn!(
                %session_id,
                "comms drain spawn transition emitted no obligation"
            );
            return false;
        };

        // The runtime spawns the drain task synchronously (as a tokio task
        // above), so we immediately close the spawn obligation.
        match protocol_comms_drain_spawn::submit_task_spawned(&mut slot.authority, obligation) {
            Ok(_effects) => {}
            Err(e) => {
                tracing::trace!(error = %e, "comms drain authority rejected TaskSpawned");
            }
        }
        true
    }

    /// Notify the authority that a drain task has exited with the given reason.
    ///
    /// Called from drain task exit paths (or by wrappers that detect task
    /// completion). The authority decides whether to enter ExitedRespawnable
    /// (PersistentHost + Failed) or Stopped.
    pub async fn notify_comms_drain_exited(
        self: &Arc<Self>,
        session_id: &SessionId,
        reason: DrainExitReason,
    ) {
        let _ = self
            .execute_meerkat_machine_command(
                Some(Arc::clone(self)),
                MeerkatMachineCommand::NotifyDrainExited {
                    session_id: session_id.clone(),
                    reason,
                },
            )
            .await;
    }

    pub(super) async fn notify_comms_drain_exited_inner(
        &self,
        session_id: &SessionId,
        reason: DrainExitReason,
    ) {
        let mut slots = self.comms_drain_slots.write().await;
        if let Some(slot) = slots.get_mut(session_id) {
            slot.handle.take(); // clean up finished handle
            match protocol_comms_drain_spawn::notify_task_exited(&mut slot.authority, reason) {
                Ok(_effects) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "comms drain authority rejected TaskExited");
                }
            }
        }
    }

    /// Abort all active comms drain tasks.
    pub async fn abort_comms_drains(&self) {
        let _ = self
            .execute_meerkat_machine_command(None, MeerkatMachineCommand::AbortAll)
            .await;
    }

    /// Abort the comms drain task for a specific session.
    pub async fn abort_comms_drain(&self, session_id: &SessionId) {
        let _ = self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Abort {
                    session_id: session_id.clone(),
                },
            )
            .await;
    }

    /// Wait for a session's comms drain task to finish.
    ///
    /// Returns immediately if no drain is active for the session.
    /// If the task already notified the authority (normal exit), this is a no-op
    /// for authority state. If the task panicked without notifying, this submits
    /// `TaskExited { Failed }` as a safety net.
    pub async fn wait_comms_drain(&self, session_id: &SessionId) {
        let _ = self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Wait {
                    session_id: session_id.clone(),
                },
            )
            .await;
    }
}
