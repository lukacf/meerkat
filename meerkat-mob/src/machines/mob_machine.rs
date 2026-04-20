//! MobMachine — DSL-generated canonical state.
//!
//! The generated `MobMachineState` is the machine-owned portion of mob state.
//! It covers lifecycle phase, roster membership, run tracking, spawn tracking,
//! and coordinator binding. Shell infrastructure (channels, stores, services,
//! handles, etc.) is NOT modeled here.

use meerkat_machine_dsl::machine;

// ---------------------------------------------------------------------------
// Bridging newtypes
// ---------------------------------------------------------------------------
//
// These types bridge between the DSL's flat representation and the real mob
// domain types in `crate::ids`. The DSL needs Ord+Hash+Clone for Set/Map;
// these newtypes satisfy that while providing From/Into mappings.

/// Bridging type for agent identity. Maps to `crate::ids::AgentIdentity`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentIdentity(pub String);

impl<T: Into<String>> From<T> for AgentIdentity {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Bridging type for agent runtime ID. Maps to `crate::ids::AgentRuntimeId`.
///
/// The real `AgentRuntimeId` is a struct `{ identity: AgentIdentity, generation: Generation }`.
/// The DSL uses a single string key `"identity:generation"` for Set/Map operations.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentRuntimeId(pub String);

impl<T: Into<String>> From<T> for AgentRuntimeId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Bridging type for fence token. Maps to `crate::ids::FenceToken`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FenceToken(pub u64);

impl From<u64> for FenceToken {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

/// Bridging type for generation counter. Maps to `crate::ids::Generation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(pub u64);

impl From<u64> for Generation {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

/// Bridging type for work reference. Maps to `crate::ids::WorkRef`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkId(pub String);

impl<T: Into<String>> From<T> for WorkId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

// ---------------------------------------------------------------------------
// Projection helpers: domain types → bridging types
// ---------------------------------------------------------------------------

impl AgentRuntimeId {
    /// Project a real `AgentRuntimeId` into the DSL bridging type.
    pub fn from_domain(rid: &crate::ids::AgentRuntimeId) -> Self {
        Self(rid.to_string()) // "identity:generation"
    }
}

impl AgentIdentity {
    /// Project a real `AgentIdentity` into the DSL bridging type.
    pub fn from_domain(id: &crate::ids::AgentIdentity) -> Self {
        Self(id.to_string())
    }
}

impl FenceToken {
    /// Project a real `FenceToken` into the DSL bridging type.
    pub fn from_domain(ft: crate::ids::FenceToken) -> Self {
        Self(ft.get())
    }
}

impl Generation {
    /// Project a real `Generation` into the DSL bridging type.
    pub fn from_domain(generation: crate::ids::Generation) -> Self {
        Self(generation.get())
    }
}

impl WorkId {
    /// Project a real `WorkRef` into the DSL bridging type.
    pub fn from_work_ref(wr: &crate::ids::WorkRef) -> Self {
        Self(wr.to_string())
    }
}

/// Bridging type for task identifier. Maps to a shell-side task reference.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(pub String);

impl<T: Into<String>> From<T> for TaskId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Kickoff lifecycle phase for a member's initial autonomous turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KickoffPhase {
    Pending,
    Starting,
    CallbackPending,
    Started,
    Failed,
    Cancelled,
}

/// Task lifecycle status. DSL guards enumerate these directly
/// (`TaskStatus::Pending`, `TaskStatus::InProgress`,
/// `TaskStatus::Completed`, `TaskStatus::Cancelled`). `Completed` and
/// `Cancelled` are the two terminal statuses; neither may be transitioned
/// away from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum TaskStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

/// Opaque task payload carried through the DSL. The full domain type is
/// richer than what the DSL models; only `tasks.contains(id)` is observed in
/// guards. Field projection lives in shell code consuming the DSL state.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct MobTask {
    pub subject: String,
    pub description: String,
    pub status: TaskStatus,
    pub owner: Option<AgentIdentity>,
    pub blocked_by: Vec<TaskId>,
}

/// Per-runtime lifecycle marker tracking whether a member is actively serving
/// work or draining toward retirement. Opaque to DSL guards — observed only
/// at the shell layer for work-routing decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobMemberState {
    #[default]
    Active,
    Retiring,
}

/// Undirected wiring edge between two identities. Callers MUST normalize
/// to `(smaller, larger)` before constructing so that edge equality is
/// independent of insertion order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WiringEdge {
    pub a: AgentIdentity,
    pub b: AgentIdentity,
}

impl WiringEdge {
    /// Constructs an edge, normalizing so `a <= b`.
    pub fn new(lhs: AgentIdentity, rhs: AgentIdentity) -> Self {
        if lhs <= rhs {
            Self { a: lhs, b: rhs }
        } else {
            Self { a: rhs, b: lhs }
        }
    }
}

// ---------------------------------------------------------------------------
// Machine definition
// ---------------------------------------------------------------------------

