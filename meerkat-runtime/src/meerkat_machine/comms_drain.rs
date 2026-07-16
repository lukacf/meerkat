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
    /// initial bind, a same-authority `AuthorizeSupervisor` version advance,
    /// or a completed durable rotation operation; `epoch` is monotonic.
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

/// Cancellation guard for the spawn-before-slot-publication interval.
///
/// Tokio detaches a task when its bare `JoinHandle` is dropped.  The machine
/// must instead abort any drain whose handle has not yet been installed in the
/// session-owned slot; otherwise request cancellation can leave an invisible
/// producer that survives attachment replacement.
struct UnpublishedCommsDrainTask {
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl UnpublishedCommsDrainTask {
    fn new(handle: tokio::task::JoinHandle<()>) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    fn publish(mut self) -> Result<tokio::task::JoinHandle<()>, RuntimeDriverError> {
        self.handle.take().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "unpublished comms drain task was already published".to_string(),
            )
        })
    }
}

impl Drop for UnpublishedCommsDrainTask {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

struct SupervisorRotationTask {
    handle: crate::tokio::task::JoinHandle<()>,
}

/// Session-owned driver slot for a durable supervisor rotation. The task is
/// only a liveness mechanism; operation identity, phase, and terminal receipts
/// remain generated-machine authority and survive task/process loss.
pub(crate) struct SupervisorRotationTaskSlot {
    task: crate::tokio::sync::Mutex<Option<SupervisorRotationTask>>,
}

impl SupervisorRotationTaskSlot {
    pub(crate) fn new() -> Self {
        Self {
            task: crate::tokio::sync::Mutex::new(None),
        }
    }

    pub(crate) async fn install(
        &self,
        _operation_id: String,
        _runtime: Arc<dyn meerkat_core::agent::CommsRuntime>,
        handle: crate::tokio::task::JoinHandle<()>,
    ) -> bool {
        let mut task = self.task.lock().await;
        if let Some(_current) = task
            .as_ref()
            .filter(|current| !current.handle.is_finished())
        {
            // A live carrier may only be replaced by the explicit
            // peer-ingress-runtime replacement path while that path holds the
            // session mutation gate. An arbitrary later caller cannot prove
            // that its runtime is newer, so last-writer-wins here would allow
            // an old drain to ABA-replace the current carrier and mutate its
            // stale router.
            handle.abort();
            let _ = handle.await;
            return false;
        } else if let Some(previous) = task.take() {
            // Reap a finished task before replacing its slot.
            let _ = previous.handle.await;
        }
        *task = Some(SupervisorRotationTask { handle });
        true
    }

    pub(crate) async fn abort_and_wait(&self) {
        if let Some(handle) = self.abort_keeping_handle().await {
            let _ = handle.await;
        }
    }

    /// Signal cancellation while retaining the join handle so unregister can
    /// prove this session-scoped producer quiesced before final commit.
    pub(crate) async fn abort_keeping_handle(&self) -> Option<crate::tokio::task::JoinHandle<()>> {
        let task = self.task.lock().await.take()?;
        task.handle.abort();
        Some(task.handle)
    }
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

    pub(crate) fn task_is_live_for(
        &self,
        runtime: &Arc<dyn meerkat_core::agent::CommsRuntime>,
    ) -> bool {
        self.task_runtime_matches(runtime)
            && self
                .handle
                .as_ref()
                .is_some_and(|handle| !handle.is_finished())
    }

    pub(crate) fn task_runtime(&self) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
        self.task_runtime.clone()
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

