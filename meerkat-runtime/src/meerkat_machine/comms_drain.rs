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

impl From<crate::meerkat_machine::dsl::DrainPhase> for CommsDrainPhase {
    fn from(phase: crate::meerkat_machine::dsl::DrainPhase) -> Self {
        match phase {
            crate::meerkat_machine::dsl::DrainPhase::Inactive => Self::Inactive,
            crate::meerkat_machine::dsl::DrainPhase::Running => Self::Running,
            crate::meerkat_machine::dsl::DrainPhase::Stopped => Self::Stopped,
            crate::meerkat_machine::dsl::DrainPhase::ExitedRespawnable => Self::ExitedRespawnable,
        }
    }
}

impl From<crate::meerkat_machine::dsl::DrainMode> for CommsDrainMode {
    fn from(mode: crate::meerkat_machine::dsl::DrainMode) -> Self {
        match mode {
            crate::meerkat_machine::dsl::DrainMode::Timed => Self::Timed,
            crate::meerkat_machine::dsl::DrainMode::AttachedSession => Self::AttachedSession,
            crate::meerkat_machine::dsl::DrainMode::PersistentHost => Self::PersistentHost,
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
/// (`supervisor_binding_kind` +
/// `supervisor_bound_{name, peer_id, address, signing_public_key, epoch}`).
/// The `supervisor_binding_consistency` invariant guarantees the companion
/// fields are populated exactly when the kind is `Bound`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SupervisorBinding {
    /// No supervisor bound. The initial state and the state after a
    /// successful `RevokeSupervisor`.
    Unbound,
    /// Supervisor authorized. The companion fields travel together:
    /// `name` + `peer_id` + `address` + `signing_public_key` derive from the
    /// initial bind or the latest `AuthorizeSupervisor` rotation; `epoch`
    /// monotonically increases across rotations.
    Bound {
        name: String,
        peer_id: String,
        address: String,
        signing_public_key: String,
        epoch: u64,
    },
}

pub struct CommsDrainSlot {
    handle: Option<tokio::task::JoinHandle<()>>,
    task_runtime: Option<Arc<dyn meerkat_core::agent::CommsRuntime>>,
}

impl CommsDrainSlot {
    pub fn new() -> Self {
        Self {
            handle: None,
            task_runtime: None,
        }
    }

    pub(crate) fn task_runtime_matches(
        &self,
        runtime: &Arc<dyn meerkat_core::agent::CommsRuntime>,
    ) -> bool {
        self.task_runtime
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, runtime))
    }

    pub(crate) fn handle_present(&self) -> bool {
        self.handle.is_some()
    }

    pub(crate) fn install_task(
        &mut self,
        runtime: Arc<dyn meerkat_core::agent::CommsRuntime>,
        handle: tokio::task::JoinHandle<()>,
    ) {
        if let Some(existing) = self.handle.take() {
            existing.abort();
        }
        self.task_runtime = Some(runtime);
        self.handle = Some(handle);
    }

    pub(crate) fn take_handle(&mut self) -> Option<tokio::task::JoinHandle<()>> {
        self.handle.take()
    }

    pub(crate) fn clear_after_exit(&mut self, keep_runtime: bool) {
        self.handle.take();
        if !keep_runtime {
            self.task_runtime = None;
        }
    }

