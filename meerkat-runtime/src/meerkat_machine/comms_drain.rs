use super::*;

use std::fmt;

/// Phase of the comms drain slot.
///
/// Shell-side mechanics tracking for the runtime-owned drain task. The DSL's
/// `drain_phase` is the canonical lifecycle authority; this slot phase is the
/// mechanical companion that tracks whether a tokio `JoinHandle` is in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommsDrainPhase {
    Inactive,
    Starting,
    Running,
    ExitedRespawnable,
    Stopped,
}

impl fmt::Display for CommsDrainPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inactive => write!(f, "Inactive"),
            Self::Starting => write!(f, "Starting"),
            Self::Running => write!(f, "Running"),
            Self::ExitedRespawnable => write!(f, "ExitedRespawnable"),
            Self::Stopped => write!(f, "Stopped"),
        }
    }
}

/// Mode for the comms drain task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommsDrainMode {
    /// Legacy timed drain with idle timeout.
    Timed,
    /// Live session ingress while a runtime-backed session is attached.
    AttachedSession,
    /// Long-lived host drain (no idle timeout, respawnable on failure).
    PersistentHost,
}

/// Reason the drain task exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainExitReason {
    IdleTimeout,
    Dismissed,
    Failed,
    Aborted,
    SessionShutdown,
}

impl From<DrainExitReason> for crate::meerkat_machine::dsl::DrainExitReason {
    fn from(reason: DrainExitReason) -> Self {
        match reason {
            DrainExitReason::IdleTimeout => Self::IdleTimeout,
            DrainExitReason::Dismissed => Self::Dismissed,
            DrainExitReason::Failed => Self::Failed,
            DrainExitReason::Aborted => Self::Aborted,
            DrainExitReason::SessionShutdown => Self::SessionShutdown,
        }
    }
}

impl From<DrainExitReason> for meerkat_core::handles::DrainExitReason {
    fn from(reason: DrainExitReason) -> Self {
        match reason {
            DrainExitReason::IdleTimeout => Self::IdleTimeout,
            DrainExitReason::Dismissed => Self::Dismissed,
            DrainExitReason::Failed => Self::Failed,
            DrainExitReason::Aborted => Self::Aborted,
            DrainExitReason::SessionShutdown => Self::SessionShutdown,
        }
    }
}

/// Typed view of the peer-ingress transport capability owner (W2-G).
///
/// Projected from the DSL's tagged-union state
/// (`peer_ingress_owner_kind` + companion fields). The
/// `peer_ingress_owner_consistency` invariant guarantees the companion
/// fields are populated exactly for variants that name them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PeerIngressOwner {
    Unattached,
    SessionOwned {
        comms_runtime_id: crate::meerkat_machine::dsl::CommsRuntimeId,
    },
    MobOwned {
        comms_runtime_id: crate::meerkat_machine::dsl::CommsRuntimeId,
        mob_id: crate::meerkat_machine::dsl::MobId,
    },
}

impl PeerIngressOwner {
    /// Returns `true` iff the owner is `MobOwned`.
    pub fn is_mob_owned(&self) -> bool {
        matches!(self, PeerIngressOwner::MobOwned { .. })
    }
}

/// Typed view of the per-session supervisor-bridge binding (Wave 3 D Row 21).
///
/// Projected from the DSL's tagged-union state
/// (`supervisor_binding_kind` + `supervisor_bound_{name, peer_id, address, epoch}`).
/// The `supervisor_binding_consistency` invariant guarantees the companion
/// fields are populated exactly when the kind is `Bound`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SupervisorBinding {
    /// No supervisor bound. The initial state and the state after a
    /// successful `RevokeSupervisor`.
    Unbound,
    /// Supervisor authorized. The four companion fields travel together:
    /// `name` + `peer_id` + `address` derive from the initial bind or the
    /// latest `AuthorizeSupervisor` rotation; `epoch` monotonically
    /// increases across rotations.
    Bound {
        name: String,
        peer_id: String,
        address: String,
        epoch: u64,
    },
}

pub struct CommsDrainSlot {
    pub phase: CommsDrainPhase,
    pub mode: Option<CommsDrainMode>,
    pub handle: Option<tokio::task::JoinHandle<()>>,
    pub bound_runtime: Option<Arc<dyn meerkat_core::agent::CommsRuntime>>,
}