    /// Signal cancellation on the drain task and return its `JoinHandle` so the
    /// caller can await quiescence. The two-phase unregister drain uses this to
    /// abort the drain task *and* join it before committing teardown; awaiting
    /// the returned handle yields `Err(JoinError::is_cancelled())`, which is
    /// benign. The slot is left empty (no runtime, no handle).
    pub(crate) fn abort_keeping_handle(&mut self) -> Option<tokio::task::JoinHandle<()>> {
        self.task_runtime = None;
        let handle = self.handle.take()?;
        handle.abort();
        Some(handle)
    }
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
    #[cfg(test)]
    pub(crate) async fn test_authorize_direct_comms_drain_runtime(
        &self,
        session_id: &SessionId,
        runtime: Arc<dyn meerkat_core::agent::CommsRuntime>,
    ) -> Result<(), String> {
        let placeholder = crate::tokio::spawn(async {});
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or_else(|| "direct-drain test session is not registered".to_string())?;
        {
            let mut authority = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                crate::meerkat_machine::dsl::MeerkatMachineInput::AttachSessionIngress {
                    comms_runtime_id: crate::meerkat_machine::dsl::CommsRuntimeId::from_runtime(
                        &runtime,
                    ),
                },
            )
            .map_err(|error| error.to_string())?;
            crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                crate::meerkat_machine::dsl::MeerkatMachineInput::SpawnDrain {
                    mode: crate::meerkat_machine::dsl::DrainMode::Timed,
                },
            )
            .map_err(|error| error.to_string())?;
        }
        entry.drain_slot.install_task(runtime, placeholder);
        Ok(())
    }

    /// Fence startup mechanics to the peer-ingress runtime currently owned by
    /// the generated machine and installed in the drain slot.
    pub(crate) async fn peer_ingress_runtime_is_current(
        &self,
        session_id: &SessionId,
        runtime: &Arc<dyn meerkat_core::agent::CommsRuntime>,
    ) -> Result<bool, SupervisorBindingStageError> {
        let Some(_gate_guard) = self.lock_current_session_mutation_gate(session_id).await else {
            return Err(SupervisorBindingStageError::SessionNotRegistered);
        };
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(session_id)
            .ok_or(SupervisorBindingStageError::SessionNotRegistered)?;
        let runtime_id = crate::meerkat_machine::dsl::CommsRuntimeId::from_runtime(runtime);
        let authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(
            authority.state().peer_ingress_comms_runtime_id.as_ref() == Some(&runtime_id)
                && entry.drain_slot.task_runtime_matches(runtime),
        )
    }

    /// Install a rotation carrier only while the same session mutation gate
    /// proves the candidate runtime is both generated-current and the runtime
    /// mechanically installed in the comms-drain slot. A stale drain loses the
    /// race without displacing the current carrier.
    pub(crate) async fn install_supervisor_rotation_task_if_current(
        &self,
        session_id: &SessionId,
        operation_id: String,
        runtime: Arc<dyn meerkat_core::agent::CommsRuntime>,
        handle: crate::tokio::task::JoinHandle<()>,
    ) -> Result<bool, SupervisorBindingStageError> {
        let Some(_gate_guard) = self.lock_current_session_mutation_gate(session_id).await else {
            handle.abort();
            let _ = handle.await;
            return Err(SupervisorBindingStageError::SessionNotRegistered);
        };
        let slot = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(SupervisorBindingStageError::SessionNotRegistered)?;
            let runtime_id = crate::meerkat_machine::dsl::CommsRuntimeId::from_runtime(&runtime);
            let authority = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let state = authority.state();
            let pending_operation_matches = state.supervisor_rotation_operation_id.as_deref()
                == Some(operation_id.as_str())
                && matches!(
                    state.supervisor_rotation_phase,
                    Some(
                        crate::meerkat_machine::dsl::SupervisorRotationPhase::PreviousRevokePending
                            | crate::meerkat_machine::dsl::SupervisorRotationPhase::NextPublishPending
                    )
                );
            let is_current = state.peer_ingress_comms_runtime_id.as_ref() == Some(&runtime_id)
                && entry.drain_slot.task_runtime_matches(&runtime)
                && pending_operation_matches;
            if is_current {
                Some(Arc::clone(&entry.supervisor_rotation_task))
            } else {
                None
            }
        };
        let Some(slot) = slot else {
            handle.abort();
            let _ = handle.await;
            return Ok(false);
        };
        Ok(slot.install(operation_id, runtime, handle).await)
    }

    pub(crate) async fn active_supervisor_rotation(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<GeneratedSupervisorRotationReceipt>, SupervisorBindingStageError> {
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(session_id)
            .ok_or(SupervisorBindingStageError::SessionNotRegistered)?;
        let authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = authority.state();
        let Some(operation_id) = state.supervisor_rotation_operation_id.clone() else {
            return Ok(None);
        };
        let (
            Some(phase),
            Some(previous_name),
            Some(previous_peer_id),
            Some(previous_address),
            Some(previous_signing_public_key),
            Some(previous_epoch),
            Some(next_name),
            Some(next_peer_id),
            Some(next_address),
            Some(next_signing_public_key),
            Some(next_epoch),
        ) = (
            state.supervisor_rotation_phase,
            state.supervisor_rotation_previous_name.clone(),
            state.supervisor_rotation_previous_peer_id.clone(),
            state.supervisor_rotation_previous_address.clone(),
            state
                .supervisor_rotation_previous_signing_public_key
                .clone(),
            state.supervisor_rotation_previous_epoch,
            state.supervisor_rotation_next_name.clone(),
            state.supervisor_rotation_next_peer_id.clone(),
            state.supervisor_rotation_next_address.clone(),
            state.supervisor_rotation_next_signing_public_key.clone(),
            state.supervisor_rotation_next_epoch,
        )
        else {
            return Err(SupervisorBindingStageError::Persistence(
                "generated supervisor rotation state was partial".to_string(),
            ));
        };
        Ok(Some(GeneratedSupervisorRotationReceipt {
            operation_id,
            phase,
            rejection: state.supervisor_rotation_rejection,
            previous: GeneratedSupervisorBinding {
                name: previous_name,
                peer_id: previous_peer_id,
                address: previous_address,
                signing_public_key: previous_signing_public_key,
                epoch: previous_epoch,
            },
            next: GeneratedSupervisorBinding {
                name: next_name,
                peer_id: next_peer_id,
                address: next_address,
                signing_public_key: next_signing_public_key,
                epoch: next_epoch,
            },
        }))
    }

    async fn apply_supervisor_binding_input(
        &self,
        session_id: &SessionId,
        input: crate::meerkat_machine::dsl::MeerkatMachineInput,
    ) -> Result<crate::meerkat_machine::dsl::MeerkatMachineTransition, SupervisorBindingStageError>
    {
        let (gate, authority) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(SupervisorBindingStageError::SessionNotRegistered)?;
            (
                Arc::clone(&entry.mutation_gate),
                Arc::clone(&entry.dsl_authority),
            )
        };
        let _gate_guard = Arc::clone(&gate).lock_owned().await;
        {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(SupervisorBindingStageError::SessionNotRegistered)?;
            if !Arc::ptr_eq(&entry.mutation_gate, &gate)
                || !Arc::ptr_eq(&entry.dsl_authority, &authority)
            {
                return Err(SupervisorBindingStageError::SessionNotRegistered);
            }
        }
        let mut authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(&mut *authority, input)
            .map_err(SupervisorBindingStageError::Dsl)
    }

    /// Preview a supervisor-authority transition, durably replace only its
    /// closed projection, then apply the same generated input to live state.
    ///
    /// The preview-before-persist ordering is intentional. Peer-ingress handles
    /// share this DSL authority but do not take the session mutation gate; a
    /// whole-authority snapshot restore after asynchronous store I/O could erase
    /// their concurrent receive/dequeue bookkeeping. Supervisor mutations are
    /// serialized by the gate, so the post-persist live input remains valid and
    /// updates only its generated supervisor fields.
    async fn apply_persisted_supervisor_authority_input(
        &self,
        session_id: &SessionId,
        input: crate::meerkat_machine::dsl::MeerkatMachineInput,
        context: &'static str,
    ) -> Result<crate::meerkat_machine::dsl::MeerkatMachineTransition, SupervisorBindingStageError>
    {
        let (gate, driver, authority) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(SupervisorBindingStageError::SessionNotRegistered)?;
            (
                Arc::clone(&entry.mutation_gate),
                Arc::clone(&entry.driver),
                Arc::clone(&entry.dsl_authority),
            )
        };
        let _gate_guard = Arc::clone(&gate).lock_owned().await;
        {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(SupervisorBindingStageError::SessionNotRegistered)?;
            if !Arc::ptr_eq(&entry.mutation_gate, &gate)
                || !Arc::ptr_eq(&entry.driver, &driver)
                || !Arc::ptr_eq(&entry.dsl_authority, &authority)
            {
                return Err(SupervisorBindingStageError::SessionNotRegistered);
            }
        }
        let projected_supervisor_authority = {
            let authority = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut preview =
                crate::meerkat_machine::dsl::MeerkatMachineAuthority::recover_from_state(
                    authority.state().clone(),
                )
                .map_err(SupervisorBindingStageError::Dsl)?;
            crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(&mut preview, input.clone())
                .map_err(SupervisorBindingStageError::Dsl)?;
            crate::driver::EphemeralRuntimeDriver::supervisor_authority_snapshot_from_state(
                preview.state(),
            )
        };

        driver
            .lock()
            .await
            .persist_current_machine_lifecycle_with_supervisor_authority(
                context,
                projected_supervisor_authority,
            )
            .await
            .map_err(|error| SupervisorBindingStageError::Persistence(error.to_string()))?;

        let mut authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(&mut *authority, input)
            .map_err(SupervisorBindingStageError::Dsl)
    }

    pub async fn update_peer_ingress_context(
        self: &Arc<Self>,
        session_id: &SessionId,
        keep_alive: bool,
        comms_runtime: Option<Arc<dyn meerkat_core::agent::CommsRuntime>>,
    ) -> Result<bool, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_drain_command(MeerkatMachineCommand::SetPeerIngressContext {
                session_id: session_id.clone(),
                keep_alive,
                comms_runtime,
                expected_attachment: None,
                mob_id: None,
            })
            .await?
        {
            MeerkatMachineCommandResult::Spawned(spawned) => Ok(spawned),
            other => Err(RuntimeDriverError::Internal(format!(
                "update_peer_ingress_context: unexpected command result variant: {other:?}"
            ))),
        }
    }

    /// Update session-owned peer ingress only while `witness` remains the
    /// exact committed executor attachment for the session.
    ///
    /// This is the publication seam for surfaces that pair actor-owned comms
    /// state with an executor attachment.  The witness is checked under the
    /// same machine mutation gate that applies the peer-ingress transition,
    /// closing delayed-publication ABA across same-session replacement.
    pub async fn update_peer_ingress_context_if_current(
        self: &Arc<Self>,
        witness: &RuntimeExecutorAttachmentWitness,
        keep_alive: bool,
        comms_runtime: Option<Arc<dyn meerkat_core::agent::CommsRuntime>>,
    ) -> Result<bool, RuntimeDriverError> {
        if !witness.belongs_to(self) {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: "peer-ingress attachment witness belongs to another machine".to_string(),
            });
        }
        match self
            .execute_meerkat_machine_drain_command(MeerkatMachineCommand::SetPeerIngressContext {
                session_id: witness.session_id().clone(),
                keep_alive,
                comms_runtime,
                expected_attachment: Some(witness.clone()),
                mob_id: None,
            })
            .await?
        {
            MeerkatMachineCommandResult::Spawned(spawned) => Ok(spawned),
            other => Err(RuntimeDriverError::Internal(format!(
                "update_peer_ingress_context_if_current: unexpected command result variant: {other:?}"
            ))),
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
    ) -> Result<bool, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_drain_command(MeerkatMachineCommand::SetPeerIngressContext {
                session_id: session_id.clone(),
                keep_alive,
                comms_runtime,
                expected_attachment: None,
                mob_id: None,
            })
            .await?
        {
            MeerkatMachineCommandResult::Spawned(spawned) => Ok(spawned),
            other => Err(RuntimeDriverError::Internal(format!(
                "maybe_spawn_comms_drain: unexpected command result variant: {other:?}"
            ))),
        }
    }

    /// Refresh a session-owned peer ingress drain without re-attaching
    /// ownership.
    ///
    /// This is intentionally narrower than
    /// [`MeerkatMachine::update_peer_ingress_context`]: callers that only need
    /// the existing authorized session-owned transport to be healthy can
    /// respawn a missing/respawnable drain task without staging
    /// `AttachSessionIngress` with a possibly different runtime handle.
    /// Mob-owned ingress and missing cached session runtimes are left
    /// untouched.
    pub async fn refresh_session_owned_peer_ingress(
        self: &Arc<Self>,
        session_id: &SessionId,
    ) -> Result<bool, RuntimeDriverError> {
        if !self.sessions.read().await.contains_key(session_id) {
            return Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            });
        }
        if matches!(
            self.existing_session_runtime_state(session_id).await,
            Some(RuntimeState::Destroyed)
        ) && !self
            .has_terminal_supervisor_cleanup_authority(session_id)
            .await
        {
            return Err(RuntimeDriverError::Destroyed);
        }

        let gate = self.session_mutation_gate(session_id).await;
        let _gate_guard = match gate {
            Some(ref g) => Some(g.lock().await),
            None => None,
        };

        let Some(comms_runtime) = self.session_owned_drain_runtime(session_id).await else {
            return Ok(false);
        };

        self.update_peer_ingress_context_inner(session_id, true, Some(comms_runtime))
            .await
    }

    pub(super) async fn has_terminal_supervisor_cleanup_authority(
        &self,
        session_id: &SessionId,
    ) -> bool {
        let sessions = self.sessions.read().await;
        let Some(entry) = sessions.get(session_id) else {
            return false;
        };
        let authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = authority.state();
        state.supervisor_binding_kind == crate::meerkat_machine::dsl::SupervisorBindingKind::Bound
            || state.supervisor_revoke_pending_peer_id.is_some()
            || state.supervisor_revoked_peer_id.is_some()
            || state.supervisor_rotation_phase.is_some()
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
    ) -> Result<bool, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_drain_command(MeerkatMachineCommand::SetPeerIngressContext {
                session_id: session_id.clone(),
                keep_alive: true,
                comms_runtime: Some(comms_runtime),
                expected_attachment: None,
                mob_id: Some(mob_id),
            })
            .await?
        {
            MeerkatMachineCommandResult::Spawned(spawned) => Ok(spawned),
            other => Err(RuntimeDriverError::Internal(format!(
                "maybe_spawn_mob_comms_drain: unexpected command result variant: {other:?}"
            ))),
        }
    }

    /// Observation-grade serving witness for one materialized member runtime.
    ///
    /// A registered session, generated `Active` executor claim, or retained
    /// comms runtime handle is not independently evidence that the member can
    /// serve peer ingress. This samples the machine lifecycle and drain
    /// authority together with the two process-local task carriers and only
    /// reports `true` for the exact concrete mob-owned comms runtime whose
    /// executor and drain tasks are both still live.
    pub async fn materialized_member_runtime_is_serving(
        &self,
        session_id: &SessionId,
        comms_runtime: &Arc<dyn meerkat_core::agent::CommsRuntime>,
    ) -> Result<bool, RuntimeDriverError> {
        let sessions = self.sessions.read().await;
        let Some(entry) = sessions.get(session_id) else {
            return Ok(false);
        };
        if !entry.generated_executor_registration_active()
            || !entry.has_live_attachment()
            || !entry.drain_slot.task_is_live_for(comms_runtime)
        {
            return Ok(false);
        }

        let control = entry.control_snapshot();
        let authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = authority.state();
        let dsl_phase = super::dsl_authority::runtime_phase_from_authority(&authority);
        let dsl_pre_run_phase = super::dsl_authority::pre_run_phase_from_authority(&authority);
        let visible_phase = super::resolve_visible_runtime_phase(
            dsl_phase,
            dsl_pre_run_phase,
            control.phase,
            control.pre_run_phase,
            self.has_runtime_persistence(),
        )
        .map_err(RuntimeDriverError::Internal)?
        .visible_phase;
        let expected_runtime_id =
            crate::meerkat_machine::dsl::CommsRuntimeId::from_runtime(comms_runtime);

        Ok(matches!(
            visible_phase,
            RuntimeState::Attached | RuntimeState::Running
        ) && state.drain_phase == crate::meerkat_machine::dsl::DrainPhase::Running
            && state.peer_ingress_owner_kind
                == crate::meerkat_machine::dsl::PeerIngressOwnerKind::MobOwned
            && state.peer_ingress_comms_runtime_id.as_ref() == Some(&expected_runtime_id))
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

    async fn session_owned_drain_runtime(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id)?;
        let authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = authority.state();
        if state.peer_ingress_owner_kind
            != crate::meerkat_machine::dsl::PeerIngressOwnerKind::SessionOwned
        {
            return None;
        }
        let expected_runtime_id = state.peer_ingress_comms_runtime_id.as_ref()?;
        let runtime = entry.drain_slot.task_runtime()?;
        let actual_runtime_id = crate::meerkat_machine::dsl::CommsRuntimeId::from_runtime(&runtime);
        (expected_runtime_id == &actual_runtime_id).then_some(runtime)
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
    ) -> Result<bool, RuntimeDriverError> {
        if !keep_alive {
            // The outer SetPeerIngressContext command already holds the
            // session mutation gate. Stop the producers through the
            // gate-assuming helper rather than dispatching Abort, whose public
            // command path acquires the same non-reentrant gate.
            self.stop_and_abort_session_comms_producers(session_id, true)
                .await;
            return Ok(false);
        }

        let mode = CommsDrainMode::PersistentHost;

        let comms = match comms_runtime {
            Some(c) => c,
            None => return Ok(false),
        };

        let runtime_id = crate::meerkat_machine::dsl::CommsRuntimeId::from_runtime(&comms);
        let Some(authority_state) = self.drain_authority_state(session_id).await else {
            tracing::warn!(
                %session_id,
                "refusing to spawn comms drain without generated drain authority"
            );
            return Ok(false);
        };
        if !authority_state.has_peer_runtime(&runtime_id) {
            tracing::warn!(
                %session_id,
                "refusing to spawn comms drain without matching generated peer-ingress authority"
            );
            return Ok(false);
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
                return Ok(false);
            };
            !entry.drain_slot.handle_present() || !entry.drain_slot.task_runtime_matches(&comms)
        } else {
            false
        };

        if !needs_spawn && !needs_task_refresh {
            return Ok(false);
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
                return Ok(false);
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
        let unpublished_task =
            UnpublishedCommsDrainTask::new(crate::comms_drain::spawn_comms_drain(
                Arc::clone(self),
                session_id.clone(),
                comms.clone(),
                idle_timeout,
            ));
        let mut sessions = self.sessions.write().await;
        if let Some(entry) = sessions.get_mut(session_id) {
            entry
                .drain_slot
                .install_task(comms.clone(), unpublished_task.publish()?);
        } else {
            return Ok(false);
        }

        Ok(true)
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
    ) -> Result<(), RuntimeDriverError> {
        self.execute_meerkat_machine_command(
            Some(Arc::clone(self)),
            MeerkatMachineCommand::NotifyDrainExited {
                session_id: session_id.clone(),
                reason,
            },
        )
        .await
        .map_err(MeerkatMachine::driver_error_from_command_error)?;
        Ok(())
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
        if !keep_runtime {
            let rotation_slot = {
                let sessions = self.sessions.read().await;
                sessions
                    .get(session_id)
                    .map(|entry| Arc::clone(&entry.supervisor_rotation_task))
            };
            if let Some(rotation_slot) = rotation_slot {
                rotation_slot.abort_and_wait().await;
            }
        }
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

    pub(crate) async fn project_comms_drain_failed_safety_net(&self, session_id: &SessionId) {
        let keep_runtime = match self.drain_authority_state(session_id).await {
            Some(state) => {
                state.phase == crate::meerkat_machine::dsl::DrainPhase::ExitedRespawnable
            }
            None => false,
        };
        if !keep_runtime {
            let rotation_slot = {
                let sessions = self.sessions.read().await;
                sessions
                    .get(session_id)
                    .map(|entry| Arc::clone(&entry.supervisor_rotation_task))
            };
            if let Some(rotation_slot) = rotation_slot {
                rotation_slot.abort_and_wait().await;
            }
        }
        let mut sessions = self.sessions.write().await;
        if let Some(entry) = sessions.get_mut(session_id) {
            entry.drain_slot.clear_after_exit(keep_runtime);
        }
    }

    /// Abort all active comms drain tasks.
    pub async fn abort_comms_drains(&self) -> Result<(), RuntimeDriverError> {
        self.execute_meerkat_machine_command(None, MeerkatMachineCommand::AbortAll)
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?;
        Ok(())
    }

    /// Abort the comms drain task for a specific session.
    pub async fn abort_comms_drain(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        self.execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::Abort {
                session_id: session_id.clone(),
            },
        )
        .await
        .map_err(MeerkatMachine::driver_error_from_command_error)?;
        Ok(())
    }

    /// Wait for a session's comms drain task to finish.
    ///
    /// Returns immediately if no drain is active for the session.
    /// If the task already notified the authority (normal exit), this is a no-op
    /// for authority state. If the task panicked without notifying, this submits
    /// `TaskExited { Failed }` as a safety net.
    pub async fn wait_comms_drain(&self, session_id: &SessionId) -> Result<(), RuntimeDriverError> {
        self.execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::Wait {
                session_id: session_id.clone(),
            },
        )
        .await
        .map_err(MeerkatMachine::driver_error_from_command_error)?;
        Ok(())
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
        Ok(crate::meerkat_machine::dsl::PeerEndpoint::new(
            name,
            peer_id.to_string(),
            address,
            pubkey,
        ))
    }

    /// Publish the target runtime's own endpoint into MeerkatMachine before
    /// generated trust handoffs mint authority scoped to that trust store.
    pub async fn stage_local_endpoint_for_comms_runtime(
        &self,
        session_id: &SessionId,
        comms_runtime: &dyn meerkat_core::agent::CommsRuntime,
    ) -> Result<(), SupervisorBindingStageError> {
        tracing::debug!(
            %session_id,
            "MeerkatMachine::stage_local_endpoint_for_comms_runtime building endpoint"
        );
        let endpoint = Self::local_endpoint_for_comms_runtime(comms_runtime)
            .map_err(SupervisorBindingStageError::LocalEndpoint)?;
        tracing::debug!(
            %session_id,
            "MeerkatMachine::stage_local_endpoint_for_comms_runtime built endpoint"
        );
        #[cfg(target_arch = "wasm32")]
        let mut sessions = self
            .sessions
            .try_write()
            .map_err(|_| SupervisorBindingStageError::SessionRegistryBusy)?;
        #[cfg(not(target_arch = "wasm32"))]
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(SupervisorBindingStageError::SessionNotRegistered)?;
        #[cfg(target_arch = "wasm32")]
        let mut authority = entry
            .dsl_authority
            .try_lock()
            .map_err(|_| SupervisorBindingStageError::SessionAuthorityBusy)?;
        #[cfg(not(target_arch = "wasm32"))]
        let mut authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tracing::debug!(
            %session_id,
            "MeerkatMachine::stage_local_endpoint_for_comms_runtime applying endpoint"
        );
        if authority.state().local_endpoint.as_ref() == Some(&endpoint) {
            tracing::debug!(
                %session_id,
                "MeerkatMachine::stage_local_endpoint_for_comms_runtime endpoint already applied"
            );
            return Ok(());
        }
        crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
            &mut *authority,
            crate::meerkat_machine::dsl::MeerkatMachineInput::PublishLocalEndpoint { endpoint },
        )
        .map_err(SupervisorBindingStageError::Dsl)?;
        tracing::debug!(
            %session_id,
            "MeerkatMachine::stage_local_endpoint_for_comms_runtime applied endpoint"
        );
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
        self.apply_supervisor_binding_input(
            session_id,
            crate::meerkat_machine::dsl::MeerkatMachineInput::BindSupervisor {
                name,
                peer_id,
                address,
                signing_public_key,
                epoch,
            },
        )
        .await
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
    /// Advances metadata/version for the same bound peer and signing key.
    /// Identity rotation is owned by `SubmitSupervisorRotation`.
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
        self.apply_supervisor_binding_input(
            session_id,
            crate::meerkat_machine::dsl::MeerkatMachineInput::AuthorizeSupervisor {
                name,
                peer_id,
                address,
                signing_public_key,
                epoch,
            },
        )
        .await
    }

    pub(crate) async fn stage_supervisor_binding_route_refresh(
        &self,
        session_id: &SessionId,
        binding: GeneratedSupervisorBinding,
        sender_peer_id: Option<String>,
    ) -> Result<crate::meerkat_machine::dsl::MeerkatMachineTransition, SupervisorBindingStageError>
    {
        self.apply_supervisor_binding_input(
            session_id,
            crate::meerkat_machine::dsl::MeerkatMachineInput::RefreshSupervisorBindingRoute {
                name: binding.name,
                peer_id: binding.peer_id,
                address: binding.address,
                signing_public_key: binding.signing_public_key,
                epoch: binding.epoch,
                sender_peer_id,
            },
        )
        .await
    }

    pub(crate) async fn submit_supervisor_rotation(
        &self,
        session_id: &SessionId,
        submission: GeneratedSupervisorRotationSubmit,
    ) -> Result<SupervisorRotationSubmission, SupervisorBindingStageError> {
        let transition = self
            .apply_persisted_supervisor_authority_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::SubmitSupervisorRotation {
                    operation_id: submission.operation_id,
                    next_name: submission.next.name,
                    next_peer_id: submission.next.peer_id,
                    next_address: submission.next.address,
                    next_signing_public_key: submission.next.signing_public_key,
                    next_epoch: submission.next.epoch,
                    preflight_rejection: submission.preflight_rejection,
                    sender_peer_id: submission.sender_peer_id,
                    sender_signing_public_key: submission.sender_signing_public_key,
                },
                "supervisor rotation submit",
            )
            .await?;
        let resolved = transition.effects().iter().find_map(|effect| match effect {
            crate::meerkat_machine::dsl::MeerkatMachineEffect::SupervisorRotationSubmissionResolved {
                result,
                rejection,
                previous_name,
                previous_peer_id,
                previous_address,
                previous_signing_public_key,
                previous_epoch,
                ..
            } => Some((
                *result,
                *rejection,
                previous_name.clone(),
                previous_peer_id.clone(),
                previous_address.clone(),
                previous_signing_public_key.clone(),
                *previous_epoch,
            )),
            _ => None,
        });
        let Some((result, rejection, name, peer_id, address, signing_public_key, epoch)) = resolved
        else {
            return Err(SupervisorBindingStageError::Persistence(
                "generated supervisor rotation submission omitted its feedback effect".to_string(),
            ));
        };
        let previous = match (name, peer_id, address, signing_public_key, epoch) {
            (Some(name), Some(peer_id), Some(address), Some(signing_public_key), Some(epoch)) => {
                Some(GeneratedSupervisorBinding {
                    name,
                    peer_id,
                    address,
                    signing_public_key,
                    epoch,
                })
            }
            (None, None, None, None, None) => None,
            _ => {
                return Err(SupervisorBindingStageError::Persistence(
                    "generated supervisor rotation submission carried a partial previous binding"
                        .to_string(),
                ));
            }
        };
        use crate::meerkat_machine::dsl::SupervisorRotationSubmissionResultKind as ResultKind;
        match (result, rejection, previous) {
            (ResultKind::New, None, Some(previous)) => {
                Ok(SupervisorRotationSubmission::New(previous))
            }
            (ResultKind::ExistingPending, None, Some(previous)) => {
                Ok(SupervisorRotationSubmission::ExistingPending(previous))
            }
            (ResultKind::ExistingTerminal, _, _) => {
                Ok(SupervisorRotationSubmission::ExistingTerminal)
            }
            (ResultKind::Rejected, Some(rejection), _) => {
                Ok(SupervisorRotationSubmission::Rejected(rejection))
            }
            (ResultKind::Conflict, Some(rejection), _) => {
                Ok(SupervisorRotationSubmission::Conflict(rejection))
            }
            _ => Err(SupervisorBindingStageError::Persistence(
                "generated supervisor rotation submission feedback was inconsistent".to_string(),
            )),
        }
    }

    pub async fn stage_supervisor_rotation_resume(
        &self,
        session_id: &SessionId,
        operation_id: String,
    ) -> Result<crate::meerkat_machine::dsl::MeerkatMachineTransition, SupervisorBindingStageError>
    {
        self.apply_supervisor_binding_input(
            session_id,
            crate::meerkat_machine::dsl::MeerkatMachineInput::ResumeSupervisorRotation {
                operation_id,
            },
        )
        .await
    }

    pub async fn stage_supervisor_rotation_previous_revoked(
        &self,
        session_id: &SessionId,
        operation_id: String,
        peer_id: String,
        epoch: u64,
    ) -> Result<crate::meerkat_machine::dsl::MeerkatMachineTransition, SupervisorBindingStageError>
    {
        self.apply_persisted_supervisor_authority_input(
            session_id,
            crate::meerkat_machine::dsl::MeerkatMachineInput::SupervisorRotationPreviousRevoked {
                operation_id,
                peer_id,
                epoch,
            },
            "supervisor rotation previous revoked",
        )
        .await
    }

    pub async fn stage_supervisor_rotation_next_published(
        &self,
        session_id: &SessionId,
        operation_id: String,
        peer_id: String,
        epoch: u64,
    ) -> Result<crate::meerkat_machine::dsl::MeerkatMachineTransition, SupervisorBindingStageError>
    {
        self.apply_persisted_supervisor_authority_input(
            session_id,
            crate::meerkat_machine::dsl::MeerkatMachineInput::SupervisorRotationNextPublished {
                operation_id,
                peer_id,
                epoch,
            },
            "supervisor rotation next published",
        )
        .await
    }

    pub(crate) async fn observe_supervisor_rotation(
        &self,
        session_id: &SessionId,
        operation_id: String,
        observer: &GeneratedSupervisorBinding,
    ) -> Result<SupervisorRotationObservation, SupervisorBindingStageError> {
        let transition = self
            .apply_supervisor_binding_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::ObserveSupervisorRotation {
                    operation_id,
                    observer_peer_id: Some(observer.peer_id.clone()),
                    observer_signing_public_key: Some(observer.signing_public_key.clone()),
                    observer_epoch: observer.epoch,
                },
            )
            .await?;
        let resolved = transition.effects().iter().find_map(|effect| match effect {
            crate::meerkat_machine::dsl::MeerkatMachineEffect::SupervisorRotationObservationResolved {
                operation_id,
                status,
                rejection,
                previous_name,
                previous_peer_id,
                previous_address,
                previous_signing_public_key,
                previous_epoch,
                next_name,
                next_peer_id,
                next_address,
                next_signing_public_key,
                next_epoch,
            } => Some((
                operation_id.clone(),
                *status,
                *rejection,
                previous_name.clone(),
                previous_peer_id.clone(),
                previous_address.clone(),
                previous_signing_public_key.clone(),
                *previous_epoch,
                next_name.clone(),
                next_peer_id.clone(),
                next_address.clone(),
                next_signing_public_key.clone(),
                *next_epoch,
            )),
            _ => None,
        });
        let Some((
            operation_id,
            status,
            rejection,
            previous_name,
            previous_peer_id,
            previous_address,
            previous_signing_public_key,
            previous_epoch,
            next_name,
            next_peer_id,
            next_address,
            next_signing_public_key,
            next_epoch,
        )) = resolved
        else {
            return Err(SupervisorBindingStageError::Persistence(
                "generated supervisor rotation observation omitted its feedback effect".to_string(),
            ));
        };
        use crate::meerkat_machine::dsl::SupervisorRotationObservationStatusKind as Status;
        if status == Status::NotFound {
            return Ok(SupervisorRotationObservation::NotFound);
        }
        let (
            Some(previous_name),
            Some(previous_peer_id),
            Some(previous_address),
            Some(previous_signing_public_key),
            Some(previous_epoch),
            Some(next_name),
            Some(next_peer_id),
            Some(next_address),
            Some(next_signing_public_key),
            Some(next_epoch),
        ) = (
            previous_name,
            previous_peer_id,
            previous_address,
            previous_signing_public_key,
            previous_epoch,
            next_name,
            next_peer_id,
            next_address,
            next_signing_public_key,
            next_epoch,
        )
        else {
            return Err(SupervisorBindingStageError::Persistence(
                "generated supervisor rotation observation carried a partial receipt".to_string(),
            ));
        };
        let phase = match status {
            Status::PreviousRevokePending => {
                crate::meerkat_machine::dsl::SupervisorRotationPhase::PreviousRevokePending
            }
            Status::NextPublishPending => {
                crate::meerkat_machine::dsl::SupervisorRotationPhase::NextPublishPending
            }
            Status::Completed => crate::meerkat_machine::dsl::SupervisorRotationPhase::Completed,
            Status::Rejected => crate::meerkat_machine::dsl::SupervisorRotationPhase::Rejected,
            Status::NotFound => unreachable!("not-found observation returned above"),
        };
        Ok(SupervisorRotationObservation::Found(
            GeneratedSupervisorRotationReceipt {
                operation_id,
                phase,
                rejection,
                previous: GeneratedSupervisorBinding {
                    name: previous_name,
                    peer_id: previous_peer_id,
                    address: previous_address,
                    signing_public_key: previous_signing_public_key,
                    epoch: previous_epoch,
                },
                next: GeneratedSupervisorBinding {
                    name: next_name,
                    peer_id: next_peer_id,
                    address: next_address,
                    signing_public_key: next_signing_public_key,
                    epoch: next_epoch,
                },
            },
        ))
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
        self.apply_supervisor_binding_input(
            session_id,
            crate::meerkat_machine::dsl::MeerkatMachineInput::RequestSupervisorTrustPublish {
                name,
                peer_id,
                address,
                signing_public_key,
                epoch,
            },
        )
        .await
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
        self.apply_persisted_supervisor_authority_input(
            session_id,
            crate::meerkat_machine::dsl::MeerkatMachineInput::RevokeSupervisor { peer_id, epoch },
            "supervisor revoke begin",
        )
        .await
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
        self.apply_persisted_supervisor_authority_input(
            session_id,
            crate::meerkat_machine::dsl::MeerkatMachineInput::SupervisorTrustEdgePublished {
                peer_id,
                epoch,
            },
            "supervisor trust publish commit",
        )
        .await?;
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
        self.apply_supervisor_binding_input(
            session_id,
            crate::meerkat_machine::dsl::MeerkatMachineInput::SupervisorTrustEdgePublishFailed {
                peer_id,
                epoch,
                reason,
            },
        )
        .await?;
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
        self.apply_persisted_supervisor_authority_input(
            session_id,
            crate::meerkat_machine::dsl::MeerkatMachineInput::SupervisorTrustEdgeRevoked {
                peer_id,
                epoch,
            },
            "supervisor revoke receipt",
        )
        .await?;
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
        self.apply_supervisor_binding_input(
            session_id,
            crate::meerkat_machine::dsl::MeerkatMachineInput::SupervisorTrustEdgeRevokeFailed {
                peer_id,
                epoch,
                reason,
            },
        )
        .await?;
        Ok(())
    }
}

