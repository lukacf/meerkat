//! MobMachine — DSL-generated canonical state.
//!
//! The generated `MobMachineState` is the machine-owned portion of mob state.
//! It covers lifecycle phase, roster membership, run tracking, spawn tracking,
//! and coordinator binding. Shell infrastructure (channels, stores, services,
//! handles, etc.) is NOT modeled here.

use meerkat_machine_schema::catalog::dsl::OptionValueExt;
pub use meerkat_machine_schema::catalog::dsl::mob_machine::{
    FlowFrameReducerCommandKind, FlowRunPublicResultClassKind, FlowRunReducerCommandKind,
    LoopIterationReducerCommandKind, MobLifecycleJournalKind,
};

pub type MobToolCallerProvenance = meerkat_core::service::MobToolCallerProvenance;
pub type OpaquePrincipalToken = meerkat_core::service::OpaquePrincipalToken;

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

/// Canonical peer identity for respawn topology-restore feedback. Local
/// member edges use `AgentIdentity`; external peer edges use `PeerId`, not the
/// display-only peer name.
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
pub struct RespawnTopologyPeerId(pub String);

impl<T: Into<String>> From<T> for RespawnTopologyPeerId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Bridging type for agent runtime ID. Maps to `crate::ids::AgentRuntimeId`.
///
/// The real `AgentRuntimeId` is a struct `{ identity: AgentIdentity, generation: Generation }`.
/// The DSL uses a single string key `"identity:generation"` for Set/Map operations.
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

/// Per-runtime lifecycle marker tracking whether a member is actively serving
/// work or draining toward retirement. Generated SubmitWork guards consume this
/// marker so work-routing admission stays inside MobMachine authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobMemberState {
    #[default]
    Active,
    Retiring,
}

/// Typed public wait-admission result for member waits. MobMachine emits this
/// class before wait surfaces decide whether an absent runtime-material
/// snapshot is a hard failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MemberWaitClassificationKind {
    #[default]
    RuntimeMaterialPresent,
    MissingRuntimeMaterial,
}

/// Pure flow-topology rule-match verdict. The shell extracts this from the
/// declarative `TopologyRules` via `evaluate_topology` and feeds it to
/// MobMachine as an observation; MobMachine — not the shell — derives the
/// admission verdict from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobFlowDelegationEdgeRuleVerdictKind {
    #[default]
    Allow,
    Deny,
}

/// Configured topology enforcement mode for a flow delegation edge. Mirrors
/// the shell `PolicyMode`; fed alongside the rule verdict so MobMachine can
/// decide whether a denial blocks (Strict) or merely warns (Advisory).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobFlowDelegationEdgeModeKind {
    #[default]
    Advisory,
    Strict,
}

/// Machine-owned admission verdict for a flow delegation edge. The shell
/// mirrors this: `DeniedStrict` blocks the delegation step
/// (`MobError::TopologyViolation`), `DeniedAdvisory` emits an advisory notice
/// and proceeds, `Admitted` proceeds silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobFlowDelegationEdgeAdmissionKind {
    #[default]
    Admitted,
    DeniedStrict,
    DeniedAdvisory,
}

/// Pure observation of a remote member's runtime state, extracted by the bridge
/// consumer from the wire `BridgeMemberRuntimeState` projection and fed to
/// MobMachine for terminality classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobRemoteMemberRuntimeObservedState {
    #[default]
    Initializing,
    Idle,
    Attached,
    Running,
    Retired,
    Stopped,
    Destroyed,
}

/// Machine-owned terminality verdict for an observed remote-member runtime
/// state. The bridge shell mirrors this: `Terminal` lets cleanup stop,
/// `NonTerminal` forces a destroy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobRemoteMemberRuntimeTerminality {
    #[default]
    NonTerminal,
    Terminal,
}

/// Machine-owned composite spawn-member operator admission verdict. The tool
/// shell mirrors this: `Denied` -> `access_denied`, `Allowed` -> proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobSpawnMemberAdmissionKind {
    #[default]
    Denied,
    Allowed,
}

/// Machine-owned per-mob operator admission verdict for current-mob-scoped
/// tools. The tool shell mirrors this: `Denied` -> `access_denied`, `Allowed`
/// -> proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobCurrentMobAdmissionKind {
    #[default]
    Denied,
    Allowed,
}

/// Machine-owned coarse spawn-tool admission verdict for the spawn-member tool
/// surfaces. The tool shell mirrors this: `Denied` -> `access_denied`,
/// `Allowed` -> proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobSpawnToolAdmissionKind {
    #[default]
    Denied,
    Allowed,
}

/// Machine-owned operator create-mob admission verdict for the mob-creation
/// tool. The tool shell mirrors this: `Denied` -> `access_denied`, `Allowed`
/// -> proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobCreateMobAdmissionKind {
    #[default]
    Denied,
    Allowed,
}

/// Machine-owned operator profile-mutation admission verdict for realm-profile
/// mutation tools. The tool shell mirrors this: `Denied` -> `access_denied`,
/// `Allowed` -> proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobProfileMutationAdmissionKind {
    #[default]
    Denied,
    Allowed,
}

/// Machine-owned eligibility verdict for a within-mob member operation (spawn
/// finalization, peer messaging, respawn finalization). The actor mirrors this:
/// `DeniedNotRunning` -> `InvalidTransition` to `Running`, `Admitted` -> proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobMemberOperationEligibilityKind {
    #[default]
    DeniedNotRunning,
    Admitted,
}

/// Pure wire-projection bridge rejection cause, mirroring every variant of the
/// wire `BridgeRejectionCause`. The mob bridge consumer maps the typed wire
/// cause onto this and feeds it to MobMachine for recovery classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobBridgeRejectionCause {
    #[default]
    NotBound,
    StaleSupervisor,
    SenderMismatch,
    AlreadyBound,
    InvalidBootstrapToken,
    UnsupportedProtocolVersion,
    InvalidSupervisorSpec,
    InvalidPeerSpec,
    AddressMismatch,
    Unsupported,
    Internal,
}

/// Machine-owned bridge-rejection recovery verdict. The mob shell mirrors this:
/// `RebindRecover` re-runs `BindMember`, `FatalBubbleUp` bubbles the rejection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobBridgeRejectionRecovery {
    #[default]
    FatalBubbleUp,
    RebindRecover,
}

/// Machine-owned pending-supervisor-acceptance verdict for a re-verified
/// already-accepted remote peer during supervisor rotation. The actor mirrors
/// this: `NotConfirmedReattempt` drops the accepted peer and re-attempts the
/// rotation against it; `StalePendingAuthority` errors with the stale-pending
/// message; `Fatal` bubbles the rejection up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobPendingSupervisorAcceptanceKind {
    #[default]
    Fatal,
    NotConfirmedReattempt,
    StalePendingAuthority,
}

/// Machine-owned frame-seed idempotency disposition. `CreateFrameSeed` emits
/// `Seeded` for a fresh seed and `AlreadySeeded` (a no-op) when re-seeding an
/// already-tracked frame. The flow shell mirrors both as success — replacing
/// the former guard-name string match (`frame_seed_is_new`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobFrameSeedDisposition {
    #[default]
    Seeded,
    AlreadySeeded,
}

/// Typed public rejection class for [`MobMachineInput::SubmitWork`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SubmitWorkRejectReasonKind {
    #[default]
    MobNotRunning,
    MemberNotFound,
    StaleFenceToken,
    NotExternallyAddressable,
}

/// Typed public rejection class for [`MobMachineInput::CancelAllWork`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum CancelAllWorkRejectReasonKind {
    #[default]
    MobNotRunning,
    MemberNotFound,
    StaleFenceToken,
}

/// Typed public rejection class for generated agent event subscription
/// authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum EventSubscriptionRejectReasonKind {
    #[default]
    MemberNotFound,
    NoSessionBinding,
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

/// Typed runtime-mode override carried by generated spawn-policy resolution
/// handoff. The runtime callback is observation only; MobMachine records this
/// closed value before unknown-member work may auto-spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SpawnPolicyRuntimeMode {
    #[default]
    AutonomousHost,
    TurnDriven,
}

/// Typed public result class for the respawn topology-restore follow-up.
/// The shell observes concrete peer restoration attempts, but MobMachine owns
/// whether the public respawn envelope is complete or topology-restoration
/// partial failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RespawnTopologyRestoreResultKind {
    #[default]
    Completed,
    TopologyRestoreFailed,
}

/// Typed shell observation for a per-row `mob/spawn_many` failure.
/// MobMachine maps this observation to the public failure cause before any
/// surface can serialize the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobSpawnManyFailureObservationKind {
    #[default]
    ProfileNotFound,
    MemberNotFound,
    MemberAlreadyExists,
    NotExternallyAddressable,
    InvalidTransition,
    WiringError,
    SupervisorRotationIncomplete,
    BridgeCommandRejected,
    MemberRestoreFailed,
    KickoffWaitTimedOut,
    ReadyWaitTimedOut,
    DefinitionError,
    FlowNotFound,
    FlowFailed,
    RunNotFound,
    RunCanceled,
    FlowTurnTimedOut,
    FrameDepthLimitExceeded,
    FrameAtomicPersistenceUnavailable,
    SpecRevisionConflict,
    SchemaValidation,
    InsufficientTargets,
    TopologyViolation,
    BridgeDeliveryRejected,
    SupervisorEscalation,
    UnsupportedForMode,
    MissingMemberCapability,
    ResetBarrier,
    StorageError,
    SessionError,
    CommsError,
    CallbackPending,
    StaleFenceToken,
    StaleEventCursor,
    WorkNotFound,
    Internal,
}

/// Typed public result class for per-row `mob/spawn_many` failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobSpawnManyFailureCauseKind {
    #[default]
    ProfileNotFound,
    MemberNotFound,
    MemberAlreadyExists,
    NotExternallyAddressable,
    InvalidTransition,
    WiringError,
    BridgeCommandRejected,
    MemberRestoreFailed,
    KickoffWaitTimedOut,
    ReadyWaitTimedOut,
    DefinitionError,
    FlowNotFound,
    FlowFailed,
    RunNotFound,
    RunCanceled,
    FlowTurnTimedOut,
    FrameDepthLimitExceeded,
    FrameAtomicPersistenceUnavailable,
    SpecRevisionConflict,
    SchemaValidation,
    InsufficientTargets,
    TopologyViolation,
    BridgeDeliveryRejected,
    SupervisorEscalation,
    UnsupportedForMode,
    MissingMemberCapability,
    ResetBarrier,
    StorageError,
    SessionError,
    CommsError,
    CallbackPending,
    StaleFenceToken,
    StaleEventCursor,
    WorkNotFound,
    Internal,
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

impl From<crate::MobRuntimeMode> for SpawnPolicyRuntimeMode {
    fn from(mode: crate::MobRuntimeMode) -> Self {
        match mode {
            crate::MobRuntimeMode::AutonomousHost => Self::AutonomousHost,
            crate::MobRuntimeMode::TurnDriven => Self::TurnDriven,
        }
    }
}