impl CommsDrainSlot {
    pub fn new() -> Self {
        Self {
            phase: CommsDrainPhase::Inactive,
            mode: None,
            handle: None,
            bound_runtime: None,
        }
    }

    fn bound_runtime_matches(&self, runtime: &Arc<dyn meerkat_core::agent::CommsRuntime>) -> bool {
        self.bound_runtime
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, runtime))
    }

    fn can_ensure_running(&self) -> bool {
        matches!(
            self.phase,
            CommsDrainPhase::Inactive
                | CommsDrainPhase::Stopped
                | CommsDrainPhase::ExitedRespawnable
        )
    }

    fn begin_running(
        &mut self,
        mode: CommsDrainMode,
        runtime: Arc<dyn meerkat_core::agent::CommsRuntime>,
    ) -> bool {
        if !self.can_ensure_running() {
            return false;
        }
        self.mode = Some(mode);
        self.bound_runtime = Some(runtime);
        self.phase = CommsDrainPhase::Starting;
        true
    }

    fn begin_rebind(
        &mut self,
        mode: CommsDrainMode,
        runtime: Arc<dyn meerkat_core::agent::CommsRuntime>,
    ) -> bool {
        if self.phase != CommsDrainPhase::Running || self.mode != Some(mode) {
            return false;
        }
        if self.bound_runtime_matches(&runtime) {
            return false;
        }
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
        self.bound_runtime = Some(runtime);
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
                self.bound_runtime = None;
                CommsDrainPhase::Stopped
            };
        }
    }

    pub(crate) fn abort(&mut self) {
        self.phase = CommsDrainPhase::Stopped;
        self.bound_runtime = None;
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
                    mob_id: None,
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
                    mob_id: None,
                },
            )
            .await
        {
            Ok(MeerkatMachineCommandResult::Spawned(spawned)) => spawned,
            _ => false,
        }
    }

    /// Mob-owned variant of [`MeerkatMachine::maybe_spawn_comms_drain`]
    /// (W2-G / issue #264).
    ///
    /// Shell calls this from the mob provisioning path to claim peer-ingress
    /// ownership as `MobOwned { comms_runtime_id, mob_id }`. The DSL
    /// transition permits promotion from `Unattached` or `SessionOwned`, so
    /// a mob can take over a session-owned drain at spawn; silent downgrades
    /// back to `SessionOwned` are impossible by construction.
    pub async fn maybe_spawn_mob_comms_drain(
        self: &Arc<Self>,
        session_id: &SessionId,
        comms_runtime: Arc<dyn meerkat_core::agent::CommsRuntime>,
        mob_id: crate::meerkat_machine::dsl::MobId,
    ) -> bool {
        match self
            .execute_meerkat_machine_command(
                Some(Arc::clone(self)),
                MeerkatMachineCommand::SetPeerIngressContext {
                    session_id: session_id.clone(),
                    keep_alive: true,
                    comms_runtime: Some(comms_runtime),
                    mob_id: Some(mob_id),
                },
            )
            .await
        {
            Ok(MeerkatMachineCommandResult::Spawned(spawned)) => spawned,
            _ => false,
        }
    }

    /// Read the current peer-ingress owner from DSL state.
    ///
    /// Returns `PeerIngressOwner::Unattached` for sessions that have no
    /// registered DSL state (unknown / destroyed sessions). Used by the
    /// session-runtime to refuse reconfiguration of mob-owned drains at
    /// turn-start.
    ///
    /// The `peer_ingress_owner_consistency` invariant guarantees that
    /// companion fields are populated for non-`Unattached` kinds, but if
    /// the invariant were ever violated at runtime, we gracefully degrade
    /// to `Unattached` rather than panic.
    pub async fn peer_ingress_owner(&self, session_id: &SessionId) -> PeerIngressOwner {
        let sessions = self.sessions.read().await;
        let Some(entry) = sessions.get(session_id) else {
            return PeerIngressOwner::Unattached;
        };
        let authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match authority.state.peer_ingress_owner_kind {
            crate::meerkat_machine::dsl::PeerIngressOwnerKind::Unattached => {
                PeerIngressOwner::Unattached
            }
            crate::meerkat_machine::dsl::PeerIngressOwnerKind::SessionOwned => {
                match authority.state.peer_ingress_comms_runtime_id.clone() {
                    Some(comms_runtime_id) => PeerIngressOwner::SessionOwned { comms_runtime_id },
                    None => {
                        tracing::error!(
                            %session_id,
                            "peer_ingress_owner_consistency invariant violation: SessionOwned without comms_runtime_id"
                        );
                        PeerIngressOwner::Unattached
                    }
                }
            }
            crate::meerkat_machine::dsl::PeerIngressOwnerKind::MobOwned => {
                match (
                    authority.state.peer_ingress_comms_runtime_id.clone(),
                    authority.state.peer_ingress_mob_id.clone(),
                ) {
                    (Some(comms_runtime_id), Some(mob_id)) => PeerIngressOwner::MobOwned {
                        comms_runtime_id,
                        mob_id,
                    },
                    _ => {
                        tracing::error!(
                            %session_id,
                            "peer_ingress_owner_consistency invariant violation: MobOwned without companion fields"
                        );
                        PeerIngressOwner::Unattached
                    }
                }
            }
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

        let needs_rebind = slot.begin_rebind(mode, comms.clone());
        let needs_spawn = if needs_rebind {
            false
        } else {
            slot.begin_running(mode, comms.clone())
        };

        if !needs_rebind && !needs_spawn {
            return false;
        }
        drop(slots);
        drop(sessions);

        if needs_spawn {
            // Stage DSL SpawnDrain only when the machine is transitioning from
            // not-running into running. A runtime-instance rebind keeps the
            // conceptual drain alive and only swaps the bound transport task.
            let mut sessions = self.sessions.write().await;
            if let Some(entry) = sessions.get_mut(session_id) {
                let apply_result = {
                    let mut authority = entry
                        .dsl_authority
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                        &mut *authority,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::SpawnDrain {
                            mode: crate::meerkat_machine::dsl::DrainMode::from(mode),
                        },
                    )
                };
                if let Err(err) = apply_result {
                    tracing::warn!(
                        %session_id,
                        error = %crate::meerkat_machine::dsl_authority::map_error(err, "SpawnDrain"),
                        "DSL rejected SpawnDrain; skipping drain spawn"
                    );
                    let mut slots = self.comms_drain_slots.write().await;
                    if let Some(slot) = slots.get_mut(session_id) {
                        slot.phase = CommsDrainPhase::Stopped;
                        slot.bound_runtime = None;
                    }
                    return false;
                }
            } else {
                tracing::warn!(
                    %session_id,
                    "refusing to spawn comms drain for unregistered session"
                );
                let mut slots = self.comms_drain_slots.write().await;
                if let Some(slot) = slots.get_mut(session_id) {
                    slot.phase = CommsDrainPhase::Stopped;
                    slot.bound_runtime = None;
                }
                return false;
            }
        } else if needs_rebind {
            tracing::warn!(
                %session_id,
                "rebinding persistent comms drain to a new comms runtime instance"
            );
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
        let mut slots = self.comms_drain_slots.write().await;
        let slot = slots
            .entry(session_id.clone())
            .or_insert_with(CommsDrainSlot::new);
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
            if let Some(entry) = sessions.get_mut(session_id) {
                let mut authority = entry
                    .dsl_authority
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Err(err) = crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                    &mut *authority,
                    dsl_input,
                ) {
                    tracing::warn!(
                        %session_id,
                        error = %crate::meerkat_machine::dsl_authority::map_error(err, context),
                        "DSL rejected drain exit notification; proceeding with slot cleanup"
                    );
                }
            }
        }
        if std::env::var_os("RKAT_TRACE_COMMS_DRAIN_BIND").is_some() {
            tracing::info!(
                %session_id,
                ?reason,
                respawnable = is_respawnable,
                "comms drain exited"
            );
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

    /// Read the current supervisor binding from DSL state (Wave 3 D Row 21).
    ///
    /// Returns `SupervisorBinding::Unbound` for sessions that have no
    /// registered DSL state (unknown / destroyed sessions). The
    /// `supervisor_binding_consistency` invariant guarantees the four
    /// companion fields are populated exactly when the kind is `Bound`; if
    /// that invariant were ever violated at runtime, we gracefully degrade
    /// to `Unbound` rather than panic.
    pub async fn supervisor_binding(&self, session_id: &SessionId) -> SupervisorBinding {
        let sessions = self.sessions.read().await;
        let Some(entry) = sessions.get(session_id) else {
            return SupervisorBinding::Unbound;
        };
        let authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match authority.state.supervisor_binding_kind {
            crate::meerkat_machine::dsl::SupervisorBindingKind::Unbound => {
                SupervisorBinding::Unbound
            }
            crate::meerkat_machine::dsl::SupervisorBindingKind::Bound => {
                match (
                    authority.state.supervisor_bound_name.clone(),
                    authority.state.supervisor_bound_peer_id.clone(),
                    authority.state.supervisor_bound_address.clone(),
                    authority.state.supervisor_bound_epoch,
                ) {
                    (Some(name), Some(peer_id), Some(address), Some(epoch)) => {
                        SupervisorBinding::Bound {
                            name,
                            peer_id,
                            address,
                            epoch,
                        }
                    }
                    _ => {
                        tracing::error!(
                            %session_id,
                            "supervisor_binding_consistency invariant violation: Bound without all companion fields"
                        );
                        SupervisorBinding::Unbound
                    }
                }
            }
        }
    }

    /// Stage a DSL `BindSupervisor` input (Wave 3 D Row 21).
    ///
    /// Returns the classified result from the DSL mutator so callers can
    /// surface typed rejections (e.g. "already bound"). The shell uses
    /// this after validating the incoming bridge request's bootstrap
    /// token; the DSL is the authority that flips `Unbound → Bound`.
    pub async fn stage_supervisor_bind(
        &self,
        session_id: &SessionId,
        name: String,
        peer_id: String,
        address: String,
        epoch: u64,
    ) -> Result<(), SupervisorBindingStageError> {
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(SupervisorBindingStageError::SessionNotRegistered)?;
        let mut authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
            &mut *authority,
            crate::meerkat_machine::dsl::MeerkatMachineInput::BindSupervisor {
                name,
                peer_id,
                address,
                epoch,
            },
        )
        .map_err(SupervisorBindingStageError::Dsl)?;
        Ok(())
    }

    /// Stage a DSL `AuthorizeSupervisor` input (Wave 3 D Row 21).
    ///
    /// Rotates the current binding to a new supervisor + epoch. The shell
    /// must have already verified the rotation is authorized by the
    /// *current* supervisor before calling this method.
    pub async fn stage_supervisor_authorize(
        &self,
        session_id: &SessionId,
        name: String,
        peer_id: String,
        address: String,
        epoch: u64,
    ) -> Result<(), SupervisorBindingStageError> {
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(SupervisorBindingStageError::SessionNotRegistered)?;
        let mut authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
            &mut *authority,
            crate::meerkat_machine::dsl::MeerkatMachineInput::AuthorizeSupervisor {
                name,
                peer_id,
                address,
                epoch,
            },
        )
        .map_err(SupervisorBindingStageError::Dsl)?;
        Ok(())
    }

    /// Stage a DSL `RevokeSupervisor` input (Wave 3 D Row 21).
    ///
    /// Returns to `Unbound`. The DSL guard enforces that the supplied
    /// `peer_id` and `epoch` match the current binding exactly; a stale
    /// revoke cannot tear down a freshly rotated binding.
    pub async fn stage_supervisor_revoke(
        &self,
        session_id: &SessionId,
        peer_id: String,
        epoch: u64,
    ) -> Result<(), SupervisorBindingStageError> {
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(SupervisorBindingStageError::SessionNotRegistered)?;
        let mut authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
            &mut *authority,
            crate::meerkat_machine::dsl::MeerkatMachineInput::RevokeSupervisor { peer_id, epoch },
        )
        .map_err(SupervisorBindingStageError::Dsl)?;
        Ok(())
    }
}

/// Errors raised when staging a supervisor-binding input against the DSL
/// (Wave 3 D Row 21).
#[derive(Debug)]
pub enum SupervisorBindingStageError {
    /// The session is not registered with the runtime.
    SessionNotRegistered,
    /// The DSL mutator rejected the transition (e.g. guard failure). The
    /// boxed inner is the typed DSL transition error; callers that need to
    /// distinguish guard rejections from missing-transition failures can
    /// match on it.
    Dsl(crate::meerkat_machine::dsl::MeerkatMachineTransitionError),
}

impl std::fmt::Display for SupervisorBindingStageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionNotRegistered => write!(f, "session not registered with runtime"),
            Self::Dsl(err) => write!(f, "DSL rejected supervisor binding input: {err}"),
        }
    }
}

impl std::error::Error for SupervisorBindingStageError {}