    pub(crate) fn abort(&mut self) {
        self.task_runtime = None;
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

pub fn abort_slot(slot: &mut CommsDrainSlot) {
    slot.abort();
}

#[derive(Debug, Clone)]
pub(super) struct DrainAuthorityState {
    pub phase: crate::meerkat_machine::dsl::DrainPhase,
    pub mode: Option<crate::meerkat_machine::dsl::DrainMode>,
    pub peer_owner_kind: crate::meerkat_machine::dsl::PeerIngressOwnerKind,
    pub peer_runtime_id: Option<crate::meerkat_machine::dsl::CommsRuntimeId>,
}

impl DrainAuthorityState {
    pub(super) fn can_spawn(&self) -> bool {
        matches!(
            self.phase,
            crate::meerkat_machine::dsl::DrainPhase::Inactive
                | crate::meerkat_machine::dsl::DrainPhase::Stopped
                | crate::meerkat_machine::dsl::DrainPhase::ExitedRespawnable
        )
    }

    pub(super) fn has_peer_runtime(
        &self,
        runtime_id: &crate::meerkat_machine::dsl::CommsRuntimeId,
    ) -> bool {
        self.peer_owner_kind != crate::meerkat_machine::dsl::PeerIngressOwnerKind::Unattached
            && self.peer_runtime_id.as_ref() == Some(runtime_id)
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
        match authority.state().peer_ingress_owner_kind {
            crate::meerkat_machine::dsl::PeerIngressOwnerKind::Unattached => {
                PeerIngressOwner::Unattached
            }
            crate::meerkat_machine::dsl::PeerIngressOwnerKind::SessionOwned => {
                match authority.state().peer_ingress_comms_runtime_id.clone() {
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
                    authority.state().peer_ingress_comms_runtime_id.clone(),
                    authority.state().peer_ingress_mob_id.clone(),
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

    pub(super) async fn drain_authority_state(
        &self,
        session_id: &SessionId,
    ) -> Option<DrainAuthorityState> {
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id)?;
        let authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = authority.state();
        Some(DrainAuthorityState {
            phase: state.drain_phase,
            mode: state.drain_mode,
            peer_owner_kind: state.peer_ingress_owner_kind,
            peer_runtime_id: state.peer_ingress_comms_runtime_id.clone(),
        })
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

        let runtime_id = crate::meerkat_machine::dsl::CommsRuntimeId::from_runtime(&comms);
        let Some(authority_state) = self.drain_authority_state(session_id).await else {
            tracing::warn!(
                %session_id,
                "refusing to spawn comms drain without generated drain authority"
            );
            return false;
        };
        if !authority_state.has_peer_runtime(&runtime_id) {
            tracing::warn!(
                %session_id,
                "refusing to spawn comms drain without matching generated peer-ingress authority"
            );
            return false;
        }

        let dsl_mode = crate::meerkat_machine::dsl::DrainMode::from(mode);
        let needs_spawn = authority_state.can_spawn();
        let needs_task_refresh = if needs_spawn {
            false
        } else if authority_state.phase == crate::meerkat_machine::dsl::DrainPhase::Running
            && authority_state.mode == Some(dsl_mode)
        {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                tracing::warn!(
                    %session_id,
                    "refusing to spawn comms drain for unregistered session"
                );
                return false;
            };
            !entry.drain_slot.handle_present() || !entry.drain_slot.task_runtime_matches(&comms)
        } else {
            false
        };

        if !needs_spawn && !needs_task_refresh {
            return false;
        }

        if needs_spawn {
            // Stage DSL SpawnDrain only when the machine is transitioning from
            // not-running into running. A runtime-instance refresh keeps the
            // conceptual drain alive and only swaps the mechanical task after
            // peer-ingress authority has accepted the runtime identity.
            if let Err(err) = self
                .stage_session_dsl_input(
                    session_id,
                    crate::meerkat_machine::dsl::MeerkatMachineInput::SpawnDrain { mode: dsl_mode },
                    "SpawnDrain",
                )
                .await
            {
                tracing::warn!(
                    %session_id,
                    error = %err,
                    "DSL rejected SpawnDrain; skipping drain spawn"
                );
                return false;
            }
        } else if needs_task_refresh {
            tracing::warn!(
                %session_id,
                "refreshing persistent comms drain task from generated peer-ingress authority"
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
        let mut sessions = self.sessions.write().await;
        if let Some(entry) = sessions.get_mut(session_id) {
            entry.drain_slot.install_task(comms.clone(), handle);
        } else {
            handle.abort();
            return false;
        }

        true
    }

    /// Notify the authority that a drain task has exited with the given reason.
    ///
    /// Called from drain task exit paths (or by wrappers that detect task
    /// completion). The generated `NotifyDrainExited` input owns whether the
    /// exit enters `ExitedRespawnable` or `Stopped`; this method only projects
    /// the accepted authority state into task-handle mechanics.
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
        let keep_runtime = self
            .drain_authority_state(session_id)
            .await
            .is_some_and(|state| {
                state.phase == crate::meerkat_machine::dsl::DrainPhase::ExitedRespawnable
            });
        let mut sessions = self.sessions.write().await;
        if let Some(entry) = sessions.get_mut(session_id) {
            entry.drain_slot.clear_after_exit(keep_runtime);
        }
        if std::env::var_os("RKAT_TRACE_COMMS_DRAIN_BIND").is_some() {
            tracing::info!(
                %session_id,
                ?reason,
                respawnable = keep_runtime,
                "comms drain exited"
            );
        }
    }

    pub(super) async fn project_comms_drain_failed_safety_net(&self, session_id: &SessionId) {
        let keep_runtime = match self.drain_authority_state(session_id).await {
            Some(state) => {
                state.phase == crate::meerkat_machine::dsl::DrainPhase::ExitedRespawnable
            }
            None => false,
        };
        let mut sessions = self.sessions.write().await;
        if let Some(entry) = sessions.get_mut(session_id) {
            entry.drain_slot.clear_after_exit(keep_runtime);
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
        match authority.state().supervisor_binding_kind {
            crate::meerkat_machine::dsl::SupervisorBindingKind::Unbound => {
                SupervisorBinding::Unbound
            }
            crate::meerkat_machine::dsl::SupervisorBindingKind::Bound => {
                match (
                    authority.state().supervisor_bound_name.clone(),
                    authority.state().supervisor_bound_peer_id.clone(),
                    authority.state().supervisor_bound_address.clone(),
                    authority
                        .state()
                        .supervisor_bound_signing_public_key
                        .clone(),
                    authority.state().supervisor_bound_epoch,
                ) {
                    (
                        Some(name),
                        Some(peer_id),
                        Some(address),
                        Some(signing_public_key),
                        Some(epoch),
                    ) => SupervisorBinding::Bound {
                        name,
                        peer_id,
                        address,
                        signing_public_key,
                        epoch,
                    },
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

    fn local_endpoint_for_comms_runtime(
        comms_runtime: &dyn meerkat_core::agent::CommsRuntime,
    ) -> Result<crate::meerkat_machine::dsl::PeerEndpoint, String> {
        let peer_id = comms_runtime
            .peer_id()
            .ok_or_else(|| "runtime peer_id unavailable".to_string())?;
        let name = comms_runtime
            .comms_name()
            .ok_or_else(|| "runtime comms_name unavailable".to_string())?;
        let address = comms_runtime
            .advertised_address()
            .ok_or_else(|| "runtime advertised_address unavailable".to_string())?;
        let pubkey = comms_runtime
            .public_key_bytes()
            .ok_or_else(|| "runtime public_key_bytes unavailable".to_string())?;
        let descriptor = meerkat_core::comms::TrustedPeerDescriptor::unsigned_with_pubkey(
            name,
            peer_id.to_string(),
            pubkey,
            address,
        )
        .map_err(|error| format!("runtime local endpoint descriptor invalid: {error}"))?;
        Ok(crate::meerkat_machine::dsl::PeerEndpoint::from(&descriptor))
    }

    /// Publish the target runtime's own endpoint into MeerkatMachine before
    /// generated trust handoffs mint authority scoped to that trust store.
    pub async fn stage_local_endpoint_for_comms_runtime(
        &self,
        session_id: &SessionId,
        comms_runtime: &dyn meerkat_core::agent::CommsRuntime,
    ) -> Result<(), SupervisorBindingStageError> {
        let endpoint = Self::local_endpoint_for_comms_runtime(comms_runtime)
            .map_err(SupervisorBindingStageError::LocalEndpoint)?;
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
            crate::meerkat_machine::dsl::MeerkatMachineInput::PublishLocalEndpoint { endpoint },
        )
        .map_err(SupervisorBindingStageError::Dsl)?;
        Ok(())
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
        signing_public_key: String,
        epoch: u64,
    ) -> Result<crate::meerkat_machine::dsl::MeerkatMachineTransition, SupervisorBindingStageError>
    {
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(SupervisorBindingStageError::SessionNotRegistered)?;
        let mut authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let transition = crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
            &mut *authority,
            crate::meerkat_machine::dsl::MeerkatMachineInput::BindSupervisor {
                name,
                peer_id,
                address,
                signing_public_key,
                epoch,
            },
        )
        .map_err(SupervisorBindingStageError::Dsl)?;
        Ok(transition)
    }

    pub async fn supervisor_trust_publish_freshness_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<
        crate::protocol_supervisor_trust_publish::SupervisorTrustFreshnessAuthority,
        SupervisorBindingStageError,
    > {
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(session_id)
            .ok_or(SupervisorBindingStageError::SessionNotRegistered)?;
        Ok(
            crate::protocol_supervisor_trust_publish::SupervisorTrustFreshnessAuthority::from_authority(
                Arc::clone(&entry.dsl_authority),
            ),
        )
    }

    pub async fn generated_authority_owner_token(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn std::any::Any + Send + Sync>> {
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id)?;
        let authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Some(authority.generated_authority_owner_token())
    }

    pub async fn supervisor_trust_revoke_freshness_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<
        crate::protocol_supervisor_trust_revoke::SupervisorTrustFreshnessAuthority,
        SupervisorBindingStageError,
    > {
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(session_id)
            .ok_or(SupervisorBindingStageError::SessionNotRegistered)?;
        Ok(
            crate::protocol_supervisor_trust_revoke::SupervisorTrustFreshnessAuthority::from_authority(
                Arc::clone(&entry.dsl_authority),
            ),
        )
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
        signing_public_key: String,
        epoch: u64,
    ) -> Result<crate::meerkat_machine::dsl::MeerkatMachineTransition, SupervisorBindingStageError>
    {
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(SupervisorBindingStageError::SessionNotRegistered)?;
        let mut authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let transition = crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
            &mut *authority,
            crate::meerkat_machine::dsl::MeerkatMachineInput::AuthorizeSupervisor {
                name,
                peer_id,
                address,
                signing_public_key,
                epoch,
            },
        )
        .map_err(SupervisorBindingStageError::Dsl)?;
        Ok(transition)
    }

    /// Stage a DSL `RequestSupervisorTrustPublish` input.
    ///
    /// Used when the current supervisor binding is already correct but
    /// the shell still needs a fresh generated publish obligation before
    /// repairing or reasserting the concrete trust edge.
    pub async fn stage_supervisor_trust_publish_request(
        &self,
        session_id: &SessionId,
        name: String,
        peer_id: String,
        address: String,
        signing_public_key: String,
        epoch: u64,
    ) -> Result<crate::meerkat_machine::dsl::MeerkatMachineTransition, SupervisorBindingStageError>
    {
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(SupervisorBindingStageError::SessionNotRegistered)?;
        let mut authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let transition = crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
            &mut *authority,
            crate::meerkat_machine::dsl::MeerkatMachineInput::RequestSupervisorTrustPublish {
                name,
                peer_id,
                address,
                signing_public_key,
                epoch,
            },
        )
        .map_err(SupervisorBindingStageError::Dsl)?;
        Ok(transition)
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
    ) -> Result<crate::meerkat_machine::dsl::MeerkatMachineTransition, SupervisorBindingStageError>
    {
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(SupervisorBindingStageError::SessionNotRegistered)?;
        let mut authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let transition = crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
            &mut *authority,
            crate::meerkat_machine::dsl::MeerkatMachineInput::RevokeSupervisor { peer_id, epoch },
        )
        .map_err(SupervisorBindingStageError::Dsl)?;
        Ok(transition)
    }

    /// Stage a DSL `SupervisorTrustEdgePublished` feedback input (C-F2 /
    /// wave-d D-d).
    ///
    /// Invoked by `try_handle_supervisor_bridge_command` after a
    /// successful `Router::add_trusted_peer` call. The `epoch` passed
    /// through is the one observed on the originating
    /// `PublishSupervisorTrustEdge` effect (i.e. the epoch of the
    /// `BindSupervisor` / `AuthorizeSupervisor` commit that triggered
    /// the publication). The DSL guard rejects the ack if the binding
    /// has since rotated forward — a stale ack cannot close the
    /// outstanding obligation for the newer epoch.
    pub async fn stage_supervisor_trust_published(
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
            crate::meerkat_machine::dsl::MeerkatMachineInput::SupervisorTrustEdgePublished {
                peer_id,
                epoch,
            },
        )
        .map_err(SupervisorBindingStageError::Dsl)?;
        Ok(())
    }

    /// Stage a DSL `SupervisorTrustEdgePublishFailed` feedback input
    /// (C-F2 / wave-d D-d).
    ///
    /// Invoked when `Router::add_trusted_peer` returns an error. The
    /// `epoch` comes from the originating producer effect; the DSL
    /// guard rejects a stale-epoch ack arriving after the binding has
    /// rotated forward.
    pub async fn stage_supervisor_trust_publish_failed(
        &self,
        session_id: &SessionId,
        peer_id: String,
        epoch: u64,
        reason: String,
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
            crate::meerkat_machine::dsl::MeerkatMachineInput::SupervisorTrustEdgePublishFailed {
                peer_id,
                epoch,
                reason,
            },
        )
        .map_err(SupervisorBindingStageError::Dsl)?;
        Ok(())
    }

    /// Stage a DSL `SupervisorTrustEdgeRevoked` feedback input (C-F2 /
    /// wave-d D-d).
    ///
    /// Invoked after a successful `Router::remove_trusted_peer` call.
    /// Epoch guard semantics mirror `stage_supervisor_trust_published`.
    pub async fn stage_supervisor_trust_revoked(
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
            crate::meerkat_machine::dsl::MeerkatMachineInput::SupervisorTrustEdgeRevoked {
                peer_id,
                epoch,
            },
        )
        .map_err(SupervisorBindingStageError::Dsl)?;
        Ok(())
    }

    /// Stage a DSL `SupervisorTrustEdgeRevokeFailed` feedback input
    /// (C-F2 / wave-d D-d).
    ///
    /// Invoked when `Router::remove_trusted_peer` returns an error.
    /// Epoch guard semantics mirror `stage_supervisor_trust_published`.
    pub async fn stage_supervisor_trust_revoke_failed(
        &self,
        session_id: &SessionId,
        peer_id: String,
        epoch: u64,
        reason: String,
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
            crate::meerkat_machine::dsl::MeerkatMachineInput::SupervisorTrustEdgeRevokeFailed {
                peer_id,
                epoch,
                reason,
            },
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
    /// The target runtime did not expose a complete typed local endpoint for
    /// generated trust-store ownership.
    LocalEndpoint(String),
}

impl std::fmt::Display for SupervisorBindingStageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionNotRegistered => write!(f, "session not registered with runtime"),
            Self::Dsl(err) => write!(f, "DSL rejected supervisor binding input: {err}"),
            Self::LocalEndpoint(err) => {
                write!(f, "local endpoint unavailable for supervisor trust: {err}")
            }
        }
    }
}

