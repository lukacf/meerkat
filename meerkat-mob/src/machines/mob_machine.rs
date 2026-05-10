//! MobMachine — DSL-generated canonical state.
//!
//! The generated `MobMachineState` is the machine-owned portion of mob state.
//! It covers lifecycle phase, roster membership, run tracking, spawn tracking,
//! and coordinator binding. Shell infrastructure (channels, stores, services,
//! handles, etc.) is NOT modeled here.

use meerkat_machine_schema::catalog::dsl::OptionValueExt;
pub use meerkat_machine_schema::catalog::dsl::mob_machine::{
    FlowFrameReducerCommandKind, FlowRunReducerCommandKind, LoopIterationReducerCommandKind,
};

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

/// Bridging type for mob id. Maps to `crate::ids::MobId`.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct MobId(pub String);

impl<T: Into<String>> From<T> for MobId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl MobId {
    pub fn from_domain(id: &crate::ids::MobId) -> Self {
        Self(id.to_string())
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
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
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

/// Composite key for frame-scoped node state projected into MobMachine.
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
pub struct FrameNodeKey(pub String);

impl<T: Into<String>> From<T> for FrameNodeKey {
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
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum DependencyMode {
    #[default]
    All,
    Any,
}

/// Collection policy for a step's fan-out execution.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum CollectionPolicyKind {
    #[default]
    All,
    Any,
    Quorum,
}

/// Canonical flow-run lifecycle state once run-local semantics are absorbed.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
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
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum FrameStatus {
    #[default]
    Running,
    Completed,
    Failed,
    Canceled,
}

/// Canonical loop lifecycle state once loop-local semantics are absorbed.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum LoopStatus {
    #[default]
    Running,
    Completed,
    Exhausted,
    Failed,
    Canceled,
}

/// Canonical step execution status once run-local semantics are absorbed.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum StepRunStatus {
    #[default]
    Dispatched,
    Completed,
    Failed,
    Skipped,
    Canceled,
}

/// Root-vs-body frame scope for a frame snapshot.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum FrameScope {
    #[default]
    Root,
    Body,
}

/// Flow node kind inside a frame DAG.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum FlowNodeKind {
    #[default]
    Step,
    Loop,
}

/// Per-node execution status within a frame.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
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
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
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
            WorkOrigin::Ingest => Err("WorkOrigin::Ingest has no meerkat-mob domain counterpart"),
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

/// Descriptor-bearing external peer trust endpoint. Unlike `WiringEdge`, this
/// preserves the routing id, transport address, and signing key that make an
/// external trust edge authoritative.
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
pub struct ExternalPeerEndpoint {
    pub name: PeerName,
    pub peer_id: PeerId,
    pub address: PeerAddress,
    pub signing_key: PeerSigningKey,
}