/// Errors raised when staging a supervisor-binding input against the DSL
/// (Wave 3 D Row 21).
#[derive(Debug)]
pub enum SupervisorBindingStageError {
    /// The session is not registered with the runtime.
    SessionNotRegistered,
    /// The runtime session registry was already borrowed in a non-reentrant
    /// WASM turn while staging supervisor binding authority.
    SessionRegistryBusy,
    /// The per-session DSL authority was already borrowed in a non-reentrant
    /// WASM turn while staging supervisor binding authority.
    SessionAuthorityBusy,
    /// The DSL mutator rejected the transition (e.g. guard failure). The
    /// boxed inner is the typed DSL transition error; callers that need to
    /// distinguish guard rejections from missing-transition failures can
    /// match on it.
    Dsl(crate::meerkat_machine::dsl::MeerkatMachineTransitionError),
    /// The target runtime did not expose a complete typed local endpoint for
    /// generated trust-store ownership.
    LocalEndpoint(String),
    /// Durable supervisor-authority replacement failed; live DSL state was
    /// restored to the pre-transition snapshot.
    Persistence(String),
}

impl std::fmt::Display for SupervisorBindingStageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionNotRegistered => write!(f, "session not registered with runtime"),
            Self::SessionRegistryBusy => {
                write!(f, "runtime session registry busy during supervisor binding")
            }
            Self::SessionAuthorityBusy => {
                write!(f, "session authority busy during supervisor binding")
            }
            Self::Dsl(err) => write!(f, "DSL rejected supervisor binding input: {err}"),
            Self::LocalEndpoint(err) => {
                write!(f, "local endpoint unavailable for supervisor trust: {err}")
            }
            Self::Persistence(err) => {
                write!(f, "supervisor authority persistence failed: {err}")
            }
        }
    }
}