impl std::error::Error for SupervisorBindingStageError {}

impl MeerkatMachine {
    /// D-track-b: stage an `AddDirectPeerEndpoint` DSL input and drive
    /// trust reconciliation against the caller-supplied runtime.
    ///
    /// Closes the emitter→consumer gap documented in
    /// `docs/wave-d-prep/track-b-producer-wiring.md`: the DSL owns the
    /// declarative peer set (`direct_peer_endpoints` +
    /// `mob_overlay_peer_endpoints`) and emits
    /// `CommsTrustReconcileRequested`; the reconciler consumes that
    /// effect and mechanically reconciles the underlying
    /// [`meerkat_core::agent::CommsRuntime`] trust store.
    ///
    /// The caller supplies the session's current `CommsRuntime`.
    /// Reconciliation reads that runtime's canonical trust-store
    /// snapshot every pass, so rebinds do not pin peer projection to
    /// an older transport instance.
    pub async fn stage_add_direct_peer_endpoint(
        &self,
        session_id: &SessionId,
        endpoint: crate::meerkat_machine::dsl::PeerEndpoint,
        comms_runtime: Arc<dyn meerkat_core::agent::CommsRuntime>,
    ) -> Result<(), PeerEndpointStageError> {
        let (reconciler, reconcile_obligation) = self
            .stage_peer_projection_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::AddDirectPeerEndpoint {
                    endpoint,
                },
                comms_runtime,
            )
            .await?;
        drive_reconciler(&reconciler, reconcile_obligation).await
    }

    /// D-track-b: stage a `RemoveDirectPeerEndpoint` DSL input and
    /// drive trust reconciliation. See
    /// [`Self::stage_add_direct_peer_endpoint`] for the architectural
    /// contract.
    pub async fn stage_remove_direct_peer_endpoint(
        &self,
        session_id: &SessionId,
        endpoint: crate::meerkat_machine::dsl::PeerEndpoint,
        comms_runtime: Arc<dyn meerkat_core::agent::CommsRuntime>,
    ) -> Result<(), PeerEndpointStageError> {
        let (reconciler, reconcile_obligation) = self
            .stage_peer_projection_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RemoveDirectPeerEndpoint {
                    endpoint,
                },
                comms_runtime,
            )
            .await?;
        drive_reconciler(&reconciler, reconcile_obligation).await
    }

    /// D-track-b: stage an `ApplyMobPeerOverlay` DSL input and drive
    /// trust reconciliation. Used by composition drivers
    /// that recompute the mob-overlay peer set from the MobMachine
    /// wiring graph.
    pub async fn stage_apply_mob_peer_overlay(
        &self,
        session_id: &SessionId,
        epoch: u64,
        endpoints: BTreeSet<crate::meerkat_machine::dsl::PeerEndpoint>,
        comms_runtime: Arc<dyn meerkat_core::agent::CommsRuntime>,
    ) -> Result<(), PeerEndpointStageError> {
        let (reconciler, reconcile_obligation) = self
            .stage_peer_projection_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::ApplyMobPeerOverlay {
                    epoch,
                    endpoints,
                },
                comms_runtime,
            )
            .await?;
        drive_reconciler(&reconciler, reconcile_obligation).await
    }

    /// Apply a peer-projection DSL input, sample the emitted
    /// `CommsTrustReconcileRequested` effect under the same DSL lock,
    /// and return a reconciler for the current runtime with the generated
    /// obligation carrying the post-transition effective peer facts.
    ///
    /// The reconciler is driven OUTSIDE the `sessions` RwLock to avoid
    /// blocking other adapter operations behind trust-store I/O. There
    /// is no helper-local applied truth: each reconcile pass diffs the
    /// supplied runtime's canonical trust-store snapshot against the
    /// DSL-owned effective peer set carried by the generated obligation.
    async fn stage_peer_projection_input(
        &self,
        session_id: &SessionId,
        input: crate::meerkat_machine::dsl::MeerkatMachineInput,
        comms_runtime: Arc<dyn meerkat_core::agent::CommsRuntime>,
    ) -> Result<
        (
            Arc<crate::comms_trust_reconcile::CommsTrustReconciler>,
            crate::protocol_comms_trust_reconcile::CommsTrustReconcileObligation,
        ),
        PeerEndpointStageError,
    > {
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(PeerEndpointStageError::SessionNotRegistered)?;
        let local_endpoint = Self::local_endpoint_for_comms_runtime(comms_runtime.as_ref())
            .map_err(PeerEndpointStageError::LocalEndpoint)?;

        let reconcile_obligation = {
            let freshness_authority =
                crate::protocol_comms_trust_reconcile::PeerProjectionFreshnessAuthority::from_authority(
                    Arc::clone(&entry.dsl_authority),
                );
            let mut authority = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                crate::meerkat_machine::dsl::MeerkatMachineInput::PublishLocalEndpoint {
                    endpoint: local_endpoint,
                },
            )
            .map_err(PeerEndpointStageError::Dsl)?;
            let transition =
                crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(&mut *authority, input)
                    .map_err(PeerEndpointStageError::Dsl)?;
            crate::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
                &transition,
                freshness_authority,
            )
            .into_iter()
            .next()
            .ok_or(PeerEndpointStageError::MissingReconcileEffect)?
        };

        let reconciler = Arc::new(crate::comms_trust_reconcile::CommsTrustReconciler::new(
            comms_runtime,
        ));

        Ok((reconciler, reconcile_obligation))
    }
}

