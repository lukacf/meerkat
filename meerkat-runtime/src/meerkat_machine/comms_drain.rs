use super::*;

use meerkat_core::comms_drain_lifecycle_authority::DrainExitReason;
use meerkat_core::comms_drain_lifecycle_authority::{CommsDrainMode, CommsDrainPhase};

pub struct CommsDrainSlot {
    pub phase: CommsDrainPhase,
    pub mode: Option<CommsDrainMode>,
    pub handle: Option<tokio::task::JoinHandle<()>>,
}

impl CommsDrainSlot {
    pub fn new() -> Self {
        Self {
            phase: CommsDrainPhase::Inactive,
            mode: None,
            handle: None,
        }
    }

    fn can_ensure_running(&self) -> bool {
        matches!(
            self.phase,
            CommsDrainPhase::Inactive
                | CommsDrainPhase::Stopped
                | CommsDrainPhase::ExitedRespawnable
        )
    }

    fn begin_running(&mut self, mode: CommsDrainMode) -> bool {
        if !self.can_ensure_running() {
            return false;
        }
        self.mode = Some(mode);
        self.phase = CommsDrainPhase::Starting;
        true
    }

    fn mark_task_spawned(&mut self) {
        if self.phase == CommsDrainPhase::Starting {
            self.phase = CommsDrainPhase::Running;
        }
    }

    fn mark_task_exited(&mut self, reason: DrainExitReason) {
        if matches!(
            self.phase,
            CommsDrainPhase::Starting | CommsDrainPhase::Running
        ) {
            self.phase = if self.mode == Some(CommsDrainMode::PersistentHost)
                && reason == DrainExitReason::Failed
            {
                CommsDrainPhase::ExitedRespawnable
            } else {
                CommsDrainPhase::Stopped
            };
        }
    }

    pub(crate) fn abort(&mut self) {
        self.phase = CommsDrainPhase::Stopped;
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }

    pub(crate) fn mark_task_exit_if_running_for_safety(&mut self, reason: DrainExitReason) {
        if self.phase == CommsDrainPhase::Running {
            self.mark_task_exited(reason);
        }
    }
}

pub fn abort_slot(slot: &mut CommsDrainSlot) {
    slot.abort();
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

        // Stage DSL SpawnDrain input BEFORE mutating the drain slot.
        // drain_phase is maintained by DSL transitions (SpawnDrain/StopDrain/
        // DrainExited*); shell-side slot.phase is not re-projected into DSL.
        {
            let mut sessions = self.sessions.write().await;
            if let Some(entry) = sessions.get_mut(session_id) {
                let mode_str = format!("{mode:?}");
                if let Err(err) = crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                    &mut *entry.dsl_authority,
                    crate::meerkat_machine::dsl::MeerkatMachineInput::SpawnDrain { mode: mode_str },
                ) {
                    tracing::warn!(
                        %session_id,
                        error = %crate::meerkat_machine::dsl_authority::map_error(err, "SpawnDrain"),
                        "DSL rejected SpawnDrain; skipping drain spawn"
                    );
                    return false;
                }
            } else {
                tracing::warn!(
                    %session_id,
                    "refusing to spawn comms drain for unregistered session"
                );
                return false;
            }
        }

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

        if !slot.begin_running(mode) {
            return false;
        }

        let idle_timeout = match mode {
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
        slot.mark_task_spawned();

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
        // Stage DSL drain exit input BEFORE mutating the drain slot.
        // Determine whether this is a clean exit or a respawnable exit
        // based on the slot's current mode and the exit reason.
        let is_respawnable = {
            let slots = self.comms_drain_slots.read().await;
            slots.get(session_id).is_some_and(|s| {
                s.mode == Some(CommsDrainMode::PersistentHost) && reason == DrainExitReason::Failed
            })
        };
        {
            let dsl_input = if is_respawnable {
                crate::meerkat_machine::dsl::MeerkatMachineInput::DrainExitedRespawnable
            } else {
                crate::meerkat_machine::dsl::MeerkatMachineInput::DrainExitedClean
            };
            let context = if is_respawnable {
                "DrainExitedRespawnable"
            } else {
                "DrainExitedClean"
            };
            let mut sessions = self.sessions.write().await;
            if let Some(entry) = sessions.get_mut(session_id)
                && let Err(err) = crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                    &mut *entry.dsl_authority,
                    dsl_input,
                )
            {
                tracing::warn!(
                    %session_id,
                    error = %crate::meerkat_machine::dsl_authority::map_error(err, context),
                    "DSL rejected drain exit notification; proceeding with slot cleanup"
                );
            }
        }

        let mut slots = self.comms_drain_slots.write().await;
        if let Some(slot) = slots.get_mut(session_id) {
            slot.handle.take(); // clean up finished handle
            slot.mark_task_exited(reason);
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