impl std::error::Error for SupervisorBindingStageError {}

/// Complete supervisor binding carried across generated admission and
/// rotation seams. The shell uses it mechanically and never classifies its
/// contents outside MeerkatMachine authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GeneratedSupervisorBinding {
    pub name: String,
    pub peer_id: String,
    pub address: String,
    pub signing_public_key: String,
    pub epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GeneratedSupervisorRotationSubmit {
    pub operation_id: String,
    pub next: GeneratedSupervisorBinding,
    pub preflight_rejection: Option<crate::meerkat_machine::dsl::SupervisorRotationRejectionKind>,
    pub sender_peer_id: Option<String>,
    pub sender_signing_public_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GeneratedSupervisorRotationReceipt {
    pub operation_id: String,
    pub phase: crate::meerkat_machine::dsl::SupervisorRotationPhase,
    pub rejection: Option<crate::meerkat_machine::dsl::SupervisorRotationRejectionKind>,
    pub previous: GeneratedSupervisorBinding,
    pub next: GeneratedSupervisorBinding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SupervisorRotationSubmission {
    New(GeneratedSupervisorBinding),
    ExistingPending(GeneratedSupervisorBinding),
    ExistingTerminal,
    Rejected(crate::meerkat_machine::dsl::SupervisorRotationRejectionKind),
    Conflict(crate::meerkat_machine::dsl::SupervisorRotationRejectionKind),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SupervisorRotationObservation {
    Found(GeneratedSupervisorRotationReceipt),
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SupervisorBindAdmission {
    Bootstrap,
    IdempotentAck,
    Rejected(crate::meerkat_machine::dsl::SupervisorBindRejectionKind),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SupervisorAuthorizeAdmission {
    Proceed(GeneratedSupervisorBinding),
    IdempotentAck,
    Rejected(crate::meerkat_machine::dsl::SupervisorAuthorizeRejectionKind),
}

#[derive(Debug)]
pub(crate) enum SupervisorAdmissionStageError {
    SessionNotRegistered,
    Dsl(crate::meerkat_machine::dsl::MeerkatMachineTransitionError),
    MissingAdmissionEffect(&'static str),
    MalformedAdmissionEffect(&'static str),
}

impl std::fmt::Display for SupervisorAdmissionStageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionNotRegistered => write!(f, "session not registered with runtime"),
            Self::Dsl(err) => write!(f, "DSL rejected supervisor admission input: {err}"),
            Self::MissingAdmissionEffect(context) => write!(
                f,
                "{context} admission transition committed without admission feedback"
            ),
            Self::MalformedAdmissionEffect(context) => write!(
                f,
                "{context} admission feedback carried inconsistent result fields"
            ),
        }
    }
}

impl std::error::Error for SupervisorAdmissionStageError {}

impl MeerkatMachine {
    pub(crate) async fn resolve_supervisor_bind_admission(
        &self,
        session_id: &SessionId,
        supervisor_peer_id: String,
        supervisor_epoch: u64,
        sender_peer_id: Option<String>,
    ) -> Result<SupervisorBindAdmission, SupervisorAdmissionStageError> {
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(SupervisorAdmissionStageError::SessionNotRegistered)?;
        let effects = {
            let mut authority = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveSupervisorBindAdmission {
                    supervisor_peer_id,
                    supervisor_epoch,
                    sender_peer_id,
                },
            )
            .map_err(SupervisorAdmissionStageError::Dsl)?
            .into_effects()
        };
        effects
            .iter()
            .find_map(|effect| {
                match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::SupervisorBindAdmissionResolved {
                    result,
                    rejection,
                } => Some((*result, *rejection)),
                _ => None,
            }
            })
            .ok_or(SupervisorAdmissionStageError::MissingAdmissionEffect(
                "bind supervisor",
            ))
            .and_then(|(result, rejection)| match (result, rejection) {
                (
                    crate::meerkat_machine::dsl::SupervisorBindAdmissionResultKind::Bootstrap,
                    None,
                ) => Ok(SupervisorBindAdmission::Bootstrap),
                (
                    crate::meerkat_machine::dsl::SupervisorBindAdmissionResultKind::IdempotentAck,
                    None,
                ) => Ok(SupervisorBindAdmission::IdempotentAck),
                (
                    crate::meerkat_machine::dsl::SupervisorBindAdmissionResultKind::Reject,
                    Some(rejection),
                ) => Ok(SupervisorBindAdmission::Rejected(rejection)),
                _ => Err(SupervisorAdmissionStageError::MalformedAdmissionEffect(
                    "bind supervisor",
                )),
            })
    }

    /// Resolve the material `BindMember` admission verdict (advertised-address
    /// match, raw supervisor-peer sender match, expected runtime peer-id match,
    /// bootstrap-token match) through MeerkatMachine authority. The shell
    /// supplies the four pure boolean observations it already computes; the
    /// machine emits the verdict in the precedence order address → sender →
    /// peer-id → token, else accept. The shell mirrors the returned verdict.
    pub(crate) async fn resolve_supervisor_bind_material_admission(
        &self,
        session_id: &SessionId,
        address_matches: bool,
        sender_matches_supervisor: bool,
        expected_peer_id_matches: bool,
        bootstrap_token_matches: bool,
    ) -> Result<
        crate::meerkat_machine::dsl::SupervisorBindMaterialAdmissionKind,
        SupervisorAdmissionStageError,
    > {
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(SupervisorAdmissionStageError::SessionNotRegistered)?;
        let effects = {
            let mut authority = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveSupervisorBindMaterialAdmission {
                    address_matches,
                    sender_matches_supervisor,
                    expected_peer_id_matches,
                    bootstrap_token_matches,
                },
            )
            .map_err(SupervisorAdmissionStageError::Dsl)?
            .into_effects()
        };
        effects
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::SupervisorBindMaterialAdmissionResolved {
                    verdict,
                } => Some(*verdict),
                _ => None,
            })
            .ok_or(SupervisorAdmissionStageError::MissingAdmissionEffect(
                "bind supervisor material",
            ))
    }

    pub(crate) async fn resolve_supervisor_authorize_admission(
        &self,
        session_id: &SessionId,
        supervisor: GeneratedSupervisorBinding,
        sender_peer_id: Option<String>,
        sender_signing_public_key: Option<String>,
    ) -> Result<SupervisorAuthorizeAdmission, SupervisorAdmissionStageError> {
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(SupervisorAdmissionStageError::SessionNotRegistered)?;
        let effects = {
            let mut authority = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveSupervisorAuthorizeAdmission {
                    supervisor_name: supervisor.name,
                    supervisor_peer_id: supervisor.peer_id,
                    supervisor_address: supervisor.address,
                    supervisor_signing_public_key: supervisor.signing_public_key,
                    supervisor_epoch: supervisor.epoch,
                    sender_peer_id,
                    sender_signing_public_key,
                },
            )
            .map_err(SupervisorAdmissionStageError::Dsl)?
            .into_effects()
        };
        effects
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::SupervisorAuthorizeAdmissionResolved {
                    result,
                    rejection,
                    previous_name,
                    previous_peer_id,
                    previous_address,
                    previous_signing_public_key,
                    previous_epoch,
                } => Some((
                    *result,
                    *rejection,
                    previous_name.clone(),
                    previous_peer_id.clone(),
                    previous_address.clone(),
                    previous_signing_public_key.clone(),
                    *previous_epoch,
                )),
                _ => None,
            })
            .ok_or(SupervisorAdmissionStageError::MissingAdmissionEffect(
                "authorize supervisor",
            ))
            .and_then(
                |(
                    result,
                    rejection,
                    previous_name,
                    previous_peer_id,
                    previous_address,
                    previous_signing_public_key,
                    previous_epoch,
                )| {
                    match (
                        result,
                        rejection,
                        previous_name,
                        previous_peer_id,
                        previous_address,
                        previous_signing_public_key,
                        previous_epoch,
                    ) {
                        (
                            crate::meerkat_machine::dsl::SupervisorAuthorizeAdmissionResultKind::Proceed,
                            None,
                            Some(name),
                            Some(peer_id),
                            Some(address),
                            Some(signing_public_key),
                            Some(epoch),
                        ) => Ok(SupervisorAuthorizeAdmission::Proceed(
                            GeneratedSupervisorBinding {
                                name,
                                peer_id,
                                address,
                                signing_public_key,
                                epoch,
                            },
                        )),
                        (
                            crate::meerkat_machine::dsl::SupervisorAuthorizeAdmissionResultKind::IdempotentAck,
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                        ) => Ok(SupervisorAuthorizeAdmission::IdempotentAck),
                        (
                            crate::meerkat_machine::dsl::SupervisorAuthorizeAdmissionResultKind::Reject,
                            Some(rejection),
                            None,
                            None,
                            None,
                            None,
                            None,
                        ) => Ok(SupervisorAuthorizeAdmission::Rejected(rejection)),
                        _ => Err(SupervisorAdmissionStageError::MalformedAdmissionEffect(
                            "authorize supervisor",
                        )),
                    }
                },
            )
    }
}