async fn drive_reconciler(
    reconciler: &crate::comms_trust_reconcile::CommsTrustReconciler,
    reconcile_obligation: crate::protocol_comms_trust_reconcile::CommsTrustReconcileObligation,
) -> Result<(), PeerEndpointStageError> {
    reconciler
        .reconcile(&reconcile_obligation)
        .await
        .map(|_report| ())
        .map_err(PeerEndpointStageError::Reconcile)
}

/// Errors raised when staging a peer-projection input against the DSL
/// and driving the session-scoped trust reconciler (D-track-b).
#[derive(Debug)]
pub enum PeerEndpointStageError {
    /// The session is not registered with the runtime.
    SessionNotRegistered,
    /// The DSL mutator rejected the transition (e.g. duplicate endpoint,
    /// stale overlay epoch, or per-phase guard failure).
    Dsl(crate::meerkat_machine::dsl::MeerkatMachineTransitionError),
    /// The DSL transition committed but did not emit
    /// `CommsTrustReconcileRequested`. This indicates a contract
    /// violation between the schema and the runtime — the three
    /// peer-projection transitions are specified to emit the effect
    /// unconditionally.
    MissingReconcileEffect,
    /// The target runtime did not expose a complete typed local endpoint for
    /// generated trust-store ownership.
    LocalEndpoint(String),
    /// The reconciler failed to mechanically reconcile the trust
    /// store.
    Reconcile(crate::comms_trust_reconcile::CommsTrustReconcileError),
}

impl std::fmt::Display for PeerEndpointStageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionNotRegistered => write!(f, "session not registered with runtime"),
            Self::Dsl(err) => write!(f, "DSL rejected peer-projection input: {err}"),
            Self::MissingReconcileEffect => write!(
                f,
                "peer-projection DSL transition committed without emitting CommsTrustReconcileRequested"
            ),
            Self::LocalEndpoint(err) => {
                write!(
                    f,
                    "local endpoint unavailable for trust reconciliation: {err}"
                )
            }
            Self::Reconcile(err) => write!(f, "trust reconciliation failed: {err}"),
        }
    }
}

impl std::error::Error for PeerEndpointStageError {}