impl From<&meerkat_core::comms::TrustedPeerDescriptor> for ExternalPeerEndpoint {
    fn from(spec: &meerkat_core::comms::TrustedPeerDescriptor) -> Self {
        Self {
            name: PeerName(spec.name.as_str().to_owned()),
            peer_id: PeerId(spec.peer_id.to_string()),
            address: PeerAddress(spec.address.to_string()),
            signing_key: PeerSigningKey(spec.pubkey),
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ExternalPeerEdge {
    pub local: AgentIdentity,
    pub endpoint: ExternalPeerEndpoint,
}

impl ExternalPeerEdge {
    pub fn new(local: AgentIdentity, endpoint: ExternalPeerEndpoint) -> Self {
        Self { local, endpoint }
    }
}

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
pub struct PeerName(pub String);
impl<T: Into<String>> From<T> for PeerName {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

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
pub struct PeerId(pub String);
impl<T: Into<String>> From<T> for PeerId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

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
pub struct PeerAddress(pub String);
impl<T: Into<String>> From<T> for PeerAddress {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct PeerSigningKey(pub [u8; 32]);
impl From<[u8; 32]> for PeerSigningKey {
    fn from(key: [u8; 32]) -> Self {
        Self(key)
    }
}

// ---------------------------------------------------------------------------
// Machine definition
// ---------------------------------------------------------------------------

meerkat_machine_schema::mob_catalog_machine_dsl!("meerkat-mob", "machines::mob_machine");

// ---------------------------------------------------------------------------
// MobMachine-owned projection helpers
// ---------------------------------------------------------------------------

/// Machine-owned lifecycle status for a mob member.
///
/// Runtime projections may map this into public handle DTOs, but the decision
/// itself is derived from `MobMachineState` so projection code does not invent
/// terminal/member truth from roster or session observations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobMemberLifecycleStatus {
    Unknown,
    Active,
    Retiring,
    Broken,
    Completed,
}

/// Machine-owned terminal classification for a mob member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobMemberTerminalClass {
    Running,
    TerminalFailure,
    TerminalUnknown,
    TerminalCompleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobMemberLifecycleMaterial {
    pub status: MobMemberLifecycleStatus,
    pub terminal_class: MobMemberTerminalClass,
    pub error: Option<String>,
}

impl MobMemberLifecycleStatus {
    pub const fn terminal_class(self) -> MobMemberTerminalClass {
        match self {
            Self::Active | Self::Retiring => MobMemberTerminalClass::Running,
            Self::Broken => MobMemberTerminalClass::TerminalFailure,
            Self::Completed => MobMemberTerminalClass::TerminalCompleted,
            Self::Unknown => MobMemberTerminalClass::TerminalUnknown,
        }
    }
}

impl MobMemberTerminalClass {
    pub const fn is_terminal(self) -> bool {
        match self {
            Self::Running => false,
            Self::TerminalFailure | Self::TerminalUnknown | Self::TerminalCompleted => true,
        }
    }
}

impl MobMemberLifecycleMaterial {
    pub const fn is_terminal(&self) -> bool {
        self.terminal_class.is_terminal()
    }
}

impl MobMachineState {
    /// Project lifecycle truth for an identity from the machine's membership
    /// maps. `member_present` is the roster/event-projection presence bit; it
    /// only tells the machine whether a public member row exists for this
    /// identity, not what lifecycle state that row should report.
    pub fn member_lifecycle_for_identity(
        &self,
        agent_identity: &AgentIdentity,
        member_present: bool,
    ) -> MobMemberLifecycleMaterial {
        let restore_failure = member_present
            .then(|| self.member_restore_failures.get(agent_identity).cloned())
            .flatten();
        let status = if !member_present {
            MobMemberLifecycleStatus::Unknown
        } else if restore_failure.is_some() {
            MobMemberLifecycleStatus::Broken
        } else if let Some(runtime_id) = self.identity_to_runtime.get(agent_identity) {
            if self.member_state_markers.get(runtime_id) == Some(&MobMemberState::Retiring) {
                MobMemberLifecycleStatus::Retiring
            } else if self.live_runtime_ids.contains(runtime_id) {
                MobMemberLifecycleStatus::Active
            } else {
                MobMemberLifecycleStatus::Completed
            }
        } else {
            MobMemberLifecycleStatus::Unknown
        };

        MobMemberLifecycleMaterial {
            status,
            terminal_class: status.terminal_class(),
            error: restore_failure,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_run(authority: &mut MobMachineAuthority, run_id: &RunId) {
        MobMachineMutator::apply(
            authority,
            MobMachineInput::CreateRunSeed {
                run_id: run_id.clone(),
                step_ids: Default::default(),
                ordered_steps: Vec::new(),
                step_has_conditions: Default::default(),
                step_dependencies: Default::default(),
                step_dependency_modes: Default::default(),
                step_branches: Default::default(),
                step_collection_policies: Default::default(),
                step_quorum_thresholds: Default::default(),
                escalation_threshold: 0,
                max_step_retries: 0,
                max_active_nodes: 0,
                max_active_frames: 0,
                max_frame_depth: 0,
            },
        )
        .expect("CreateRunSeed should be accepted before child seed");
    }

    fn seed_root_frame(
        authority: &mut MobMachineAuthority,
        run_id: &RunId,
        frame_id: &FrameId,
        node_id: &FlowNodeId,
    ) {
        seed_run(authority, run_id);
        MobMachineMutator::apply(
            authority,
            MobMachineInput::CreateFrameSeed {
                run_id: run_id.clone(),
                frame_id: frame_id.clone(),
                frame_scope: FrameScope::Root,
                loop_instance_id: None,
                iteration: 0,
                tracked_nodes: [node_id.clone()].into_iter().collect(),
                ordered_nodes: vec![node_id.clone()],
                node_kind: [(node_id.clone(), FlowNodeKind::Loop)]
                    .into_iter()
                    .collect(),
                node_dependencies: [(node_id.clone(), Vec::new())].into_iter().collect(),
                node_dependency_modes: [(node_id.clone(), DependencyMode::All)]
                    .into_iter()
                    .collect(),
                node_branches: [(node_id.clone(), None)].into_iter().collect(),
                node_step_ids: Default::default(),
                node_loop_ids: [(node_id.clone(), LoopId::from("repeat"))]
                    .into_iter()
                    .collect(),
                node_status: [(node_id.clone(), NodeRunStatus::Ready)]
                    .into_iter()
                    .collect(),
                ready_queue: vec![node_id.clone()],
            },
        )
        .expect("CreateFrameSeed should be accepted before child loop seed");
    }

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
        seed_run(&mut authority, &run_id);

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
                node_step_ids: [(node_id.clone(), StepId::from("step-a"))]
                    .into_iter()
                    .collect(),
                node_loop_ids: Default::default(),
                node_status: [(node_id.clone(), NodeRunStatus::Ready)]
                    .into_iter()
                    .collect(),
                ready_queue: vec![node_id.clone()],
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
        seed_root_frame(&mut authority, &RunId::from("run-1"), &frame_id, &node_id);

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
        assert_eq!(
            authority
                .state
                .loop_current_iteration
                .get(&loop_instance_id),
            Some(&0)
        );
        assert_eq!(
            authority
                .state
                .loop_last_completed_iteration
                .get(&loop_instance_id),
            Some(&0)
        );
    }

    #[test]
    fn loop_until_feedback_is_recorded_by_mob_machine() {
        let mut authority = MobMachineAuthority::new();
        let loop_instance_id = LoopInstanceId::from("loop-1");
        let parent_frame_id = FrameId::from("frame-root");
        let parent_node_id = FlowNodeId::from("loop-node");
        seed_root_frame(
            &mut authority,
            &RunId::from("run-1"),
            &parent_frame_id,
            &parent_node_id,
        );

        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::CreateLoopSeed {
                loop_instance_id: loop_instance_id.clone(),
                parent_frame_id,
                parent_node_id,
                loop_id: LoopId::from("repeat"),
                depth: 1,
                max_iterations: 2,
            },
        )
        .expect("CreateLoopSeed should be accepted");
        authority.state.loop_stage.insert(
            loop_instance_id.clone(),
            LoopIterationStage::BodyFrameActive,
        );

        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RecordLoopBodyFrameCompleted {
                loop_instance_id: loop_instance_id.clone(),
                iteration: 0,
            },
        )
        .expect("body completion should be accepted");
        assert_eq!(
            authority.state.loop_stage.get(&loop_instance_id),
            Some(&LoopIterationStage::AwaitingUntilEvaluation)
        );
        assert_eq!(
            authority
                .state
                .loop_current_iteration
                .get(&loop_instance_id),
            Some(&1)
        );

        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RecordLoopUntilConditionFailed {
                loop_instance_id: loop_instance_id.clone(),
                iteration: 0,
            },
        )
        .expect("until=false should request another body frame");
        assert_eq!(
            authority.state.loop_stage.get(&loop_instance_id),
            Some(&LoopIterationStage::AwaitingBodyFrame)
        );

        authority.state.loop_stage.insert(
            loop_instance_id.clone(),
            LoopIterationStage::BodyFrameActive,
        );
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RecordLoopBodyFrameCompleted {
                loop_instance_id: loop_instance_id.clone(),
                iteration: 1,
            },
        )
        .expect("second body completion should be accepted");
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RecordLoopUntilConditionMet {
                loop_instance_id: loop_instance_id.clone(),
                iteration: 1,
            },
        )
        .expect("until=true should complete the loop");
        assert_eq!(
            authority.state.loop_phase.get(&loop_instance_id),
            Some(&LoopStatus::Completed)
        );
    }