impl From<SpawnPolicyRuntimeMode> for crate::MobRuntimeMode {
    fn from(mode: SpawnPolicyRuntimeMode) -> Self {
        match mode {
            SpawnPolicyRuntimeMode::AutonomousHost => Self::AutonomousHost,
            SpawnPolicyRuntimeMode::TurnDriven => Self::TurnDriven,
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

/// Descriptor-bearing member trust endpoint. MobMachine owns this fact when a
/// member runtime registers its comms identity, so member trust wiring can
/// authorize the exact peer descriptor that will be installed.
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
pub struct MemberPeerEndpoint {
    pub name: PeerName,
    pub peer_id: PeerId,
    pub address: PeerAddress,
    pub signing_key: PeerSigningKey,
}

impl From<&meerkat_core::comms::TrustedPeerDescriptor> for MemberPeerEndpoint {
    fn from(spec: &meerkat_core::comms::TrustedPeerDescriptor) -> Self {
        Self {
            name: PeerName(spec.name.as_str().to_owned()),
            peer_id: PeerId(spec.peer_id.to_string()),
            address: PeerAddress(spec.address.to_string()),
            signing_key: PeerSigningKey(spec.pubkey),
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

impl Default for ExternalPeerEdge {
    fn default() -> Self {
        Self {
            local: AgentIdentity(String::new()),
            endpoint: ExternalPeerEndpoint::default(),
        }
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
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ExternalPeerKey {
    pub local: AgentIdentity,
    pub name: PeerName,
}

impl ExternalPeerKey {
    pub fn new(local: AgentIdentity, name: PeerName) -> Self {
        Self { local, name }
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
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct SupervisorProtocolVersion(pub String);
impl From<String> for SupervisorProtocolVersion {
    fn from(s: String) -> Self {
        Self(s)
    }
}
impl From<&str> for SupervisorProtocolVersion {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<meerkat_contracts::wire::supervisor_bridge::BridgeProtocolVersion>
    for SupervisorProtocolVersion
{
    fn from(version: meerkat_contracts::wire::supervisor_bridge::BridgeProtocolVersion) -> Self {
        Self(version.to_string())
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
// Mob coordination board bridging newtypes / enums (folded). Mirror the
// product-neutral coordination domain types in `crate::coordination`. The DSL
// needs Ord+Hash+Clone+Default for Set/Map machinery; these satisfy that and
// provide conversions to/from the domain projection types.
// ---------------------------------------------------------------------------

/// Bridging type for a work intent id. Maps to `crate::coordination::WorkIntentId`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct WorkIntentId(pub String);

impl<T: Into<String>> From<T> for WorkIntentId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl From<&crate::coordination::WorkIntentId> for WorkIntentId {
    fn from(id: &crate::coordination::WorkIntentId) -> Self {
        Self(id.as_str().to_owned())
    }
}

impl WorkIntentId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Bridging type for a resource claim id. Maps to `crate::coordination::ResourceClaimId`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ResourceClaimId(pub String);

impl<T: Into<String>> From<T> for ResourceClaimId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl From<&crate::coordination::ResourceClaimId> for ResourceClaimId {
    fn from(id: &crate::coordination::ResourceClaimId) -> Self {
        Self(id.as_str().to_owned())
    }
}

impl ResourceClaimId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Bridging type for a coordination resource ref. Maps to
/// `crate::coordination::CoordinationResourceRef`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct CoordinationResourceRef(pub String);

impl<T: Into<String>> From<T> for CoordinationResourceRef {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl From<&crate::coordination::CoordinationResourceRef> for CoordinationResourceRef {
    fn from(value: &crate::coordination::CoordinationResourceRef) -> Self {
        Self(value.as_str().to_owned())
    }
}

impl CoordinationResourceRef {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Work-intent lifecycle status. Mirrors `crate::coordination::WorkIntentStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobCoordinationWorkIntentStatus {
    #[default]
    Planned,
    Active,
    Blocked,
    Completed,
    Cancelled,
}

impl From<crate::coordination::WorkIntentStatus> for MobCoordinationWorkIntentStatus {
    fn from(status: crate::coordination::WorkIntentStatus) -> Self {
        match status {
            crate::coordination::WorkIntentStatus::Planned => Self::Planned,
            crate::coordination::WorkIntentStatus::Active => Self::Active,
            crate::coordination::WorkIntentStatus::Blocked => Self::Blocked,
            crate::coordination::WorkIntentStatus::Completed => Self::Completed,
            crate::coordination::WorkIntentStatus::Cancelled => Self::Cancelled,
        }
    }
}

impl From<MobCoordinationWorkIntentStatus> for crate::coordination::WorkIntentStatus {
    fn from(status: MobCoordinationWorkIntentStatus) -> Self {
        match status {
            MobCoordinationWorkIntentStatus::Planned => Self::Planned,
            MobCoordinationWorkIntentStatus::Active => Self::Active,
            MobCoordinationWorkIntentStatus::Blocked => Self::Blocked,
            MobCoordinationWorkIntentStatus::Completed => Self::Completed,
            MobCoordinationWorkIntentStatus::Cancelled => Self::Cancelled,
        }
    }
}

/// Resource-claim lifecycle status. Mirrors `crate::coordination::ResourceClaimStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobCoordinationResourceClaimStatus {
    #[default]
    Active,
    Released,
    Expired,
    Cancelled,
}

impl From<crate::coordination::ResourceClaimStatus> for MobCoordinationResourceClaimStatus {
    fn from(status: crate::coordination::ResourceClaimStatus) -> Self {
        match status {
            crate::coordination::ResourceClaimStatus::Active => Self::Active,
            crate::coordination::ResourceClaimStatus::Released => Self::Released,
            crate::coordination::ResourceClaimStatus::Expired => Self::Expired,
            crate::coordination::ResourceClaimStatus::Cancelled => Self::Cancelled,
        }
    }
}

impl From<MobCoordinationResourceClaimStatus> for crate::coordination::ResourceClaimStatus {
    fn from(status: MobCoordinationResourceClaimStatus) -> Self {
        match status {
            MobCoordinationResourceClaimStatus::Active => Self::Active,
            MobCoordinationResourceClaimStatus::Released => Self::Released,
            MobCoordinationResourceClaimStatus::Expired => Self::Expired,
            MobCoordinationResourceClaimStatus::Cancelled => Self::Cancelled,
        }
    }
}

/// Resource-claim advisory strength. Mirrors `crate::coordination::ResourceClaimKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobCoordinationResourceClaimKind {
    #[default]
    Advisory,
    SoftReservation,
    Exclusive,
}

impl From<crate::coordination::ResourceClaimKind> for MobCoordinationResourceClaimKind {
    fn from(kind: crate::coordination::ResourceClaimKind) -> Self {
        match kind {
            crate::coordination::ResourceClaimKind::Advisory => Self::Advisory,
            crate::coordination::ResourceClaimKind::SoftReservation => Self::SoftReservation,
            crate::coordination::ResourceClaimKind::Exclusive => Self::Exclusive,
        }
    }
}

impl From<MobCoordinationResourceClaimKind> for crate::coordination::ResourceClaimKind {
    fn from(kind: MobCoordinationResourceClaimKind) -> Self {
        match kind {
            MobCoordinationResourceClaimKind::Advisory => Self::Advisory,
            MobCoordinationResourceClaimKind::SoftReservation => Self::SoftReservation,
            MobCoordinationResourceClaimKind::Exclusive => Self::Exclusive,
        }
    }
}

/// Coordination event discriminant. Mirrors the variant tags of
/// `crate::coordination::MobCoordinationEventKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobCoordinationEventKind {
    #[default]
    WorkIntentRecorded,
    WorkIntentStatusChanged,
    ResourceClaimRecorded,
    ResourceClaimStatusChanged,
    ResourceClaimOverlapObserved,
}

// ---------------------------------------------------------------------------
// Machine definition
// ---------------------------------------------------------------------------

meerkat_machine_schema::mob_catalog_machine_dsl!("meerkat-mob", "machines::mob_machine");

// ---------------------------------------------------------------------------
// MobMachine-owned projection helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct MobMemberRuntimeMaterial {
    pub agent_runtime_id: AgentRuntimeId,
    pub generation: Generation,
    pub fence_token: FenceToken,
}

impl MobMemberRuntimeMaterial {
    pub fn to_domain_for_identity(
        &self,
        identity: &crate::ids::AgentIdentity,
    ) -> (crate::ids::AgentRuntimeId, crate::ids::FenceToken) {
        (
            crate::ids::AgentRuntimeId::new(
                identity.clone(),
                crate::ids::Generation::new(self.generation.0),
            ),
            crate::ids::FenceToken::new(self.fence_token.0),
        )
    }
}

impl MobMachineState {
    /// Project the machine-owned spawn profile name for an identity.
    pub fn member_profile_name_for_identity(&self, agent_identity: &AgentIdentity) -> Option<&str> {
        self.member_profile_names
            .get(agent_identity)
            .map(String::as_str)
    }

    /// Project the machine-owned runtime mode for an identity.
    pub fn member_runtime_mode_for_identity(
        &self,
        agent_identity: &AgentIdentity,
    ) -> Option<crate::MobRuntimeMode> {
        self.member_runtime_modes
            .get(agent_identity)
            .copied()
            .map(crate::MobRuntimeMode::from)
    }

    pub fn member_runtime_material_for_identity(
        &self,
        agent_identity: &AgentIdentity,
    ) -> Option<MobMemberRuntimeMaterial> {
        Some(MobMemberRuntimeMaterial {
            agent_runtime_id: self.identity_to_runtime.get(agent_identity)?.clone(),
            generation: *self.identity_runtime_generations.get(agent_identity)?,
            fence_token: *self.identity_runtime_fence_tokens.get(agent_identity)?,
        })
    }
}

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

/// Machine-owned kickoff lifecycle projection for a mob member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobMemberKickoffMaterial {
    pub phase: KickoffPhase,
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
    /// maps.
    pub fn member_lifecycle_for_identity(
        &self,
        agent_identity: &AgentIdentity,
    ) -> MobMemberLifecycleMaterial {
        let restore_failure = self.member_restore_failures.get(agent_identity).cloned();
        let status = if restore_failure.is_some() {
            MobMemberLifecycleStatus::Broken
        } else if let Some(runtime_id) = self.identity_to_runtime.get(agent_identity) {
            if self.member_state_markers.get(runtime_id) == Some(&MobMemberState::Retiring)
                || self
                    .pending_session_ingress_detach_runtime_ids
                    .contains(runtime_id)
            {
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

    /// Project kickoff truth for a member from the generated phase sets.
    pub fn kickoff_material_for_member_id(
        &self,
        member_id: &str,
    ) -> Option<MobMemberKickoffMaterial> {
        let mut phase = None;
        for (contains, candidate) in [
            (
                self.member_kickoff_pending.contains(member_id),
                KickoffPhase::Pending,
            ),
            (
                self.member_kickoff_starting.contains(member_id),
                KickoffPhase::Starting,
            ),
            (
                self.member_kickoff_callback_pending.contains(member_id),
                KickoffPhase::CallbackPending,
            ),
            (
                self.member_kickoff_started.contains(member_id),
                KickoffPhase::Started,
            ),
            (
                self.member_kickoff_failed.contains(member_id),
                KickoffPhase::Failed,
            ),
            (
                self.member_kickoff_cancelled.contains(member_id),
                KickoffPhase::Cancelled,
            ),
        ] {
            if contains {
                if phase.replace(candidate).is_some() {
                    return None;
                }
            }
        }
        let phase = phase?;
        let error = match phase {
            KickoffPhase::Failed => Some(self.member_kickoff_error.get(member_id)?.clone()),
            KickoffPhase::Pending
            | KickoffPhase::Starting
            | KickoffPhase::CallbackPending
            | KickoffPhase::Started
            | KickoffPhase::Cancelled => None,
        };
        Some(MobMemberKickoffMaterial { phase, error })
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
                step_status: Default::default(),
                output_recorded: Default::default(),
                step_condition_results: Default::default(),
                step_has_conditions: Default::default(),
                step_dependencies: Default::default(),
                step_dependency_modes: Default::default(),
                step_branches: Default::default(),
                step_collection_policies: Default::default(),
                step_quorum_thresholds: Default::default(),
                step_target_counts: Default::default(),
                step_target_success_counts: Default::default(),
                step_target_terminal_failure_counts: Default::default(),
                escalation_threshold: 0,
                max_step_retries: 0,
                max_active_nodes: 0,
                max_active_frames: 0,
                max_frame_depth: 0,
            },
        )
        .expect("CreateRunSeed should be accepted before child seed");
    }

    fn seed_live_member(
        authority: &mut MobMachineAuthority,
        identity: &AgentIdentity,
        runtime_id: &AgentRuntimeId,
    ) -> SessionId {
        let bridge_session_id = SessionId::from(format!("session-{}", identity.0.as_str()));
        let profile_material_digest = format!("test-profile-digest-{}", identity.0.as_str());
        MobMachineMutator::apply(
            authority,
            MobMachineInput::AuthorizeSpawnProfile {
                agent_identity: identity.clone(),
                profile_name: "test".to_string(),
                model: "test-model".to_string(),
                profile_material_digest: profile_material_digest.clone(),
                tool_config_digest: "test-tool-config-digest".to_string(),
                skills_digest: "test-skills-digest".to_string(),
                provider_params_digest: None,
                output_schema_digest: None,
                external_addressable: true,
            },
        )
        .expect("AuthorizeSpawnProfile should seed live member authority");
        MobMachineMutator::apply(
            authority,
            MobMachineInput::Spawn {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_id.clone(),
                fence_token: FenceToken(7),
                generation: Generation(1),
                profile_material_digest,
                external_addressable: true,
                runtime_mode: SpawnPolicyRuntimeMode::AutonomousHost,
                bridge_session_id: Some(bridge_session_id.clone()),
                replacing: None,
            },
        )
        .expect("Spawn should seed a live member through machine authority");
        bridge_session_id
    }

    #[test]
    fn spawn_many_failure_cause_is_generated_from_observation() {
        let mut authority = MobMachineAuthority::new();
        let transition = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ClassifySpawnManyFailure {
                observation: MobSpawnManyFailureObservationKind::SupervisorRotationIncomplete,
            },
        )
        .expect("spawn_many failure observation should be classified");
        assert!(transition.effects().iter().any(|effect| {
            matches!(
                effect,
                MobMachineEffect::SpawnManyFailureClassified {
                    observation: MobSpawnManyFailureObservationKind::SupervisorRotationIncomplete,
                    cause: MobSpawnManyFailureCauseKind::WiringError,
                }
            )
        }));

        let transition = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ClassifySpawnManyFailure {
                observation: MobSpawnManyFailureObservationKind::ProfileNotFound,
            },
        )
        .expect("spawn_many profile observation should be classified");
        assert!(transition.effects().iter().any(|effect| {
            matches!(
                effect,
                MobMachineEffect::SpawnManyFailureClassified {
                    observation: MobSpawnManyFailureObservationKind::ProfileNotFound,
                    cause: MobSpawnManyFailureCauseKind::ProfileNotFound,
                }
            )
        }));
    }

    #[test]
    fn flow_topology_edge_admission_verdict_is_decided_by_machine() {
        // Allow rule -> Admitted, regardless of mode.
        let mut authority = MobMachineAuthority::new();
        let transition = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ResolveFlowDelegationEdgeAdmission {
                from_role: "lead".to_owned(),
                to_role: "worker".to_owned(),
                rule_verdict: MobFlowDelegationEdgeRuleVerdictKind::Allow,
                mode: MobFlowDelegationEdgeModeKind::Strict,
            },
        )
        .expect("allow rule should resolve an admission verdict");
        assert!(transition.effects().iter().any(|effect| {
            matches!(
                effect,
                MobMachineEffect::FlowDelegationEdgeAdmissionResolved {
                    admission: MobFlowDelegationEdgeAdmissionKind::Admitted,
                    ..
                }
            )
        }));

        // Deny rule + Strict mode -> DeniedStrict (the shell mirrors this as a
        // TopologyViolation block). The block decision is the machine's, not
        // the shell's.
        let transition = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ResolveFlowDelegationEdgeAdmission {
                from_role: "lead".to_owned(),
                to_role: "worker".to_owned(),
                rule_verdict: MobFlowDelegationEdgeRuleVerdictKind::Deny,
                mode: MobFlowDelegationEdgeModeKind::Strict,
            },
        )
        .expect("deny rule in strict mode should resolve an admission verdict");
        assert!(transition.effects().iter().any(|effect| {
            matches!(
                effect,
                MobMachineEffect::FlowDelegationEdgeAdmissionResolved {
                    admission: MobFlowDelegationEdgeAdmissionKind::DeniedStrict,
                    ..
                }
            )
        }));

        // Deny rule + Advisory mode -> DeniedAdvisory (warn-and-proceed).
        let transition = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ResolveFlowDelegationEdgeAdmission {
                from_role: "lead".to_owned(),
                to_role: "worker".to_owned(),
                rule_verdict: MobFlowDelegationEdgeRuleVerdictKind::Deny,
                mode: MobFlowDelegationEdgeModeKind::Advisory,
            },
        )
        .expect("deny rule in advisory mode should resolve an admission verdict");
        let admission = transition.effects().iter().find_map(|effect| match effect {
            MobMachineEffect::FlowDelegationEdgeAdmissionResolved {
                from_role,
                to_role,
                admission,
            } => Some((from_role.clone(), to_role.clone(), *admission)),
            _ => None,
        });
        assert_eq!(
            admission,
            Some((
                "lead".to_owned(),
                "worker".to_owned(),
                MobFlowDelegationEdgeAdmissionKind::DeniedAdvisory
            ))
        );
    }

    #[test]
    fn remote_member_runtime_terminality_is_decided_by_machine() {
        use MobRemoteMemberRuntimeObservedState as Observed;
        use MobRemoteMemberRuntimeTerminality as Terminality;
        let terminal_cases = [Observed::Retired, Observed::Stopped, Observed::Destroyed];
        let non_terminal_cases = [
            Observed::Initializing,
            Observed::Idle,
            Observed::Attached,
            Observed::Running,
        ];
        for (cases, expected) in [
            (terminal_cases.as_slice(), Terminality::Terminal),
            (non_terminal_cases.as_slice(), Terminality::NonTerminal),
        ] {
            for observed in cases {
                let mut authority = MobMachineAuthority::new();
                let transition = MobMachineMutator::apply(
                    &mut authority,
                    MobMachineInput::ClassifyRemoteMemberRuntimeObservation {
                        observed_state: *observed,
                    },
                )
                .expect("runtime observation should resolve a terminality verdict");
                let verdict = transition.effects().iter().find_map(|effect| match effect {
                    MobMachineEffect::RemoteMemberRuntimeTerminalityClassified {
                        observed_state,
                        terminality,
                    } => Some((*observed_state, *terminality)),
                    _ => None,
                });
                assert_eq!(verdict, Some((*observed, expected)), "for {observed:?}");
            }
        }
    }

    /// FOLD B: the machine — not the shell — owns the privileged-argument SET
    /// membership policy (OR-ing each per-argument presence fact) and the
    /// `manage_scope_present || profile_scope_contains` disjunction. The shell
    /// feeds RAW per-argument presence bools and a raw per-profile set-membership
    /// fact; the machine composes the verdict.
    #[test]
    fn spawn_member_admission_is_decided_by_machine() {
        use MobSpawnMemberAdmissionKind as Admission;

        // Helper: build the input with all-false privileged args except the
        // selected ones, set by a mutator closure.
        fn input(
            manage: bool,
            profile_contains: bool,
            set_privileged: impl FnOnce(&mut MobMachineInput),
        ) -> MobMachineInput {
            let mut input = MobMachineInput::ResolveSpawnMemberAdmission {
                manage_scope_present: manage,
                profile_scope_contains: profile_contains,
                privileged_resume_bridge_session_present: false,
                privileged_resume_session_present: false,
                privileged_backend_present: false,
                privileged_runtime_mode_present: false,
                privileged_launch_mode_present: false,
                privileged_tool_access_policy_present: false,
                privileged_budget_split_policy_present: false,
                privileged_tooling_present: false,
                privileged_auth_binding_present: false,
            };
            set_privileged(&mut input);
            input
        }

        fn resolve(input: MobMachineInput) -> Option<MobSpawnMemberAdmissionKind> {
            let mut authority = MobMachineAuthority::new();
            let transition = MobMachineMutator::apply(&mut authority, input)
                .expect("spawn-member admission should resolve a verdict");
            transition.effects().iter().find_map(|effect| match effect {
                MobMachineEffect::SpawnMemberAdmissionResolved { admission } => Some(*admission),
                _ => None,
            })
        }

        // manage scope present -> Allowed regardless of profile/privileged.
        assert_eq!(
            resolve(input(true, false, |_| {})),
            Some(Admission::Allowed),
            "manage scope allows"
        );
        assert_eq!(
            resolve(input(true, true, |i| {
                if let MobMachineInput::ResolveSpawnMemberAdmission {
                    privileged_backend_present,
                    ..
                } = i
                {
                    *privileged_backend_present = true;
                }
            })),
            Some(Admission::Allowed),
            "manage scope allows even with privileged args"
        );

        // No manage scope + ANY privileged arg present -> Denied. Exercise EACH
        // privileged field independently to prove the machine ORs the full SET.
        let privileged_setters: [(&str, fn(&mut MobMachineInput)); 9] = [
            ("resume_bridge_session", |i| {
                if let MobMachineInput::ResolveSpawnMemberAdmission {
                    privileged_resume_bridge_session_present,
                    ..
                } = i
                {
                    *privileged_resume_bridge_session_present = true;
                }
            }),
            ("resume_session", |i| {
                if let MobMachineInput::ResolveSpawnMemberAdmission {
                    privileged_resume_session_present,
                    ..
                } = i
                {
                    *privileged_resume_session_present = true;
                }
            }),
            ("backend", |i| {
                if let MobMachineInput::ResolveSpawnMemberAdmission {
                    privileged_backend_present,
                    ..
                } = i
                {
                    *privileged_backend_present = true;
                }
            }),
            ("runtime_mode", |i| {
                if let MobMachineInput::ResolveSpawnMemberAdmission {
                    privileged_runtime_mode_present,
                    ..
                } = i
                {
                    *privileged_runtime_mode_present = true;
                }
            }),
            ("launch_mode", |i| {
                if let MobMachineInput::ResolveSpawnMemberAdmission {
                    privileged_launch_mode_present,
                    ..
                } = i
                {
                    *privileged_launch_mode_present = true;
                }
            }),
            ("tool_access_policy", |i| {
                if let MobMachineInput::ResolveSpawnMemberAdmission {
                    privileged_tool_access_policy_present,
                    ..
                } = i
                {
                    *privileged_tool_access_policy_present = true;
                }
            }),
            ("budget_split_policy", |i| {
                if let MobMachineInput::ResolveSpawnMemberAdmission {
                    privileged_budget_split_policy_present,
                    ..
                } = i
                {
                    *privileged_budget_split_policy_present = true;
                }
            }),
            ("tooling", |i| {
                if let MobMachineInput::ResolveSpawnMemberAdmission {
                    privileged_tooling_present,
                    ..
                } = i
                {
                    *privileged_tooling_present = true;
                }
            }),
            ("auth_binding", |i| {
                if let MobMachineInput::ResolveSpawnMemberAdmission {
                    privileged_auth_binding_present,
                    ..
                } = i
                {
                    *privileged_auth_binding_present = true;
                }
            }),
        ];
        for (label, setter) in privileged_setters {
            // Even with profile scope, a privileged arg without manage scope denies.
            assert_eq!(
                resolve(input(false, true, setter)),
                Some(Admission::Denied),
                "privileged arg {label} without manage scope must deny"
            );
        }

        // No manage scope, no privileged args, profile scope contains -> Allowed.
        assert_eq!(
            resolve(input(false, true, |_| {})),
            Some(Admission::Allowed),
            "profile scope allows when no privileged args and no manage scope"
        );

        // No manage scope, no privileged args, no profile scope -> Denied.
        assert_eq!(
            resolve(input(false, false, |_| {})),
            Some(Admission::Denied),
            "no scope denies"
        );
    }

    #[test]
    fn current_mob_admission_is_decided_by_machine() {
        use MobCurrentMobAdmissionKind as Admission;
        // can_manage_mob -> expected verdict.
        let cases = [(true, Admission::Allowed), (false, Admission::Denied)];
        for (can_manage_mob, expected) in cases {
            let mut authority = MobMachineAuthority::new();
            let transition = MobMachineMutator::apply(
                &mut authority,
                MobMachineInput::ResolveCurrentMobAdmission { can_manage_mob },
            )
            .expect("current-mob admission should resolve a verdict");
            let admission = transition.effects().iter().find_map(|effect| match effect {
                MobMachineEffect::CurrentMobAdmissionResolved { admission } => Some(*admission),
                _ => None,
            });
            assert_eq!(
                admission,
                Some(expected),
                "for can_manage_mob={can_manage_mob}"
            );
        }
    }

    #[test]
    fn spawn_tool_admission_is_decided_by_machine() {
        use MobSpawnToolAdmissionKind as Admission;
        // The shell feeds the TWO raw facts; the machine composes the
        // disjunction (`can_manage_mob || spawn_profile_scope_present`). Only
        // false/false denies — this is the empty-specs spawn_many deny case.
        let cases = [
            (true, true, Admission::Allowed),
            (true, false, Admission::Allowed),
            (false, true, Admission::Allowed),
            (false, false, Admission::Denied),
        ];
        for (can_manage_mob, spawn_profile_scope_present, expected) in cases {
            let mut authority = MobMachineAuthority::new();
            let transition = MobMachineMutator::apply(
                &mut authority,
                MobMachineInput::ResolveSpawnToolAdmission {
                    can_manage_mob,
                    spawn_profile_scope_present,
                },
            )
            .expect("spawn-tool admission should resolve a verdict");
            let admission = transition.effects().iter().find_map(|effect| match effect {
                MobMachineEffect::SpawnToolAdmissionResolved { admission } => Some(*admission),
                _ => None,
            });
            assert_eq!(
                admission,
                Some(expected),
                "for can_manage_mob={can_manage_mob} spawn_profile_scope_present={spawn_profile_scope_present}"
            );
        }
    }

    #[test]
    fn create_mob_admission_is_decided_by_machine() {
        use MobCreateMobAdmissionKind as Admission;
        // can_create_mobs -> expected verdict.
        let cases = [(true, Admission::Allowed), (false, Admission::Denied)];
        for (can_create_mobs, expected) in cases {
            let mut authority = MobMachineAuthority::new();
            let transition = MobMachineMutator::apply(
                &mut authority,
                MobMachineInput::ResolveCreateMobAdmission { can_create_mobs },
            )
            .expect("create-mob admission should resolve a verdict");
            let admission = transition.effects().iter().find_map(|effect| match effect {
                MobMachineEffect::CreateMobAdmissionResolved { admission } => Some(*admission),
                _ => None,
            });
            assert_eq!(
                admission,
                Some(expected),
                "for can_create_mobs={can_create_mobs}"
            );
        }
    }

    #[test]
    fn profile_mutation_admission_is_decided_by_machine() {
        use MobProfileMutationAdmissionKind as Admission;
        // can_mutate_profiles -> expected verdict.
        let cases = [(true, Admission::Allowed), (false, Admission::Denied)];
        for (can_mutate_profiles, expected) in cases {
            let mut authority = MobMachineAuthority::new();
            let transition = MobMachineMutator::apply(
                &mut authority,
                MobMachineInput::ResolveProfileMutationAdmission {
                    can_mutate_profiles,
                },
            )
            .expect("profile-mutation admission should resolve a verdict");
            let admission = transition.effects().iter().find_map(|effect| match effect {
                MobMachineEffect::ProfileMutationAdmissionResolved { admission } => {
                    Some(*admission)
                }
                _ => None,
            });
            assert_eq!(
                admission,
                Some(expected),
                "for can_mutate_profiles={can_mutate_profiles}"
            );
        }
    }

    /// Ratchets the recoverable/fatal partition of bridge rejection causes,
    /// now sourced from MobMachine instead of the deleted
    /// `BridgeRejectionCause::class()` shell reducer. Every one of the eleven
    /// wire causes must resolve to exactly one recovery verdict, and the
    /// partition must match the historical class() mapping exactly.
    #[test]
    fn bridge_rejection_recovery_is_decided_by_machine() {
        use MobBridgeRejectionCause as Cause;
        use MobBridgeRejectionRecovery as Recovery;
        let recoverable = [
            Cause::NotBound,
            Cause::StaleSupervisor,
            Cause::SenderMismatch,
        ];
        let fatal = [
            Cause::AlreadyBound,
            Cause::InvalidBootstrapToken,
            Cause::UnsupportedProtocolVersion,
            Cause::InvalidSupervisorSpec,
            Cause::InvalidPeerSpec,
            Cause::AddressMismatch,
            Cause::Unsupported,
            Cause::Internal,
        ];
        // Defensive completeness: the recoverable + fatal sets must cover all
        // eleven causes with no overlap, so a forgotten future variant fails to
        // enumerate here.
        assert_eq!(
            recoverable.len() + fatal.len(),
            11,
            "every MobBridgeRejectionCause variant must be partitioned"
        );
        for (causes, expected) in [
            (recoverable.as_slice(), Recovery::RebindRecover),
            (fatal.as_slice(), Recovery::FatalBubbleUp),
        ] {
            for cause in causes {
                let mut authority = MobMachineAuthority::new();
                let transition = MobMachineMutator::apply(
                    &mut authority,
                    MobMachineInput::ClassifyBridgeRejectionRecovery {
                        rejection_cause: *cause,
                    },
                )
                .expect("bridge rejection cause should resolve a recovery verdict");
                let verdict = transition.effects().iter().find_map(|effect| match effect {
                    MobMachineEffect::BridgeRejectionRecoveryClassified {
                        rejection_cause,
                        recovery,
                    } => Some((*rejection_cause, *recovery)),
                    _ => None,
                });
                assert_eq!(verdict, Some((*cause, expected)), "for {cause:?}");
            }
        }
    }

    /// Ratchets the pending-supervisor-acceptance partition, sourced from
    /// MobMachine instead of the former handwritten
    /// `pending_supervisor_acceptance_confirmed` shell reducer over the raw
    /// wire cause. Every one of the eleven wire causes must resolve to exactly
    /// one acceptance verdict, and the partition must match the historical
    /// shell mapping exactly: NotBound / SenderMismatch ->
    /// NotConfirmedReattempt; StaleSupervisor -> StalePendingAuthority; every
    /// other cause -> Fatal.
    #[test]
    fn pending_supervisor_acceptance_is_decided_by_machine() {
        use MobBridgeRejectionCause as Cause;
        use MobPendingSupervisorAcceptanceKind as Verdict;
        let not_confirmed = [Cause::NotBound, Cause::SenderMismatch];
        let stale = [Cause::StaleSupervisor];
        let fatal = [
            Cause::AlreadyBound,
            Cause::InvalidBootstrapToken,
            Cause::UnsupportedProtocolVersion,
            Cause::InvalidSupervisorSpec,
            Cause::InvalidPeerSpec,
            Cause::AddressMismatch,
            Cause::Unsupported,
            Cause::Internal,
        ];
        // Defensive completeness: the three partitions must cover all eleven
        // causes with no overlap, so a forgotten future variant fails to
        // enumerate here.
        assert_eq!(
            not_confirmed.len() + stale.len() + fatal.len(),
            11,
            "every MobBridgeRejectionCause variant must be partitioned"
        );
        for (causes, expected) in [
            (not_confirmed.as_slice(), Verdict::NotConfirmedReattempt),
            (stale.as_slice(), Verdict::StalePendingAuthority),
            (fatal.as_slice(), Verdict::Fatal),
        ] {
            for cause in causes {
                let mut authority = MobMachineAuthority::new();
                let transition = MobMachineMutator::apply(
                    &mut authority,
                    MobMachineInput::ClassifyPendingSupervisorAcceptance {
                        rejection_cause: *cause,
                    },
                )
                .expect("pending supervisor acceptance cause should resolve a verdict");
                let verdict = transition.effects().iter().find_map(|effect| match effect {
                    MobMachineEffect::PendingSupervisorAcceptanceClassified {
                        rejection_cause,
                        verdict,
                    } => Some((*rejection_cause, *verdict)),
                    _ => None,
                });
                assert_eq!(verdict, Some((*cause, expected)), "for {cause:?}");
            }
        }
    }

    fn root_frame_seed_input(
        run_id: &RunId,
        frame_id: &FrameId,
        node_id: &FlowNodeId,
    ) -> MobMachineInput {
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
            output_recorded: [(node_id.clone(), false)].into_iter().collect(),
            node_condition_results: [(node_id.clone(), None)].into_iter().collect(),
            last_admitted_node: None,
        }
    }

    fn seed_root_frame(
        authority: &mut MobMachineAuthority,
        run_id: &RunId,
        frame_id: &FrameId,
        node_id: &FlowNodeId,
    ) {
        seed_run(authority, run_id);
        MobMachineMutator::apply(authority, root_frame_seed_input(run_id, frame_id, node_id))
            .expect("CreateFrameSeed should be accepted before child loop seed");
    }

    /// Ratchets the machine-owned frame-seed idempotency disposition, replacing
    /// the former `error.to_string().contains("frame_seed_is_new")` shell string
    /// folklore. A fresh `CreateFrameSeed` emits `Seeded`; re-applying the same
    /// seed for an already-tracked frame is a no-op that emits `AlreadySeeded`
    /// (NOT a guard rejection) — detected purely from the typed effect.
    #[test]
    fn create_frame_seed_idempotency_is_decided_by_machine() {
        let run_id = RunId::from("run-frame-seed");
        let frame_id = FrameId::from("frame-root");
        let node_id = FlowNodeId::from("node-a");

        let mut authority = MobMachineAuthority::new();
        seed_run(&mut authority, &run_id);
        let seed = root_frame_seed_input(&run_id, &frame_id, &node_id);

        // First application: fresh seed.
        let first = MobMachineMutator::apply(&mut authority, seed.clone())
            .expect("fresh CreateFrameSeed should be accepted");
        let first_disposition = first.effects().iter().find_map(|effect| match effect {
            MobMachineEffect::FrameSeedConfirmed { disposition, .. } => Some(*disposition),
            _ => None,
        });
        assert_eq!(
            first_disposition,
            Some(MobFrameSeedDisposition::Seeded),
            "fresh seed must emit the Seeded disposition"
        );

        // Second application of the identical seed: idempotent no-op, NOT a
        // rejection. The disposition is read from the typed effect, never from
        // an error string.
        let second = MobMachineMutator::apply(&mut authority, seed)
            .expect("re-seeding an already-tracked frame must be accepted as a no-op");
        let second_disposition = second.effects().iter().find_map(|effect| match effect {
            MobMachineEffect::FrameSeedConfirmed { disposition, .. } => Some(*disposition),
            _ => None,
        });
        assert_eq!(
            second_disposition,
            Some(MobFrameSeedDisposition::AlreadySeeded),
            "re-seed must emit the AlreadySeeded disposition without rejecting"
        );
    }

    fn external_peer_edge_for_test(local: &str, name: &str) -> ExternalPeerEdge {
        ExternalPeerEdge::new(
            AgentIdentity::from(local),
            ExternalPeerEndpoint {
                name: PeerName::from(name),
                peer_id: PeerId::from(format!("{name}-peer")),
                address: PeerAddress::from(format!("https://{name}.example.test")),
                signing_key: PeerSigningKey::from([7; 32]),
            },
        )
    }

    #[test]
    fn external_peer_key_must_match_edge_payload() {
        let edge = external_peer_edge_for_test("local-a", "peer-a");
        let mismatched_local =
            ExternalPeerKey::new(AgentIdentity::from("local-b"), edge.endpoint.name.clone());
        let mut authority = MobMachineAuthority::new();

        let rejected = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::WireExternalPeer {
                key: mismatched_local,
                edge: edge.clone(),
            },
        );
        assert!(
            rejected.is_err(),
            "generated MobMachine authority must reject key.local mismatches"
        );
        assert!(authority.state().external_peer_edges.is_empty());
        assert!(authority.state().external_peer_edges_by_key.is_empty());

        let mismatched_name =
            ExternalPeerKey::new(edge.local.clone(), PeerName::from("different-peer"));
        let mut authority = MobMachineAuthority::new();
        let rejected = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::WireExternalPeer {
                key: mismatched_name,
                edge,
            },
        );
        assert!(
            rejected.is_err(),
            "generated MobMachine authority must reject key.name mismatches"
        );
        assert!(authority.state().external_peer_edges.is_empty());
        assert!(authority.state().external_peer_edges_by_key.is_empty());
    }

    #[test]
    fn recover_rejects_incoherent_external_peer_edges() {
        let edge = external_peer_edge_for_test("local-a", "peer-a");
        let matching_key = ExternalPeerKey::new(edge.local.clone(), edge.endpoint.name.clone());
        let mismatched_key =
            ExternalPeerKey::new(AgentIdentity::from("local-b"), edge.endpoint.name.clone());

        let mut mismatched_key_state = MobMachineState::default();
        mismatched_key_state
            .external_peer_edges
            .insert(edge.clone());
        mismatched_key_state
            .external_peer_edges_by_key
            .insert(mismatched_key, edge.clone());
        assert!(
            MobMachineAuthority::recover_from_state(mismatched_key_state).is_err(),
            "generated recovery invariant must reject key/payload mismatch"
        );

        let mut missing_set_state = MobMachineState::default();
        missing_set_state
            .external_peer_edges_by_key
            .insert(matching_key.clone(), edge.clone());
        assert!(
            MobMachineAuthority::recover_from_state(missing_set_state).is_err(),
            "generated recovery invariant must reject keyed edges missing from edge set"
        );

        let mut missing_key_state = MobMachineState::default();
        missing_key_state.external_peer_edges.insert(edge);
        assert!(
            MobMachineAuthority::recover_from_state(missing_key_state).is_err(),
            "generated recovery invariant must reject edge set entries missing keyed ownership"
        );
    }

    fn seed_body_frame(
        authority: &mut MobMachineAuthority,
        run_id: &RunId,
        frame_id: &FrameId,
        loop_instance_id: &LoopInstanceId,
        iteration: u32,
    ) {
        MobMachineMutator::apply(
            authority,
            MobMachineInput::CreateFrameSeed {
                run_id: run_id.clone(),
                frame_id: frame_id.clone(),
                frame_scope: FrameScope::Body,
                loop_instance_id: Some(loop_instance_id.clone()),
                iteration,
                tracked_nodes: Default::default(),
                ordered_nodes: Vec::new(),
                node_kind: Default::default(),
                node_dependencies: Default::default(),
                node_dependency_modes: Default::default(),
                node_branches: Default::default(),
                node_step_ids: Default::default(),
                node_loop_ids: Default::default(),
                node_status: Default::default(),
                ready_queue: Vec::new(),
                output_recorded: Default::default(),
                node_condition_results: Default::default(),
                last_admitted_node: None,
            },
        )
        .expect("CreateFrameSeed should activate a loop body frame");
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
                step_status: [(step_id.clone(), None)].into_iter().collect(),
                output_recorded: [(step_id.clone(), false)].into_iter().collect(),
                step_condition_results: [(step_id.clone(), None)].into_iter().collect(),
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
                step_target_counts: [(step_id.clone(), 0)].into_iter().collect(),
                step_target_success_counts: [(step_id.clone(), 0)].into_iter().collect(),
                step_target_terminal_failure_counts: [(step_id.clone(), 0)].into_iter().collect(),
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
            authority.state().run_status.get(&run_id),
            Some(&FlowRunStatus::Pending)
        );
        assert_eq!(
            authority.state().run_ordered_steps.get(&run_id),
            Some(&vec![step_id.clone()])
        );
        assert_eq!(
            authority
                .state()
                .run_step_dependency_modes
                .get(&run_id)
                .and_then(|map| map.get(&step_id)),
            Some(&DependencyMode::All)
        );
        assert_eq!(
            authority.state().run_max_active_nodes.get(&run_id),
            Some(&2)
        );
        assert_eq!(
            authority.state().run_ready_frames.get(&run_id),
            Some(&Vec::new())
        );
    }

    #[test]
    fn spawn_policy_resolution_records_machine_owned_revision_and_value() {
        let mut authority = MobMachineAuthority::new();
        let identity = AgentIdentity::from("policy-worker");

        assert!(!authority.state().spawn_policy_enabled);
        assert_eq!(authority.state().spawn_policy_revision, 0);

        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::SetSpawnPolicy { enabled: true },
        )
        .expect("SetSpawnPolicy should enable generated policy authority");
        assert!(authority.state().spawn_policy_enabled);
        assert_eq!(authority.state().spawn_policy_revision, 1);

        let transition = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ResolveSpawnPolicy {
                agent_identity: identity.clone(),
                revision: 1,
                profile_name: Some("worker".to_string()),
                runtime_mode: Some(SpawnPolicyRuntimeMode::TurnDriven),
            },
        )
        .expect("matching policy revision should record typed resolution");
        assert!(transition.effects().iter().any(|effect| {
            matches!(
                effect,
                MobMachineEffect::SpawnPolicyResolutionRecorded {
                    agent_identity,
                    revision: 1,
                    profile_name: Some(profile),
                    runtime_mode: Some(SpawnPolicyRuntimeMode::TurnDriven),
                } if *agent_identity == identity && profile == "worker"
            )
        }));
        assert_eq!(
            authority
                .state()
                .spawn_policy_resolution_revision
                .get(&identity),
            Some(&1)
        );
        assert_eq!(
            authority
                .state()
                .spawn_policy_resolution_profiles
                .get(&identity),
            Some(&"worker".to_string())
        );
        assert_eq!(
            authority
                .state()
                .spawn_policy_resolution_runtime_modes
                .get(&identity),
            Some(&Some(SpawnPolicyRuntimeMode::TurnDriven))
        );

        let stale = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ResolveSpawnPolicy {
                agent_identity: AgentIdentity::from("stale-worker"),
                revision: 0,
                profile_name: Some("worker".to_string()),
                runtime_mode: None,
            },
        );
        assert!(
            stale.is_err(),
            "spawn-policy resolution must fail closed for stale generated revisions"
        );

        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::SetSpawnPolicy { enabled: false },
        )
        .expect("SetSpawnPolicy should clear generated policy authority");
        assert!(!authority.state().spawn_policy_enabled);
        assert_eq!(authority.state().spawn_policy_revision, 2);
        assert!(
            authority
                .state()
                .spawn_policy_resolution_profiles
                .is_empty()
        );

