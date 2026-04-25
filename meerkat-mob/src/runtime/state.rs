use super::*;
use crate::machines::mob_machine as mob_dsl;
use crate::run::MobRun;
#[cfg(target_arch = "wasm32")]
use crate::tokio;

// ---------------------------------------------------------------------------
// MobState
// ---------------------------------------------------------------------------

/// Lifecycle state of a mob. Projected from the DSL authority on demand —
/// no shadow truth (see dogma #1, #13, #17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MobState {
    Creating = 0,
    Running = 1,
    Stopped = 2,
    Completed = 3,
    Destroyed = 4,
}

impl MobState {
    /// Human-readable name for the state.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "Creating",
            Self::Running => "Running",
            Self::Stopped => "Stopped",
            Self::Completed => "Completed",
            Self::Destroyed => "Destroyed",
        }
    }
}

impl std::fmt::Display for MobState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Diagnostic snapshots (DSL-projected, shell-owned)
// ---------------------------------------------------------------------------

/// Observable snapshot of mob orchestrator-facing state.
///
/// Projected from the MobMachine DSL state plus shell-owned metadata that is
/// not tracked by the DSL authority (`topology_revision`, `supervisor_active`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobOrchestratorSnapshot {
    pub phase: MobState,
    pub coordinator_bound: bool,
    pub pending_spawn_count: u32,
    pub active_flow_count: u32,
    pub topology_revision: u32,
    pub supervisor_active: bool,
}

impl Default for MobOrchestratorSnapshot {
    fn default() -> Self {
        Self {
            phase: MobState::Creating,
            coordinator_bound: false,
            pending_spawn_count: 0,
            active_flow_count: 0,
            topology_revision: 0,
            supervisor_active: false,
        }
    }
}

/// Observable snapshot of mob lifecycle-facing state.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MobLifecycleSnapshot {
    pub phase: MobState,
    pub active_run_count: u32,
    pub cleanup_pending: bool,
}

#[cfg(test)]
impl Default for MobLifecycleSnapshot {
    fn default() -> Self {
        Self {
            phase: MobState::Running,
            active_run_count: 0,
            cleanup_pending: false,
        }
    }
}