/// Generated admission result for a supervisor bridge command that requires
/// the currently bound supervisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SupervisorBridgeCommandAdmission {
    Accepted,
    ResumePendingRevoke,
    Rejected(crate::meerkat_machine::dsl::SupervisorBridgeCommandRejectionKind),
}

#[derive(Debug)]
pub(crate) enum SupervisorBridgeCommandAdmissionStageError {
    SessionNotRegistered,
    Dsl(crate::meerkat_machine::dsl::MeerkatMachineTransitionError),
    MissingAdmissionEffect,
    MalformedAdmissionEffect,
}

impl std::fmt::Display for SupervisorBridgeCommandAdmissionStageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionNotRegistered => write!(f, "session not registered with runtime"),
            Self::Dsl(err) => write!(f, "DSL rejected supervisor bridge admission input: {err}"),
            Self::MissingAdmissionEffect => write!(
                f,
                "supervisor bridge admission transition committed without admission feedback"
            ),
            Self::MalformedAdmissionEffect => write!(
                f,
                "supervisor bridge admission feedback carried inconsistent result fields"
            ),
        }
    }
}

impl std::error::Error for SupervisorBridgeCommandAdmissionStageError {}

impl MeerkatMachine {
    /// Return the generated MeerkatMachine-owned direct peer endpoint set for
    /// callers that must target an exact `RemoveDirectPeerEndpoint` input.
    pub async fn direct_peer_endpoints(
        &self,
        session_id: &SessionId,
    ) -> Result<BTreeSet<crate::meerkat_machine::dsl::PeerEndpoint>, PeerEndpointStageError> {
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(session_id)
            .ok_or(PeerEndpointStageError::SessionNotRegistered)?;
        let authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(authority.state().direct_peer_endpoints.clone())
    }

