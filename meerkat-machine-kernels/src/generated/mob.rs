mod source {
    #![allow(clippy::expect_used, clippy::assign_op_pattern)]
    //! MobMachine — DSL-generated canonical state.
    //!
    //! The generated `MobMachineState` is the machine-owned portion of mob state.
    //! It covers lifecycle phase, roster membership, run tracking, spawn tracking,
    //! and coordinator binding. Shell infrastructure (channels, stores, services,
    //! handles, etc.) is NOT modeled here.

    use meerkat_machine_dsl::machine;

    /// Extension trait providing `.get()` on `Option<T>` to support the
    /// `option_value` schema pattern emitted by the `machine!` DSL
    /// (`Expr::MapGet { map: Field(...), key: String("value") }`). The DSL's
    /// `replacing.get("value")` / `releasing.get("value")` expressions lower to
    /// this trait so conditional emit payloads can extract `T` from an
    /// `Option<T>` that guards have already constrained to `Some(_)`.
    trait OptionValueExt<T: Clone> {
        fn get(&self, _key: &str) -> T;
    }
    impl<T: Clone + Default> OptionValueExt<T> for Option<T> {
        fn get(&self, _key: &str) -> T {
            self.clone().unwrap_or_default()
        }
    }

    // ---------------------------------------------------------------------------
    // Bridging newtypes
    // ---------------------------------------------------------------------------
    //
    // These types bridge between the DSL's flat representation and the real mob
    // domain types in `crate::ids`. The DSL needs Ord+Hash+Clone for Set/Map;
    // these newtypes satisfy that while providing From/Into mappings.

    /// Bridging type for agent identity. Maps to `crate::ids::AgentIdentity`.
    #[derive(
        Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
    )]
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
    #[derive(
        Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
    )]
    pub struct AgentRuntimeId(pub String);

    impl<T: Into<String>> From<T> for AgentRuntimeId {
        fn from(s: T) -> Self {
            Self(s.into())
        }
    }
    impl AgentRuntimeId {
        pub fn as_str(&self) -> &str {
            &self.0
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
    #[derive(
        Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
    )]
    pub struct WorkId(pub String);

    impl<T: Into<String>> From<T> for WorkId {
        fn from(s: T) -> Self {
            Self(s.into())
        }
    }

    /// Bridging type for flow run identity. Maps to `crate::ids::RunId`.
    #[derive(
        Debug,
        Clone,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
        Hash,
        Default,
        serde::Serialize,
        serde::Deserialize,
    )]
    pub struct RunId(pub String);

    impl<T: Into<String>> From<T> for RunId {
        fn from(s: T) -> Self {
            Self(s.into())
        }
    }

    /// Bridging type for frame identity. Maps to `crate::ids::FrameId`.
    #[derive(
        Debug,
        Clone,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
        Hash,
        Default,
        serde::Serialize,
        serde::Deserialize,
    )]
    pub struct FrameId(pub String);

    impl<T: Into<String>> From<T> for FrameId {
        fn from(s: T) -> Self {
            Self(s.into())
        }
    }
    impl FrameId {
        pub fn as_str(&self) -> &str {
            &self.0
        }
    }

    /// Bridging type for loop instance identity. Maps to `crate::ids::LoopInstanceId`.
    #[derive(
        Debug,
        Clone,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
        Hash,
        Default,
        serde::Serialize,
        serde::Deserialize,
    )]
    pub struct LoopInstanceId(pub String);

    impl<T: Into<String>> From<T> for LoopInstanceId {
        fn from(s: T) -> Self {
            Self(s.into())
        }
    }
    impl LoopInstanceId {
        pub fn as_str(&self) -> &str {
            &self.0
        }
    }

    /// Bridging type for loop definition identity. Maps to `crate::ids::LoopId`.
    #[derive(
        Debug,
        Clone,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
        Hash,
        Default,
        serde::Serialize,
        serde::Deserialize,
    )]
    pub struct LoopId(pub String);

    impl<T: Into<String>> From<T> for LoopId {
        fn from(s: T) -> Self {
            Self(s.into())
        }
    }

    /// Bridging type for flow-node identity. Maps to `crate::ids::FlowNodeId`.
    #[derive(
        Debug,
        Clone,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
        Hash,
        Default,
        serde::Serialize,
        serde::Deserialize,
    )]
    pub struct FlowNodeId(pub String);

    impl<T: Into<String>> From<T> for FlowNodeId {
        fn from(s: T) -> Self {
            Self(s.into())
        }
    }

    /// Bridging type for branch identity. Maps to `crate::ids::BranchId`.
    #[derive(
        Debug,
        Clone,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
        Hash,
        Default,
        serde::Serialize,
        serde::Deserialize,
    )]
    pub struct BranchId(pub String);

    impl<T: Into<String>> From<T> for BranchId {
        fn from(s: T) -> Self {
            Self(s.into())
        }
    }

    /// Bridging type for step identity. Maps to `crate::ids::StepId`.
    #[derive(
        Debug,
        Clone,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
        Hash,
        Default,
        serde::Serialize,
        serde::Deserialize,
    )]
    pub struct StepId(pub String);

    impl<T: Into<String>> From<T> for StepId {
        fn from(s: T) -> Self {
            Self(s.into())
        }
    }
    impl StepId {
        pub fn as_str(&self) -> &str {
            &self.0
        }
    }

    /// Composite key for run-scoped step state projected into MobMachine.
    #[derive(
        Debug,
        Clone,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
        Hash,
        Default,
        serde::Serialize,
        serde::Deserialize,
    )]
    pub struct RunStepKey(pub String);

    impl<T: Into<String>> From<T> for RunStepKey {
        fn from(s: T) -> Self {
            Self(s.into())
        }
    }

    /// Bridging type for bridge session id. Maps to
    /// `meerkat_core::session::SessionId` — the bridge session a mob member is
    /// attached to for the current runtime generation. The DSL only needs the
    /// stringified form for Ord/Hash/Clone/Default; the realtime WS observer
    /// materializes it back into the typed core id.
    #[derive(
        Debug,
        Clone,
        Default,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
        Hash,
        serde::Serialize,
        serde::Deserialize,
    )]
    pub struct SessionId(pub String);

    impl<T: Into<String>> From<T> for SessionId {
        fn from(s: T) -> Self {
            Self(s.into())
        }
    }

    impl SessionId {
        /// Project a real `meerkat_core::types::SessionId` into the DSL bridging type.
        pub fn from_domain(id: &meerkat_core::types::SessionId) -> Self {
            Self(id.to_string())
        }
    }

    /// Per-identity realtime binding state. Lives in MobMachine as the canonical
    /// join between identity continuity (MobMachine-owned) and the realtime
    /// attachment's concrete session target (MeerkatMachine-owned).
    ///
    /// `Unbound` entries are never actually stored — absence of a key in
    /// `member_session_bindings` is the Unbound state. The variant exists so
    /// that the DSL has a tagged type to reason about and shell consumers can
    /// pattern match on it without relying on map-absence semantics.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub enum RealtimeBindingState {
        #[default]
        Unbound,
        BoundToSession {
            session_id: SessionId,
        },
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

    /// Dependency satisfaction mode for a step or frame node.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub enum DependencyMode {
        #[default]
        All,
        Any,
    }

    /// Collection policy for a step's fan-out execution.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub enum CollectionPolicyKind {
        #[default]
        All,
        Any,
        Quorum,
    }

    /// Canonical flow-run lifecycle state once run-local semantics are absorbed.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub enum FlowRunStatus {
        #[default]
        Absent,
        Pending,
        Running,
        Completed,
        Failed,
        Canceled,
    }

    /// Canonical frame lifecycle state once frame-local semantics are absorbed.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub enum FrameStatus {
        #[default]
        Running,
        Completed,
        Failed,
        Canceled,
    }

    /// Canonical loop lifecycle state once loop-local semantics are absorbed.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub enum LoopStatus {
        #[default]
        Running,
        Completed,
        Exhausted,
        Failed,
        Canceled,
    }

    /// Canonical step execution status once run-local semantics are absorbed.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub enum StepRunStatus {
        #[default]
        Dispatched,
        Completed,
        Failed,
        Skipped,
        Canceled,
    }

    /// Root-vs-body frame scope for a frame snapshot.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub enum FrameScope {
        #[default]
        Root,
        Body,
    }

    /// Flow node kind inside a frame DAG.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub enum FlowNodeKind {
        #[default]
        Step,
        Loop,
    }

    /// Per-node execution status within a frame.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub enum NodeRunStatus {
        Pending,
        #[default]
        Ready,
        Running,
        Completed,
        Failed,
        Skipped,
        Canceled,
    }

    /// Loop-body/evaluate lifecycle stage for an active repeat-until node.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub enum LoopIterationStage {
        #[default]
        AwaitingBodyFrame,
        BodyFrameActive,
        AwaitingUntilEvaluation,
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

    /// Typed work-origin classification for
    /// [`MobMachineInput::SubmitWork`] / [`MobMachineEffect::RequestRuntimeIngress`].
    /// Closed mirror of [`crate::ids::WorkOrigin`] — the DSL uses this enum as
    /// guard-visible truth instead of the former `origin == "External"` /
    /// `origin == "Internal"` string compares. The `Ingest` variant is only
    /// valid on the receiving side of the admission seam
    /// (`MeerkatMachine::Ingest` fired by the runtime control plane); mob
    /// transitions never produce it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub enum WorkOrigin {
        #[default]
        External,
        Internal,
        Ingest,
    }

    impl From<crate::ids::WorkOrigin> for WorkOrigin {
        fn from(origin: crate::ids::WorkOrigin) -> Self {
            match origin {
                crate::ids::WorkOrigin::External => Self::External,
                crate::ids::WorkOrigin::Internal => Self::Internal,
            }
        }
    }

    /// Fallible reverse mapping: the `Ingest` variant has no counterpart in the
    /// shell-side [`crate::ids::WorkOrigin`] (which only classifies mob-submitted
    /// work lanes); callers on the mob-domain side assert it away and surface a
    /// domain error if the DSL ever produces it back across the seam.
    impl TryFrom<WorkOrigin> for crate::ids::WorkOrigin {
        type Error = &'static str;

        fn try_from(origin: WorkOrigin) -> Result<Self, Self::Error> {
            match origin {
                WorkOrigin::External => Ok(Self::External),
                WorkOrigin::Internal => Ok(Self::Internal),
                WorkOrigin::Ingest => {
                    Err("WorkOrigin::Ingest has no meerkat-mob domain counterpart")
                }
            }
        }
    }

    /// Typed member lifecycle notice kind. Replaces the former literal-string
    /// `kind` field on [`MobMachineEffect::EmitMemberLifecycleNotice`] — closed
    /// set of observed member-lifecycle transitions the orchestrator emits.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub enum MemberLifecycleKind {
        #[default]
        Spawned,
        Retiring,
        Retired,
        Reset,
        Respawned,
        Completed,
        Destroyed,
    }

    impl MemberLifecycleKind {
        /// Stable discriminant for logging / wire surfaces.
        pub const fn as_str(self) -> &'static str {
            match self {
                Self::Spawned => "spawned",
                Self::Retiring => "retiring",
                Self::Retired => "retired",
                Self::Reset => "reset",
                Self::Respawned => "respawned",
                Self::Completed => "completed",
                Self::Destroyed => "destroyed",
            }
        }
    }

    impl std::fmt::Display for MemberLifecycleKind {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.as_str())
        }
    }

    /// Typed wiring lifecycle notice kind for
    /// [`MobMachineEffect::EmitWiringLifecycleNotice`]. Pair-valued (edge-keyed)
    /// counterpart to [`MemberLifecycleKind`] (member-keyed). Emitted alongside
    /// [`MobMachineEffect::WiringGraphChanged`] by `WireMembers`/`UnwireMembers`
    /// transitions so external observers can reconstruct which identity pair
    /// was wired or unwired.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub enum WiringLifecycleKind {
        #[default]
        Wired,
        Unwired,
    }

    impl WiringLifecycleKind {
        /// Stable discriminant for logging / wire surfaces.
        pub const fn as_str(self) -> &'static str {
            match self {
                Self::Wired => "wired",
                Self::Unwired => "unwired",
            }
        }
    }

    impl std::fmt::Display for WiringLifecycleKind {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.as_str())
        }
    }

    /// Typed kickoff-notice intent. Replaces the former literal-string `intent`
    /// field on [`MobMachineEffect::EmitKickoffLifecycleNotice`] — closed mirror
    /// of [`KickoffPhase`] with an additional `Started` intent variant for the
    /// `KickoffResolveStarted` input.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub enum KickoffIntent {
        #[default]
        Pending,
        Starting,
        Started,
        CallbackPending,
        Failed,
        Cancelled,
    }

    impl KickoffIntent {
        /// Stable discriminant for logging / wire surfaces.
        pub const fn as_str(self) -> &'static str {
            match self {
                Self::Pending => "Pending",
                Self::Starting => "Starting",
                Self::Started => "Started",
                Self::CallbackPending => "CallbackPending",
                Self::Failed => "Failed",
                Self::Cancelled => "Cancelled",
            }
        }
    }

    impl std::fmt::Display for KickoffIntent {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.as_str())
        }
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
                run_status: Map<RunId, Enum<FlowRunStatus>>,
                run_ordered_steps: Map<RunId, Seq<StepId>>,
                run_tracked_steps: Map<RunId, Set<StepId>>,
                run_step_status: Map<RunId, Map<StepId, Option<Enum<StepRunStatus>>>>,
                run_step_status_flat: Map<RunStepKey, Enum<StepRunStatus>>,
                run_output_recorded: Map<RunId, Map<StepId, bool>>,
                run_step_condition_results: Map<RunId, Map<StepId, Option<bool>>>,
                run_step_has_conditions: Map<RunId, Map<StepId, bool>>,
                run_step_dependencies: Map<RunId, Map<StepId, Seq<StepId>>>,
                run_step_dependency_modes: Map<RunId, Map<StepId, Enum<DependencyMode>>>,
                run_step_branches: Map<RunId, Map<StepId, Option<BranchId>>>,
                run_step_collection_policies: Map<RunId, Map<StepId, Enum<CollectionPolicyKind>>>,
                run_step_quorum_thresholds: Map<RunId, Map<StepId, u32>>,
                run_step_target_counts: Map<RunId, Map<StepId, u32>>,
                run_step_target_success_counts: Map<RunId, Map<StepId, u32>>,
                run_step_target_terminal_failure_counts: Map<RunId, Map<StepId, u32>>,
                run_output_recorded_flat: Map<RunStepKey, bool>,
                run_target_retry_counts: Map<RunId, Map<String, u32>>,
                run_escalation_threshold: Map<RunId, u32>,
                run_max_step_retries: Map<RunId, u32>,
                run_ready_frames: Map<RunId, Seq<FrameId>>,
                run_ready_frame_membership: Map<RunId, Set<FrameId>>,
                run_pending_body_frame_loops: Map<RunId, Seq<LoopInstanceId>>,
                run_pending_body_frame_loop_membership: Map<RunId, Set<LoopInstanceId>>,
                run_max_active_nodes: Map<RunId, u32>,
                run_max_active_frames: Map<RunId, u32>,
                run_max_frame_depth: Map<RunId, u32>,
                frame_scope: Map<FrameId, Enum<FrameScope>>,
                frame_phase: Map<FrameId, Enum<FrameStatus>>,
                frame_run: Map<FrameId, RunId>,
                frame_parent_loop: Map<FrameId, Option<LoopInstanceId>>,
                frame_iteration: Map<FrameId, u32>,
                frame_tracked_nodes: Map<FrameId, Set<FlowNodeId>>,
                frame_ordered_nodes: Map<FrameId, Seq<FlowNodeId>>,
                frame_node_kind: Map<FrameId, Map<FlowNodeId, Enum<FlowNodeKind>>>,
                frame_node_dependencies: Map<FrameId, Map<FlowNodeId, Seq<FlowNodeId>>>,
                frame_node_dependency_modes: Map<FrameId, Map<FlowNodeId, Enum<DependencyMode>>>,
                frame_node_step_ids: Map<FrameId, Map<FlowNodeId, StepId>>,
                frame_node_loop_ids: Map<FrameId, Map<FlowNodeId, LoopId>>,
                frame_node_status: Map<FrameId, Map<FlowNodeId, Enum<NodeRunStatus>>>,
                frame_ready_queue: Map<FrameId, Seq<FlowNodeId>>,
                frame_output_recorded: Map<FrameId, Map<FlowNodeId, bool>>,
                frame_node_condition_results: Map<FrameId, Map<FlowNodeId, Option<bool>>>,
                frame_node_branches: Map<FrameId, Map<FlowNodeId, Option<BranchId>>>,
                loop_phase: Map<LoopInstanceId, Enum<LoopStatus>>,
                loop_parent_frame: Map<LoopInstanceId, FrameId>,
                loop_parent_node: Map<LoopInstanceId, FlowNodeId>,
                loop_definition: Map<LoopInstanceId, LoopId>,
                loop_depth: Map<LoopInstanceId, u32>,
                loop_stage: Map<LoopInstanceId, Enum<LoopIterationStage>>,
                loop_current_iteration: Map<LoopInstanceId, u32>,
                loop_last_completed_iteration: Map<LoopInstanceId, u32>,
                loop_max_iterations: Map<LoopInstanceId, u32>,
                loop_active_body_frame: Map<LoopInstanceId, Option<FrameId>>,
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
                member_session_bindings: Map<AgentIdentity, SessionId>,
                topology_epoch: u64,
            }

            init(Running) {
                live_runtime_ids = EmptySet,
                externally_addressable_runtime_ids = EmptySet,
                runtime_fence_tokens = EmptyMap,
                active_run_count = 0,
                run_status = EmptyMap,
                run_ordered_steps = EmptyMap,
                run_tracked_steps = EmptyMap,
                run_step_status = EmptyMap,
                run_step_status_flat = EmptyMap,
                run_output_recorded = EmptyMap,
                run_step_condition_results = EmptyMap,
                run_step_has_conditions = EmptyMap,
                run_step_dependencies = EmptyMap,
                run_step_dependency_modes = EmptyMap,
                run_step_branches = EmptyMap,
                run_step_collection_policies = EmptyMap,
                run_step_quorum_thresholds = EmptyMap,
                run_step_target_counts = EmptyMap,
                run_step_target_success_counts = EmptyMap,
                run_step_target_terminal_failure_counts = EmptyMap,
                run_output_recorded_flat = EmptyMap,
                run_target_retry_counts = EmptyMap,
                run_escalation_threshold = EmptyMap,
                run_max_step_retries = EmptyMap,
                run_ready_frames = EmptyMap,
                run_ready_frame_membership = EmptyMap,
                run_pending_body_frame_loops = EmptyMap,
                run_pending_body_frame_loop_membership = EmptyMap,
                run_max_active_nodes = EmptyMap,
                run_max_active_frames = EmptyMap,
                run_max_frame_depth = EmptyMap,
                frame_scope = EmptyMap,
                frame_phase = EmptyMap,
                frame_run = EmptyMap,
                frame_parent_loop = EmptyMap,
                frame_iteration = EmptyMap,
                frame_tracked_nodes = EmptyMap,
                frame_ordered_nodes = EmptyMap,
                frame_node_kind = EmptyMap,
                frame_node_dependencies = EmptyMap,
                frame_node_dependency_modes = EmptyMap,
                frame_node_step_ids = EmptyMap,
                frame_node_loop_ids = EmptyMap,
                frame_node_status = EmptyMap,
                frame_ready_queue = EmptyMap,
                frame_output_recorded = EmptyMap,
                frame_node_condition_results = EmptyMap,
                frame_node_branches = EmptyMap,
                loop_phase = EmptyMap,
                loop_parent_frame = EmptyMap,
                loop_parent_node = EmptyMap,
                loop_definition = EmptyMap,
                loop_depth = EmptyMap,
                loop_stage = EmptyMap,
                loop_current_iteration = EmptyMap,
                loop_last_completed_iteration = EmptyMap,
                loop_max_iterations = EmptyMap,
                loop_active_body_frame = EmptyMap,
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
                member_session_bindings = EmptyMap,
                topology_epoch = 0,
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
                CreateRunSeed {
                    run_id: RunId,
                    step_ids: Set<StepId>,
                    ordered_steps: Seq<StepId>,
                    step_has_conditions: Map<StepId, bool>,
                    step_dependencies: Map<StepId, Seq<StepId>>,
                    step_dependency_modes: Map<StepId, Enum<DependencyMode>>,
                    step_branches: Map<StepId, Option<BranchId>>,
                    step_collection_policies: Map<StepId, Enum<CollectionPolicyKind>>,
                    step_quorum_thresholds: Map<StepId, u32>,
                    escalation_threshold: u32,
                    max_step_retries: u32,
                    max_active_nodes: u32,
                    max_active_frames: u32,
                    max_frame_depth: u32,
                },
                CreateFrameSeed {
                    run_id: RunId,
                    frame_id: FrameId,
                    frame_scope: Enum<FrameScope>,
                    loop_instance_id: Option<LoopInstanceId>,
                    iteration: u32,
                    tracked_nodes: Set<FlowNodeId>,
                    ordered_nodes: Seq<FlowNodeId>,
                    node_kind: Map<FlowNodeId, Enum<FlowNodeKind>>,
                    node_dependencies: Map<FlowNodeId, Seq<FlowNodeId>>,
                    node_dependency_modes: Map<FlowNodeId, Enum<DependencyMode>>,
                    node_branches: Map<FlowNodeId, Option<BranchId>>,
                },
                CreateLoopSeed {
                    loop_instance_id: LoopInstanceId,
                    parent_frame_id: FrameId,
                    parent_node_id: FlowNodeId,
                    loop_id: LoopId,
                    depth: u32,
                    max_iterations: u32,
                },
                ProjectRunStatus {
                    run_id: RunId,
                    status: Enum<FlowRunStatus>,
                },
                ProjectRunStepStatus {
                    run_step: RunStepKey,
                    status: Enum<StepRunStatus>,
                    output_recorded: bool,
                },
                ProjectFramePhase {
                    frame_id: FrameId,
                    phase: Enum<FrameStatus>,
                },
                ProjectLoopState {
                    loop_instance_id: LoopInstanceId,
                    phase: Enum<LoopStatus>,
                    stage: Enum<LoopIterationStage>,
                    active_body_frame_id: Option<FrameId>,
                },
                CancelFlow,
                FlowStatus,
                Spawn { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, external_addressable: bool, bridge_session_id: SessionId, replacing: Option<SessionId> },
                Retire { agent_runtime_id: AgentRuntimeId, agent_identity: AgentIdentity, releasing: Option<SessionId> },
                Respawn { agent_runtime_id: AgentRuntimeId },
                RetireAll,
                // Track-B (R5): explicit identity-level wiring and session-binding
                // mutation inputs. These drive `wiring_edges` and
                // `member_session_bindings` directly at DSL authority,
                // independent of the Spawn/Retire lifecycle.
                //
                // The `edge` field on `WireMembers`/`UnwireMembers` carries a
                // pre-normalized `WiringEdge` (a <= b). Callers construct the
                // edge via `WiringEdge::new(a, b)` before submitting.
                WireMembers { edge: WiringEdge },
                UnwireMembers { edge: WiringEdge },
                BindMemberSession { agent_identity: AgentIdentity, session_id: SessionId },
                RotateMemberSession { agent_identity: AgentIdentity, old_session_id: SessionId, new_session_id: SessionId },
                ReleaseMemberSession { agent_identity: AgentIdentity, session_id: SessionId },
                SubmitWork { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId, origin: Enum<WorkOrigin> },
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
                RequestRuntimeIngress { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId, origin: Enum<WorkOrigin> },
                RequestRuntimeRetire,
                RequestRuntimeDestroy,
                EmitMemberLifecycleNotice { kind: Enum<MemberLifecycleKind> },
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
                EmitKickoffLifecycleNotice { member_id: String, intent: Enum<KickoffIntent> },
                // Track-B (R5): canonical topology-change signals consumed by
                // the `RecomputeMobPeerOverlay` composition driver.
                //
                // - `WiringGraphChanged` fires when `wiring_edges` mutates.
                // - `MemberSessionBindingChanged` fires on every binding
                //   mutation (set, rotated, or released), carrying the
                //   before/after session ids as `Option<SessionId>`. Absence
                //   of a value means "no binding on this side of the
                //   transition": `old=None, new=Some` → set, `Some, Some` →
                //   rotate, `Some, None` → release.
                //
                // Both carry the post-transition `topology_epoch` so the
                // driver can linearize recomputes against the newest topology
                // snapshot.
                WiringGraphChanged { epoch: u64 },
                MemberSessionBindingChanged { epoch: u64, agent_identity: AgentIdentity, old_session_id: Option<SessionId>, new_session_id: Option<SessionId> },
                // D-wiring-observability (#27): pair-valued notice emitted from
                // `WireMembers`/`UnwireMembers` alongside `WiringGraphChanged`.
                // Unlike `WiringGraphChanged` (opaque epoch bump), this carries
                // the `WiringEdge` so external observers (event store,
                // telemetry) can reconstruct which identity pair was wired or
                // unwired. Separate from `EmitMemberLifecycleNotice` because
                // wiring is pair-valued, not per-member.
                EmitWiringLifecycleNotice { kind: Enum<WiringLifecycleKind>, edge: WiringEdge },
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
            disposition WiringGraphChanged => external,
            disposition MemberSessionBindingChanged => external,
            disposition EmitWiringLifecycleNotice => external,

            // =====================================================================
            // Invariants
            // =====================================================================

            // W3-H / dogma #4: "no zombie realtime binding" — every identity that
            // has a bound session must also appear in `identity_to_runtime` (i.e.
            // must be an identity MobMachine has spawned). Ensures the binding map
            // cannot reference identities the machine has never admitted. Paired
            // with the Retire transition's `member_session_bindings.remove` and
            // Spawn's guard/state consistency: keys(bindings) ⊆ keys(identity_to_runtime).
            invariant bindings_require_known_identity {
                for_all(id in self.member_session_bindings.keys(), self.identity_to_runtime.contains_key(id))
            }

            // =====================================================================
            // Direct transitions
            // =====================================================================

            // W3-H: Spawn splits into two guarded variants — Fresh (no prior
            // realtime binding for the identity) and Replacing (identity already
            // has a `BoundToSession`, i.e. this Spawn is the second half of a
            // shell-orchestrated respawn). Guards check BOTH the input's
            // `replacing` witness AND the state's `member_session_bindings` key
            // presence so a caller cannot invoke the wrong branch — the DSL
            // enforces caller/state consistency, and a mismatched caller fails
            // loudly with "no transition matched" rather than silently picking a
            // wrong branch.
            transition SpawnRunningFresh {
                on input Spawn { agent_identity, agent_runtime_id, fence_token, generation, external_addressable, bridge_session_id, replacing }
                guard { self.lifecycle_phase == Phase::Running }
                guard "coordinator_bound" { self.coordinator_bound == true }
                guard "no_prior_session_binding" { self.member_session_bindings.contains_key(agent_identity) == false }
                guard "replacing_absent" { replacing == None }
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
                    self.member_session_bindings.insert(agent_identity, bridge_session_id);
                }
                to Running
                emit RequestRuntimeBinding { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, generation: generation }
                emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Spawned }
            }

            transition SpawnRunningReplacing {
                on input Spawn { agent_identity, agent_runtime_id, fence_token, generation, external_addressable, bridge_session_id, replacing }
                guard { self.lifecycle_phase == Phase::Running }
                guard "coordinator_bound" { self.coordinator_bound == true }
                guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
                guard "replacing_present" { replacing != None }
                update {
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
                    self.member_session_bindings.insert(agent_identity, bridge_session_id);
                }
                to Running
                emit RequestRuntimeBinding { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, generation: generation }
                emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Spawned }
            }

            transition ObserveRuntimeReady {
                on signal ObserveRuntimeReady { agent_runtime_id, fence_token }
                guard { self.lifecycle_phase == Phase::Running }
                guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
                guard "fence_token_present" { fence_token == fence_token }
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
                guard "fence_token_present" { fence_token == fence_token }
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
                emit EmitKickoffLifecycleNotice { member_id: member_id, intent: KickoffIntent::Pending }
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
                emit EmitKickoffLifecycleNotice { member_id: member_id, intent: KickoffIntent::Starting }
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
                emit EmitKickoffLifecycleNotice { member_id: member_id, intent: KickoffIntent::Started }
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
                emit EmitKickoffLifecycleNotice { member_id: member_id, intent: KickoffIntent::CallbackPending }
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
                emit EmitKickoffLifecycleNotice { member_id: member_id, intent: KickoffIntent::Failed }
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
                emit EmitKickoffLifecycleNotice { member_id: member_id, intent: KickoffIntent::Cancelled }
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
                emit EmitKickoffLifecycleNotice { member_id: member_id, intent: KickoffIntent::Cancelled }
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
                guard "external_origin" { origin == WorkOrigin::External }
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
                guard "internal_origin" { origin == WorkOrigin::Internal }
                update {}
                to Running
                emit RequestRuntimeIngress { agent_runtime_id: agent_runtime_id, fence_token: fence_token, work_id: work_id, origin: origin }
            }

            transition RetireMember {
                on signal RetireMember { agent_runtime_id, fence_token }
                guard { self.lifecycle_phase == Phase::Running }
                guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
                guard "fence_token_present" { fence_token == fence_token }
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
                guard "fence_token_present" { fence_token == fence_token }
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
                emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Retired }
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
                emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Reset }
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
                emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Respawned }
            }

            transition MarkCompleted {
                on signal MarkCompleted
                guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped }
                guard "no_active_runs" { self.active_run_count == 0 }
                update {}
                to Completed
                emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Completed }
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
                guard "fence_token_present" { fence_token == fence_token }
                update {
                    self.live_runtime_ids = EmptySet;
                    self.runtime_fence_tokens = EmptyMap;
                    self.member_state_markers = EmptyMap;
                    self.active_run_count = 0;
                    self.pending_spawn_count = 0;
                    self.coordinator_bound = false;
                }
                to Destroyed
                emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Destroyed }
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

            // =====================================================================
            // Track-B (R5): identity-level wiring mutations.
            //
            // `WireMembers`/`UnwireMembers` mutate `wiring_edges` at DSL
            // authority and bump `topology_epoch`. The `WiringGraphChanged`
            // effect lets the `RecomputeMobPeerOverlay` composition driver
            // linearize peer-overlay recomputation against graph changes.
            // =====================================================================

            transition WireMembersRunning {
                on input WireMembers { edge }
                guard { self.lifecycle_phase == Phase::Running }
                guard "edge_not_already_wired" { self.wiring_edges.contains(edge) == false }
                update {
                    self.wiring_edges.insert(edge);
                    self.topology_epoch += 1;
                }
                to Running
                emit WiringGraphChanged { epoch: self.topology_epoch }
                emit EmitWiringLifecycleNotice { kind: WiringLifecycleKind::Wired, edge: edge }
            }

            transition UnwireMembersRunning {
                on input UnwireMembers { edge }
                guard { self.lifecycle_phase == Phase::Running }
                guard "edge_currently_wired" { self.wiring_edges.contains(edge) == true }
                update {
                    self.wiring_edges.remove(edge);
                    self.topology_epoch += 1;
                }
                to Running
                emit WiringGraphChanged { epoch: self.topology_epoch }
                emit EmitWiringLifecycleNotice { kind: WiringLifecycleKind::Unwired, edge: edge }
            }

            // =====================================================================
            // Track-B (R5): identity-level session-binding mutations.
            //
            // `BindMemberSession`/`RotateMemberSession`/`ReleaseMemberSession`
            // are the explicit-driven counterparts to the Spawn/Retire-coupled
            // binding updates. Each bumps `topology_epoch` and emits
            // `MemberSessionBindingChanged { epoch, agent_identity, old, new }`
            // so the composition driver can recompute overlay endpoints
            // keyed on the updated session.
            // =====================================================================

            transition BindMemberSessionRunning {
                on input BindMemberSession { agent_identity, session_id }
                guard { self.lifecycle_phase == Phase::Running }
                guard "identity_has_runtime" { self.identity_to_runtime.contains_key(agent_identity) == true }
                guard "no_prior_session_binding" { self.member_session_bindings.contains_key(agent_identity) == false }
                update {
                    self.member_session_bindings.insert(agent_identity, session_id);
                    self.topology_epoch += 1;
                }
                to Running
                emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: None, new_session_id: Some(session_id) }
            }

            transition RotateMemberSessionRunning {
                on input RotateMemberSession { agent_identity, old_session_id, new_session_id }
                guard { self.lifecycle_phase == Phase::Running }
                guard "identity_has_runtime" { self.identity_to_runtime.contains_key(agent_identity) == true }
                guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
                // PR #340 review item #5: verify the caller's witness of
                // the prior session matches the current binding.
                guard "old_session_id_matches_current" {
                    self.member_session_bindings.get_cloned(agent_identity) == Some(old_session_id)
                }
                update {
                    self.member_session_bindings.insert(agent_identity, new_session_id);
                    self.topology_epoch += 1;
                }
                to Running
                emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: Some(old_session_id), new_session_id: Some(new_session_id) }
            }

            transition ReleaseMemberSessionRunning {
                on input ReleaseMemberSession { agent_identity, session_id }
                guard { self.lifecycle_phase == Phase::Running }
                guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
                // PR #340 review item #5: verify the caller's witness of
                // the session being released matches the current binding.
                guard "session_id_matches_current" {
                    self.member_session_bindings.get_cloned(agent_identity) == Some(session_id)
                }
                update {
                    self.member_session_bindings.remove(agent_identity);
                    self.topology_epoch += 1;
                }
                to Running
                emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: Some(session_id), new_session_id: None }
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

            transition CreateRunSeedRunning {
                on input CreateRunSeed { run_id, step_ids, ordered_steps, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, escalation_threshold, max_step_retries, max_active_nodes, max_active_frames, max_frame_depth }
                guard { self.lifecycle_phase == Phase::Running }
                update {
                    self.run_status.insert(run_id, FlowRunStatus::Pending);
                    self.run_tracked_steps.insert(run_id, step_ids);
                    self.run_ordered_steps.insert(run_id, ordered_steps);
                    self.run_step_status.insert(run_id, EmptyMap);
                    self.run_output_recorded.insert(run_id, EmptyMap);
                    self.run_step_condition_results.insert(run_id, EmptyMap);
                    self.run_step_has_conditions.insert(run_id, step_has_conditions);
                    self.run_step_dependencies.insert(run_id, step_dependencies);
                    self.run_step_dependency_modes.insert(run_id, step_dependency_modes);
                    self.run_step_branches.insert(run_id, step_branches);
                    self.run_step_collection_policies.insert(run_id, step_collection_policies);
                    self.run_step_quorum_thresholds.insert(run_id, step_quorum_thresholds);
                    self.run_step_target_counts.insert(run_id, EmptyMap);
                    self.run_step_target_success_counts.insert(run_id, EmptyMap);
                    self.run_step_target_terminal_failure_counts.insert(run_id, EmptyMap);
                    self.run_target_retry_counts.insert(run_id, EmptyMap);
                    self.run_escalation_threshold.insert(run_id, escalation_threshold);
                    self.run_max_step_retries.insert(run_id, max_step_retries);
                    self.run_ready_frames.insert(run_id, EmptySeq);
                    self.run_ready_frame_membership.insert(run_id, EmptySet);
                    self.run_pending_body_frame_loops.insert(run_id, EmptySeq);
                    self.run_pending_body_frame_loop_membership.insert(run_id, EmptySet);
                    self.run_max_active_nodes.insert(run_id, max_active_nodes);
                    self.run_max_active_frames.insert(run_id, max_active_frames);
                    self.run_max_frame_depth.insert(run_id, max_frame_depth);
                }
                to Running
                emit EmitRunLifecycleNotice
            }

            transition CreateFrameSeedRunning {
                on input CreateFrameSeed { run_id, frame_id, frame_scope, loop_instance_id, iteration, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches }
                guard { self.lifecycle_phase == Phase::Running }
                update {
                    self.frame_scope.insert(frame_id, frame_scope);
                    self.frame_phase.insert(frame_id, FrameStatus::Running);
                    self.frame_run.insert(frame_id, run_id);
                    self.frame_parent_loop.insert(frame_id, loop_instance_id);
                    self.frame_iteration.insert(frame_id, iteration);
                    self.frame_tracked_nodes.insert(frame_id, tracked_nodes);
                    self.frame_ordered_nodes.insert(frame_id, ordered_nodes);
                    self.frame_node_kind.insert(frame_id, node_kind);
                    self.frame_node_dependencies.insert(frame_id, node_dependencies);
                    self.frame_node_dependency_modes.insert(frame_id, node_dependency_modes);
                    self.frame_node_branches.insert(frame_id, node_branches);
                    self.frame_node_status.insert(frame_id, EmptyMap);
                    self.frame_ready_queue.insert(frame_id, EmptySeq);
                    self.frame_output_recorded.insert(frame_id, EmptyMap);
                    self.frame_node_condition_results.insert(frame_id, EmptyMap);
                }
                to Running
                emit EmitRunLifecycleNotice
            }

            transition CreateLoopSeedRunning {
                on input CreateLoopSeed { loop_instance_id, parent_frame_id, parent_node_id, loop_id, depth, max_iterations }
                guard { self.lifecycle_phase == Phase::Running }
                update {
                    self.loop_phase.insert(loop_instance_id, LoopStatus::Running);
                    self.loop_parent_frame.insert(loop_instance_id, parent_frame_id);
                    self.loop_parent_node.insert(loop_instance_id, parent_node_id);
                    self.loop_definition.insert(loop_instance_id, loop_id);
                    self.loop_depth.insert(loop_instance_id, depth);
                    self.loop_stage.insert(loop_instance_id, LoopIterationStage::AwaitingBodyFrame);
                    self.loop_max_iterations.insert(loop_instance_id, max_iterations);
                    self.loop_active_body_frame.insert(loop_instance_id, None);
                }
                to Running
                emit EmitRunLifecycleNotice
            }

            transition ProjectRunStatusRunning {
                on input ProjectRunStatus { run_id, status }
                guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
                update {
                    self.run_status.insert(run_id, status);
                }
                to Running
                emit EmitRunLifecycleNotice
            }

            transition ProjectRunStepStatusRunning {
                on input ProjectRunStepStatus { run_step, status, output_recorded }
                guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
                update {
                    self.run_step_status_flat.insert(run_step, status);
                    self.run_output_recorded_flat.insert(run_step, output_recorded);
                }
                to Running
                emit EmitRunLifecycleNotice
            }

            transition ProjectFramePhaseRunning {
                on input ProjectFramePhase { frame_id, phase }
                guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
                update {
                    self.frame_phase.insert(frame_id, phase);
                }
                to Running
                emit EmitRunLifecycleNotice
            }

            transition ProjectLoopStateRunning {
                on input ProjectLoopState { loop_instance_id, phase, stage, active_body_frame_id }
                guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
                update {
                    self.loop_phase.insert(loop_instance_id, phase);
                    self.loop_stage.insert(loop_instance_id, stage);
                    self.loop_active_body_frame.insert(loop_instance_id, active_body_frame_id);
                }
                to Running
                emit EmitRunLifecycleNotice
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

            transition RetireRunningReleasing {
                on input Retire { agent_runtime_id, agent_identity, releasing }
                guard { self.lifecycle_phase == Phase::Running }
                guard "active_members_present" { self.live_runtime_ids != EmptySet }
                guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
                guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
                guard "releasing_present" { releasing != None }
                update {
                    self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
                    self.member_session_bindings.remove(agent_identity);
                }
                to Running
                emit RequestRuntimeRetire
            }

            transition RetireRunningPreservingBinding {
                on input Retire { agent_runtime_id, agent_identity, releasing }
                guard { self.lifecycle_phase == Phase::Running }
                guard "active_members_present" { self.live_runtime_ids != EmptySet }
                guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
                guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
                guard "releasing_absent" { releasing == None }
                update {
                    self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
                }
                to Running
                emit RequestRuntimeRetire
            }

            transition RetireRunningNoBinding {
                on input Retire { agent_runtime_id, agent_identity, releasing }
                guard { self.lifecycle_phase == Phase::Running }
                guard "active_members_present" { self.live_runtime_ids != EmptySet }
                guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
                guard "no_prior_session_binding" { self.member_session_bindings.contains_key(agent_identity) == false }
                guard "releasing_absent" { releasing == None }
                update {
                    self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
                }
                to Running
                emit RequestRuntimeRetire
            }

            transition RetireStoppedReleasing {
                on input Retire { agent_runtime_id, agent_identity, releasing }
                guard { self.lifecycle_phase == Phase::Stopped }
                guard "active_members_present" { self.live_runtime_ids != EmptySet }
                guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
                guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
                guard "releasing_present" { releasing != None }
                update {
                    self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
                    self.member_session_bindings.remove(agent_identity);
                }
                to Stopped
                emit RequestRuntimeRetire
            }

            transition RetireStoppedPreservingBinding {
                on input Retire { agent_runtime_id, agent_identity, releasing }
                guard { self.lifecycle_phase == Phase::Stopped }
                guard "active_members_present" { self.live_runtime_ids != EmptySet }
                guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
                guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
                guard "releasing_absent" { releasing == None }
                update {
                    self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
                }
                to Stopped
                emit RequestRuntimeRetire
            }

            transition RetireStoppedNoBinding {
                on input Retire { agent_runtime_id, agent_identity, releasing }
                guard { self.lifecycle_phase == Phase::Stopped }
                guard "active_members_present" { self.live_runtime_ids != EmptySet }
                guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
                guard "no_prior_session_binding" { self.member_session_bindings.contains_key(agent_identity) == false }
                guard "releasing_absent" { releasing == None }
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
                emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Retiring }
            }

            transition RetireAllStopped {
                on input RetireAll
                guard { self.lifecycle_phase == Phase::Stopped }
                update {
                    self.live_runtime_ids = EmptySet;
                    self.runtime_fence_tokens = EmptyMap;
                }
                to Stopped
                emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Retiring }
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
                emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Spawned }
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
                guard "fence_token_present" { fence_token == fence_token }
                update {
                    self.active_run_count = 0;
                }
                to Running
                emit FlowTerminalized
            }

        }
    }

    #[cfg(any())]
    mod tests {
        use super::*;

        #[test]
        fn create_run_seed_populates_canonical_run_maps() {
            let mut authority = MobMachineAuthority::new();
            let run_id = RunId::from("run-1");
            let step_id = StepId::from("step-a");
            let transition = MobMachineMutator::apply(
                &mut authority,
                MobMachineInput::CreateRunSeed {
                    run_id: run_id.clone(),
                    step_ids: [step_id.clone()].into_iter().collect(),
                    ordered_steps: vec![step_id.clone()],
                    step_has_conditions: [(step_id.clone(), false)].into_iter().collect(),
                    step_dependencies: [(step_id.clone(), Vec::new())].into_iter().collect(),
                    step_dependency_modes: [(step_id.clone(), DependencyMode::All)]
                        .into_iter()
                        .collect(),
                    step_branches: [(step_id.clone(), None)].into_iter().collect(),
                    step_collection_policies: [(step_id.clone(), CollectionPolicyKind::All)]
                        .into_iter()
                        .collect(),
                    step_quorum_thresholds: [(step_id.clone(), 0)].into_iter().collect(),
                    escalation_threshold: 0,
                    max_step_retries: 0,
                    max_active_nodes: 2,
                    max_active_frames: 3,
                    max_frame_depth: 4,
                },
            )
            .expect("CreateRunSeed should be accepted");

            assert_eq!(transition.to_phase, MobPhase::Running);
            assert_eq!(
                authority.state.run_status.get(&run_id),
                Some(&FlowRunStatus::Pending)
            );
            assert_eq!(
                authority.state.run_ordered_steps.get(&run_id),
                Some(&vec![step_id.clone()])
            );
            assert_eq!(
                authority
                    .state
                    .run_step_dependency_modes
                    .get(&run_id)
                    .and_then(|map| map.get(&step_id)),
                Some(&DependencyMode::All)
            );
            assert_eq!(authority.state.run_max_active_nodes.get(&run_id), Some(&2));
            assert_eq!(
                authority.state.run_ready_frames.get(&run_id),
                Some(&Vec::new())
            );
        }

        #[test]
        fn create_frame_seed_populates_canonical_frame_maps() {
            let mut authority = MobMachineAuthority::new();
            let run_id = RunId::from("run-1");
            let frame_id = FrameId::from("frame-root");
            let node_id = FlowNodeId::from("node-a");

            let transition = MobMachineMutator::apply(
                &mut authority,
                MobMachineInput::CreateFrameSeed {
                    run_id: run_id.clone(),
                    frame_id: frame_id.clone(),
                    frame_scope: FrameScope::Root,
                    loop_instance_id: None,
                    iteration: 0,
                    tracked_nodes: [node_id.clone()].into_iter().collect(),
                    ordered_nodes: vec![node_id.clone()],
                    node_kind: [(node_id.clone(), FlowNodeKind::Step)]
                        .into_iter()
                        .collect(),
                    node_dependencies: [(node_id.clone(), Vec::new())].into_iter().collect(),
                    node_dependency_modes: [(node_id.clone(), DependencyMode::All)]
                        .into_iter()
                        .collect(),
                    node_branches: [(node_id.clone(), None)].into_iter().collect(),
                },
            )
            .expect("CreateFrameSeed should be accepted");

            assert_eq!(transition.to_phase, MobPhase::Running);
            assert_eq!(
                authority.state.frame_scope.get(&frame_id),
                Some(&FrameScope::Root)
            );
            assert_eq!(authority.state.frame_run.get(&frame_id), Some(&run_id));
            assert_eq!(
                authority.state.frame_ordered_nodes.get(&frame_id),
                Some(&vec![node_id.clone()])
            );
            assert_eq!(
                authority
                    .state
                    .frame_node_kind
                    .get(&frame_id)
                    .and_then(|map| map.get(&node_id)),
                Some(&FlowNodeKind::Step)
            );
        }

        #[test]
        fn create_loop_seed_populates_canonical_loop_maps() {
            let mut authority = MobMachineAuthority::new();
            let loop_instance_id = LoopInstanceId::from("loop-1");
            let frame_id = FrameId::from("frame-root");
            let node_id = FlowNodeId::from("loop-node");
            let loop_id = LoopId::from("repeat");

            let transition = MobMachineMutator::apply(
                &mut authority,
                MobMachineInput::CreateLoopSeed {
                    loop_instance_id: loop_instance_id.clone(),
                    parent_frame_id: frame_id.clone(),
                    parent_node_id: node_id.clone(),
                    loop_id: loop_id.clone(),
                    depth: 2,
                    max_iterations: 5,
                },
            )
            .expect("CreateLoopSeed should be accepted");

            assert_eq!(transition.to_phase, MobPhase::Running);
            assert_eq!(
                authority.state.loop_parent_frame.get(&loop_instance_id),
                Some(&frame_id)
            );
            assert_eq!(
                authority.state.loop_parent_node.get(&loop_instance_id),
                Some(&node_id)
            );
            assert_eq!(
                authority.state.loop_definition.get(&loop_instance_id),
                Some(&loop_id)
            );
            assert_eq!(
                authority.state.loop_stage.get(&loop_instance_id),
                Some(&LoopIterationStage::AwaitingBodyFrame)
            );
        }
    }
}
pub use source::*;

pub fn schema() -> meerkat_machine_schema::MachineSchema {
    meerkat_machine_schema::catalog::dsl::dsl_mob_machine()
}