    #[test]
    fn observe_runtime_retired_clears_member_binding_without_stopping_mob() {
        let mut authority = MobMachineAuthority::new();
        let runtime_id = AgentRuntimeId::from("worker:1");
        let fence_token = FenceToken(7);
        authority.state.live_runtime_ids.insert(runtime_id.clone());
        authority
            .state
            .externally_addressable_runtime_ids
            .insert(runtime_id.clone());
        authority
            .state
            .runtime_fence_tokens
            .insert(runtime_id.clone(), fence_token);
        authority
            .state
            .member_state_markers
            .insert(runtime_id.clone(), MobMemberState::Retiring);
        authority.state.active_run_count = 3;

        let transition = authority
            .apply_signal(MobMachineSignal::ObserveRuntimeRetired {
                agent_runtime_id: runtime_id.clone(),
                fence_token,
            })
            .expect("runtime retire observation should be accepted");

        assert_eq!(transition.to_phase, MobPhase::Running);
        assert_eq!(authority.state.lifecycle_phase, MobPhase::Running);
        assert!(!authority.state.live_runtime_ids.contains(&runtime_id));
        assert!(
            !authority
                .state
                .externally_addressable_runtime_ids
                .contains(&runtime_id)
        );
        assert!(
            !authority
                .state
                .runtime_fence_tokens
                .contains_key(&runtime_id)
        );
        assert!(
            !authority
                .state
                .member_state_markers
                .contains_key(&runtime_id)
        );
        assert_eq!(authority.state.active_run_count, 0);
    }