machine! {
    machine MobMachine {
        version: 1,
        rust: "meerkat-mob" / "machines::mob_machine",

        state {
            lifecycle_phase: MobPhase,
            live_runtime_ids: Set<AgentRuntimeId>,
            externally_addressable_runtime_ids: Set<AgentRuntimeId>,
            runtime_fence_tokens: Map<AgentRuntimeId, FenceToken>,
            active_run_count: u64,
            pending_spawn_count: u64,
            coordinator_bound: bool,
            member_startup_binding_requested: Set<AgentRuntimeId>,
            member_startup_runtime_ready: Set<AgentRuntimeId>,
            member_startup_ready: Set<AgentRuntimeId>,
            member_kickoff_pending: Set<String>,
            member_kickoff_starting: Set<String>,
            member_kickoff_callback_pending: Set<String>,
            member_kickoff_started: Set<String>,
            member_kickoff_failed: Set<String>,
            member_kickoff_cancelled: Set<String>,
            member_kickoff_error: Map<String, String>,
            // Per-runtime lifecycle marker (Active vs Retiring). Tracks the
            // draining/retiring sub-state independently of the mob-level
            // lifecycle phase so the shell can decide whether to route fresh
            // work to a member while retire-drain is in flight.
            member_state_markers: Map<AgentRuntimeId, MobMemberState>,
            // Undirected wiring edges between agent identities. Stored as
            // ordered pairs (smaller identity first) wrapped in WiringEdge
            // so the DSL sees a single opaque key type.
            wiring_edges: Set<WiringEdge>,
            // Identity → current runtime binding. Survives within a
            // generation; respawn replaces the runtime id for the same
            // identity.
            identity_to_runtime: Map<AgentIdentity, AgentRuntimeId>,
            // Task board: the full MobTask payload is opaque at DSL level;
            // lifecycle fields below provide the guard-visible projection.
            tasks: Map<TaskId, MobTask>,
            // Projected status index: guards use these to reject unknown-id
            // updates and illegal status transitions (e.g. Completed→Pending).
            in_progress_task_ids: Set<TaskId>,
            completed_task_ids: Set<TaskId>,
        }

        init(Running) {
            live_runtime_ids = EmptySet,
            externally_addressable_runtime_ids = EmptySet,
            runtime_fence_tokens = EmptyMap,
            active_run_count = 0,
            pending_spawn_count = 0,
            coordinator_bound = true,
            member_startup_binding_requested = EmptySet,
            member_startup_runtime_ready = EmptySet,
            member_startup_ready = EmptySet,
            member_kickoff_pending = EmptySet,
            member_kickoff_starting = EmptySet,
            member_kickoff_callback_pending = EmptySet,
            member_kickoff_started = EmptySet,
            member_kickoff_failed = EmptySet,
            member_kickoff_cancelled = EmptySet,
            member_kickoff_error = EmptyMap,
            member_state_markers = EmptyMap,
            wiring_edges = EmptySet,
            identity_to_runtime = EmptyMap,
            tasks = EmptyMap,
            in_progress_task_ids = EmptySet,
            completed_task_ids = EmptySet,
        }

        terminal [Destroyed]

        phase MobPhase {
            Running,
            Stopped,
            Completed,
            Destroyed,
        }

        input MobMachineInput {
            RunFlow,
            CancelFlow,
            FlowStatus,
            Spawn { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, external_addressable: bool },
            Retire { agent_runtime_id: AgentRuntimeId },
            Respawn { agent_runtime_id: AgentRuntimeId },
            RetireAll,
            Wire,
            Unwire,
            ExternalTurn,
            InternalTurn,
            SubmitWork { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId, origin: String },
            CancelWork { work_id: WorkId },
            CancelAllWork { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            Stop,
            Resume,
            Complete,
            Reset,
            Destroy,
            TaskCreate { task_id: TaskId, task_payload: MobTask },
            TaskUpdate { task_id: TaskId, new_status: TaskStatus },
            TaskList,
            TaskGet,
            McpServerStates,
            RosterSnapshot,
            ListMembers,
            ListMembersIncludingRetiring,
            ListAllMembers,
            MemberStatus,
            SubscribeAgentEvents,
            SubscribeAllAgentEvents,
            SubscribeMobEvents,
            PollEvents,
            ReplayAllEvents,
            RecordOperatorActionProvenance,
            GetMember,
            SetSpawnPolicy,
            Shutdown,
            ForceCancel,
            KickoffMarkPending { member_id: String },
            KickoffMarkStarting { member_id: String },
            StartupMarkReady { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            KickoffResolveStarted { member_id: String },
            KickoffResolveCallbackPending { member_id: String },
            KickoffResolveFailed { member_id: String, error: String },
            KickoffResolveCancelled { member_id: String },
            KickoffCancelRequested { member_id: String },
            KickoffClear { member_id: String },
        }

        surface_only [
            FlowStatus,
            TaskList,
            TaskGet,
            McpServerStates,
            RosterSnapshot,
            ListMembers,
            ListMembersIncludingRetiring,
            ListAllMembers,
            MemberStatus,
            CancelWork,
            PollEvents,
            ReplayAllEvents,
            GetMember
        ]

        signal MobMachineSignal {
            ObserveRuntimeReady { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            RetireMember { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            ObserveRuntimeRetired { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            ResetMember { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, external_addressable: bool },
            RespawnMember { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, external_addressable: bool },
            DestroyMob,
            ObserveRuntimeDestroyed { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            MarkCompleted,
            StartRun,
            FinishRun,
            BeginCleanup,
            FinishCleanup,
            InitializeOrchestrator,
            BindCoordinator,
            UnbindCoordinator,
            StageSpawn,
            CompleteSpawn,
            StartFlow,
            CompleteFlow,
            StopOrchestrator,
            ResumeOrchestrator,
            DestroyOrchestrator,
            ForceCancelMember,
            MemberPeerExposed,
            MemberTerminalized,
            OperationPeerTrusted,
            PeerInputAdmitted,
            CreateRun,
        }

        effect MobMachineEffect {
            RequestRuntimeBinding { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation },
            RequestRuntimeIngress { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId, origin: String },
            RequestRuntimeRetire,
            RequestRuntimeDestroy,
            EmitMemberLifecycleNotice { kind: String },
            EmitRunLifecycleNotice,
            EmitFlowRunNotice,
            AppendFailureLedger,
            FlowTerminalized,
            EscalateSupervisor,
            NotifyCoordinator,
            ExposePendingSpawn,
            EmitMemberTerminalNotice,
            AdmitPeerInput,
            EmitProgressNote,
            EmitTaskNotice,
            PersistKickoffUpdate { member_id: String, phase: KickoffPhase },
            PersistKickoffFailureUpdate { member_id: String, phase: KickoffPhase, error: String },
            EmitKickoffLifecycleNotice { member_id: String, intent: String },
        }

        disposition RequestRuntimeBinding => routed [MeerkatMachine],
        disposition RequestRuntimeIngress => routed [MeerkatMachine],
        disposition RequestRuntimeRetire => routed [MeerkatMachine],
        disposition RequestRuntimeDestroy => routed [MeerkatMachine],
        disposition EmitMemberLifecycleNotice => external,
        disposition EmitRunLifecycleNotice => external,
        disposition EmitFlowRunNotice => external,
        disposition AppendFailureLedger => local,
        disposition FlowTerminalized => external,
        disposition EscalateSupervisor => external,
        disposition NotifyCoordinator => external,
        disposition ExposePendingSpawn => external,
        disposition EmitMemberTerminalNotice => external,
        disposition AdmitPeerInput => external,
        disposition EmitProgressNote => external,
        disposition EmitTaskNotice => external,
        disposition PersistKickoffUpdate => local,
        disposition PersistKickoffFailureUpdate => local,
        disposition EmitKickoffLifecycleNotice => external,

        // =====================================================================
        // Direct transitions
        // =====================================================================

        transition SpawnRunning {
            on input Spawn { agent_identity, agent_runtime_id, fence_token, generation, external_addressable }
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            update {
                // Spawn is the "member joined live_runtime_ids" fact. The
                // pending_spawn_count lifecycle is owned by StageSpawn (+1)
                // and CompleteSpawn (-1) signals; Spawn itself leaves the
                // counter untouched. Concurrent spawn batches rely on that
                // separation — zeroing or double-decrementing here breaks
                // alignment between DSL pending_spawn_count and the actor's
                // pending_spawns map
                // (test_concurrent_spawns_parallelize_provisioning).
                // active_run_count is unrelated to spawn and stays untouched
                // (previously zeroed here by mistake).
                self.live_runtime_ids.insert(agent_runtime_id);
                if external_addressable {
                    self.externally_addressable_runtime_ids.insert(agent_runtime_id);
                } else {
                    self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                }
                self.runtime_fence_tokens.insert(agent_runtime_id, fence_token);
                self.identity_to_runtime.insert(agent_identity, agent_runtime_id);
                self.member_startup_binding_requested.insert(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
            }
            to Running
            emit RequestRuntimeBinding { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, generation: generation }
            emit EmitMemberLifecycleNotice { kind: "spawned" }
        }

        transition ObserveRuntimeReady {
            on signal ObserveRuntimeReady { agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Running }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            update {
                self.member_startup_binding_requested.remove(agent_runtime_id);
                self.member_startup_runtime_ready.insert(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
            }
            to Running
        }

        transition StartupMarkReady {
            per_phase [Running, Stopped, Completed]
            on input StartupMarkReady { agent_runtime_id, fence_token }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            update {
                self.member_startup_binding_requested.remove(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.insert(agent_runtime_id);
            }
            to Running
        }

        transition KickoffMarkPending {
            per_phase [Running, Stopped, Completed]
            on input KickoffMarkPending { member_id }
            guard "kickoff_not_started" {
                !self.member_kickoff_pending.contains(member_id)
                && !self.member_kickoff_starting.contains(member_id)
                && !self.member_kickoff_callback_pending.contains(member_id)
                && !self.member_kickoff_started.contains(member_id)
                && !self.member_kickoff_failed.contains(member_id)
                && !self.member_kickoff_cancelled.contains(member_id)
            }
            update {
                self.member_kickoff_pending.insert(member_id);
                self.member_kickoff_starting.remove(member_id);
                self.member_kickoff_callback_pending.remove(member_id);
                self.member_kickoff_started.remove(member_id);
                self.member_kickoff_failed.remove(member_id);
                self.member_kickoff_cancelled.remove(member_id);
                self.member_kickoff_error.remove(member_id);
            }
            to Running
            emit PersistKickoffUpdate { member_id: member_id, phase: KickoffPhase::Pending }
            emit EmitKickoffLifecycleNotice { member_id: member_id, intent: "Pending" }
        }

        transition KickoffMarkStarting {
            per_phase [Running, Stopped, Completed]
            on input KickoffMarkStarting { member_id }
            guard "kickoff_pending" { self.member_kickoff_pending.contains(member_id) }
            update {
                self.member_kickoff_pending.remove(member_id);
                self.member_kickoff_starting.insert(member_id);
                self.member_kickoff_callback_pending.remove(member_id);
                self.member_kickoff_started.remove(member_id);
                self.member_kickoff_failed.remove(member_id);
                self.member_kickoff_cancelled.remove(member_id);
                self.member_kickoff_error.remove(member_id);
            }
            to Running
            emit PersistKickoffUpdate { member_id: member_id, phase: KickoffPhase::Starting }
            emit EmitKickoffLifecycleNotice { member_id: member_id, intent: "Starting" }
        }

        transition KickoffResolveStarted {
            per_phase [Running, Stopped, Completed]
            on input KickoffResolveStarted { member_id }
            guard "kickoff_starting" { self.member_kickoff_starting.contains(member_id) }
            update {
                self.member_kickoff_pending.remove(member_id);
                self.member_kickoff_starting.remove(member_id);
                self.member_kickoff_callback_pending.remove(member_id);
                self.member_kickoff_started.insert(member_id);
                self.member_kickoff_failed.remove(member_id);
                self.member_kickoff_cancelled.remove(member_id);
                self.member_kickoff_error.remove(member_id);
            }
            to Running
            emit PersistKickoffUpdate { member_id: member_id, phase: KickoffPhase::Started }
            emit EmitKickoffLifecycleNotice { member_id: member_id, intent: "Started" }
        }

        transition KickoffResolveCallbackPending {
            per_phase [Running, Stopped, Completed]
            on input KickoffResolveCallbackPending { member_id }
            guard "kickoff_starting" { self.member_kickoff_starting.contains(member_id) }
            update {
                self.member_kickoff_pending.remove(member_id);
                self.member_kickoff_starting.remove(member_id);
                self.member_kickoff_callback_pending.insert(member_id);
                self.member_kickoff_started.remove(member_id);
                self.member_kickoff_failed.remove(member_id);
                self.member_kickoff_cancelled.remove(member_id);
                self.member_kickoff_error.remove(member_id);
            }
            to Running
            emit PersistKickoffUpdate { member_id: member_id, phase: KickoffPhase::CallbackPending }
            emit EmitKickoffLifecycleNotice { member_id: member_id, intent: "CallbackPending" }
        }

        transition KickoffResolveFailedFromStarting {
            per_phase [Running, Stopped, Completed]
            on input KickoffResolveFailed { member_id, error }
            guard "kickoff_active_failed" {
                (self.member_kickoff_pending.contains(member_id)
                    || self.member_kickoff_starting.contains(member_id)
                    || self.member_kickoff_callback_pending.contains(member_id))
            }
            update {
                self.member_kickoff_pending.remove(member_id);
                self.member_kickoff_starting.remove(member_id);
                self.member_kickoff_callback_pending.remove(member_id);
                self.member_kickoff_started.remove(member_id);
                self.member_kickoff_failed.insert(member_id);
                self.member_kickoff_cancelled.remove(member_id);
                self.member_kickoff_error.insert(member_id, error);
            }
            to Running
            emit PersistKickoffFailureUpdate { member_id: member_id, phase: KickoffPhase::Failed, error: error }
            emit EmitKickoffLifecycleNotice { member_id: member_id, intent: "Failed" }
        }

        transition KickoffResolveCancelled {
            per_phase [Running, Stopped, Completed]
            on input KickoffResolveCancelled { member_id }
            guard "kickoff_cancelled" { !self.member_kickoff_started.contains(member_id) }
            update {
                self.member_kickoff_pending.remove(member_id);
                self.member_kickoff_starting.remove(member_id);
                self.member_kickoff_callback_pending.remove(member_id);
                self.member_kickoff_started.remove(member_id);
                self.member_kickoff_failed.remove(member_id);
                self.member_kickoff_cancelled.insert(member_id);
                self.member_kickoff_error.remove(member_id);
            }
            to Running
            emit PersistKickoffUpdate { member_id: member_id, phase: KickoffPhase::Cancelled }
            emit EmitKickoffLifecycleNotice { member_id: member_id, intent: "Cancelled" }
        }

        transition KickoffCancelRequested {
            per_phase [Running, Stopped, Completed]
            on input KickoffCancelRequested { member_id }
            guard "kickoff_cancellable" {
                (self.member_kickoff_pending.contains(member_id)
                    || self.member_kickoff_starting.contains(member_id)
                    || self.member_kickoff_callback_pending.contains(member_id))
            }
            update {
                self.member_kickoff_pending.remove(member_id);
                self.member_kickoff_starting.remove(member_id);
                self.member_kickoff_callback_pending.remove(member_id);
                self.member_kickoff_started.remove(member_id);
                self.member_kickoff_failed.remove(member_id);
                self.member_kickoff_cancelled.insert(member_id);
                self.member_kickoff_error.remove(member_id);
            }
            to Running
            emit PersistKickoffUpdate { member_id: member_id, phase: KickoffPhase::Cancelled }
            emit EmitKickoffLifecycleNotice { member_id: member_id, intent: "Cancelled" }
        }

        transition KickoffClear {
            per_phase [Running, Stopped, Completed]
            on input KickoffClear { member_id }
            update {
                self.member_kickoff_pending.remove(member_id);
                self.member_kickoff_starting.remove(member_id);
                self.member_kickoff_callback_pending.remove(member_id);
                self.member_kickoff_started.remove(member_id);
                self.member_kickoff_failed.remove(member_id);
                self.member_kickoff_cancelled.remove(member_id);
                self.member_kickoff_error.remove(member_id);
            }
            to Running
        }

        transition SubmitWorkRunningExternal {
            on input SubmitWork { agent_runtime_id, fence_token, work_id, origin }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "external_origin" { origin == "External" }
            guard "runtime_externally_addressable" { self.externally_addressable_runtime_ids.contains(agent_runtime_id) }
            update {}
            to Running
            emit RequestRuntimeIngress { agent_runtime_id: agent_runtime_id, fence_token: fence_token, work_id: work_id, origin: origin }
        }

        transition SubmitWorkRunningInternal {
            on input SubmitWork { agent_runtime_id, fence_token, work_id, origin }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "internal_origin" { origin == "Internal" }
            update {}
            to Running
            emit RequestRuntimeIngress { agent_runtime_id: agent_runtime_id, fence_token: fence_token, work_id: work_id, origin: origin }
        }

        transition RetireMember {
            on signal RetireMember { agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Running }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
            }
            to Running
            emit RequestRuntimeRetire
        }

        transition ObserveRuntimeRetired {
            on signal ObserveRuntimeRetired { agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Running }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            update {
                self.live_runtime_ids.remove(agent_runtime_id);
                self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                self.runtime_fence_tokens.remove(agent_runtime_id);
                self.member_startup_binding_requested.remove(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_state_markers.remove(agent_runtime_id);
                self.active_run_count = 0;
            }
            to Stopped
            emit EmitMemberLifecycleNotice { kind: "retired" }
        }

        transition ResetMember {
            on signal ResetMember { agent_identity, agent_runtime_id, fence_token, generation, external_addressable }
            guard {
                self.lifecycle_phase == Phase::Running
                || self.lifecycle_phase == Phase::Stopped
            }
            update {
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.live_runtime_ids.insert(agent_runtime_id);
                if external_addressable {
                    self.externally_addressable_runtime_ids.insert(agent_runtime_id);
                } else {
                    self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                }
                self.runtime_fence_tokens.insert(agent_runtime_id, fence_token);
                self.identity_to_runtime.insert(agent_identity, agent_runtime_id);
                self.member_startup_binding_requested.insert(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
            }
            to Running
            emit RequestRuntimeBinding { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, generation: generation }
            emit EmitMemberLifecycleNotice { kind: "reset" }
        }

        transition RespawnMember {
            on signal RespawnMember { agent_identity, agent_runtime_id, fence_token, generation, external_addressable }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.live_runtime_ids.insert(agent_runtime_id);
                if external_addressable {
                    self.externally_addressable_runtime_ids.insert(agent_runtime_id);
                } else {
                    self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                }
                self.runtime_fence_tokens.insert(agent_runtime_id, fence_token);
                self.identity_to_runtime.insert(agent_identity, agent_runtime_id);
                self.member_startup_binding_requested.insert(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
            }
            to Running
            emit RequestRuntimeBinding { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, generation: generation }
            emit EmitMemberLifecycleNotice { kind: "respawned" }
        }

        transition MarkCompleted {
            on signal MarkCompleted
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped }
            guard "no_active_runs" { self.active_run_count == 0 }
            update {}
            to Completed
            emit EmitMemberLifecycleNotice { kind: "completed" }
        }

        transition DestroyMob {
            on signal DestroyMob
            guard {
                self.lifecycle_phase == Phase::Running
                || self.lifecycle_phase == Phase::Stopped
                || self.lifecycle_phase == Phase::Completed
            }
            update {
                self.live_runtime_ids = EmptySet;
                self.runtime_fence_tokens = EmptyMap;
                self.member_startup_binding_requested = EmptySet;
                self.member_startup_runtime_ready = EmptySet;
                self.member_startup_ready = EmptySet;
                self.member_state_markers = EmptyMap;
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.coordinator_bound = false;
            }
            to Destroyed
            emit RequestRuntimeDestroy
        }

        transition ObserveRuntimeDestroyed {
            on signal ObserveRuntimeDestroyed { agent_runtime_id, fence_token }
            guard {
                self.lifecycle_phase == Phase::Running
                || self.lifecycle_phase == Phase::Stopped
                || self.lifecycle_phase == Phase::Completed
                || self.lifecycle_phase == Phase::Destroyed
            }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            update {
                self.live_runtime_ids = EmptySet;
                self.runtime_fence_tokens = EmptyMap;
                self.member_state_markers = EmptyMap;
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.coordinator_bound = false;
            }
            to Destroyed
            emit EmitMemberLifecycleNotice { kind: "destroyed" }
        }

        // =====================================================================
        // Absorbed transitions: per-phase self-loops
        // =====================================================================

        transition RecordOperatorActionProvenanceRunning {
            on input RecordOperatorActionProvenance
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
        }
        transition RecordOperatorActionProvenanceStopped {
            on input RecordOperatorActionProvenance
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
        }
        transition RecordOperatorActionProvenanceCompleted {
            on input RecordOperatorActionProvenance
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Completed
        }
        transition RecordOperatorActionProvenanceDestroyed {
            on input RecordOperatorActionProvenance
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
        }

        transition SetSpawnPolicyRunning {
            on input SetSpawnPolicy
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
        }
        transition SetSpawnPolicyStopped {
            on input SetSpawnPolicy
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
        }
        transition SetSpawnPolicyCompleted {
            on input SetSpawnPolicy
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Completed
        }
        transition SetSpawnPolicyDestroyed {
            on input SetSpawnPolicy
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
        }

        // =====================================================================
        // Phase-changing transitions
        // =====================================================================

        transition StopRunning {
            on input Stop
            guard { self.lifecycle_phase == Phase::Running }
            guard "no_active_runs" { self.active_run_count == 0 }
            update {
                self.coordinator_bound = false;
                self.active_run_count = 0;
            }
            to Stopped
            emit EmitRunLifecycleNotice
        }

        transition ResumeStopped {
            on input Resume
            guard { self.lifecycle_phase == Phase::Stopped }
            update {
                self.coordinator_bound = true;
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition CompleteRunning {
            on input Complete
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_run_count = 0;
            }
            to Completed
            emit EmitRunLifecycleNotice
        }

        transition ResetToRunning {
            on input Reset
            guard {
                self.lifecycle_phase == Phase::Running
                || self.lifecycle_phase == Phase::Stopped
                || self.lifecycle_phase == Phase::Completed
            }
            update {
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.coordinator_bound = true;
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        // =====================================================================
        // Running self-loops (inputs)
        // =====================================================================

        transition WireRunning {
            on input Wire
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit NotifyCoordinator
        }

        transition ExternalTurnRunning {
            on input ExternalTurn
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit EmitProgressNote
        }

        transition InternalTurnRunning {
            on input InternalTurn
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit EmitProgressNote
        }

        // TaskCreate: real mutator. Rejects duplicate task ids.
        transition TaskCreateRunning {
            on input TaskCreate { task_id, task_payload }
            guard { self.lifecycle_phase == Phase::Running }
            guard "task_id_unused" { self.tasks.contains_key(task_id) == false }
            update {
                self.tasks.insert(task_id, task_payload);
            }
            to Running
            emit EmitTaskNotice
        }

        // TaskUpdate: status transition authority.
        //
        // Split into one transition per target status. Each enforces:
        //   * task_id must refer to an existing task (unknown ids rejected)
        //   * the source status is not a terminal one we disallow rolling
        //     back from (Completed → Pending / InProgress is rejected).
        //
        // The status-projection sets (`in_progress_task_ids`,
        // `completed_task_ids`) are the guard-visible truth; `tasks` carries
        // the opaque payload for shell-side projection.
        transition TaskUpdateRunningPending {
            on input TaskUpdate { task_id, new_status }
            guard { self.lifecycle_phase == Phase::Running }
            guard "target_pending" { new_status == TaskStatus::Pending }
            guard "task_known" { self.tasks.contains_key(task_id) == true }
            guard "not_completed" { self.completed_task_ids.contains(task_id) == false }
            update {
                self.in_progress_task_ids.remove(task_id);
            }
            to Running
            emit EmitTaskNotice
        }

        transition TaskUpdateRunningInProgress {
            on input TaskUpdate { task_id, new_status }
            guard { self.lifecycle_phase == Phase::Running }
            guard "target_in_progress" { new_status == TaskStatus::InProgress }
            guard "task_known" { self.tasks.contains_key(task_id) == true }
            guard "not_completed" { self.completed_task_ids.contains(task_id) == false }
            update {
                self.in_progress_task_ids.insert(task_id);
            }
            to Running
            emit EmitTaskNotice
        }

        transition TaskUpdateRunningCompleted {
            on input TaskUpdate { task_id, new_status }
            guard { self.lifecycle_phase == Phase::Running }
            guard "target_completed" { new_status == TaskStatus::Completed }
            guard "task_known" { self.tasks.contains_key(task_id) == true }
            update {
                self.in_progress_task_ids.remove(task_id);
                self.completed_task_ids.insert(task_id);
            }
            to Running
            emit EmitTaskNotice
        }

        transition TaskUpdateRunningCancelled {
            on input TaskUpdate { task_id, new_status }
            guard { self.lifecycle_phase == Phase::Running }
            guard "target_cancelled" { new_status == TaskStatus::Cancelled }
            guard "task_known" { self.tasks.contains_key(task_id) == true }
            guard "not_completed" { self.completed_task_ids.contains(task_id) == false }
            update {
                self.in_progress_task_ids.remove(task_id);
            }
            to Running
            emit EmitTaskNotice
        }

        transition ForceCancelRunning {
            on input ForceCancel
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_run_count = 0;
            }
            to Running
            emit FlowTerminalized
        }

        // =====================================================================
        // Subscribe commands
        // =====================================================================

        transition SubscribeAgentEventsRunning {
            on input SubscribeAgentEvents
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            update {}
            to Running
        }
        transition SubscribeAgentEventsStopped {
            on input SubscribeAgentEvents
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            update {}
            to Stopped
        }
        transition SubscribeAgentEventsCompleted {
            on input SubscribeAgentEvents
            guard { self.lifecycle_phase == Phase::Completed }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            update {}
            to Completed
        }
        transition SubscribeAgentEventsDestroyed {
            on input SubscribeAgentEvents
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            update {}
            to Destroyed
        }

        transition SubscribeAllAgentEventsRunning {
            on input SubscribeAllAgentEvents
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
        }
        transition SubscribeAllAgentEventsStopped {
            on input SubscribeAllAgentEvents
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
        }
        transition SubscribeAllAgentEventsCompleted {
            on input SubscribeAllAgentEvents
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Completed
        }
        transition SubscribeAllAgentEventsDestroyed {
            on input SubscribeAllAgentEvents
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
        }

        transition SubscribeMobEventsRunning {
            on input SubscribeMobEvents
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
        }
        transition SubscribeMobEventsStopped {
            on input SubscribeMobEvents
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
        }
        transition SubscribeMobEventsCompleted {
            on input SubscribeMobEvents
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Completed
        }
        transition SubscribeMobEventsDestroyed {
            on input SubscribeMobEvents
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
        }

        // =====================================================================
        // Shutdown: from any non-Destroyed state
        // =====================================================================

        transition ShutdownRunning {
            on input Shutdown
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.coordinator_bound = false;
                self.active_run_count = 0;
            }
            to Stopped
            emit EmitRunLifecycleNotice
        }

        transition ShutdownStopped {
            on input Shutdown
            guard { self.lifecycle_phase == Phase::Stopped }
            update {
                self.coordinator_bound = false;
                self.active_run_count = 0;
            }
            to Stopped
            emit EmitRunLifecycleNotice
        }

        transition ShutdownCompleted {
            on input Shutdown
            guard { self.lifecycle_phase == Phase::Completed }
            update {
                self.coordinator_bound = false;
                self.active_run_count = 0;
            }
            to Completed
            emit EmitRunLifecycleNotice
        }

        // =====================================================================
        // Signal-driven Running self-loops
        // =====================================================================

        transition CancelFlowRunning {
            on input CancelFlow
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_run_count = 0;
            }
            to Running
            emit FlowTerminalized
        }

        transition InitializeOrchestratorRunning {
            on signal InitializeOrchestrator
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.coordinator_bound = true;
            }
            to Running
            emit NotifyCoordinator
        }

        transition BindCoordinatorRunning {
            on signal BindCoordinator
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.coordinator_bound = true;
            }
            to Running
            emit NotifyCoordinator
        }

        transition UnbindCoordinatorRunning {
            on signal UnbindCoordinator
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.coordinator_bound = false;
            }
            to Running
            emit NotifyCoordinator
        }

        transition StageSpawnRunning {
            on signal StageSpawn
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.pending_spawn_count += 1;
            }
            to Running
            emit ExposePendingSpawn
        }

        transition StopOrchestratorRunning {
            on signal StopOrchestrator
            guard { self.lifecycle_phase == Phase::Running }
            update { self.coordinator_bound = false; }
            to Running
            emit NotifyCoordinator
        }
        transition StopOrchestratorStopped {
            on signal StopOrchestrator
            guard { self.lifecycle_phase == Phase::Stopped }
            update { self.coordinator_bound = false; }
            to Stopped
            emit NotifyCoordinator
        }
        transition StopOrchestratorCompleted {
            on signal StopOrchestrator
            guard { self.lifecycle_phase == Phase::Completed }
            update { self.coordinator_bound = false; }
            to Completed
            emit NotifyCoordinator
        }

        transition ResumeOrchestratorRunning {
            on signal ResumeOrchestrator
            guard { self.lifecycle_phase == Phase::Running }
            update { self.coordinator_bound = true; }
            to Running
            emit NotifyCoordinator
        }
        transition ResumeOrchestratorStopped {
            on signal ResumeOrchestrator
            guard { self.lifecycle_phase == Phase::Stopped }
            update { self.coordinator_bound = true; }
            to Stopped
            emit NotifyCoordinator
        }
        transition ResumeOrchestratorCompleted {
            on signal ResumeOrchestrator
            guard { self.lifecycle_phase == Phase::Completed }
            update { self.coordinator_bound = true; }
            to Completed
            emit NotifyCoordinator
        }

        transition DestroyOrchestratorRunning {
            on signal DestroyOrchestrator
            guard { self.lifecycle_phase == Phase::Running }
            update { self.coordinator_bound = false; }
            to Running
            emit NotifyCoordinator
        }
        transition DestroyOrchestratorStopped {
            on signal DestroyOrchestrator
            guard { self.lifecycle_phase == Phase::Stopped }
            update { self.coordinator_bound = false; }
            to Stopped
            emit NotifyCoordinator
        }
        transition DestroyOrchestratorCompleted {
            on signal DestroyOrchestrator
            guard { self.lifecycle_phase == Phase::Completed }
            update { self.coordinator_bound = false; }
            to Completed
            emit NotifyCoordinator
        }

        transition ForceCancelMemberRunning {
            on signal ForceCancelMember
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit EmitMemberTerminalNotice
        }

        transition MemberPeerExposedRunning {
            on signal MemberPeerExposed
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit AdmitPeerInput
        }

        transition MemberTerminalizedRunning {
            on signal MemberTerminalized
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit EmitMemberTerminalNotice
        }

        transition OperationPeerTrustedRunning {
            on signal OperationPeerTrusted
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit AdmitPeerInput
        }

        transition PeerInputAdmittedRunning {
            on signal PeerInputAdmitted
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit AdmitPeerInput
        }

        // =====================================================================
        // BeginCleanup / FinishCleanup
        // =====================================================================

        transition BeginCleanupStopped {
            on signal BeginCleanup
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
            emit EmitRunLifecycleNotice
        }

        transition BeginCleanupCompleted {
            on signal BeginCleanup
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Stopped
            emit EmitRunLifecycleNotice
        }

        transition FinishCleanupStopped {
            on signal FinishCleanup
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
            emit EmitRunLifecycleNotice
        }

        transition FinishCleanupCompleted {
            on signal FinishCleanup
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Stopped
            emit EmitRunLifecycleNotice
        }

        // =====================================================================
        // RunFlow / StartFlow / CreateRun / StartRun
        // =====================================================================

        transition RunFlowRunning {
            on input RunFlow
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            update {
                self.active_run_count += 1;
            }
            to Running
            emit EmitFlowRunNotice
        }

        transition StartFlowRunning {
            on signal StartFlow
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            update {
                self.active_run_count += 1;
            }
            to Running
            emit EmitFlowRunNotice
        }

        transition CreateRunRunning {
            on signal CreateRun
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_run_count += 1;
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition StartRunRunning {
            on signal StartRun
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_run_count += 1;
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        // =====================================================================
        // Unwire
        // =====================================================================

        transition UnwireRunning {
            on input Unwire
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit NotifyCoordinator
        }

        // =====================================================================
        // CompleteFlow / FinishRun
        // =====================================================================
        //
        // Two independent flow-terminalization paths drive the authority to
        // the same state:
        //   1. Natural completion: a run's task finishes and
        //      `handle_flow_cleanup` fires `CompleteFlow` + `FinishRun`
        //      (decrementing `active_run_count`, clearing the run-tracker).
        //   2. Destroy-driven cancel: `cancel_all_flow_tasks` iterates the
        //      run-tracker and fires the same signals for any run that has
        //      not already been cleaned up.
        // Because actor-command ordering is unordered between these two
        // paths, they race. Whichever lands first drives
        // `active_run_count` from 1 → 0; the other arrives with the counter
        // already at 0. The *Zero transitions below model "CompleteFlow /
        // FinishRun at count 0" as a legitimate terminal convergence
        // (no-op update, same target phase) rather than as an error the
        // caller must paper over — dogma requires that convergence
        // semantics live in the machine authority.

        transition CompleteFlowRunning {
            on signal CompleteFlow
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Completed }
            guard "active_runs_present" { self.active_run_count > 0 }
            update {
                self.active_run_count -= 1;
            }
            to Running
            emit FlowTerminalized
        }

        transition CompleteFlowRunningZero {
            on signal CompleteFlow
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Completed }
            guard "no_active_runs" { self.active_run_count == 0 }
            update {}
            to Running
            emit NotifyCoordinator
        }

        transition FinishRunRunning {
            on signal FinishRun
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped }
            guard "active_runs_present" { self.active_run_count > 0 }
            update {
                self.active_run_count -= 1;
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition FinishRunRunningZero {
            on signal FinishRun
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped }
            guard "no_active_runs" { self.active_run_count == 0 }
            update {}
            to Running
            emit NotifyCoordinator
        }

        // =====================================================================
        // Retire / RetireAll
        // =====================================================================

        transition RetireRunning {
            on input Retire { agent_runtime_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
            }
            to Running
            emit RequestRuntimeRetire
        }

        transition RetireStopped {
            on input Retire { agent_runtime_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
            }
            to Stopped
            emit RequestRuntimeRetire
        }

        transition RetireAllRunning {
            on input RetireAll
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.live_runtime_ids = EmptySet;
                self.runtime_fence_tokens = EmptyMap;
            }
            to Running
            emit EmitMemberLifecycleNotice { kind: "retiring" }
        }

        transition RetireAllStopped {
            on input RetireAll
            guard { self.lifecycle_phase == Phase::Stopped }
            update {
                self.live_runtime_ids = EmptySet;
                self.runtime_fence_tokens = EmptyMap;
            }
            to Stopped
            emit EmitMemberLifecycleNotice { kind: "retiring" }
        }

        // =====================================================================
        // CompleteSpawn
        // =====================================================================

        transition CompleteSpawnRunning {
            on signal CompleteSpawn
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped }
            guard "pending_spawns_present" { self.pending_spawn_count > 0 }
            update {
                self.pending_spawn_count -= 1;
            }
            to Running
            emit EmitMemberLifecycleNotice { kind: "spawned" }
        }

        // =====================================================================
        // Destroy (input)
        // =====================================================================

        transition DestroyFromAny {
            on input Destroy
            guard {
                self.lifecycle_phase == Phase::Running
                || self.lifecycle_phase == Phase::Stopped
                || self.lifecycle_phase == Phase::Completed
            }
            update {
                self.live_runtime_ids = EmptySet;
                self.runtime_fence_tokens = EmptyMap;
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.coordinator_bound = false;
            }
            to Destroyed
        }

        // =====================================================================
        // Respawn (input, Running self-loop)
        // =====================================================================

        transition RespawnRunning {
            on input Respawn { agent_runtime_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "coordinator_bound" { self.coordinator_bound == true }
            update {}
            to Running
            emit ExposePendingSpawn
        }

        // =====================================================================
        // CancelAllWork
        // =====================================================================

        transition CancelAllWorkRunning {
            on input CancelAllWork { agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            update {
                self.active_run_count = 0;
            }
            to Running
            emit FlowTerminalized
        }

    }
}