    pub(crate) async fn resolve_supervisor_bridge_command_admission(
        &self,
        session_id: &SessionId,
        supervisor_peer_id: String,
        supervisor_epoch: u64,
        sender_peer_id: Option<String>,
    ) -> Result<SupervisorBridgeCommandAdmission, SupervisorBridgeCommandAdmissionStageError> {
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(SupervisorBridgeCommandAdmissionStageError::SessionNotRegistered)?;
        let effects = {
            let mut authority = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveSupervisorBridgeCommandAdmission {
                    supervisor_peer_id,
                    supervisor_epoch,
                    sender_peer_id,
                },
            )
            .map_err(SupervisorBridgeCommandAdmissionStageError::Dsl)?
            .into_effects()
        };
        effects
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::SupervisorBridgeCommandAdmissionResolved {
                    result,
                    rejection,
                } => Some((*result, *rejection)),
                _ => None,
            })
            .ok_or(SupervisorBridgeCommandAdmissionStageError::MissingAdmissionEffect)
            .and_then(|(result, rejection)| match (result, rejection) {
                (
                    crate::meerkat_machine::dsl::SupervisorBridgeCommandAdmissionResultKind::Accept,
                    None,
                ) => Ok(SupervisorBridgeCommandAdmission::Accepted),
                (
                    crate::meerkat_machine::dsl::SupervisorBridgeCommandAdmissionResultKind::Reject,
                    Some(rejection),
                ) => Ok(SupervisorBridgeCommandAdmission::Rejected(rejection)),
                _ => Err(SupervisorBridgeCommandAdmissionStageError::MalformedAdmissionEffect),
            })
    }

    /// Resolve lifecycle-aware supervisor cleanup admission. This is the only
    /// bridge admission surface that remains available after the runtime stops
    /// accepting ordinary commands; the DSL owns the command/phase matrix.
    pub(crate) async fn resolve_supervisor_cleanup_command_admission(
        &self,
        session_id: &SessionId,
        command_kind: crate::meerkat_machine::dsl::SupervisorCleanupCommandKind,
        supervisor_peer_id: String,
        supervisor_epoch: u64,
        sender_peer_id: Option<String>,
    ) -> Result<
        (
            SupervisorBridgeCommandAdmission,
            crate::meerkat_machine::dsl::MeerkatPhase,
        ),
        SupervisorBridgeCommandAdmissionStageError,
    > {
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(SupervisorBridgeCommandAdmissionStageError::SessionNotRegistered)?;
        let (phase, effects) = {
            let mut authority = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let phase = authority.state().lifecycle_phase;
            let effects = crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveSupervisorCleanupCommandAdmission {
                    command_kind,
                    supervisor_peer_id,
                    supervisor_epoch,
                    sender_peer_id,
                },
            )
            .map_err(SupervisorBridgeCommandAdmissionStageError::Dsl)?
            .into_effects();
            (phase, effects)
        };
        effects
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::SupervisorBridgeCommandAdmissionResolved {
                    result,
                    rejection,
                } => Some((*result, *rejection)),
                _ => None,
            })
            .ok_or(SupervisorBridgeCommandAdmissionStageError::MissingAdmissionEffect)
            .and_then(|(result, rejection)| match (result, rejection) {
                (
                    crate::meerkat_machine::dsl::SupervisorBridgeCommandAdmissionResultKind::Accept,
                    None,
                ) => Ok((SupervisorBridgeCommandAdmission::Accepted, phase)),
                (
                    crate::meerkat_machine::dsl::SupervisorBridgeCommandAdmissionResultKind::ResumePendingRevoke,
                    None,
                ) => Ok((SupervisorBridgeCommandAdmission::ResumePendingRevoke, phase)),
                (
                    crate::meerkat_machine::dsl::SupervisorBridgeCommandAdmissionResultKind::Reject,
                    Some(rejection),
                ) => Ok((SupervisorBridgeCommandAdmission::Rejected(rejection), phase)),
                _ => Err(SupervisorBridgeCommandAdmissionStageError::MalformedAdmissionEffect),
            })
    }

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
        // Parse-at-boundary: reject a malformed endpoint BEFORE it mutates the
        // machine peer set or emits CommsTrustReconcileRequested.
        validate_peer_endpoint_for_stage(&endpoint)?;
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

    /// Stage the generated absent-endpoint repair path for a direct peer id.
    ///
    /// This is used when machine state already says the direct endpoint is
    /// absent, but the caller needs to re-emit the generated reconciliation
    /// effect so stale trust-store projection rows cannot stay behaviorally
    /// active.
    pub async fn stage_repair_remove_direct_peer_id(
        &self,
        session_id: &SessionId,
        peer_id: String,
        comms_runtime: Arc<dyn meerkat_core::agent::CommsRuntime>,
    ) -> Result<(), PeerEndpointStageError> {
        let endpoint = crate::meerkat_machine::dsl::PeerEndpoint::new(
            "generated-remove-repair",
            peer_id,
            "generated-repair://absent-direct-peer",
            [0; 32],
        );
        self.stage_remove_direct_peer_endpoint(session_id, endpoint, comms_runtime)
            .await
    }

    /// Stage a supervisor-observed mob peer overlay through generated
    /// MeerkatMachine authority before driving trust reconciliation.
    #[allow(clippy::too_many_arguments)]
    pub async fn stage_authorized_supervisor_mob_peer_overlay(
        &self,
        session_id: &SessionId,
        supervisor_peer_id: String,
        supervisor_epoch: u64,
        recipient_peer_id: String,
        overlay_epoch: u64,
        endpoints: BTreeSet<crate::meerkat_machine::dsl::PeerEndpoint>,
        endpoint_count: u64,
        command_peer_id: String,
        command_endpoint: crate::meerkat_machine::dsl::PeerEndpoint,
        command_kind: crate::meerkat_machine::dsl::MobPeerOverlayCommandKind,
        comms_runtime: Arc<dyn meerkat_core::agent::CommsRuntime>,
    ) -> Result<(), PeerEndpointStageError> {
        // Parse-at-boundary: reject any malformed overlay endpoint (the overlay
        // set and the command endpoint) BEFORE mutating the machine peer set.
        for endpoint in &endpoints {
            validate_peer_endpoint_for_stage(endpoint)?;
        }
        validate_peer_endpoint_for_stage(&command_endpoint)?;
        let (reconciler, reconcile_obligation) = self
            .stage_peer_projection_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::AuthorizeSupervisorMobPeerOverlay {
                    supervisor_peer_id,
                    supervisor_epoch,
                    recipient_peer_id,
                    overlay_epoch,
                    endpoints,
                    endpoint_count,
                    command_peer_id,
                    command_endpoint,
                    command_kind,
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
    /// A staged `PeerEndpoint` carried a malformed `peer_id`/`address`/`name`.
    /// Rejected at the ingress boundary (parse-at-boundary) BEFORE any machine
    /// peer-set mutation or effect emission, so invalid identity atoms never
    /// reach `direct_peer_endpoints`/`mob_overlay_peer_endpoints`.
    InvalidEndpoint(crate::comms_trust_reconcile::CommsTrustReconcileError),
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
            Self::InvalidEndpoint(err) => {
                write!(f, "peer endpoint rejected at ingress boundary: {err}")
            }
        }
    }
}

impl std::error::Error for PeerEndpointStageError {}

/// Parse-at-boundary validation for a peer endpoint about to be staged into the
/// MeerkatMachine peer set. Reuses the canonical
/// [`endpoint_to_descriptor`](crate::comms_trust_reconcile::endpoint_to_descriptor)
/// parse so the machine never admits a malformed `peer_id`/`address`/`name`.
fn validate_peer_endpoint_for_stage(
    endpoint: &crate::meerkat_machine::dsl::PeerEndpoint,
) -> Result<(), PeerEndpointStageError> {
    crate::comms_trust_reconcile::endpoint_to_descriptor(endpoint)
        .map(|_| ())
        .map_err(PeerEndpointStageError::InvalidEndpoint)
}