    #[test]
    fn member_lifecycle_projection_is_derived_from_machine_membership() {
        let mut authority = MobMachineAuthority::new();
        let identity = AgentIdentity::from("worker");
        let runtime_id = AgentRuntimeId::from("worker:1");

        assert_eq!(
            authority
                .state
                .member_lifecycle_for_identity(&identity, true),
            MobMemberLifecycleMaterial {
                status: MobMemberLifecycleStatus::Unknown,
                terminal_class: MobMemberTerminalClass::TerminalUnknown,
                error: None,
            }
        );

        authority
            .state
            .identity_to_runtime
            .insert(identity.clone(), runtime_id.clone());
        authority.state.live_runtime_ids.insert(runtime_id.clone());
        let active = authority
            .state
            .member_lifecycle_for_identity(&identity, true);
        assert_eq!(
            active,
            MobMemberLifecycleMaterial {
                status: MobMemberLifecycleStatus::Active,
                terminal_class: MobMemberTerminalClass::Running,
                error: None,
            }
        );
        assert!(!active.is_terminal());

        authority
            .state
            .member_state_markers
            .insert(runtime_id.clone(), MobMemberState::Retiring);
        let retiring = authority
            .state
            .member_lifecycle_for_identity(&identity, true);
        assert_eq!(
            retiring,
            MobMemberLifecycleMaterial {
                status: MobMemberLifecycleStatus::Retiring,
                terminal_class: MobMemberTerminalClass::Running,
                error: None,
            }
        );
        assert!(!retiring.is_terminal());

        authority.state.member_state_markers.remove(&runtime_id);
        authority.state.live_runtime_ids.remove(&runtime_id);
        let completed = authority
            .state
            .member_lifecycle_for_identity(&identity, true);
        assert_eq!(
            completed,
            MobMemberLifecycleMaterial {
                status: MobMemberLifecycleStatus::Completed,
                terminal_class: MobMemberTerminalClass::TerminalCompleted,
                error: None,
            }
        );
        assert!(completed.is_terminal());
    }

    #[test]
    fn member_lifecycle_restore_failure_is_machine_terminal_truth() {
        let mut authority = MobMachineAuthority::new();
        let identity = AgentIdentity::from("worker");
        let runtime_id = AgentRuntimeId::from("worker:1");

        authority
            .state
            .identity_to_runtime
            .insert(identity.clone(), runtime_id.clone());
        authority.state.live_runtime_ids.insert(runtime_id);
        authority
            .state
            .member_restore_failures
            .insert(identity.clone(), "missing durable session".to_string());

        assert_eq!(
            authority
                .state
                .member_lifecycle_for_identity(&identity, true),
            MobMemberLifecycleMaterial {
                status: MobMemberLifecycleStatus::Broken,
                terminal_class: MobMemberTerminalClass::TerminalFailure,
                error: Some("missing durable session".to_string()),
            }
        );
    }
}