        let disabled = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ResolveSpawnPolicy {
                agent_identity: AgentIdentity::from("disabled-worker"),
                revision: 2,
                profile_name: Some("worker".to_string()),
                runtime_mode: None,
            },
        );
        assert!(
            disabled.is_err(),
            "spawn-policy resolution must fail closed when generated policy authority is disabled"
        );
    }

    #[test]
    fn spawn_profile_material_emits_typed_authorization() {
        let mut authority = MobMachineAuthority::new();
        let identity = AgentIdentity::from("profile-worker");

        let transition = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::AuthorizeSpawnProfile {
                agent_identity: identity.clone(),
                profile_name: "worker".to_string(),
                model: "claude-sonnet-4-5".to_string(),
                profile_material_digest: "profile-digest".to_string(),
                tool_config_digest: "tool-config-digest".to_string(),
                skills_digest: "skills-digest".to_string(),
                provider_params_digest: Some("provider-digest".to_string()),
                output_schema_digest: None,
                external_addressable: true,
            },
        )
        .expect("running MobMachine should authorize effective spawn profile material");

        assert!(transition.effects().iter().any(|effect| {
            matches!(
                effect,
                MobMachineEffect::SpawnProfileAuthorized {
                    agent_identity,
                    profile_name,
                    model,
                    profile_material_digest,
                    tool_config_digest,
                    skills_digest,
                    provider_params_digest: Some(digest),
                    output_schema_digest: None,
                    external_addressable: true,
                } if *agent_identity == identity
                    && profile_name == "worker"
                    && model == "claude-sonnet-4-5"
                    && profile_material_digest == "profile-digest"
                    && tool_config_digest == "tool-config-digest"
                    && skills_digest == "skills-digest"
                    && digest == "provider-digest"
            )
        }));
    }

    fn register_test_member_peer(
        authority: &mut MobMachineAuthority,
        identity: &AgentIdentity,
        name: &str,
        signing_key: [u8; 32],
    ) {
        MobMachineMutator::apply(
            authority,
            MobMachineInput::RegisterMemberPeer {
                agent_identity: identity.clone(),
                peer_endpoint: MemberPeerEndpoint {
                    name: PeerName(name.to_string()),
                    peer_id: PeerId(
                        meerkat_core::comms::PeerId::from_ed25519_pubkey(&signing_key).to_string(),
                    ),
                    address: PeerAddress(format!("inproc://{name}")),
                    signing_key: PeerSigningKey(signing_key),
                },
            },
        )
        .expect("register test member peer endpoint");
    }

    #[test]
    fn prepared_member_trust_handoff_rejects_obligation_after_live_epoch_advance() {
        let mut authority = MobMachineAuthority::new();
        let a = AgentIdentity::from("member-a");
        let b = AgentIdentity::from("member-b");
        let c = AgentIdentity::from("member-c");

        seed_live_member(&mut authority, &a, &AgentRuntimeId::from("member-a:1"));
        seed_live_member(&mut authority, &b, &AgentRuntimeId::from("member-b:1"));
        seed_live_member(&mut authority, &c, &AgentRuntimeId::from("member-c:1"));
        register_test_member_peer(&mut authority, &a, "member-a", [1; 32]);
        register_test_member_peer(&mut authority, &b, "member-b", [2; 32]);
        register_test_member_peer(&mut authority, &c, "member-c", [3; 32]);

        let prepared_batch_authority =
            crate::generated::protocol_mob_member_trust_wiring::MobTopologyPreparedBatchAuthority::from_live_authority(
                &authority,
            );
        let mut prepared_authority =
            MobMachineAuthority::recover_from_state(authority.state().clone())
                .expect("recover prepared batch authority");
        let first_edge = WiringEdge::new(a.clone(), b.clone());
        let first_transition = MobMachineMutator::apply(
            &mut prepared_authority,
            MobMachineInput::WireMembersWithTrust {
                edge: first_edge.clone(),
                a_identity: first_edge.a.clone(),
                b_identity: first_edge.b.clone(),
            },
        )
        .expect("prepared authority should wire first member pair");
        let stale_edge = WiringEdge::new(b.clone(), c.clone());
        let stale_transition = MobMachineMutator::apply(
            &mut prepared_authority,
            MobMachineInput::WireMembersWithTrust {
                edge: stale_edge.clone(),
                a_identity: stale_edge.a.clone(),
                b_identity: stale_edge.b.clone(),
            },
        )
        .expect("prepared authority should wire second member pair");

        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::WireMembersWithTrust {
                edge: first_edge.clone(),
                a_identity: first_edge.a.clone(),
                b_identity: first_edge.b.clone(),
            },
        )
        .expect("live authority should advance after first member pair");
        let error = prepared_batch_authority
            .freshness_for_prepared_transitions(&authority, [&first_transition, &stale_transition])
            .expect_err("stale prepared batch must not bind freshness after live epoch advances");
        assert!(
            error.contains("stale generated MobMachine prepared trust batch"),
            "unexpected stale prepared batch error: {error}"
        );
    }

    #[test]
    fn prepared_member_trust_handoff_rejects_live_epoch_advance_after_binding() {
        let mut authority = MobMachineAuthority::new();
        let a = AgentIdentity::from("member-a");
        let b = AgentIdentity::from("member-b");

        seed_live_member(&mut authority, &a, &AgentRuntimeId::from("member-a:1"));
        seed_live_member(&mut authority, &b, &AgentRuntimeId::from("member-b:1"));
        register_test_member_peer(&mut authority, &a, "member-a", [1; 32]);
        register_test_member_peer(&mut authority, &b, "member-b", [2; 32]);

        let prepared_batch_authority =
            crate::generated::protocol_mob_member_trust_wiring::MobTopologyPreparedBatchAuthority::from_live_authority(
                &authority,
            );
        let first_edge = WiringEdge::new(a.clone(), b.clone());
        let mut prepared_authority =
            MobMachineAuthority::recover_from_state(authority.state().clone())
                .expect("recover prepared batch authority");
        let first_transition = MobMachineMutator::apply(
            &mut prepared_authority,
            MobMachineInput::WireMembersWithTrust {
                edge: first_edge.clone(),
                a_identity: first_edge.a.clone(),
                b_identity: first_edge.b.clone(),
            },
        )
        .expect("prepared authority should wire first member pair");
        let freshness_authority = prepared_batch_authority
            .freshness_for_prepared_transitions(&authority, [&first_transition])
            .expect("prepared batch authority should bind exact generated obligation");
        let mut obligations =
            crate::generated::protocol_mob_member_trust_wiring::extract_obligations_with_freshness(
                &first_transition,
                freshness_authority,
            );
        let obligation = obligations
            .pop()
            .expect("prepared transition should carry member trust obligation");
        let expected_peer_id = obligation.b_peer_id().0.clone();

        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::WireMembersWithTrust {
                edge: first_edge.clone(),
                a_identity: first_edge.a.clone(),
                b_identity: first_edge.b.clone(),
            },
        )
        .expect("live authority should advance after prepared freshness binding");
        let error = crate::generated::protocol_mob_member_trust_wiring::wiring_authority_for_identity_with_live_authority(
            &obligation,
            "member-b",
            &expected_peer_id,
            &authority,
        )
        .expect_err("stale prepared freshness must not mint after live epoch advances");
        assert!(
            error.contains("stale generated MobMachine prepared trust batch"),
            "unexpected stale prepared member trust error: {error}"
        );
    }

    #[test]
    fn member_trust_handoff_rejects_live_peer_change_without_epoch_advance() {
        let mut authority = MobMachineAuthority::new();
        let a = AgentIdentity::from("member-a");
        let b = AgentIdentity::from("member-b");

        seed_live_member(&mut authority, &a, &AgentRuntimeId::from("member-a:1"));
        seed_live_member(&mut authority, &b, &AgentRuntimeId::from("member-b:1"));
        register_test_member_peer(&mut authority, &a, "member-a", [1; 32]);
        register_test_member_peer(&mut authority, &b, "member-b", [2; 32]);

        let edge = WiringEdge::new(a.clone(), b.clone());
        let transition = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::WireMembersWithTrust {
                edge: edge.clone(),
                a_identity: edge.a.clone(),
                b_identity: edge.b.clone(),
            },
        )
        .expect("live authority should wire member pair");
        let bound_epoch = authority.state().topology_epoch;
        let freshness_authority =
            crate::generated::protocol_mob_member_trust_wiring::MobTopologyFreshnessAuthority::from_live_member_trust_authority(
                &authority,
            );
        let mut obligations =
            crate::generated::protocol_mob_member_trust_wiring::extract_obligations_with_freshness(
                &transition,
                freshness_authority,
            );
        let obligation = obligations
            .pop()
            .expect("live transition should carry member trust obligation");
        let expected_peer_id = obligation.b_peer_id().0.clone();

        register_test_member_peer(&mut authority, &b, "member-b-rebound", [9; 32]);
        assert_eq!(
            authority.state().topology_epoch,
            bound_epoch,
            "member peer registration should not bump topology epoch in this regression"
        );

        let error = crate::generated::protocol_mob_member_trust_wiring::wiring_authority_for_identity_with_live_authority(
            &obligation,
            "member-b",
            &expected_peer_id,
            &authority,
        )
        .expect_err("stale member peer facts must not mint trust at the same topology epoch");
        assert!(
            error.contains("stale"),
            "unexpected stale member peer fact error: {error}"
        );
    }

    #[test]
    fn member_trust_handoff_rejects_foreign_live_authority_owner() {
        let mut authority = MobMachineAuthority::new();
        let a = AgentIdentity::from("member-a");
        let b = AgentIdentity::from("member-b");

        seed_live_member(&mut authority, &a, &AgentRuntimeId::from("member-a:1"));
        seed_live_member(&mut authority, &b, &AgentRuntimeId::from("member-b:1"));
        register_test_member_peer(&mut authority, &a, "member-a", [1; 32]);
        register_test_member_peer(&mut authority, &b, "member-b", [2; 32]);

        let edge = WiringEdge::new(a.clone(), b.clone());
        let transition = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::WireMembersWithTrust {
                edge: edge.clone(),
                a_identity: edge.a.clone(),
                b_identity: edge.b.clone(),
            },
        )
        .expect("live authority should wire member pair");
        let freshness_authority =
            crate::generated::protocol_mob_member_trust_wiring::MobTopologyFreshnessAuthority::from_live_member_trust_authority(
                &authority,
            );
        let mut obligations =
            crate::generated::protocol_mob_member_trust_wiring::extract_obligations_with_freshness(
                &transition,
                freshness_authority,
            );
        let obligation = obligations
            .pop()
            .expect("live transition should carry member trust obligation");
        let expected_peer_id = obligation.b_peer_id().0.clone();
        let foreign_authority = MobMachineAuthority::recover_from_state(authority.state().clone())
            .expect("recover same-state foreign authority");
        assert!(
            !std::sync::Arc::ptr_eq(
                &authority.generated_authority_owner_token(),
                &foreign_authority.generated_authority_owner_token()
            ),
            "test setup requires distinct generated owner tokens"
        );

        let error = crate::generated::protocol_mob_member_trust_wiring::wiring_authority_for_identity_with_live_authority(
            &obligation,
            "member-b",
            &expected_peer_id,
            &foreign_authority,
        )
        .expect_err("foreign live authority owner must not mint member trust authority");
        assert!(
            error.contains("different live authority owner"),
            "unexpected foreign owner member trust error: {error}"
        );
    }

    #[test]
    fn prepared_member_trust_handoff_rejects_prepared_authority_as_live() {
        let mut authority = MobMachineAuthority::new();
        let a = AgentIdentity::from("member-a");
        let b = AgentIdentity::from("member-b");

        seed_live_member(&mut authority, &a, &AgentRuntimeId::from("member-a:1"));
        seed_live_member(&mut authority, &b, &AgentRuntimeId::from("member-b:1"));
        register_test_member_peer(&mut authority, &a, "member-a", [1; 32]);
        register_test_member_peer(&mut authority, &b, "member-b", [2; 32]);

        let prepared_batch_authority =
            crate::generated::protocol_mob_member_trust_wiring::MobTopologyPreparedBatchAuthority::from_live_authority(
                &authority,
            );
        let edge = WiringEdge::new(a.clone(), b.clone());
        let mut prepared_authority =
            MobMachineAuthority::recover_from_state(authority.state().clone())
                .expect("recover prepared authority");
        let transition = MobMachineMutator::apply(
            &mut prepared_authority,
            MobMachineInput::WireMembersWithTrust {
                edge: edge.clone(),
                a_identity: edge.a.clone(),
                b_identity: edge.b.clone(),
            },
        )
        .expect("prepared authority should wire member pair");

        let error = prepared_batch_authority
            .freshness_for_prepared_transitions(&prepared_authority, [&transition])
            .expect_err("uncommitted prepared authority must not stand in for live authority");
        assert!(
            error.contains("different live authority owner"),
            "unexpected prepared-as-live member trust error: {error}"
        );
    }

    #[test]
    fn commit_prepared_authority_preserves_live_owner_token() {
        let mut authority = MobMachineAuthority::new();
        let owner = authority.generated_authority_owner_token();
        let mut prepared = authority.prepare_authority();
        MobMachineMutator::apply(&mut prepared, MobMachineInput::Stop)
            .expect("prepared authority should accept Stop");

        authority
            .commit_prepared_authority(prepared)
            .expect("prepared authority should commit against unchanged live base");

        assert_eq!(authority.state().lifecycle_phase, MobPhase::Stopped);
        assert!(
            std::sync::Arc::ptr_eq(&owner, &authority.generated_authority_owner_token()),
            "committing a prepared generated authority must preserve the live owner token"
        );
    }

    #[test]
    fn commit_prepared_authority_rejects_stale_live_base() {
        let mut authority = MobMachineAuthority::new();
        let mut prepared = authority.prepare_authority();
        MobMachineMutator::apply(&mut prepared, MobMachineInput::Stop)
            .expect("prepared authority should accept Stop");
        MobMachineMutator::apply(&mut authority, MobMachineInput::Complete)
            .expect("live authority should move away from prepared base");

        let error = authority
            .commit_prepared_authority(prepared)
            .expect_err("prepared authority must not commit after live base changes");
        assert!(
            matches!(error, MobMachinePreparedCommitError::BaseChanged { .. }),
            "unexpected stale prepared commit error: {error}"
        );
    }

    #[test]
    fn prepared_member_trust_handoff_rejects_live_phase_change_after_binding() {
        let mut authority = MobMachineAuthority::new();
        let a = AgentIdentity::from("member-a");
        let b = AgentIdentity::from("member-b");

        seed_live_member(&mut authority, &a, &AgentRuntimeId::from("member-a:1"));
        seed_live_member(&mut authority, &b, &AgentRuntimeId::from("member-b:1"));
        register_test_member_peer(&mut authority, &a, "member-a", [1; 32]);
        register_test_member_peer(&mut authority, &b, "member-b", [2; 32]);

        let prepared_batch_authority =
            crate::generated::protocol_mob_member_trust_wiring::MobTopologyPreparedBatchAuthority::from_live_authority(
                &authority,
            );
        let edge = WiringEdge::new(a.clone(), b.clone());
        let mut prepared_authority =
            MobMachineAuthority::recover_from_state(authority.state().clone())
                .expect("recover prepared batch authority");
        let transition = MobMachineMutator::apply(
            &mut prepared_authority,
            MobMachineInput::WireMembersWithTrust {
                edge: edge.clone(),
                a_identity: edge.a.clone(),
                b_identity: edge.b.clone(),
            },
        )
        .expect("prepared authority should wire member pair");
        let freshness_authority = prepared_batch_authority
            .freshness_for_prepared_transitions(&authority, [&transition])
            .expect("prepared batch authority should bind exact generated obligation");
        let mut obligations =
            crate::generated::protocol_mob_member_trust_wiring::extract_obligations_with_freshness(
                &transition,
                freshness_authority,
            );
        let obligation = obligations
            .pop()
            .expect("prepared transition should carry member trust obligation");
        let expected_peer_id = obligation.b_peer_id().0.clone();

        MobMachineMutator::apply(&mut authority, MobMachineInput::Stop)
            .expect("live authority should stop without bumping topology");
        let error = crate::generated::protocol_mob_member_trust_wiring::wiring_authority_for_identity_with_live_authority(
            &obligation,
            "member-b",
            &expected_peer_id,
            &authority,
        )
        .expect_err("prepared member trust must not mint after live phase changes");
        assert!(
            error.contains("stale generated MobMachine prepared trust batch"),
            "unexpected stale prepared phase error: {error}"
        );
    }

    #[test]
    fn prepared_member_trust_handoff_rejects_unbound_exact_obligation() {
        let mut authority = MobMachineAuthority::new();
        let a = AgentIdentity::from("member-a");
        let b = AgentIdentity::from("member-b");
        let c = AgentIdentity::from("member-c");

        seed_live_member(&mut authority, &a, &AgentRuntimeId::from("member-a:1"));
        seed_live_member(&mut authority, &b, &AgentRuntimeId::from("member-b:1"));
        seed_live_member(&mut authority, &c, &AgentRuntimeId::from("member-c:1"));
        register_test_member_peer(&mut authority, &a, "member-a", [1; 32]);
        register_test_member_peer(&mut authority, &b, "member-b", [2; 32]);
        register_test_member_peer(&mut authority, &c, "member-c", [3; 32]);

        let prepared_batch_authority =
            crate::generated::protocol_mob_member_trust_wiring::MobTopologyPreparedBatchAuthority::from_live_authority(
                &authority,
            );
        let first_edge = WiringEdge::new(a.clone(), b.clone());
        let mut prepared_authority =
            MobMachineAuthority::recover_from_state(authority.state().clone())
                .expect("recover prepared batch authority");
        let first_transition = MobMachineMutator::apply(
            &mut prepared_authority,
            MobMachineInput::WireMembersWithTrust {
                edge: first_edge.clone(),
                a_identity: first_edge.a.clone(),
                b_identity: first_edge.b.clone(),
            },
        )
        .expect("prepared authority should wire first member pair");
        let freshness_authority = prepared_batch_authority
            .freshness_for_prepared_transitions(&authority, [&first_transition])
            .expect("prepared batch authority should bind exact generated obligation");

        let foreign_edge = WiringEdge::new(b.clone(), c.clone());
        let mut foreign_authority =
            MobMachineAuthority::recover_from_state(authority.state().clone())
                .expect("recover same-base foreign authority");
        let foreign_transition = MobMachineMutator::apply(
            &mut foreign_authority,
            MobMachineInput::WireMembersWithTrust {
                edge: foreign_edge.clone(),
                a_identity: foreign_edge.a.clone(),
                b_identity: foreign_edge.b.clone(),
            },
        )
        .expect("same-base foreign authority should wire different member pair");
        let raw_obligation =
            crate::generated::protocol_mob_member_trust_wiring::extract_obligations(
                &foreign_transition,
            )
            .pop()
            .expect("foreign transition should carry raw member trust obligation");
        let foreign_expected_peer_id = raw_obligation.b_peer_id().0.clone();
        let mut obligations =
            crate::generated::protocol_mob_member_trust_wiring::extract_obligations_with_freshness(
                &foreign_transition,
                freshness_authority,
            );
        let obligation = obligations
            .pop()
            .expect("foreign transition should carry member trust obligation");

        let error = crate::generated::protocol_mob_member_trust_wiring::wiring_authority_for_identity_with_live_authority(
            &obligation,
            "member-c",
            &foreign_expected_peer_id,
            &authority,
        )
        .expect_err("unbound exact obligation must not mint comms trust authority");
        assert!(
            error.contains("does not contain exact obligation"),
            "unexpected unbound prepared member trust error: {error}"
        );
    }

    #[test]
    fn submit_work_rejects_retiring_runtime() {
        let mut authority = MobMachineAuthority::new();
        let identity = AgentIdentity::from("worker");
        let runtime_id = AgentRuntimeId::from("worker:1");
        let session_id = seed_live_member(&mut authority, &identity, &runtime_id);

        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::SubmitWork {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_id.clone(),
                fence_token: FenceToken(7),
                work_id: WorkId::from("before-retire"),
                origin: WorkOrigin::External,
            },
        )
        .expect("live externally addressable member should accept work");

        authority
            .apply_signal(MobMachineSignal::RetireMember {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_id.clone(),
                fence_token: FenceToken(7),
                session_id: Some(session_id),
            })
            .expect("RetireMember should mark the live member as retiring");

        let rejected = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::SubmitWork {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_id.clone(),
                fence_token: FenceToken(7),
                work_id: WorkId::from("during-retire"),
                origin: WorkOrigin::External,
            },
        );
        assert!(
            rejected.is_err(),
            "Retiring work admission must be owned by generated SubmitWork guards"
        );

        let rejection = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ResolveSubmitWorkRejection {
                agent_identity: identity,
                agent_runtime_id: runtime_id,
                fence_token: FenceToken(7),
                origin: WorkOrigin::External,
            },
        )
        .expect("retiring SubmitWork rejection should have typed machine feedback");
        assert!(
            rejection.effects.iter().any(|effect| matches!(
                effect,
                MobMachineEffect::SubmitWorkRejected {
                    reason: SubmitWorkRejectReasonKind::MemberNotFound,
                    ..
                }
            )),
            "SubmitWork rejection public class must be owned by generated feedback"
        );
    }

    #[test]
    fn retire_member_rejects_absent_session_for_session_bound_member() {
        let mut authority = MobMachineAuthority::new();
        let identity = AgentIdentity::from("worker");
        let runtime_id = AgentRuntimeId::from("worker:1");
        let session_id = seed_live_member(&mut authority, &identity, &runtime_id);

        let rejected = authority.apply_signal(MobMachineSignal::RetireMember {
            agent_identity: identity.clone(),
            agent_runtime_id: runtime_id.clone(),
            fence_token: FenceToken(7),
            session_id: None,
        });
        assert!(
            rejected.is_err(),
            "session-bound RetireMember must not accept caller-supplied None"
        );
        assert!(
            !authority
                .state()
                .member_state_markers
                .contains_key(&runtime_id),
            "rejected retire must not mutate lifecycle state"
        );

        authority
            .apply_signal(MobMachineSignal::RetireMember {
                agent_identity: identity,
                agent_runtime_id: runtime_id.clone(),
                fence_token: FenceToken(7),
                session_id: Some(session_id),
            })
            .expect("matching session binding should retire");
        assert!(
            authority
                .state()
                .member_state_markers
                .contains_key(&runtime_id)
        );
    }

    #[test]
    fn retire_input_rejects_mismatched_identity_runtime_or_generation() {
        let mut authority = MobMachineAuthority::new();
        let identity = AgentIdentity::from("worker");
        let runtime_id = AgentRuntimeId::from("worker:1");
        let session_id = seed_live_member(&mut authority, &identity, &runtime_id);
        let other_identity = AgentIdentity::from("other");
        let other_runtime_id = AgentRuntimeId::from("other:1");
        seed_live_member(&mut authority, &other_identity, &other_runtime_id);

        let mismatched_runtime = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::Retire {
                mob_id: MobId::from("test-mob"),
                agent_runtime_id: other_runtime_id.clone(),
                agent_identity: identity.clone(),
                generation: Generation(1),
                releasing: None,
                session_id: Some(session_id.clone()),
            },
        );
        assert!(
            mismatched_runtime.is_err(),
            "Retire must bind caller identity to the current runtime id"
        );
        assert!(
            !authority
                .state()
                .member_state_markers
                .contains_key(&other_runtime_id),
            "rejected mismatched runtime retire must not mutate lifecycle state"
        );

        let stale_generation = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::Retire {
                mob_id: MobId::from("test-mob"),
                agent_runtime_id: runtime_id.clone(),
                agent_identity: identity.clone(),
                generation: Generation(99),
                releasing: None,
                session_id: Some(session_id.clone()),
            },
        );
        assert!(
            stale_generation.is_err(),
            "Retire must bind caller generation to generated identity state"
        );
        assert!(
            !authority
                .state()
                .member_state_markers
                .contains_key(&runtime_id),
            "rejected stale generation retire must not mutate lifecycle state"
        );

        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::Retire {
                mob_id: MobId::from("test-mob"),
                agent_runtime_id: runtime_id.clone(),
                agent_identity: identity,
                generation: Generation(1),
                releasing: None,
                session_id: Some(session_id),
            },
        )
        .expect("matching identity/runtime/generation retire should be accepted");
        assert!(
            authority
                .state()
                .member_state_markers
                .contains_key(&runtime_id)
        );
    }

    #[test]
    fn submit_work_rejects_stale_fence_token_with_typed_feedback() {
        let mut authority = MobMachineAuthority::new();
        let identity = AgentIdentity::from("worker");
        let runtime_id = AgentRuntimeId::from("worker:1");
        seed_live_member(&mut authority, &identity, &runtime_id);

        let rejected = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::SubmitWork {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_id.clone(),
                fence_token: FenceToken(99),
                work_id: WorkId::from("stale"),
                origin: WorkOrigin::Internal,
            },
        );
        assert!(
            rejected.is_err(),
            "stale fence token must be rejected by generated SubmitWork guards"
        );

        let rejection = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ResolveSubmitWorkRejection {
                agent_identity: identity,
                agent_runtime_id: runtime_id,
                fence_token: FenceToken(99),
                origin: WorkOrigin::Internal,
            },
        )
        .expect("stale SubmitWork rejection should have typed machine feedback");
        assert!(
            rejection.effects.iter().any(|effect| matches!(
                effect,
                MobMachineEffect::SubmitWorkRejected {
                    reason: SubmitWorkRejectReasonKind::StaleFenceToken,
                    expected_fence_token: Some(FenceToken(7)),
                    actual_fence_token: Some(FenceToken(99)),
                    ..
                }
            )),
            "stale SubmitWork public class and fence payload must be generated feedback"
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
                output_recorded: [(node_id.clone(), false)].into_iter().collect(),
                node_condition_results: [(node_id.clone(), None)].into_iter().collect(),
                last_admitted_node: None,
            },
        )
        .expect("CreateFrameSeed should be accepted");

        assert_eq!(transition.to_phase, MobPhase::Running);
        assert_eq!(
            authority.state().frame_scope.get(&frame_id),
            Some(&FrameScope::Root)
        );
        assert_eq!(authority.state().frame_run.get(&frame_id), Some(&run_id));
        assert_eq!(
            authority.state().frame_ordered_nodes.get(&frame_id),
            Some(&vec![node_id.clone()])
        );
        assert_eq!(
            authority
                .state()
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
            authority.state().loop_parent_frame.get(&loop_instance_id),
            Some(&frame_id)
        );
        assert_eq!(
            authority.state().loop_parent_node.get(&loop_instance_id),
            Some(&node_id)
        );
        assert_eq!(
            authority.state().loop_definition.get(&loop_instance_id),
            Some(&loop_id)
        );
        assert_eq!(
            authority.state().loop_stage.get(&loop_instance_id),
            Some(&LoopIterationStage::AwaitingBodyFrame)
        );
        assert_eq!(
            authority
                .state()
                .loop_current_iteration
                .get(&loop_instance_id),
            Some(&0)
        );
        assert_eq!(
            authority
                .state()
                .loop_last_completed_iteration
                .get(&loop_instance_id),
            Some(&0)
        );
    }

    #[test]
    fn loop_until_feedback_is_recorded_by_mob_machine() {
        let mut authority = MobMachineAuthority::new();
        let run_id = RunId::from("run-1");
        let loop_instance_id = LoopInstanceId::from("loop-1");
        let parent_frame_id = FrameId::from("frame-root");
        let parent_node_id = FlowNodeId::from("loop-node");
        seed_root_frame(&mut authority, &run_id, &parent_frame_id, &parent_node_id);

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
        seed_body_frame(
            &mut authority,
            &run_id,
            &FrameId::from("frame-body-0"),
            &loop_instance_id,
            0,
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
            authority.state().loop_stage.get(&loop_instance_id),
            Some(&LoopIterationStage::AwaitingUntilEvaluation)
        );
        assert_eq!(
            authority
                .state()
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
            authority.state().loop_stage.get(&loop_instance_id),
            Some(&LoopIterationStage::AwaitingBodyFrame)
        );

        seed_body_frame(
            &mut authority,
            &run_id,
            &FrameId::from("frame-body-1"),
            &loop_instance_id,
            1,
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
            authority.state().loop_phase.get(&loop_instance_id),
            Some(&LoopStatus::Completed)
        );
    }

    #[test]
    fn observe_runtime_retired_keeps_retiring_marker_until_archive_completion() {
        let mut authority = MobMachineAuthority::new();
        let identity = AgentIdentity::from("worker");
        let runtime_id = AgentRuntimeId::from("worker:1");
        let fence_token = FenceToken(7);
        let session_id = seed_live_member(&mut authority, &identity, &runtime_id);
        authority
            .apply_signal(MobMachineSignal::RetireMember {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_id.clone(),
                fence_token,
                session_id: Some(session_id),
            })
            .expect("RetireMember should mark the live member as retiring");
        authority
            .apply_signal(MobMachineSignal::StartRun)
            .expect("StartRun should increment active run count");
        assert_eq!(authority.state().active_run_count, 1);

        let transition = authority
            .apply_signal(MobMachineSignal::ObserveRuntimeRetired {
                agent_runtime_id: runtime_id.clone(),
                fence_token,
            })
            .expect("runtime retire observation should be accepted");

        assert_eq!(transition.to_phase, MobPhase::Running);
        assert_eq!(authority.state().lifecycle_phase, MobPhase::Running);
        assert!(!authority.state().live_runtime_ids.contains(&runtime_id));
        assert!(
            !authority
                .state()
                .externally_addressable_runtime_ids
                .contains(&runtime_id)
        );
        assert!(
            !authority
                .state()
                .runtime_fence_tokens
                .contains_key(&runtime_id)
        );
        assert!(
            authority
                .state()
                .member_state_markers
                .contains_key(&runtime_id)
        );
        assert_eq!(
            authority.state().member_lifecycle_for_identity(&identity),
            MobMemberLifecycleMaterial {
                status: MobMemberLifecycleStatus::Retiring,
                terminal_class: MobMemberTerminalClass::Running,
                error: None,
            }
        );
        assert_eq!(authority.state().active_run_count, 0);

        authority
            .apply_signal(MobMachineSignal::ObserveMemberRetirementArchived {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_id.clone(),
                fence_token,
            })
            .expect("archive completion should clear the retiring marker");
        assert!(
            !authority
                .state()
                .member_state_markers
                .contains_key(&runtime_id)
        );
        assert_eq!(
            authority.state().member_lifecycle_for_identity(&identity),
            MobMemberLifecycleMaterial {
                status: MobMemberLifecycleStatus::Completed,
                terminal_class: MobMemberTerminalClass::TerminalCompleted,
                error: None,
            }
        );
    }

    #[test]
    fn member_lifecycle_projection_is_derived_from_machine_membership() {
        let mut authority = MobMachineAuthority::new();
        let identity = AgentIdentity::from("worker");
        let runtime_id = AgentRuntimeId::from("worker:1");

        assert_eq!(
            authority.state().member_lifecycle_for_identity(&identity),
            MobMemberLifecycleMaterial {
                status: MobMemberLifecycleStatus::Unknown,
                terminal_class: MobMemberTerminalClass::TerminalUnknown,
                error: None,
            }
        );

        let session_id = seed_live_member(&mut authority, &identity, &runtime_id);
        let active = authority.state().member_lifecycle_for_identity(&identity);
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
            .apply_signal(MobMachineSignal::RetireMember {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_id.clone(),
                fence_token: FenceToken(7),
                session_id: Some(session_id),
            })
            .expect("RetireMember should mark the live member as retiring");
        let retiring = authority.state().member_lifecycle_for_identity(&identity);
        assert_eq!(
            retiring,
            MobMemberLifecycleMaterial {
                status: MobMemberLifecycleStatus::Retiring,
                terminal_class: MobMemberTerminalClass::Running,
                error: None,
            }
        );
        assert!(!retiring.is_terminal());

        authority
            .apply_signal(MobMachineSignal::ObserveRuntimeRetired {
                agent_runtime_id: runtime_id.clone(),
                fence_token: FenceToken(7),
            })
            .expect("runtime retire observation should remove runtime liveness");
        authority
            .apply_signal(MobMachineSignal::ObserveMemberRetirementArchived {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_id.clone(),
                fence_token: FenceToken(7),
            })
            .expect("archive completion should clear retiring marker");
        let completed = authority.state().member_lifecycle_for_identity(&identity);
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

        seed_live_member(&mut authority, &identity, &runtime_id);
        authority
            .apply_signal(MobMachineSignal::RecoverMemberRestoreFailure {
                agent_identity: identity.clone(),
                reason: "missing durable session".to_string(),
            })
            .expect("restore failure should be recorded by machine authority");

        assert_eq!(
            authority.state().member_lifecycle_for_identity(&identity),
            MobMemberLifecycleMaterial {
                status: MobMemberLifecycleStatus::Broken,
                terminal_class: MobMemberTerminalClass::TerminalFailure,
                error: Some("missing durable session".to_string()),
            }
        );
    }

    #[test]
    fn kickoff_cancelled_outcome_uses_machine_cancelled_truth() {
        let mut authority = MobMachineAuthority::new();
        let member_id = "worker".to_string();

        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::KickoffMarkPending {
                member_id: member_id.clone(),
            },
        )
        .expect("kickoff should enter pending state");
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::KickoffMarkStarting {
                member_id: member_id.clone(),
            },
        )
        .expect("kickoff should enter starting state");

        let cancelled = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::KickoffCancelRequested {
                member_id: member_id.clone(),
            },
        )
        .expect("generated kickoff cancellation should be accepted");

        assert!(cancelled.effects().iter().any(|effect| {
            matches!(
                effect,
                MobMachineEffect::PersistKickoffUpdate {
                    member_id: effect_member_id,
                    phase: KickoffPhase::Cancelled,
                } if effect_member_id == &member_id
            )
        }));
        assert!(
            authority
                .state()
                .member_kickoff_cancelled
                .contains(&member_id)
        );
        assert!(!authority.state().member_kickoff_failed.contains(&member_id));
        assert!(
            !authority
                .state()
                .member_kickoff_error
                .contains_key(&member_id)
        );
    }

    #[test]
    fn recovered_kickoff_lifecycle_is_machine_owned() {
        let mut authority = MobMachineAuthority::new();
        let member_id = "worker".to_string();

        authority
            .apply_signal(MobMachineSignal::RecoverMemberKickoff {
                member_id: member_id.clone(),
                phase: KickoffPhase::Starting,
                error: None,
            })
            .expect("recovered starting kickoff should be accepted");
        assert_eq!(
            authority.state().kickoff_material_for_member_id(&member_id),
            Some(MobMemberKickoffMaterial {
                phase: KickoffPhase::Starting,
                error: None,
            })
        );

        authority
            .apply_signal(MobMachineSignal::RecoverMemberKickoff {
                member_id: member_id.clone(),
                phase: KickoffPhase::Failed,
                error: Some("runtime failed".to_string()),
            })
            .expect("recovered failed kickoff should be accepted");
        assert_eq!(
            authority.state().kickoff_material_for_member_id(&member_id),
            Some(MobMemberKickoffMaterial {
                phase: KickoffPhase::Failed,
                error: Some("runtime failed".to_string()),
            })
        );
        assert!(
            !authority
                .state()
                .member_kickoff_starting
                .contains(&member_id)
        );
    }

    #[test]
    fn respawn_topology_restore_result_class_is_machine_owned() {
        let mut authority = MobMachineAuthority::new();
        let identity = AgentIdentity::from("worker");
        let runtime_id = AgentRuntimeId::from("worker:1");
        let failed_peer = RespawnTopologyPeerId::from("peer");
        let expected_failed_peer_ids = vec![failed_peer];

        seed_live_member(&mut authority, &identity, &runtime_id);

        let completed = authority
            .apply_signal(MobMachineSignal::ResolveRespawnTopologyRestore {
                agent_identity: identity.clone(),
                failed_peer_ids: Vec::new(),
            })
            .expect("machine should classify empty restore failures as completed");
        assert!(completed.effects().iter().any(|effect| {
            matches!(
                effect,
                MobMachineEffect::RespawnTopologyRestoreResolved {
                    agent_identity,
                    result: RespawnTopologyRestoreResultKind::Completed,
                    failed_peer_ids,
                } if *agent_identity == identity && failed_peer_ids.is_empty()
            )
        }));

        let failed = authority
            .apply_signal(MobMachineSignal::ResolveRespawnTopologyRestore {
                agent_identity: identity.clone(),
                failed_peer_ids: expected_failed_peer_ids.clone(),
            })
            .expect("machine should classify non-empty restore failures as topology failure");
        assert!(failed.effects().iter().any(|effect| {
            matches!(
                effect,
                MobMachineEffect::RespawnTopologyRestoreResolved {
                    agent_identity,
                    result: RespawnTopologyRestoreResultKind::TopologyRestoreFailed,
                    failed_peer_ids,
                } if *agent_identity == identity && failed_peer_ids == &expected_failed_peer_ids
            )
        }));
    }
}