/// Test-only projection of the Phase 5G / T2 DSL fields. Cloned from
/// `MobMachineAuthority.state` inside the actor so the shell sees the DSL
/// authority directly (dogma #1, #13) with no shadow truth on the handle.
/// Types mirror the DSL-scoped types declared by `machines::mob_machine` —
/// they are distinct from the public `crate::ids::*` types.
#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub(crate) struct MobDslT2Snapshot {
    pub member_state_markers: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentRuntimeId,
        crate::machines::mob_machine::MobMemberState,
    >,
    pub wiring_edges: std::collections::BTreeSet<crate::machines::mob_machine::WiringEdge>,
    pub identity_to_runtime: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        crate::machines::mob_machine::AgentRuntimeId,
    >,
    pub tasks: std::collections::BTreeMap<
        crate::machines::mob_machine::TaskId,
        crate::machines::mob_machine::MobTask,
    >,
    pub in_progress_task_ids: std::collections::BTreeSet<crate::machines::mob_machine::TaskId>,
    pub completed_task_ids: std::collections::BTreeSet<crate::machines::mob_machine::TaskId>,
    // W3-H-1: canonical identity→bridge-session binding map, projected from
    // `MobMachineAuthority.state.member_session_bindings`. Used by the
    // runtime-parity snapshot to expose the DSL's realtime binding map to
    // integration tests.
    pub member_session_bindings: std::collections::BTreeMap<
        crate::machines::mob_machine::AgentIdentity,
        crate::machines::mob_machine::SessionId,
    >,
    pub pending_session_ingress_detach_runtime_ids:
        std::collections::BTreeSet<crate::machines::mob_machine::AgentRuntimeId>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct MobStartupKickoffSnapshot {
    pub pending_kickoff_member_ids: std::collections::BTreeSet<String>,
    pub ready_runtime_ids: std::collections::BTreeSet<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct MobMemberMachineProjection {
    pub runtime_id: Option<crate::machines::mob_machine::AgentRuntimeId>,
    pub state_marker: Option<crate::machines::mob_machine::MobMemberState>,
    pub live_runtime: bool,
    pub bound_session_id: Option<crate::machines::mob_machine::SessionId>,
}

/// Heap-allocated payload for `MobCommand::SubmitWork`. Boxing keeps the
/// command-channel variant width bounded by the pointer, not by the full
/// `ContentInput` + render metadata footprint.
pub(super) struct SubmitWorkPayload {
    pub runtime_id: AgentRuntimeId,
    pub fence_token: FenceToken,
    pub work_ref: WorkRef,
    pub content: ContentInput,
    pub origin: WorkOrigin,
    pub handling_mode: meerkat_core::types::HandlingMode,
    pub render_metadata: Option<meerkat_core::types::RenderMetadata>,
}

// ---------------------------------------------------------------------------
// MobCommand
// ---------------------------------------------------------------------------

/// Commands sent from [`MobHandle`] to the [`MobActor`] for serialized processing.
pub(super) enum MobCommand {
    Spawn {
        spec: Box<super::handle::SpawnMemberSpec>,
        owner_bridge_session_id: Option<SessionId>,
        ops_registry: Option<Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>>,
        reply_tx: oneshot::Sender<Result<super::handle::MemberSpawnReceipt, MobError>>,
    },
    SpawnProvisioned {
        spawn_ticket: u64,
        result: Result<super::handle::MemberSpawnReceipt, MobError>,
    },
    Retire {
        agent_identity: MeerkatId,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    Respawn {
        agent_identity: MeerkatId,
        initial_message: Option<ContentInput>,
        reply_tx: oneshot::Sender<
            Result<super::handle::MemberRespawnReceipt, super::handle::MobRespawnError>,
        >,
    },
    RetireAll {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    /// Unified work-lane ingress: the MobMachine DSL decides work-origin
    /// legality (External vs Internal, addressability, live-runtime, phase),
    /// the shell observes the machine's `RequestRuntimeIngress` effect and
    /// dispatches the turn. There is no shell-side re-decision of origin;
    /// the `origin` field is forwarded into the DSL input verbatim.
    ///
    /// Boxed so the `ContentInput` + render/handling metadata doesn't widen
    /// the command channel's per-message stack footprint for every variant.
    SubmitWork {
        payload: Box<SubmitWorkPayload>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    /// Unified work-lane cancellation: the MobMachine DSL owns live-runtime
    /// membership and phase legality via the `CancelAllWork` transition;
    /// fence-token freshness is a shell-level concurrency invariant. The
    /// actor feeds the machine, then interrupts the member when the
    /// transition lands.
    CancelAllWork {
        runtime_id: AgentRuntimeId,
        fence_token: FenceToken,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    #[cfg(feature = "runtime-adapter")]
    KickoffOutcomeResolved {
        agent_identity: MeerkatId,
        outcome: meerkat_runtime::completion::CompletionOutcome,
        ack_tx: oneshot::Sender<()>,
    },
    RunFlow {
        flow_id: FlowId,
        activation_params: serde_json::Value,
        scoped_event_tx: Option<tokio::sync::mpsc::Sender<meerkat_core::ScopedAgentEvent>>,
        reply_tx: oneshot::Sender<Result<RunId, MobError>>,
    },
    CancelFlow {
        run_id: RunId,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    FlowStatus {
        run_id: RunId,
        reply_tx: oneshot::Sender<Result<Option<MobRun>, MobError>>,
    },
    ProjectMachineInput {
        input: Box<mob_dsl::MobMachineInput>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    ProjectMachineSignal {
        signal: mob_dsl::MobMachineSignal,
    },
    FlowFinished {
        run_id: RunId,
    },
    FlowCanceledCleanup {
        run_id: RunId,
    },
    #[cfg(test)]
    FlowTrackerCounts {
        reply_tx: oneshot::Sender<(usize, usize)>,
    },
    #[cfg(test)]
    OrchestratorSnapshot {
        reply_tx: oneshot::Sender<MobOrchestratorSnapshot>,
    },
    #[cfg(test)]
    LifecycleSnapshot {
        reply_tx: oneshot::Sender<MobLifecycleSnapshot>,
    },
    /// Snapshot the T2 DSL field projections (member state markers, wiring
    /// edges, identity→runtime map, tasks + task id sets) directly from the
    /// DSL authority. Test-only read seam used by the runtime-parity
    /// snapshot so external shell code never has to keep a shadow copy
    /// (dogma #1, #13).
    #[cfg(test)]
    DslT2Snapshot {
        reply_tx: oneshot::Sender<MobDslT2Snapshot>,
    },
    StartupKickoffSnapshot {
        reply_tx: oneshot::Sender<MobStartupKickoffSnapshot>,
    },
    MemberMachineProjection {
        agent_identity: crate::ids::AgentIdentity,
        reply_tx: oneshot::Sender<MobMemberMachineProjection>,
    },
    /// W3-H: query the current realtime binding for an identity. Returns
    /// the bridge session id currently bound (projected from the canonical
    /// `member_session_bindings` map) or `None` if the identity has no
    /// binding. Used by the realtime WS surface at open time to initialize
    /// the task-local `current_session_id` for a `MobMember` target.
    CurrentRealtimeBinding {
        agent_identity: crate::ids::AgentIdentity,
        reply_tx: oneshot::Sender<Option<meerkat_core::types::SessionId>>,
    },
    Stop {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    ResumeLifecycle {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    Complete {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    Destroy {
        reply_tx: oneshot::Sender<
            Result<super::handle::MobDestroyReport, super::handle::MobDestroyError>,
        >,
    },
    Reset {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    TaskCreate {
        subject: String,
        description: String,
        blocked_by: Vec<TaskId>,
        reply_tx: oneshot::Sender<Result<TaskId, MobError>>,
    },
    TaskUpdate {
        task_id: TaskId,
        status: TaskStatus,
        owner: Option<AgentIdentity>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    TaskList {
        reply_tx: oneshot::Sender<Vec<MobTask>>,
    },
    TaskGet {
        task_id: TaskId,
        reply_tx: oneshot::Sender<Option<MobTask>>,
    },
    McpServerStates {
        reply_tx: oneshot::Sender<BTreeMap<String, bool>>,
    },
    SubscribeAgentEvents {
        agent_identity: MeerkatId,
        reply_tx: oneshot::Sender<Result<EventStream, MobError>>,
    },
    SubscribeAllAgentEvents {
        reply_tx: oneshot::Sender<Result<Vec<(MeerkatId, EventStream)>, MobError>>,
    },
    RotateSupervisor {
        reply_tx: oneshot::Sender<Result<super::handle::SupervisorRotationReport, MobError>>,
    },
    PollEvents {
        after_cursor: u64,
        limit: usize,
        reply_tx: oneshot::Sender<Result<Vec<crate::event::MobEvent>, MobError>>,
    },
    ReplayAllEvents {
        reply_tx: oneshot::Sender<Result<Vec<crate::event::MobEvent>, MobError>>,
    },
    RecordOperatorActionProvenance {
        tool_name: String,
        authority_context: meerkat_core::service::MobToolAuthorityContext,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    ForceCancel {
        agent_identity: MeerkatId,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    /// Wire a local mob member to a peer target.
    ///
    /// D-wire-handler (#26): the MobMachine DSL owns wiring-graph authority
    /// via `MobMachineInput::WireMembers { edge }`. This command is the
    /// thin shell forward: the actor normalizes `(local, target)` into a
    /// `WiringEdge`, applies the DSL input, and records
    /// `MobEventKind::MembersWired` on acceptance. No shell-side
    /// reconciliation of comms trust edges or peer-added notifications is
    /// performed here (those were shell-authority patterns deleted in
    /// Wave A and not restored in Wave D).
    Wire {
        local: MeerkatId,
        target: super::handle::PeerTarget,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    /// Unwire a local mob member from a peer target. Mirror of `Wire`.
    /// Forwards to `MobMachineInput::UnwireMembers { edge }` and records
    /// `MobEventKind::MembersUnwired` on acceptance.
    Unwire {
        local: MeerkatId,
        target: super::handle::PeerTarget,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    SetSpawnPolicy {
        policy: Option<Arc<dyn super::spawn_policy::SpawnPolicy>>,
        reply_tx: oneshot::Sender<()>,
    },
    Shutdown {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    /// Read the current lifecycle phase directly from the DSL authority.
    /// Routes through the command channel so the actor returns the single
    /// canonical DSL-authority value; there is no atomic shadow (dogma #1,
    /// #13, #17).
    QueryPhase {
        reply_tx: oneshot::Sender<MobState>,
    },
}
