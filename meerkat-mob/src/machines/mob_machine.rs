//! MobMachine — DSL-generated canonical state.
//!
//! The generated `MobMachineState` is the machine-owned portion of mob state.
//! It covers lifecycle phase, roster membership, run tracking, spawn tracking,
//! and coordinator binding. Shell infrastructure (channels, stores, services,
//! handles, etc.) is NOT modeled here.

use meerkat_machine_schema::catalog::dsl::OptionValueExt;
pub use meerkat_machine_schema::catalog::dsl::mob_machine::{
    AdaptiveDecisionKind, AdaptiveLayerAdmissionKind, AdaptiveLayerDispositionKind,
    AdaptiveLayerPhase, AdaptiveLayerSetupFaultKind, AdaptiveRunPhase, AdaptiveStopReason,
    ExternalMemberRebindCapability, FlowFrameReducerCommandKind, FlowRunPublicResultClassKind,
    FlowRunReducerCommandKind, LoopIterationReducerCommandKind, MemberAdmissionVerdictKind,
    MemberHealthClass, MemberProgressEventKind, MobLifecycleJournalKind,
    PlacedCompletionLifecycleIntentKind, PolicyDecision, SpawnExecPhase, StepFaultDispositionKind,
    StepOutputFaultKind, SupervisorEscalationFailureCause, TurnTimeoutDisposition,
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

/// Bridging type for adaptive run identity.
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
pub struct AdaptiveRunId(pub String);

impl<T: Into<String>> From<T> for AdaptiveRunId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Bridging type for adaptive layer identity.
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
pub struct AdaptiveLayerId(pub String);

impl<T: Into<String>> From<T> for AdaptiveLayerId {
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
/// `Default` is required by the `OptionValueExt::get("value")` unwrap path
/// used in the multi-host remote retire/release arms.
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
pub struct FenceToken(pub u64);

impl From<u64> for FenceToken {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

/// Bridging type for generation counter. Maps to `crate::ids::Generation`.
/// `Default` is required by the `OptionValueExt::get("value")` unwrap path
/// used in the multi-host remote retire/release arms.
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
pub struct Generation(pub u64);

impl From<u64> for Generation {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

/// Stable controller-minted id for one durable placed-spawn attempt.
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
pub struct PlacedSpawnId(pub String);

/// Carrier phase expected by an exact cleanup compare-delete.
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
pub enum PlacedSpawnCarrierExpectedPhase {
    #[default]
    Pending,
    Committed,
}

/// Exact machine-owned cleanup authority for a durable carrier row.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct PlacedCarrierCleanupObligation {
    pub agent_identity: AgentIdentity,
    pub spawn_id: PlacedSpawnId,
    pub generation: Generation,
    pub fence_token: FenceToken,
    pub provision_operation_id: String,
    pub operation_owner_session_id: SessionId,
    pub expected_phase: PlacedSpawnCarrierExpectedPhase,
}

impl Default for PlacedCarrierCleanupObligation {
    fn default() -> Self {
        Self {
            agent_identity: AgentIdentity(String::new()),
            spawn_id: PlacedSpawnId::default(),
            generation: Generation::default(),
            fence_token: FenceToken::default(),
            provision_operation_id: String::new(),
            operation_owner_session_id: SessionId::default(),
            expected_phase: PlacedSpawnCarrierExpectedPhase::default(),
        }
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
///
/// Multi-host (§6.1/§15.4/§18.9, A4): ONE merged cause vocabulary for the
/// spawn-exec ladder's placement/portability denial arms. Rich payload
/// shapes (`NonPortableResource { kind }` etc.) flatten to unit variants —
/// the DSL emit grammar has no struct-variant construction; the wire layer
/// reassembles the payload form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobSpawnMemberAdmissionKind {
    #[default]
    Denied,
    Allowed,
    NonPortableRustBundles,
    NonPortablePerSpawnExternalTools,
    NonPortableMobDefaultExternalTools,
    NonPortableDefaultLlmClientOverride,
    NonPortableHostSurfaceMcpAllowlist,
    NonPortableInheritedToolFilter,
    NonPortableWorkgraphTools,
    SecretBearingShellEnv,
    SecretBearingMcpStdioEnv,
    SecretBearingMcpHttpHeaders,
    MissingHostCapabilityAutonomousMembers,
    MissingHostCapabilityDurableSessions,
    MissingHostCapabilityTrackedInputCancel,
    MissingHostCapabilityProtocolV4,
    MissingHostCapabilityMemoryStore,
    MissingHostCapabilityMcp,
    HostNotBound,
    OwnerBridgeSessionAbsent,
    LaunchModePlacementMismatch,
    ResolvedSpecDigestAbsent,
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

/// Typed shell observation of a member's live materialization at the dispatch
/// boundary: the member's current bridge session has no live runtime, and the
/// durable session snapshot is either still present (revivable) or gone
/// (terminal). The shell observes; MobMachine owns the verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MemberLiveMaterializationObservationKind {
    #[default]
    DurableSnapshotPresent,
    DurableSnapshotMissing,
}

/// Machine-owned verdict for a member live-materialization observation:
/// authorize exactly one shell revival attempt, or record the terminal Broken
/// classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MemberRevivalVerdictKind {
    #[default]
    ReviveAuthorized,
    BrokenRecorded,
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
// Multi-host mobs (§6.1/§6.2/§6.5/§15/§18) bridging types. Local twins of the
// catalog-side types (like PeerId/WiringEdge above) so domain/wire From impls
// can live here without orphan-rule violations.
// ---------------------------------------------------------------------------

/// Identity-first member-host id: the host's comms `PeerId` string. No second
/// id space; wire-boundary validation (pubkey derivation) is owned by the
/// contracts/shell seam, not this bridging newtype.
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
pub struct HostId(pub String);
impl<T: Into<String>> From<T> for HostId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}
impl HostId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Non-prunable proof that one exact host binding generation was revoked.
/// Local twin of the catalog type so the generated MobMachine uses this
/// module's bridging [`HostId`] rather than the schema crate's nominal type.
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
pub struct HostBindingGenerationTombstone {
    pub host_id: HostId,
    pub binding_generation: u64,
}

/// Host bind window phase: absence of a `host_bind_phase` entry = unbound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum HostBindPhase {
    #[default]
    Requested,
    Bound,
}

/// Scheme-qualified `ws|wss` absolute base URL for a host's live channel
/// acceptor (§16 DL5). Shape validation is a wire-boundary concern.
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
pub struct LiveWsEndpointUrl(pub String);
impl<T: Into<String>> From<T> for LiveWsEndpointUrl {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Opaque principal identity for control-scope grants (§8).
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
pub struct PrincipalId(pub String);
impl<T: Into<String>> From<T> for PrincipalId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Remote turn-directive input id (§18 O2 obligation key component).
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
pub struct InputId(pub String);
impl<T: Into<String>> From<T> for InputId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Principal→mob control-scope vocabulary (A9, ten variants; `Live` gates the
/// §16 live family; `AdminHost` is bind/revoke hosts ONLY; grant
/// administration is `AdminGrants`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ControlScope {
    #[default]
    List,
    ReadHistory,
    SubscribeEvents,
    SendCommand,
    Cancel,
    Retire,
    WireTopology,
    Live,
    AdminHost,
    AdminGrants,
}

/// Typed member-session disposal carried by the retirement-archived signals
/// (§19.L4). Flat DSL vocabulary; the wire folds `AlreadyArchived` into
/// `Archived` (both mean the durable terminal holds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MemberSessionDisposal {
    #[default]
    Archived,
    RuntimeReleasedOnlyHostOwned,
    RuntimeReleasedOnlyNoDurableSessions,
}

/// Machine-owned flow-step dispatch verdict (§18.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum FlowStepDispatchKind {
    #[default]
    Local,
    RemoteTurnDirective,
    RejectedOverlayAutonomous,
    RejectedHostIncapable,
}

/// Typed member-operator upcall rejection cause (§15 R6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MemberOperatorRejectKind {
    #[default]
    UnknownIdentity,
    SenderKeyMismatch,
    StaleGeneration,
    StaleFence,
    StaleSession,
    StaleHost,
    StaleHostBindingGeneration,
    HostRevoked,
    NoPlacement,
}

/// Route-operation discriminant (§6.2). Install may enter the pending ledger;
/// Remove is ephemeral synchronous pre-unwire authority only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RouteObligationKind {
    #[default]
    Install,
    Remove,
}

/// Host-scoped route-operation descriptor (§6.2, D4). Only Install values may
/// be outstanding; Remove values are carried only across the synchronous
/// pre-unwire input/effect handoff.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RouteInstallObligation {
    pub edge: WiringEdge,
    pub host: HostId,
    pub kind: RouteObligationKind,
}

impl Default for RouteInstallObligation {
    fn default() -> Self {
        Self {
            edge: WiringEdge::new(AgentIdentity(String::new()), AgentIdentity(String::new())),
            host: HostId::default(),
            kind: RouteObligationKind::default(),
        }
    }
}

/// Outstanding remote turn-directive outcome obligation (§18 O2).
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct RemoteTurnObligation {
    pub agent_identity: AgentIdentity,
    pub host_id: HostId,
    pub host_binding_generation: u64,
    pub member_session_id: SessionId,
    pub generation: Generation,
    pub fence_token: FenceToken,
    pub dispatch_sequence: u64,
    pub input_id: InputId,
    pub run_id: RunId,
    pub step_id: StepId,
}

impl Default for RemoteTurnObligation {
    fn default() -> Self {
        Self {
            agent_identity: AgentIdentity(String::new()),
            host_id: HostId::default(),
            host_binding_generation: 0,
            member_session_id: SessionId::default(),
            generation: Generation::default(),
            fence_token: FenceToken::default(),
            dispatch_sequence: 0,
            input_id: InputId::default(),
            run_id: RunId(String::new()),
            step_id: StepId(String::new()),
        }
    }
}

/// Exact controller cleanup custody for one ordinary placed
/// `SubmitWork(ack_mode = TurnCompleted)` interaction.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct PlacedCompletionObligation {
    pub agent_identity: AgentIdentity,
    pub host_id: HostId,
    pub host_binding_generation: u64,
    pub member_session_id: SessionId,
    pub generation: Generation,
    pub fence_token: FenceToken,
    pub dispatch_sequence: u64,
    pub input_id: InputId,
}

impl Default for PlacedCompletionObligation {
    fn default() -> Self {
        Self {
            agent_identity: AgentIdentity(String::new()),
            host_id: HostId::default(),
            host_binding_generation: 0,
            member_session_id: SessionId::default(),
            generation: Generation::default(),
            fence_token: FenceToken::default(),
            dispatch_sequence: 0,
            input_id: InputId::default(),
        }
    }
}

/// Exact controlling-side custody key for one placed autonomous kickoff.
///
/// Prompt/content metadata stays in the private durable intent carrier. The
/// machine owns the exact host/member residency, objective, and canonical
/// UUID correlation used for runtime input, idempotency, and interaction
/// terminal acknowledgement.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct PlacedKickoffObligation {
    pub agent_identity: AgentIdentity,
    pub host_id: HostId,
    pub host_binding_generation: u64,
    pub member_session_id: SessionId,
    pub generation: Generation,
    pub fence_token: FenceToken,
    pub input_id: InputId,
    pub objective_id: String,
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
pub enum PlacedKickoffOutcomeKind {
    Started,
    CallbackPending,
    Failed,
    Cancelled,
    RejectedNoEffect,
    #[default]
    Disposed,
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
pub enum PlacedKickoffClosureKind {
    #[default]
    Acknowledged,
    Disposed,
    RejectedNoEffect,
}

impl Default for PlacedKickoffObligation {
    fn default() -> Self {
        Self {
            agent_identity: AgentIdentity(String::new()),
            host_id: HostId::default(),
            host_binding_generation: 0,
            member_session_id: SessionId::default(),
            generation: Generation::default(),
            fence_token: FenceToken::default(),
            input_id: InputId::default(),
            objective_id: String::new(),
        }
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
            generation: *self.identity_runtime_generations.get(agent_identity)?,
            fence_token: *self.identity_runtime_fence_tokens.get(agent_identity)?,
        })
    }

    /// Whether a placed member's committed carrier still names the exact
    /// currently bound host generation. Logical membership may outlive this
    /// predicate while a host is revoked or the member awaits revival.
    pub fn placed_carrier_binding_active_for_identity(
        &self,
        agent_identity: &AgentIdentity,
    ) -> bool {
        let Some(host) = self.member_placement.get(agent_identity) else {
            return false;
        };
        self.host_bind_phase.get(host) == Some(&HostBindPhase::Bound)
            && self
                .current_placed_spawn_host_binding_generations
                .get(agent_identity)
                .is_some_and(|generation| {
                    *generation > 0 && self.host_binding_generations.get(host) == Some(generation)
                })
    }

    /// Machine-owned readiness projection for one placed member.
    ///
    /// Peer-only runtimes intentionally have no controlling-local
    /// `member_startup_ready` fact. Their readiness is the exact committed
    /// placement/incarnation plus settled materialization and live-runtime
    /// facts that also authorize ordinary remote work.
    pub fn placed_member_ready_for_identity(&self, agent_identity: &AgentIdentity) -> bool {
        self.member_placement.contains_key(agent_identity)
            && self.placed_carrier_binding_active_for_identity(agent_identity)
            && self.current_placed_spawn_ids.contains_key(agent_identity)
            && self
                .current_placed_spawn_provision_operation_ids
                .contains_key(agent_identity)
            && self
                .current_placed_spawn_operation_owner_session_ids
                .contains_key(agent_identity)
            && self.member_session_bindings.contains_key(agent_identity)
            && self.member_peer_endpoints.contains_key(agent_identity)
            && self
                .identity_runtime_generations
                .contains_key(agent_identity)
            && self
                .identity_runtime_fence_tokens
                .contains_key(agent_identity)
            && !self.spawn_exec_phase.contains_key(agent_identity)
            && !self.member_revival_pending.contains(agent_identity)
            && !self
                .member_materialization_failures
                .contains_key(agent_identity)
            && self.member_lifecycle_for_identity(agent_identity).status
                == MobMemberLifecycleStatus::Active
    }

    /// Return whether the exact current peer-only runtime has durably
    /// acknowledged retirement but still awaits supervisor revocation and
    /// terminal member archival.
    pub fn remote_runtime_retired_exact(
        &self,
        agent_identity: &AgentIdentity,
        agent_runtime_id: &AgentRuntimeId,
        fence_token: FenceToken,
        generation: Generation,
    ) -> bool {
        self.identity_to_runtime.get(agent_identity) == Some(agent_runtime_id)
            && self.identity_runtime_fence_tokens.get(agent_identity) == Some(&fence_token)
            && self.identity_runtime_generations.get(agent_identity) == Some(&generation)
            && self.remote_runtime_retired_ids.contains(agent_runtime_id)
    }

    /// Return whether the exact current peer-only runtime has durably
    /// acknowledged supervisor revocation. This checkpoint is valid only
    /// beneath the matching remote-runtime-retired anchor.
    pub fn remote_supervisor_revoked_exact(
        &self,
        agent_identity: &AgentIdentity,
        agent_runtime_id: &AgentRuntimeId,
        fence_token: FenceToken,
        generation: Generation,
    ) -> bool {
        self.remote_runtime_retired_exact(agent_identity, agent_runtime_id, fence_token, generation)
            && self
                .remote_supervisor_revoked_ids
                .contains(agent_runtime_id)
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
        } else if self
            .identity_runtime_generations
            .contains_key(agent_identity)
        {
            // The current runtime binding is deliberately removed only after
            // exact retirement finalization. Incarnation high-water history
            // remains so explicit status/wait projections can report the
            // completed generation and a later spawn can mint its successor.
            MobMemberLifecycleStatus::Completed
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
        // Kickoff phase sets/maps are `AgentIdentity`-keyed; build the typed key
        // once for all lookups.
        let identity = AgentIdentity::from(member_id);
        let mut phase = None;
        for (contains, candidate) in [
            (
                self.member_kickoff_pending.contains(&identity),
                KickoffPhase::Pending,
            ),
            (
                self.member_kickoff_starting.contains(&identity),
                KickoffPhase::Starting,
            ),
            (
                self.member_kickoff_callback_pending.contains(&identity),
                KickoffPhase::CallbackPending,
            ),
            (
                self.member_kickoff_started.contains(&identity),
                KickoffPhase::Started,
            ),
            (
                self.member_kickoff_failed.contains(&identity),
                KickoffPhase::Failed,
            ),
            (
                self.member_kickoff_cancelled.contains(&identity),
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
            KickoffPhase::Failed => Some(self.member_kickoff_error.get(&identity)?.clone()),
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

    fn begin_completion_lifecycle_quiesce(
        authority: &mut MobMachineAuthority,
        intent: PlacedCompletionLifecycleIntentKind,
    ) {
        MobMachineMutator::apply(
            authority,
            MobMachineInput::BeginPlacedCompletionLifecycleQuiesce { intent },
        )
        .expect("completion lifecycle must enter exact quiesce intent before terminalization");
    }

    fn quota_obligation(identity_suffix: usize, sequence: u64) -> RemoteTurnObligation {
        RemoteTurnObligation {
            agent_identity: AgentIdentity(format!("member-{identity_suffix}")),
            host_id: HostId("host-b".to_string()),
            host_binding_generation: 1,
            member_session_id: SessionId(format!("session-{identity_suffix}")),
            generation: Generation(0),
            fence_token: FenceToken(1),
            dispatch_sequence: sequence,
            input_id: InputId(format!("input-{identity_suffix}-{sequence}")),
            run_id: RunId(format!("run-{identity_suffix}-{sequence}")),
            step_id: StepId(format!("step-{identity_suffix}-{sequence}")),
        }
    }

    #[test]
    fn remote_turn_custody_quota_accepts_256_and_rejects_257_per_member_host() {
        let pending = (1..=256)
            .map(|sequence| quota_obligation(0, sequence))
            .collect::<std::collections::BTreeSet<_>>();
        let empty = std::collections::BTreeSet::new();
        assert_eq!(pending.len(), 256);
        assert!(
            !MobMachineAuthority::mob_machine_remote_turn_custody_admits(
                &pending,
                &empty,
                &empty,
                &quota_obligation(0, 257),
            ),
            "the 257th obligation for one member+host must be rejected"
        );
        let first_256 = pending.iter().take(255).cloned().collect();
        assert!(
            MobMachineAuthority::mob_machine_remote_turn_custody_admits(
                &first_256,
                &empty,
                &empty,
                &quota_obligation(0, 256),
            ),
            "the 256th obligation for one member+host remains admissible"
        );
    }

    #[test]
    fn remote_turn_custody_global_quota_rejects_row_4097_across_members() {
        let pending = (0..4096)
            .map(|member| quota_obligation(member, member as u64 + 1))
            .collect::<std::collections::BTreeSet<_>>();
        let empty = std::collections::BTreeSet::new();
        assert_eq!(pending.len(), 4096);
        assert!(
            !MobMachineAuthority::mob_machine_remote_turn_custody_admits(
                &pending,
                &empty,
                &empty,
                &quota_obligation(4096, 4097),
            ),
            "the global 4097th custody row must be rejected"
        );
    }

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

    fn seed_membership_committed_member(
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
                resolved_spec_digest: None,
            },
        )
        .expect("AuthorizeSpawnProfile should seed live member authority");
        MobMachineMutator::apply(
            authority,
            MobMachineInput::BeginSpawnExec {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_id.clone(),
                fence_token: FenceToken(7),
                generation: Generation(0),
                profile_material_digest: profile_material_digest.clone(),
                external_addressable: true,
                runtime_mode: SpawnPolicyRuntimeMode::AutonomousHost,
                bridge_session_id: Some(bridge_session_id.clone()),
                replacing: None,
                placement: None,
                workgraph_required: false,
                rust_bundles_present: false,
                per_spawn_external_tools_present: false,
                mob_default_external_tools_present: false,
                default_llm_client_override_present: false,
                host_surface_mcp_allowlist_present: false,
                inherited_tool_filter_present: false,
                shell_env_present: false,
                mcp_stdio_env_present: false,
                mcp_http_headers_present: false,
                memory_required: false,
                mcp_required: false,
                resume_session_id: None,
                placed_spawn_id: None,
                placed_provision_operation_id: None,
                placed_operation_owner_session_id: None,
                effective_profile_override_present: false,
                effective_model_override_present: false,
            },
        )
        .expect("BeginSpawnExec should open the spawn-exec phase");
        MobMachineMutator::apply(
            authority,
            MobMachineInput::CommitSpawnMembership {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_id.clone(),
                fence_token: FenceToken(7),
                generation: Generation(0),
                profile_material_digest,
                external_addressable: true,
                runtime_mode: SpawnPolicyRuntimeMode::AutonomousHost,
                bridge_session_id: Some(bridge_session_id.clone()),
                replacing: None,
                member_peer_endpoint: None,
                spec_digest_echo: None,
                ack_engine_version: None,
                placed_spawn_id: None,
                provision_operation_id: None,
            },
        )
        .expect("CommitSpawnMembership should seed a live member through machine authority");
        bridge_session_id
    }

    fn seed_live_member(
        authority: &mut MobMachineAuthority,
        identity: &AgentIdentity,
        runtime_id: &AgentRuntimeId,
    ) -> SessionId {
        let bridge_session_id = seed_membership_committed_member(authority, identity, runtime_id);
        MobMachineMutator::apply(
            authority,
            MobMachineInput::CommitSpawnActivation {
                agent_identity: identity.clone(),
            },
        )
        .expect("CommitSpawnActivation should settle the seeded live member");
        bridge_session_id
    }

    #[test]
    fn no_binding_retire_cannot_erase_pending_session_retirement() {
        for stopped in [false, true] {
            let mut authority = MobMachineAuthority::new();
            let identity = AgentIdentity::from(if stopped {
                "stopped-releasing-member"
            } else {
                "running-releasing-member"
            });
            let runtime_id = AgentRuntimeId::from(format!("{}:0", identity.0));
            let session_id = seed_live_member(&mut authority, &identity, &runtime_id);
            let mob_id = MobId::from("pending-retirement-correlation");
            if stopped {
                begin_completion_lifecycle_quiesce(
                    &mut authority,
                    PlacedCompletionLifecycleIntentKind::Stop,
                );
                MobMachineMutator::apply(&mut authority, MobMachineInput::Stop)
                    .expect("stop seeded authority");
            }

            MobMachineMutator::apply(
                &mut authority,
                MobMachineInput::Retire {
                    mob_id: mob_id.clone(),
                    agent_runtime_id: runtime_id.clone(),
                    agent_identity: identity.clone(),
                    generation: Generation(0),
                    releasing: Some(session_id.clone()),
                    session_id: Some(session_id.clone()),
                },
            )
            .expect("open releasing retirement");
            MobMachineMutator::apply(
                &mut authority,
                MobMachineInput::SessionIngressDetachedForMobDestroy {
                    mob_id: mob_id.clone(),
                    agent_runtime_id: runtime_id.clone(),
                },
            )
            .expect("close retirement ingress detach");

            assert_eq!(
                authority
                    .state()
                    .runtime_retire_pending_sessions
                    .get(&runtime_id),
                Some(&session_id)
            );
            assert!(
                MobMachineMutator::apply(
                    &mut authority,
                    MobMachineInput::Retire {
                        mob_id,
                        agent_runtime_id: runtime_id.clone(),
                        agent_identity: identity,
                        generation: Generation(0),
                        releasing: None,
                        session_id: None,
                    },
                )
                .is_err(),
                "a no-binding retry must not reclassify a session retirement"
            );
            assert_eq!(
                authority
                    .state()
                    .runtime_retire_pending_sessions
                    .get(&runtime_id),
                Some(&session_id),
                "rejected retry must retain exact session correlation"
            );
        }
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
        let privileged_setters: [(&str, fn(&mut MobMachineInput)); 8] = [
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
                resolved_spec_digest: None,
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
                    resolved_spec_digest: None,
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

    fn test_member_peer_endpoint(name: &str, signing_key: [u8; 32]) -> MemberPeerEndpoint {
        MemberPeerEndpoint {
            name: PeerName(name.to_string()),
            peer_id: PeerId(
                meerkat_core::comms::PeerId::from_ed25519_pubkey(&signing_key).to_string(),
            ),
            address: PeerAddress(format!("inproc://{name}")),
            signing_key: PeerSigningKey(signing_key),
        }
    }

    fn register_test_member_peer(
        authority: &mut MobMachineAuthority,
        identity: &AgentIdentity,
        name: &str,
        signing_key: [u8; 32],
    ) {
        let agent_runtime_id = authority
            .state()
            .identity_to_runtime
            .get(identity)
            .expect("test member current runtime")
            .clone();
        let generation = *authority
            .state()
            .identity_runtime_generations
            .get(identity)
            .expect("test member generation");
        let fence_token = *authority
            .state()
            .identity_runtime_fence_tokens
            .get(identity)
            .expect("test member fence");
        MobMachineMutator::apply(
            authority,
            MobMachineInput::RegisterMemberPeer {
                agent_identity: identity.clone(),
                agent_runtime_id,
                generation,
                fence_token,
                peer_endpoint: test_member_peer_endpoint(name, signing_key),
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
    fn member_trust_handoff_rejects_recovered_peer_rotation_after_epoch_advance() {
        let mut authority = MobMachineAuthority::new();
        let a = AgentIdentity::from("member-a");
        let b = AgentIdentity::from("member-b");
        let b_runtime_id = AgentRuntimeId::from("member-b:1");

        seed_live_member(&mut authority, &a, &AgentRuntimeId::from("member-a:1"));
        let b_session_id = seed_live_member(&mut authority, &b, &b_runtime_id);
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
        let bound_endpoint = authority
            .state()
            .member_peer_endpoints
            .get(&b)
            .cloned()
            .expect("member-b endpoint should be registered");
        let rebound_endpoint = test_member_peer_endpoint("member-b-rebound", [9; 32]);
        let b_runtime_id = authority
            .state()
            .identity_to_runtime
            .get(&b)
            .cloned()
            .expect("member-b current runtime");
        let b_generation = *authority
            .state()
            .identity_runtime_generations
            .get(&b)
            .expect("member-b current generation");
        let b_fence_token = *authority
            .state()
            .identity_runtime_fence_tokens
            .get(&b)
            .expect("member-b current fence");

        let unjournaled_rewrite = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RegisterMemberPeer {
                agent_identity: b.clone(),
                agent_runtime_id: b_runtime_id.clone(),
                generation: b_generation,
                fence_token: b_fence_token,
                peer_endpoint: rebound_endpoint.clone(),
            },
        );
        assert!(
            unjournaled_rewrite.is_err(),
            "RegisterMemberPeer must not rewrite an existing generation endpoint"
        );
        assert_eq!(
            authority.state().topology_epoch,
            bound_epoch,
            "rejected unjournaled peer rewrite must not advance topology"
        );
        assert_eq!(
            authority.state().member_peer_endpoints.get(&b),
            Some(&bound_endpoint),
            "rejected unjournaled peer rewrite must not replace the exact endpoint"
        );

        authority
            .apply_signal(MobMachineSignal::RecoverMemberPeerEndpoint {
                agent_identity: b.clone(),
                agent_runtime_id: b_runtime_id,
                bridge_session_id: b_session_id,
                peer_endpoint: rebound_endpoint,
            })
            .expect("durable endpoint recovery should rotate through generated authority");
        assert_eq!(
            authority.state().topology_epoch,
            bound_epoch + 1,
            "generated endpoint recovery must invalidate prior trust authority"
        );

        let error = crate::generated::protocol_mob_member_trust_wiring::wiring_authority_for_identity_with_live_authority(
            &obligation,
            "member-b",
            &expected_peer_id,
            &authority,
        )
        .expect_err("pre-rotation member trust obligation must be stale after recovery");
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
        MobMachineMutator::apply(
            &mut prepared,
            MobMachineInput::BeginPlacedCompletionLifecycleQuiesce {
                intent: PlacedCompletionLifecycleIntentKind::Stop,
            },
        )
        .expect("prepared authority should enter Stop quiesce");
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
        MobMachineMutator::apply(
            &mut prepared,
            MobMachineInput::BeginPlacedCompletionLifecycleQuiesce {
                intent: PlacedCompletionLifecycleIntentKind::Stop,
            },
        )
        .expect("prepared authority should enter Stop quiesce");
        MobMachineMutator::apply(&mut prepared, MobMachineInput::Stop)
            .expect("prepared authority should accept Stop");
        begin_completion_lifecycle_quiesce(
            &mut authority,
            PlacedCompletionLifecycleIntentKind::Complete,
        );
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
    fn prepared_authority_batch_applies_sequential_guards_and_commits_atomically() {
        let mut authority = MobMachineAuthority::new();
        let a = AgentIdentity::from("batch-member-a");
        let b = AgentIdentity::from("batch-member-b");
        let c = AgentIdentity::from("batch-member-c");
        seed_live_member(
            &mut authority,
            &a,
            &AgentRuntimeId::from("batch-member-a:1"),
        );
        seed_live_member(
            &mut authority,
            &b,
            &AgentRuntimeId::from("batch-member-b:1"),
        );
        seed_live_member(
            &mut authority,
            &c,
            &AgentRuntimeId::from("batch-member-c:1"),
        );
        register_test_member_peer(&mut authority, &a, "batch-member-a", [11; 32]);
        register_test_member_peer(&mut authority, &b, "batch-member-b", [12; 32]);
        register_test_member_peer(&mut authority, &c, "batch-member-c", [13; 32]);

        let first = WiringEdge::new(a.clone(), b.clone());
        let second = WiringEdge::new(b.clone(), c.clone());
        let mut prepared = authority.prepare_authority();
        let transitions = prepared
            .apply_batch([
                MobMachineInput::WireMembersWithTrust {
                    edge: first.clone(),
                    a_identity: first.a.clone(),
                    b_identity: first.b.clone(),
                },
                MobMachineInput::WireMembersWithTrust {
                    edge: second.clone(),
                    a_identity: second.a.clone(),
                    b_identity: second.b.clone(),
                },
            ])
            .expect("prepared batch should apply both guarded inputs");

        assert_eq!(transitions.len(), 2);
        assert!(prepared.state().wiring_edges.contains(&first));
        assert!(prepared.state().wiring_edges.contains(&second));
        assert!(authority.state().wiring_edges.is_empty());

        authority
            .commit_prepared_authority(prepared)
            .expect("prepared batch should commit against unchanged live base");
        assert!(authority.state().wiring_edges.contains(&first));
        assert!(authority.state().wiring_edges.contains(&second));
    }

    #[test]
    fn prepared_authority_batch_rolls_back_every_input_on_guard_rejection() {
        let mut authority = MobMachineAuthority::new();
        let a = AgentIdentity::from("rollback-member-a");
        let b = AgentIdentity::from("rollback-member-b");
        let c = AgentIdentity::from("rollback-member-c");
        seed_live_member(
            &mut authority,
            &a,
            &AgentRuntimeId::from("rollback-member-a:1"),
        );
        seed_live_member(
            &mut authority,
            &b,
            &AgentRuntimeId::from("rollback-member-b:1"),
        );
        seed_live_member(
            &mut authority,
            &c,
            &AgentRuntimeId::from("rollback-member-c:1"),
        );
        register_test_member_peer(&mut authority, &a, "rollback-member-a", [21; 32]);
        register_test_member_peer(&mut authority, &b, "rollback-member-b", [22; 32]);
        register_test_member_peer(&mut authority, &c, "rollback-member-c", [23; 32]);

        let valid = WiringEdge::new(a.clone(), b.clone());
        let rejected = WiringEdge::new(b.clone(), c.clone());
        let mut prepared = authority.prepare_authority();
        let before = prepared.state().clone();
        prepared
            .apply_batch([
                MobMachineInput::WireMembersWithTrust {
                    edge: valid.clone(),
                    a_identity: valid.a.clone(),
                    b_identity: valid.b.clone(),
                },
                MobMachineInput::WireMembersWithTrust {
                    edge: rejected,
                    a_identity: a,
                    b_identity: c,
                },
            ])
            .expect_err("a rejected later input must abort the whole prepared batch");

        assert_eq!(prepared.state(), &before);
        assert!(authority.state().wiring_edges.is_empty());
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

        begin_completion_lifecycle_quiesce(
            &mut authority,
            PlacedCompletionLifecycleIntentKind::Stop,
        );
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
                generation: Generation(0),
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
                generation: Generation(0),
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
                session_id: Some(session_id.clone()),
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
                generation: Generation(0),
                session_id: Some(session_id),
                disposal: MemberSessionDisposal::Archived,
                preserve_machine_topology: false,
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
        assert!(
            !authority
                .state()
                .identity_to_runtime
                .contains_key(&identity),
            "exact retirement final must remove the current runtime binding"
        );
        assert_eq!(
            authority
                .state()
                .identity_runtime_generations
                .get(&identity),
            Some(&Generation(0)),
            "retirement final must retain generation high-water history"
        );
        assert_eq!(
            authority
                .state()
                .identity_runtime_fence_tokens
                .get(&identity),
            Some(&fence_token),
            "retirement final must retain fence history"
        );
    }

    #[test]
    fn peer_only_retirement_final_settles_membership_before_respawn() {
        let mut authority = MobMachineAuthority::new();
        let identity = AgentIdentity::from("peer-only-respawn");
        let runtime_v0 = AgentRuntimeId::from("peer-only-respawn:0");
        let runtime_v1 = AgentRuntimeId::from("peer-only-respawn:1");
        let profile_digest = "peer-only-profile-digest".to_string();

        let authorize = |authority: &mut MobMachineAuthority, digest: String| {
            MobMachineMutator::apply(
                authority,
                MobMachineInput::AuthorizeSpawnProfile {
                    agent_identity: identity.clone(),
                    profile_name: "peer-only".to_string(),
                    model: "test-model".to_string(),
                    profile_material_digest: digest,
                    tool_config_digest: "test-tool-config-digest".to_string(),
                    skills_digest: "test-skills-digest".to_string(),
                    provider_params_digest: None,
                    output_schema_digest: None,
                    external_addressable: true,
                    resolved_spec_digest: None,
                },
            )
            .expect("authorize peer-only spawn profile");
        };
        let begin = |authority: &mut MobMachineAuthority,
                     runtime_id: AgentRuntimeId,
                     fence_token: FenceToken,
                     generation: Generation,
                     digest: String| {
            MobMachineMutator::apply(
                authority,
                MobMachineInput::BeginSpawnExec {
                    agent_identity: identity.clone(),
                    agent_runtime_id: runtime_id,
                    fence_token,
                    generation,
                    profile_material_digest: digest,
                    external_addressable: true,
                    runtime_mode: SpawnPolicyRuntimeMode::AutonomousHost,
                    bridge_session_id: None,
                    replacing: None,
                    placement: None,
                    workgraph_required: false,
                    rust_bundles_present: false,
                    per_spawn_external_tools_present: false,
                    mob_default_external_tools_present: false,
                    default_llm_client_override_present: false,
                    host_surface_mcp_allowlist_present: false,
                    inherited_tool_filter_present: false,
                    shell_env_present: false,
                    mcp_stdio_env_present: false,
                    mcp_http_headers_present: false,
                    memory_required: false,
                    mcp_required: false,
                    resume_session_id: None,
                    placed_spawn_id: None,
                    placed_provision_operation_id: None,
                    placed_operation_owner_session_id: None,
                    effective_profile_override_present: false,
                    effective_model_override_present: false,
                },
            )
            .expect("begin peer-only spawn execution");
        };

        authorize(&mut authority, profile_digest.clone());
        begin(
            &mut authority,
            runtime_v0.clone(),
            FenceToken(7),
            Generation(0),
            profile_digest.clone(),
        );
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::CommitSpawnMembership {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_v0.clone(),
                fence_token: FenceToken(7),
                generation: Generation(0),
                profile_material_digest: profile_digest,
                external_addressable: true,
                runtime_mode: SpawnPolicyRuntimeMode::AutonomousHost,
                bridge_session_id: None,
                replacing: None,
                member_peer_endpoint: None,
                spec_digest_echo: None,
                ack_engine_version: None,
                placed_spawn_id: None,
                provision_operation_id: None,
            },
        )
        .expect("commit peer-only membership without an activation rung");
        assert_eq!(
            authority.state().spawn_exec_phase.get(&identity),
            Some(&SpawnExecPhase::MembershipCommitted)
        );

        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::Retire {
                mob_id: MobId::from("peer-only-respawn-mob"),
                agent_runtime_id: runtime_v0.clone(),
                agent_identity: identity.clone(),
                generation: Generation(0),
                releasing: None,
                session_id: None,
            },
        )
        .expect("admit peer-only retirement");
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RecordRemoteMemberRuntimeRetired {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_v0.clone(),
                fence_token: FenceToken(7),
                generation: Generation(0),
            },
        )
        .expect("record peer-only runtime retirement");
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RecordRemoteMemberSupervisorRevoked {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_v0.clone(),
                fence_token: FenceToken(7),
                generation: Generation(0),
            },
        )
        .expect("record peer-only supervisor revocation");
        authority
            .apply_signal(
                MobMachineSignal::ObserveRemoteMemberRetirementArchivedAndSupervisorRevoked {
                    agent_identity: identity.clone(),
                    agent_runtime_id: runtime_v0,
                    fence_token: FenceToken(7),
                    generation: Generation(0),
                    preserve_machine_topology: false,
                },
            )
            .expect("publish peer-only retirement terminal");

        assert!(
            !authority
                .state()
                .identity_to_runtime
                .contains_key(&identity)
                && !authority.state().spawn_exec_phase.contains_key(&identity),
            "peer-only retirement terminal must settle current membership and spawn execution"
        );
        assert_eq!(
            authority
                .state()
                .identity_runtime_generations
                .get(&identity),
            Some(&Generation(0)),
            "retirement must retain the generation high-water"
        );

        let replacement_digest = "peer-only-replacement-digest".to_string();
        authorize(&mut authority, replacement_digest.clone());
        begin(
            &mut authority,
            runtime_v1,
            FenceToken(8),
            Generation(1),
            replacement_digest,
        );
        assert_eq!(
            authority.state().spawn_exec_phase.get(&identity),
            Some(&SpawnExecPhase::Opened),
            "the exact next peer-only generation must open after retirement settles"
        );
    }

    #[test]
    fn non_live_retirement_finals_reject_stale_fence_in_every_phase_and_family() {
        for (destroy, stopped) in [(false, false), (false, true), (true, false), (true, true)] {
            let label = format!(
                "{}-{}",
                if destroy { "destroy" } else { "ordinary" },
                if stopped { "stopped" } else { "running" }
            );
            let mut authority = MobMachineAuthority::new();
            let identity = AgentIdentity::from(format!("stale-fence-{label}"));
            let runtime_id = AgentRuntimeId::from(format!("stale-fence-{label}:0"));
            let session_id = seed_live_member(&mut authority, &identity, &runtime_id);

            if destroy {
                if stopped {
                    begin_completion_lifecycle_quiesce(
                        &mut authority,
                        PlacedCompletionLifecycleIntentKind::Stop,
                    );
                    MobMachineMutator::apply(&mut authority, MobMachineInput::Stop)
                        .expect("move destroy-family retirement to stopped");
                }
                begin_completion_lifecycle_quiesce(
                    &mut authority,
                    PlacedCompletionLifecycleIntentKind::Destroy,
                );
                authority
                    .apply_signal(MobMachineSignal::AdmitDestroyCleanup)
                    .expect("admit destroy cleanup");
                authority
                    .apply_signal(MobMachineSignal::AdmitDestroyMemberRetire {
                        mob_id: MobId::from("test-mob"),
                        agent_identity: identity.clone(),
                        agent_runtime_id: runtime_id.clone(),
                        fence_token: FenceToken(7),
                        generation: Generation(0),
                        session_id: Some(session_id.clone()),
                    })
                    .expect("admit destroy member retirement");
                MobMachineMutator::apply(
                    &mut authority,
                    MobMachineInput::SessionIngressDetachedForMobDestroy {
                        mob_id: MobId::from("test-mob"),
                        agent_runtime_id: runtime_id.clone(),
                    },
                )
                .expect("close destroy session-ingress detach");
            } else {
                authority
                    .apply_signal(MobMachineSignal::RetireMember {
                        agent_identity: identity.clone(),
                        agent_runtime_id: runtime_id.clone(),
                        fence_token: FenceToken(7),
                        session_id: Some(session_id.clone()),
                    })
                    .expect("admit ordinary member retirement");
            }
            authority
                .apply_signal(MobMachineSignal::ObserveRuntimeRetired {
                    agent_runtime_id: runtime_id.clone(),
                    fence_token: FenceToken(7),
                })
                .expect("observe runtime retirement");
            if stopped && !destroy {
                begin_completion_lifecycle_quiesce(
                    &mut authority,
                    PlacedCompletionLifecycleIntentKind::Stop,
                );
                MobMachineMutator::apply(&mut authority, MobMachineInput::Stop)
                    .expect("move non-live retirement to stopped");
            }

            let final_signal = |fence_token| {
                if destroy {
                    MobMachineSignal::ObserveDestroyMemberRetirementArchived {
                        agent_identity: identity.clone(),
                        agent_runtime_id: runtime_id.clone(),
                        fence_token,
                        generation: Generation(0),
                        session_id: Some(session_id.clone()),
                        disposal: MemberSessionDisposal::Archived,
                    }
                } else {
                    MobMachineSignal::ObserveMemberRetirementArchived {
                        agent_identity: identity.clone(),
                        agent_runtime_id: runtime_id.clone(),
                        fence_token,
                        generation: Generation(0),
                        session_id: Some(session_id.clone()),
                        disposal: MemberSessionDisposal::Archived,
                        preserve_machine_topology: false,
                    }
                }
            };

            assert!(
                authority
                    .apply_signal(final_signal(FenceToken(99)))
                    .is_err(),
                "{label}: a non-live exact final must reject a stale fence"
            );
            assert_eq!(
                authority.state().identity_to_runtime.get(&identity),
                Some(&runtime_id),
                "{label}: rejected final must retain the current binding"
            );
            assert_eq!(
                authority.state().member_state_markers.get(&runtime_id),
                Some(&MobMemberState::Retiring),
                "{label}: rejected final must retain retirement progress"
            );

            authority
                .apply_signal(final_signal(FenceToken(7)))
                .unwrap_or_else(|error| panic!("{label}: exact fence must finalize: {error}"));
            assert!(
                !authority
                    .state()
                    .identity_to_runtime
                    .contains_key(&identity),
                "{label}: exact final removes only the current binding"
            );
            assert_eq!(
                authority
                    .state()
                    .identity_runtime_fence_tokens
                    .get(&identity),
                Some(&FenceToken(7)),
                "{label}: exact final retains fence high-water history"
            );
        }
    }

    #[test]
    fn recovered_reset_requires_old_history_exact_successor_and_new_fence() {
        let identity = AgentIdentity::from("reset-history");
        let runtime_v0 = AgentRuntimeId::from("reset-history:0");
        let runtime_v1 = AgentRuntimeId::from("reset-history:1");
        let mut seeded = MobMachineAuthority::new();
        seed_live_member(&mut seeded, &identity, &runtime_v0);
        MobMachineMutator::apply(
            &mut seeded,
            MobMachineInput::KickoffMarkPending {
                member_id: identity.clone(),
                objective_id: "00000000-0000-0000-0000-000000000001".into(),
            },
        )
        .expect("seed reset kickoff pending");
        MobMachineMutator::apply(
            &mut seeded,
            MobMachineInput::KickoffMarkStarting {
                member_id: identity.clone(),
            },
        )
        .expect("seed reset kickoff starting");
        MobMachineMutator::apply(
            &mut seeded,
            MobMachineInput::KickoffResolveFailed {
                member_id: identity.clone(),
                error: "old kickoff failure".to_string(),
            },
        )
        .expect("seed reset kickoff failure and error");
        let base = seeded.state().clone();

        let reset_signal = |previous_generation, generation, fence_token| {
            MobMachineSignal::RecoverRosterMemberReset {
                agent_identity: identity.clone(),
                previous_agent_runtime_id: runtime_v0.clone(),
                previous_generation,
                agent_runtime_id: runtime_v1.clone(),
                fence_token,
                generation,
            }
        };
        for (label, previous_generation, generation, fence_token) in [
            (
                "wrong retained generation",
                Generation(1),
                Generation(2),
                FenceToken(8),
            ),
            (
                "skipped successor",
                Generation(0),
                Generation(2),
                FenceToken(8),
            ),
            ("reused fence", Generation(0), Generation(1), FenceToken(7)),
        ] {
            let mut authority =
                MobMachineAuthority::recover_from_state(base.clone()).expect("recover reset base");
            assert!(
                authority
                    .apply_signal(reset_signal(previous_generation, generation, fence_token))
                    .is_err(),
                "{label} must fail closed"
            );
            assert_eq!(
                authority.state().identity_to_runtime.get(&identity),
                Some(&runtime_v0),
                "{label} must leave the old incarnation current"
            );
        }

        let mut authority =
            MobMachineAuthority::recover_from_state(base).expect("recover valid base");
        authority
            .apply_signal(reset_signal(Generation(0), Generation(1), FenceToken(8)))
            .expect("exact reset replay advances the incarnation");
        assert_eq!(
            authority.state().identity_to_runtime.get(&identity),
            Some(&runtime_v1)
        );
        assert_eq!(
            authority
                .state()
                .identity_runtime_generations
                .get(&identity),
            Some(&Generation(1))
        );
        assert_eq!(
            authority
                .state()
                .identity_runtime_fence_tokens
                .get(&identity),
            Some(&FenceToken(8))
        );
        assert!(
            !authority.state().member_kickoff_pending.contains(&identity)
                && !authority
                    .state()
                    .member_kickoff_starting
                    .contains(&identity)
                && !authority
                    .state()
                    .member_kickoff_callback_pending
                    .contains(&identity)
                && !authority.state().member_kickoff_started.contains(&identity)
                && !authority.state().member_kickoff_failed.contains(&identity)
                && !authority
                    .state()
                    .member_kickoff_cancelled
                    .contains(&identity)
                && !authority
                    .state()
                    .member_kickoff_error
                    .contains_key(&identity),
            "valid reset must clear every prior kickoff fact"
        );
    }

    #[test]
    fn retired_history_requires_next_generation_and_cannot_accept_late_peer_registration() {
        let fresh_identity = AgentIdentity::from("fresh-generation");
        let mut fresh_authority = MobMachineAuthority::new();
        let fresh_generation = MobMachineMutator::apply(
            &mut fresh_authority,
            MobMachineInput::ComputeRespawnGeneration {
                agent_identity: fresh_identity,
            },
        )
        .expect("compute a fresh identity generation");
        assert!(fresh_generation.effects().iter().any(|effect| {
            matches!(
                effect,
                MobMachineEffect::RespawnGenerationComputed {
                    next_generation: Generation(0),
                    ..
                }
            )
        }));

        let mut pre_activation = MobMachineAuthority::new();
        let pre_activation_identity = AgentIdentity::from("retired-before-activation");
        let pre_activation_runtime = AgentRuntimeId::from("retired-before-activation:0");
        seed_membership_committed_member(
            &mut pre_activation,
            &pre_activation_identity,
            &pre_activation_runtime,
        );
        assert_eq!(
            pre_activation
                .state()
                .spawn_exec_phase
                .get(&pre_activation_identity),
            Some(&SpawnExecPhase::MembershipCommitted)
        );
        pre_activation
            .apply_signal(MobMachineSignal::RecoverRosterMemberRetired {
                agent_identity: pre_activation_identity.clone(),
                agent_runtime_id: pre_activation_runtime,
                generation: Generation(0),
                preserve_machine_topology: false,
                preservation_started: false,
            })
            .expect("pre-activation retirement final must settle spawn execution");
        assert!(
            !pre_activation
                .state()
                .identity_to_runtime
                .contains_key(&pre_activation_identity)
                && !pre_activation
                    .state()
                    .spawn_exec_phase
                    .contains_key(&pre_activation_identity),
            "finalization must remove both current membership and a pre-activation spawn rung"
        );

        let mut pre_activation_absent = MobMachineAuthority::new();
        let absent_identity = AgentIdentity::from("retired-before-activation-absent");
        let absent_runtime = AgentRuntimeId::from("retired-before-activation-absent:0");
        seed_membership_committed_member(
            &mut pre_activation_absent,
            &absent_identity,
            &absent_runtime,
        );
        let mut absent_state = pre_activation_absent.state().clone();
        absent_state.live_runtime_ids.remove(&absent_runtime);
        let mut pre_activation_absent = MobMachineAuthority::recover_from_state(absent_state)
            .expect("recover current membership after its runtime is already absent");
        pre_activation_absent
            .apply_signal(MobMachineSignal::RecoverRosterMemberRetired {
                agent_identity: absent_identity.clone(),
                agent_runtime_id: absent_runtime,
                generation: Generation(0),
                preserve_machine_topology: false,
                preservation_started: false,
            })
            .expect("absent-runtime retirement final must settle spawn execution");
        assert!(
            !pre_activation_absent
                .state()
                .identity_to_runtime
                .contains_key(&absent_identity)
                && !pre_activation_absent
                    .state()
                    .spawn_exec_phase
                    .contains_key(&absent_identity),
            "current absent-runtime finalization must settle a committed spawn rung"
        );

        let mut authority = MobMachineAuthority::new();
        let identity = AgentIdentity::from("retired-reuse");
        let runtime_v0 = AgentRuntimeId::from("retired-reuse:0");
        let runtime_v1 = AgentRuntimeId::from("retired-reuse:1");
        seed_live_member(&mut authority, &identity, &runtime_v0);
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::KickoffMarkPending {
                member_id: identity.clone(),
                objective_id: "00000000-0000-0000-0000-000000000001".into(),
            },
        )
        .expect("seed old-generation kickoff");
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::KickoffCancelRequested {
                member_id: identity.clone(),
            },
        )
        .expect("terminalize old-generation kickoff");
        authority
            .apply_signal(MobMachineSignal::RecoverRosterMemberRetired {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_v0.clone(),
                generation: Generation(0),
                preserve_machine_topology: false,
                preservation_started: false,
            })
            .expect("recover exact generation-zero final");

        assert!(
            !authority
                .state()
                .identity_to_runtime
                .contains_key(&identity)
        );
        assert_eq!(
            authority
                .state()
                .identity_runtime_generations
                .get(&identity),
            Some(&Generation(0))
        );
        assert!(
            authority
                .state()
                .member_kickoff_cancelled
                .contains(&identity)
        );
        authority
            .apply_signal(MobMachineSignal::RecoverRosterMemberRetired {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_v0.clone(),
                generation: Generation(0),
                preserve_machine_topology: false,
                preservation_started: false,
            })
            .expect("duplicate retirement final should converge idempotently");
        assert!(
            authority
                .state()
                .member_kickoff_cancelled
                .contains(&identity),
            "duplicate retirement final must retain old-generation kickoff history"
        );
        let successor_generation = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ComputeRespawnGeneration {
                agent_identity: identity.clone(),
            },
        )
        .expect("compute a retired identity successor generation");
        assert!(successor_generation.effects().iter().any(|effect| {
            matches!(
                effect,
                MobMachineEffect::RespawnGenerationComputed {
                    next_generation: Generation(1),
                    ..
                }
            )
        }));

        let endpoint = MemberPeerEndpoint {
            name: PeerName("late-retired-peer".to_string()),
            peer_id: PeerId(meerkat_core::comms::PeerId::from_ed25519_pubkey(&[9; 32]).to_string()),
            address: PeerAddress("inproc://late-retired-peer".to_string()),
            signing_key: PeerSigningKey([9; 32]),
        };
        assert!(
            MobMachineMutator::apply(
                &mut authority,
                MobMachineInput::RegisterMemberPeer {
                    agent_identity: identity.clone(),
                    agent_runtime_id: runtime_v0.clone(),
                    generation: Generation(0),
                    fence_token: FenceToken(7),
                    peer_endpoint: endpoint.clone(),
                },
            )
            .is_err(),
            "a delayed peer registration must not recreate routing for retired history"
        );

        let retired_state = authority.state().clone();
        let mut spawn_authority = MobMachineAuthority::recover_from_state(retired_state.clone())
            .expect("recover history");
        MobMachineMutator::apply(
            &mut spawn_authority,
            MobMachineInput::AuthorizeSpawnProfile {
                agent_identity: identity.clone(),
                profile_name: "worker".to_string(),
                model: "test-model".to_string(),
                profile_material_digest: "reuse-digest".to_string(),
                tool_config_digest: "tool-digest".to_string(),
                skills_digest: "skills-digest".to_string(),
                provider_params_digest: None,
                output_schema_digest: None,
                external_addressable: true,
                resolved_spec_digest: None,
            },
        )
        .expect("authorize replacement profile");
        let replacement_session = SessionId::from("replacement-session");
        let begin = |generation| MobMachineInput::BeginSpawnExec {
            agent_identity: identity.clone(),
            agent_runtime_id: runtime_v1.clone(),
            fence_token: FenceToken(8),
            generation,
            profile_material_digest: "reuse-digest".to_string(),
            external_addressable: true,
            runtime_mode: SpawnPolicyRuntimeMode::AutonomousHost,
            bridge_session_id: Some(replacement_session.clone()),
            replacing: None,
            placement: None,
            workgraph_required: false,
            rust_bundles_present: false,
            per_spawn_external_tools_present: false,
            mob_default_external_tools_present: false,
            default_llm_client_override_present: false,
            host_surface_mcp_allowlist_present: false,
            inherited_tool_filter_present: false,
            shell_env_present: false,
            mcp_stdio_env_present: false,
            mcp_http_headers_present: false,
            memory_required: false,
            mcp_required: false,
            resume_session_id: None,
            placed_spawn_id: None,
            placed_provision_operation_id: None,
            placed_operation_owner_session_id: None,
            effective_profile_override_present: false,
            effective_model_override_present: false,
        };
        assert!(
            MobMachineMutator::apply(&mut spawn_authority, begin(Generation(0))).is_err(),
            "equal-generation reuse must fail at the generated spawn opener"
        );
        MobMachineMutator::apply(&mut spawn_authority, begin(Generation(1)))
            .expect("strict successor should open spawn execution");
        spawn_authority
            .apply_signal(MobMachineSignal::RecoverRosterMemberRetired {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_v0.clone(),
                generation: Generation(0),
                preserve_machine_topology: false,
                preservation_started: false,
            })
            .expect("duplicate retired-generation final should remain idempotent");
        assert_eq!(
            spawn_authority.state().spawn_exec_phase.get(&identity),
            Some(&SpawnExecPhase::Opened),
            "a delayed retired-generation final must preserve the replacement spawn opener"
        );
        assert!(
            MobMachineMutator::apply(
                &mut spawn_authority,
                MobMachineInput::CommitSpawnMembership {
                    agent_identity: identity.clone(),
                    agent_runtime_id: runtime_v1.clone(),
                    fence_token: FenceToken(8),
                    generation: Generation(0),
                    profile_material_digest: "reuse-digest".to_string(),
                    external_addressable: true,
                    runtime_mode: SpawnPolicyRuntimeMode::AutonomousHost,
                    bridge_session_id: Some(replacement_session.clone()),
                    replacing: None,
                    member_peer_endpoint: None,
                    spec_digest_echo: None,
                    ack_engine_version: None,
                    placed_spawn_id: None,
                    provision_operation_id: None,
                },
            )
            .is_err(),
            "equal-generation reuse must also fail at membership commit"
        );
        MobMachineMutator::apply(
            &mut spawn_authority,
            MobMachineInput::CommitSpawnMembership {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_v1.clone(),
                fence_token: FenceToken(8),
                generation: Generation(1),
                profile_material_digest: "reuse-digest".to_string(),
                external_addressable: true,
                runtime_mode: SpawnPolicyRuntimeMode::AutonomousHost,
                bridge_session_id: Some(replacement_session),
                replacing: None,
                member_peer_endpoint: None,
                spec_digest_echo: None,
                ack_engine_version: None,
                placed_spawn_id: None,
                provision_operation_id: None,
            },
        )
        .expect("strict successor should commit membership");
        assert!(
            MobMachineMutator::apply(
                &mut spawn_authority,
                MobMachineInput::RegisterMemberPeer {
                    agent_identity: identity.clone(),
                    agent_runtime_id: runtime_v0.clone(),
                    generation: Generation(0),
                    fence_token: FenceToken(7),
                    peer_endpoint: endpoint,
                },
            )
            .is_err(),
            "generation-zero peer registration delayed until after generation one must fail"
        );
        assert!(
            !spawn_authority
                .state()
                .member_kickoff_cancelled
                .contains(&identity),
            "new membership commit must clear the prior generation kickoff terminal"
        );

        let mut replay_authority =
            MobMachineAuthority::recover_from_state(retired_state).expect("recover replay history");
        assert!(
            replay_authority
                .apply_signal(MobMachineSignal::RecoverRosterMember {
                    agent_identity: identity.clone(),
                    agent_runtime_id: runtime_v0,
                    fence_token: FenceToken(7),
                    generation: Generation(0),
                    profile_name: "worker".to_string(),
                    runtime_mode: SpawnPolicyRuntimeMode::AutonomousHost,
                    external_addressable: true,
                })
                .is_err(),
            "an equal-generation replay must not resurrect the retired incarnation"
        );
        replay_authority
            .apply_signal(MobMachineSignal::RecoverRosterMember {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_v1.clone(),
                fence_token: FenceToken(8),
                generation: Generation(1),
                profile_name: "worker".to_string(),
                runtime_mode: SpawnPolicyRuntimeMode::AutonomousHost,
                external_addressable: true,
            })
            .expect("strictly newer replay should install the current binding");
        assert_eq!(
            replay_authority.state().identity_to_runtime.get(&identity),
            Some(&runtime_v1)
        );
        assert!(
            !replay_authority
                .state()
                .member_kickoff_cancelled
                .contains(&identity),
            "new-generation replay must clear the prior kickoff terminal"
        );
    }

    #[test]
    fn exhausted_generation_history_cannot_compute_or_recover_generation_zero() {
        let identity = AgentIdentity::from("generation-exhausted");
        let runtime = AgentRuntimeId::from("generation-exhausted:max");
        let mut seeded = MobMachineAuthority::new();
        seed_live_member(&mut seeded, &identity, &runtime);
        let mut exhausted_state = seeded.state().clone();
        exhausted_state
            .identity_runtime_generations
            .insert(identity.clone(), Generation(u64::MAX));
        let mut authority = MobMachineAuthority::recover_from_state(exhausted_state)
            .expect("MAX generation is valid retained history");

        assert!(
            MobMachineMutator::apply(
                &mut authority,
                MobMachineInput::ComputeRespawnGeneration {
                    agent_identity: identity.clone(),
                },
            )
            .is_err(),
            "MAX generation has no successor"
        );
        assert!(
            authority
                .apply_signal(MobMachineSignal::RecoverRosterMemberReset {
                    agent_identity: identity,
                    previous_agent_runtime_id: runtime,
                    previous_generation: Generation(u64::MAX),
                    agent_runtime_id: AgentRuntimeId::from("generation-exhausted:0"),
                    fence_token: FenceToken(99),
                    generation: Generation(0),
                })
                .is_err(),
            "MAX generation must never wrap to generation zero during recovery"
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
                session_id: Some(session_id.clone()),
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
                generation: Generation(0),
                session_id: Some(session_id),
                disposal: MemberSessionDisposal::Archived,
                preserve_machine_topology: false,
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
        let member_id = AgentIdentity::from("worker");

        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::KickoffMarkPending {
                member_id: member_id.clone(),
                objective_id: "00000000-0000-0000-0000-000000000001".into(),
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
    fn member_progress_health_is_classified_by_machine_elapsed_time() {
        let mut authority = MobMachineAuthority::new();
        let member_id = AgentIdentity::from("worker");

        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ObserveMemberProgress {
                agent_identity: member_id.clone(),
                run_open: true,
                in_flight_work: 2,
                progress_token: "run-1:started".into(),
                observed_at_ms: 1_000,
            },
        )
        .expect("first open observation should establish progress");
        assert_eq!(
            authority.state().member_health_class.get(&member_id),
            Some(&MemberHealthClass::Healthy)
        );

        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ObserveMemberProgress {
                agent_identity: member_id.clone(),
                run_open: true,
                in_flight_work: 2,
                progress_token: "run-1:started".into(),
                observed_at_ms: 61_000,
            },
        )
        .expect("unchanged open work should become degraded at the machine threshold");
        assert_eq!(
            authority.state().member_health_class.get(&member_id),
            Some(&MemberHealthClass::Degraded)
        );

        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ObserveMemberProgress {
                agent_identity: member_id.clone(),
                run_open: true,
                in_flight_work: 2,
                progress_token: "run-1:started".into(),
                observed_at_ms: 301_000,
            },
        )
        .expect("unchanged open work should become wedged at the machine threshold");
        assert_eq!(
            authority.state().member_health_class.get(&member_id),
            Some(&MemberHealthClass::Wedged)
        );

        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ObserveMemberProgress {
                agent_identity: member_id.clone(),
                run_open: false,
                in_flight_work: 0,
                progress_token: "stale-clock-heal".into(),
                observed_at_ms: 60_000,
            },
        )
        .expect("stale wall-clock observation should be accepted as a no-op");
        assert_eq!(
            authority.state().member_health_class.get(&member_id),
            Some(&MemberHealthClass::Wedged),
            "a regressed wall clock must not heal machine-owned health"
        );
        assert_eq!(
            authority.state().member_progress_tokens.get(&member_id),
            Some(&"run-1:started".to_string())
        );

        let near_max = AgentIdentity::from("near-max-clock");
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ObserveMemberProgress {
                agent_identity: near_max.clone(),
                run_open: true,
                in_flight_work: 1,
                progress_token: "open".into(),
                observed_at_ms: u64::MAX - 1,
            },
        )
        .expect("near-max observation should establish progress");
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ObserveMemberProgress {
                agent_identity: near_max.clone(),
                run_open: true,
                in_flight_work: 1,
                progress_token: "open".into(),
                observed_at_ms: u64::MAX,
            },
        )
        .expect("elapsed classification must not overflow at u64::MAX");
        assert_eq!(
            authority.state().member_health_class.get(&near_max),
            Some(&MemberHealthClass::Healthy)
        );
    }

    #[test]
    fn objective_conclusion_is_owned_idempotent_and_outcome_stable() {
        let mut authority = MobMachineAuthority::new();
        let member_id = AgentIdentity::from("worker");
        let objective_id = "00000000-0000-0000-0000-000000000029";

        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::KickoffMarkPending {
                member_id: member_id.clone(),
                objective_id: objective_id.into(),
            },
        )
        .expect("kickoff should mint an objective binding");
        let concluded = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ConcludeObjective {
                member_id: member_id.clone(),
                objective_id: objective_id.into(),
                outcome: "completed".into(),
            },
        )
        .expect("the owning member objective should conclude");
        assert!(concluded.effects().iter().any(|effect| matches!(
            effect,
            MobMachineEffect::PersistObjectiveConclusion { objective_id: id, outcome, .. }
                if id.as_str() == objective_id && outcome.as_str() == "completed"
        )));

        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ConcludeObjective {
                member_id: member_id.clone(),
                objective_id: objective_id.into(),
                outcome: "completed".into(),
            },
        )
        .expect("an identical conclusion should be idempotent");
        assert!(
            MobMachineMutator::apply(
                &mut authority,
                MobMachineInput::ConcludeObjective {
                    member_id,
                    objective_id: objective_id.into(),
                    outcome: "failed".into(),
                },
            )
            .is_err(),
            "a concluded objective must reject outcome rewriting"
        );
    }

    #[test]
    fn recovered_kickoff_lifecycle_is_machine_owned() {
        let mut authority = MobMachineAuthority::new();
        let member_id = AgentIdentity::from("worker");

        authority
            .apply_signal(MobMachineSignal::RecoverMemberKickoff {
                member_id: member_id.clone(),
                phase: KickoffPhase::Starting,
                error: None,
            })
            .expect("recovered starting kickoff should be accepted");
        assert_eq!(
            authority
                .state()
                .kickoff_material_for_member_id(member_id.0.as_str()),
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
            authority
                .state()
                .kickoff_material_for_member_id(member_id.0.as_str()),
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

    /// Dogma row R044: the trust-install-before-authorization-terminality
    /// window is a machine-owned obligation. Record opens it, Resolve (on
    /// confirmed-accept terminality) and Rollback (after a failure path
    /// removed the installed trust) close it; all three are idempotent set
    /// operations so nested authorize-then-bind windows compose.
    #[test]
    fn pending_recipient_trust_obligation_lifecycle() {
        let mut authority = MobMachineAuthority::new();
        let peer_id = PeerId::from("peer-r044");
        assert!(authority.state().pending_recipient_trust.is_empty());

        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RecordPendingRecipientTrust {
                peer_id: peer_id.clone(),
            },
        )
        .expect("record pending recipient trust");
        assert!(
            authority.state().pending_recipient_trust.contains(&peer_id),
            "recorded obligation must be visible in machine state"
        );

        // Re-recording the same peer is idempotent (nested windows).
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RecordPendingRecipientTrust {
                peer_id: peer_id.clone(),
            },
        )
        .expect("re-record pending recipient trust");
        assert_eq!(authority.state().pending_recipient_trust.len(), 1);

        // Success terminality closes the window.
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ResolvePendingRecipientTrust {
                peer_id: peer_id.clone(),
            },
        )
        .expect("resolve pending recipient trust");
        assert!(
            authority.state().pending_recipient_trust.is_empty(),
            "obligation must be empty after success terminality"
        );

        // Failure terminality (after the shell untrusted) closes the window.
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RecordPendingRecipientTrust {
                peer_id: peer_id.clone(),
            },
        )
        .expect("record pending recipient trust before failure");
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RollbackPendingRecipientTrust {
                peer_id: peer_id.clone(),
            },
        )
        .expect("rollback pending recipient trust");
        assert!(
            authority.state().pending_recipient_trust.is_empty(),
            "obligation must be empty after failure terminality"
        );

        // Closing an absent obligation stays a no-op.
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RollbackPendingRecipientTrust { peer_id },
        )
        .expect("rollback of an absent obligation is a no-op");
        assert!(authority.state().pending_recipient_trust.is_empty());
    }

    fn placed_cleanup_obligation(
        identity: &AgentIdentity,
        spawn_id: &PlacedSpawnId,
        expected_phase: PlacedSpawnCarrierExpectedPhase,
    ) -> PlacedCarrierCleanupObligation {
        PlacedCarrierCleanupObligation {
            agent_identity: identity.clone(),
            spawn_id: spawn_id.clone(),
            generation: Generation(0),
            fence_token: FenceToken(41),
            provision_operation_id: format!("operation-{}", spawn_id.0),
            operation_owner_session_id: SessionId("owner-session".to_string()),
            expected_phase,
        }
    }

    fn authorize_remote_placed_spawn(
        authority: &mut MobMachineAuthority,
        identity: &AgentIdentity,
        host: &HostId,
        spec_digest: &str,
    ) {
        if !authority.state().mob_hosts.contains(host) {
            authority
                .apply_signal(MobMachineSignal::RecoverHostBinding {
                    host_id: host.clone(),
                    pubkey: PeerSigningKey([8; 32]),
                    endpoint: PeerAddress("tcp://placed-host.test:4100".to_string()),
                    epoch: 1,
                    binding_generation: 1,
                    protocol_min: 4,
                    protocol_max: 4,
                    engine_version: "placed-test-engine".to_string(),
                    durable_sessions: true,
                    autonomous_members: true,
                    hard_cancel_member: true,
                    tracked_input_cancel: true,
                    memory_store: true,
                    mcp: true,
                    resolvable_providers: std::collections::BTreeSet::from([
                        "anthropic".to_string()
                    ]),
                    approval_forwarding: false,
                    live_endpoint: None,
                })
                .expect("recover placed-spawn host binding");
        }
        authority
            .apply_signal(MobMachineSignal::RecoverOwnerBridgeSession {
                bridge_session_id: SessionId("owner-session".to_string()),
                destroy_on_owner_archive: false,
                implicit_delegation_mob: false,
            })
            .expect("recover placed-spawn owner session");
        MobMachineMutator::apply(
            authority,
            MobMachineInput::AuthorizeSpawnProfile {
                agent_identity: identity.clone(),
                profile_name: "worker".to_string(),
                model: "test-model".to_string(),
                profile_material_digest: "profile-digest".to_string(),
                tool_config_digest: "tool-digest".to_string(),
                skills_digest: "skills-digest".to_string(),
                provider_params_digest: None,
                output_schema_digest: None,
                external_addressable: true,
                resolved_spec_digest: Some(spec_digest.to_string()),
            },
        )
        .expect("authorize placed-spawn profile");
    }

    fn begin_remote_placed_spawn(
        identity: &AgentIdentity,
        host: &HostId,
        spawn_id: &PlacedSpawnId,
    ) -> MobMachineInput {
        MobMachineInput::BeginSpawnExec {
            agent_identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId(format!("{}:0", identity.0)),
            fence_token: FenceToken(42),
            generation: Generation(0),
            profile_material_digest: "profile-digest".to_string(),
            external_addressable: true,
            runtime_mode: SpawnPolicyRuntimeMode::TurnDriven,
            bridge_session_id: None,
            replacing: None,
            placement: Some(host.clone()),
            workgraph_required: false,
            rust_bundles_present: false,
            per_spawn_external_tools_present: false,
            mob_default_external_tools_present: false,
            default_llm_client_override_present: false,
            host_surface_mcp_allowlist_present: false,
            inherited_tool_filter_present: false,
            shell_env_present: false,
            mcp_stdio_env_present: false,
            mcp_http_headers_present: false,
            memory_required: false,
            mcp_required: false,
            resume_session_id: None,
            placed_spawn_id: Some(spawn_id.clone()),
            placed_provision_operation_id: Some(format!("operation-{}", spawn_id.0)),
            placed_operation_owner_session_id: Some(SessionId("owner-session".to_string())),
            effective_profile_override_present: true,
            effective_model_override_present: true,
        }
    }

    #[test]
    fn pending_placed_recovery_is_cleanup_only_and_blocks_successor_until_exact_resolution() {
        let identity = AgentIdentity::from("pending-placed-worker");
        let pending_spawn_id = PlacedSpawnId("pending-spawn-id".to_string());
        let successor_spawn_id = PlacedSpawnId("successor-spawn-id".to_string());
        let host = HostId("placed-host".to_string());
        let obligation = placed_cleanup_obligation(
            &identity,
            &pending_spawn_id,
            PlacedSpawnCarrierExpectedPhase::Pending,
        );
        let mut authority = MobMachineAuthority::new();
        authority
            .apply_signal(MobMachineSignal::RecoverOwnerBridgeSession {
                bridge_session_id: SessionId("owner-session".to_string()),
                destroy_on_owner_archive: false,
                implicit_delegation_mob: false,
            })
            .expect("recover owner before Pending carrier");
        authority
            .apply_signal(MobMachineSignal::RecoverHostBinding {
                host_id: host.clone(),
                pubkey: PeerSigningKey([8; 32]),
                endpoint: PeerAddress("tcp://placed-host.test:4100".to_string()),
                epoch: 1,
                binding_generation: 1,
                protocol_min: 4,
                protocol_max: 4,
                engine_version: "placed-test-engine".to_string(),
                durable_sessions: true,
                autonomous_members: true,
                hard_cancel_member: true,
                tracked_input_cancel: true,
                memory_store: true,
                mcp: true,
                resolvable_providers: std::collections::BTreeSet::from(["anthropic".to_string()]),
                approval_forwarding: false,
                live_endpoint: None,
            })
            .expect("recover host before Pending carrier");

        let recovery = authority
            .apply_signal(MobMachineSignal::RecoverPendingPlacedSpawn {
                spawn_id: pending_spawn_id.clone(),
                agent_identity: identity.clone(),
                generation: Generation(0),
                fence_token: FenceToken(41),
                host_id: host.clone(),
                host_binding_generation: 1,
                spec_digest: "spec-digest".to_string(),
                runtime_mode: SpawnPolicyRuntimeMode::AutonomousHost,
                provision_operation_id: format!("operation-{}", pending_spawn_id.0),
                operation_owner_session_id: SessionId("owner-session".to_string()),
            })
            .expect("recover pending placed carrier");
        assert!(recovery.effects().iter().any(|effect| matches!(
            effect,
            MobMachineEffect::PlacedCarrierCleanupRequested { obligation: emitted }
                if emitted == &obligation
        )));
        assert_eq!(
            authority.state().pending_placed_spawn_ids.get(&identity),
            Some(&pending_spawn_id)
        );
        assert!(
            authority
                .state()
                .pending_autonomous_placed_spawns
                .contains(&identity),
            "recovery must retain the pending autonomous capability contract"
        );
        assert!(
            !authority
                .state()
                .identity_to_runtime
                .contains_key(&identity)
                && authority.state().live_runtime_ids.is_empty(),
            "a Pending carrier must not manufacture roster membership"
        );
        assert!(
            authority
                .state()
                .pending_placed_carrier_cleanup
                .contains(&obligation)
        );

        authorize_remote_placed_spawn(&mut authority, &identity, &host, "spec-digest");
        assert!(
            MobMachineMutator::apply(
                &mut authority,
                begin_remote_placed_spawn(&identity, &host, &successor_spawn_id),
            )
            .is_err(),
            "a successor attempt must remain blocked while the exact carrier cleanup is open"
        );

        let authorized = MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::AuthorizePlacedCarrierCleanup {
                obligation: obligation.clone(),
            },
        )
        .expect("authorize exact pending carrier deletion");
        assert!(authorized.effects().iter().any(|effect| matches!(
            effect,
            MobMachineEffect::PlacedCarrierCleanupAuthorized { obligation: emitted }
                if emitted == &obligation
        )));
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ResolvePlacedCarrierCleanup {
                obligation: obligation.clone(),
            },
        )
        .expect("resolve exact pending carrier deletion");
        assert!(
            !authority
                .state()
                .pending_placed_carrier_cleanup
                .contains(&obligation)
                && !authority
                    .state()
                    .pending_placed_spawn_ids
                    .contains_key(&identity)
                && !authority
                    .state()
                    .pending_autonomous_placed_spawns
                    .contains(&identity)
        );

        let successor = MobMachineMutator::apply(
            &mut authority,
            begin_remote_placed_spawn(&identity, &host, &successor_spawn_id),
        )
        .expect("successor attempt may open only after exact cleanup resolution");
        assert!(successor.effects().iter().any(|effect| matches!(
            effect,
            MobMachineEffect::PersistPendingPlacedSpawn {
                spawn_id,
                effective_profile_override_present,
                effective_model_override_present,
                ..
            } if spawn_id == &successor_spawn_id
                && *effective_profile_override_present
                && *effective_model_override_present
        )));
    }

    #[test]
    fn committed_placed_recovery_restores_exact_roster_and_routing_facts() {
        let identity = AgentIdentity::from("committed-placed-worker");
        let runtime_id = AgentRuntimeId("committed-placed-worker:0".to_string());
        let spawn_id = PlacedSpawnId("committed-spawn-id".to_string());
        let host = HostId("placed-host".to_string());
        let session_id = SessionId("member-session".to_string());
        let endpoint = MemberPeerEndpoint {
            name: PeerName("mob/worker/committed-placed-worker".to_string()),
            peer_id: PeerId("peer-id".to_string()),
            address: PeerAddress("tcp://placed-member.test:4200".to_string()),
            signing_key: PeerSigningKey([3; 32]),
        };
        let recovery_signal = || MobMachineSignal::RecoverCommittedPlacedSpawn {
            spawn_id: spawn_id.clone(),
            agent_identity: identity.clone(),
            agent_runtime_id: runtime_id.clone(),
            generation: Generation(0),
            fence_token: FenceToken(51),
            host_id: host.clone(),
            host_binding_generation: 1,
            member_session_id: session_id.clone(),
            member_peer_endpoint: endpoint.clone(),
            profile_name: "worker".to_string(),
            runtime_mode: SpawnPolicyRuntimeMode::TurnDriven,
            external_addressable: true,
            provision_operation_id: "committed-operation-id".to_string(),
            operation_owner_session_id: SessionId("owner-session".to_string()),
        };
        assert!(
            MobMachineAuthority::new()
                .apply_signal(recovery_signal())
                .is_err(),
            "committed placed recovery must fail closed before owner authority is restored"
        );
        let mut authority = MobMachineAuthority::new();
        authority
            .apply_signal(MobMachineSignal::RecoverOwnerBridgeSession {
                bridge_session_id: SessionId("owner-session".to_string()),
                destroy_on_owner_archive: false,
                implicit_delegation_mob: false,
            })
            .expect("recover owner before committed placed carrier");
        assert!(
            authority.apply_signal(recovery_signal()).is_err(),
            "committed placed recovery must reject an absent or unbound carrier host"
        );
        authority
            .apply_signal(MobMachineSignal::RecoverHostBinding {
                host_id: host.clone(),
                pubkey: PeerSigningKey([8; 32]),
                endpoint: PeerAddress("tcp://placed-host.test:4100".to_string()),
                epoch: 1,
                binding_generation: 1,
                protocol_min: 4,
                protocol_max: 4,
                engine_version: "placed-test-engine".to_string(),
                durable_sessions: true,
                autonomous_members: true,
                hard_cancel_member: true,
                tracked_input_cancel: true,
                memory_store: true,
                mcp: true,
                resolvable_providers: std::collections::BTreeSet::from(["anthropic".to_string()]),
                approval_forwarding: false,
                live_endpoint: None,
            })
            .expect("recover bound host before committed placed carrier");

        authority
            .apply_signal(recovery_signal())
            .expect("recover committed placed carrier");

        let state = authority.state();
        assert_eq!(state.identity_to_runtime.get(&identity), Some(&runtime_id));
        assert!(state.live_runtime_ids.contains(&runtime_id));
        assert!(
            state
                .externally_addressable_runtime_ids
                .contains(&runtime_id)
        );
        assert_eq!(
            state.runtime_fence_tokens.get(&runtime_id),
            Some(&FenceToken(51))
        );
        assert_eq!(
            state.identity_runtime_generations.get(&identity),
            Some(&Generation(0))
        );
        assert_eq!(
            state.member_session_bindings.get(&identity),
            Some(&session_id)
        );
        assert_eq!(state.member_peer_endpoints.get(&identity), Some(&endpoint));
        assert_eq!(state.member_placement.get(&identity), Some(&host));
        assert_eq!(
            state.current_placed_spawn_ids.get(&identity),
            Some(&spawn_id)
        );
        assert!(!state.pending_placed_spawn_ids.contains_key(&identity));
    }

    #[test]
    fn committed_placed_recovery_rejects_peer_ids_owned_by_members_or_hosts() {
        let host = HostId("placed-host".to_string());
        let owner = SessionId("owner-session".to_string());
        let shared_endpoint = MemberPeerEndpoint {
            name: PeerName("mob/worker/shared-peer".to_string()),
            peer_id: PeerId("shared-peer-id".to_string()),
            address: PeerAddress("tcp://placed-member.test:4200".to_string()),
            signing_key: PeerSigningKey([3; 32]),
        };
        let mut authority = MobMachineAuthority::new();
        authority
            .apply_signal(MobMachineSignal::RecoverOwnerBridgeSession {
                bridge_session_id: owner.clone(),
                destroy_on_owner_archive: false,
                implicit_delegation_mob: false,
            })
            .expect("recover owner");
        authority
            .apply_signal(MobMachineSignal::RecoverHostBinding {
                host_id: host.clone(),
                pubkey: PeerSigningKey([8; 32]),
                endpoint: PeerAddress("tcp://placed-host.test:4100".to_string()),
                epoch: 1,
                binding_generation: 1,
                protocol_min: 4,
                protocol_max: 4,
                engine_version: "placed-test-engine".to_string(),
                durable_sessions: true,
                autonomous_members: true,
                hard_cancel_member: true,
                tracked_input_cancel: true,
                memory_store: true,
                mcp: true,
                resolvable_providers: std::collections::BTreeSet::from(["anthropic".to_string()]),
                approval_forwarding: false,
                live_endpoint: None,
            })
            .expect("recover host");

        let recovery = |name: &str, fence: u64| {
            let identity = AgentIdentity::from(name);
            MobMachineSignal::RecoverCommittedPlacedSpawn {
                spawn_id: PlacedSpawnId(format!("spawn-{name}")),
                agent_identity: identity.clone(),
                agent_runtime_id: AgentRuntimeId(format!("{name}:0")),
                generation: Generation(0),
                fence_token: FenceToken(fence),
                host_id: host.clone(),
                host_binding_generation: 1,
                member_session_id: SessionId(format!("session-{name}")),
                member_peer_endpoint: shared_endpoint.clone(),
                profile_name: "worker".to_string(),
                runtime_mode: SpawnPolicyRuntimeMode::TurnDriven,
                external_addressable: true,
                provision_operation_id: format!("operation-{name}"),
                operation_owner_session_id: owner.clone(),
            }
        };
        let first = AgentIdentity::from("placed-first");
        let second = AgentIdentity::from("placed-second");
        authority
            .apply_signal(recovery("placed-first", 61))
            .expect("first endpoint owner recovers");
        assert!(
            authority
                .apply_signal(recovery("placed-second", 62))
                .is_err(),
            "the same live peer id must not route a second placed identity"
        );
        assert_eq!(
            authority.state().member_peer_endpoints.get(&first),
            Some(&shared_endpoint)
        );
        assert!(!authority.state().identity_to_runtime.contains_key(&second));

        let host_collision_identity = AgentIdentity::from("placed-host-collision");
        let mut host_collision_endpoint = shared_endpoint.clone();
        host_collision_endpoint.peer_id = PeerId(host.0.clone());
        assert!(
            authority
                .apply_signal(MobMachineSignal::RecoverCommittedPlacedSpawn {
                    spawn_id: PlacedSpawnId("spawn-host-collision".to_string()),
                    agent_identity: host_collision_identity.clone(),
                    agent_runtime_id: AgentRuntimeId("placed-host-collision:0".to_string()),
                    generation: Generation(0),
                    fence_token: FenceToken(63),
                    host_id: host.clone(),
                    host_binding_generation: 1,
                    member_session_id: SessionId("session-host-collision".to_string()),
                    member_peer_endpoint: host_collision_endpoint,
                    profile_name: "worker".to_string(),
                    runtime_mode: SpawnPolicyRuntimeMode::TurnDriven,
                    external_addressable: true,
                    provision_operation_id: "operation-host-collision".to_string(),
                    operation_owner_session_id: owner,
                })
                .is_err(),
            "a member ACK must not claim a bound host's peer id"
        );
        assert!(
            !authority
                .state()
                .identity_to_runtime
                .contains_key(&host_collision_identity)
        );
    }

    #[test]
    fn placed_cleanup_obligation_blocks_destroy_until_authorized_resolution() {
        let identity = AgentIdentity::from("cleanup-blocks-destroy");
        let spawn_id = PlacedSpawnId("cleanup-spawn-id".to_string());
        let obligation = placed_cleanup_obligation(
            &identity,
            &spawn_id,
            PlacedSpawnCarrierExpectedPhase::Committed,
        );
        let mut authority = MobMachineAuthority::new();
        authority
            .apply_signal(MobMachineSignal::RecoverOwnerBridgeSession {
                bridge_session_id: obligation.operation_owner_session_id.clone(),
                destroy_on_owner_archive: false,
                implicit_delegation_mob: false,
            })
            .expect("recover cleanup operation owner");
        authority
            .apply_signal(MobMachineSignal::RecoverPlacedCarrierCleanup {
                obligation: obligation.clone(),
            })
            .expect("recover exact cleanup obligation");

        begin_completion_lifecycle_quiesce(
            &mut authority,
            PlacedCompletionLifecycleIntentKind::Destroy,
        );

        assert!(
            authority
                .apply_signal(MobMachineSignal::DestroyMob {
                    session_id: SessionId("destroy-session".to_string()),
                })
                .is_err(),
            "destroy must fail while a carrier cleanup obligation remains"
        );
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::AuthorizePlacedCarrierCleanup {
                obligation: obligation.clone(),
            },
        )
        .expect("authorize exact committed carrier deletion");
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ResolvePlacedCarrierCleanup { obligation },
        )
        .expect("resolve exact committed carrier deletion");
        authority
            .apply_signal(MobMachineSignal::DestroyMob {
                session_id: SessionId("destroy-session".to_string()),
            })
            .expect("destroy may commit after exact carrier cleanup resolves");
    }

    fn seed_preserving_retirement_with_topology() -> (
        MobMachineAuthority,
        AgentIdentity,
        AgentRuntimeId,
        FenceToken,
        SessionId,
        WiringEdge,
        ExternalPeerEdge,
        ExternalPeerKey,
    ) {
        let mut authority = MobMachineAuthority::new();
        let retiring = AgentIdentity::from("abandoning-member");
        let peer = AgentIdentity::from("surviving-peer");
        let retiring_runtime = AgentRuntimeId::from("abandoning-member:0");
        let peer_runtime = AgentRuntimeId::from("surviving-peer:0");
        let fence_token = FenceToken(7);
        let session_id = seed_live_member(&mut authority, &retiring, &retiring_runtime);
        seed_live_member(&mut authority, &peer, &peer_runtime);
        register_test_member_peer(&mut authority, &retiring, "abandoning-member", [21; 32]);
        register_test_member_peer(&mut authority, &peer, "surviving-peer", [22; 32]);

        let member_edge = WiringEdge::new(retiring.clone(), peer);
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::WireMembersWithTrust {
                a_identity: member_edge.a.clone(),
                b_identity: member_edge.b.clone(),
                edge: member_edge.clone(),
            },
        )
        .expect("wire retained member edge");
        let external_edge = external_peer_edge_for_test(retiring.0.as_str(), "external-peer");
        let external_key = ExternalPeerKey::new(
            external_edge.local.clone(),
            external_edge.endpoint.name.clone(),
        );
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::WireExternalPeer {
                key: external_key.clone(),
                edge: external_edge.clone(),
            },
        )
        .expect("wire retained external edge");

        authority
            .apply_signal(MobMachineSignal::RetireMember {
                agent_identity: retiring.clone(),
                agent_runtime_id: retiring_runtime.clone(),
                fence_token,
                session_id: Some(session_id.clone()),
            })
            .expect("admit retiring member");
        authority
            .apply_signal(
                MobMachineSignal::ObserveRespawnTopologyPreservationStarted {
                    agent_identity: retiring.clone(),
                    agent_runtime_id: retiring_runtime.clone(),
                    fence_token,
                    generation: Generation(0),
                },
            )
            .expect("record exact topology preservation start");

        (
            authority,
            retiring,
            retiring_runtime,
            fence_token,
            session_id,
            member_edge,
            external_edge,
            external_key,
        )
    }

    #[test]
    fn respawn_topology_abandonment_paths_prune_atomically_with_epoch_parity() {
        let (
            base,
            identity,
            runtime_id,
            fence_token,
            session_id,
            member_edge,
            external_edge,
            external_key,
        ) = seed_preserving_retirement_with_topology();
        let preservation_epoch = base.state().topology_epoch;
        assert!(base.state().pending_respawn_topology.contains(&identity));
        assert!(base.state().wiring_edges.contains(&member_edge));
        assert!(base.state().external_peer_edges.contains(&external_edge));
        assert_eq!(
            base.state().external_peer_edges_by_key.get(&external_key),
            Some(&external_edge)
        );

        let abandonment_signal = || MobMachineSignal::ObserveRespawnTopologyAbandoned {
            agent_identity: identity.clone(),
            agent_runtime_id: runtime_id.clone(),
            fence_token,
            generation: Generation(0),
        };

        // If abandonment is recorded while the old runtime is still retiring,
        // graph authority must remain intact for physical trust cleanup. The
        // following ordinary terminal is the single pruning transition.
        let mut preterminal = MobMachineAuthority::recover_from_state(base.state().clone())
            .expect("fork preterminal abandonment authority");
        let preterminal_marker = preterminal
            .apply_signal(abandonment_signal())
            .expect("record preterminal abandonment");
        assert!(preterminal.state().wiring_edges.contains(&member_edge));
        assert!(
            preterminal
                .state()
                .external_peer_edges
                .contains(&external_edge)
        );
        assert_eq!(
            preterminal
                .state()
                .external_peer_edges_by_key
                .get(&external_key),
            Some(&external_edge)
        );
        assert_eq!(preterminal.state().topology_epoch, preservation_epoch);
        assert!(preterminal_marker.effects().iter().any(|effect| matches!(
            effect,
            MobMachineEffect::AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::RespawnTopologyAbandoned,
                ..
            }
        )));
        assert!(
            !preterminal_marker
                .effects()
                .iter()
                .any(|effect| matches!(effect, MobMachineEffect::WiringGraphChanged { .. }))
        );
        let duplicate_preterminal = preterminal
            .apply_signal(abandonment_signal())
            .expect("duplicate preterminal abandonment is idempotent");
        assert!(duplicate_preterminal.effects().is_empty());
        assert_eq!(preterminal.state().topology_epoch, preservation_epoch);

        preterminal
            .apply_signal(MobMachineSignal::ObserveRuntimeRetired {
                agent_runtime_id: runtime_id.clone(),
                fence_token,
            })
            .expect("retire old runtime after abandonment");
        let ordinary_terminal = preterminal
            .apply_signal(MobMachineSignal::ObserveMemberRetirementArchived {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_id.clone(),
                fence_token,
                generation: Generation(0),
                session_id: Some(session_id.clone()),
                disposal: MemberSessionDisposal::Archived,
                preserve_machine_topology: false,
            })
            .expect("ordinary terminal prunes abandoned topology");
        assert!(!preterminal.state().wiring_edges.contains(&member_edge));
        assert!(
            !preterminal
                .state()
                .external_peer_edges
                .contains(&external_edge)
        );
        assert!(
            !preterminal
                .state()
                .external_peer_edges_by_key
                .contains_key(&external_key)
        );
        assert_eq!(preterminal.state().topology_epoch, preservation_epoch + 1);
        assert!(
            ordinary_terminal
                .effects()
                .iter()
                .any(|effect| matches!(effect, MobMachineEffect::WiringGraphChanged { .. }))
        );

        // If retirement reaches terminal first, the marker itself must prune
        // all three graph projections and publish the journal+epoch effects in
        // one accepted transition.
        let mut terminal = MobMachineAuthority::recover_from_state(base.state().clone())
            .expect("fork terminal abandonment authority");
        terminal
            .apply_signal(MobMachineSignal::ObserveRuntimeRetired {
                agent_runtime_id: runtime_id.clone(),
                fence_token,
            })
            .expect("retire old runtime while preserving topology");
        terminal
            .apply_signal(MobMachineSignal::ObserveMemberRetirementArchived {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_id.clone(),
                fence_token,
                generation: Generation(0),
                session_id: Some(session_id),
                disposal: MemberSessionDisposal::Archived,
                preserve_machine_topology: true,
            })
            .expect("terminal retirement retains preserved topology");
        assert!(terminal.state().wiring_edges.contains(&member_edge));
        assert!(
            terminal
                .state()
                .external_peer_edges
                .contains(&external_edge)
        );
        assert_eq!(terminal.state().topology_epoch, preservation_epoch);

        let terminal_marker = terminal
            .apply_signal(abandonment_signal())
            .expect("terminal abandonment prunes atomically");
        assert!(!terminal.state().wiring_edges.contains(&member_edge));
        assert!(
            !terminal
                .state()
                .external_peer_edges
                .contains(&external_edge)
        );
        assert!(
            !terminal
                .state()
                .external_peer_edges_by_key
                .contains_key(&external_key)
        );
        assert_eq!(terminal.state().topology_epoch, preservation_epoch + 1);
        assert_eq!(
            terminal.state().topology_epoch,
            preterminal.state().topology_epoch,
            "preterminal and terminal marker orderings must converge to the same epoch"
        );
        assert!(terminal_marker.effects().iter().any(|effect| matches!(
            effect,
            MobMachineEffect::AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::RespawnTopologyAbandoned,
                ..
            }
        )));
        assert!(terminal_marker
            .effects()
            .iter()
            .any(|effect| matches!(effect, MobMachineEffect::WiringGraphChanged { epoch } if *epoch == preservation_epoch + 1)));
        let duplicate_terminal = terminal
            .apply_signal(abandonment_signal())
            .expect("duplicate terminal abandonment is idempotent");
        assert!(duplicate_terminal.effects().is_empty());
        assert_eq!(terminal.state().topology_epoch, preservation_epoch + 1);
    }

    #[test]
    fn respawn_topology_abandonment_is_noop_after_successor_rebind() {
        let (
            mut authority,
            identity,
            runtime_id,
            fence_token,
            session_id,
            member_edge,
            external_edge,
            external_key,
        ) = seed_preserving_retirement_with_topology();
        authority
            .apply_signal(MobMachineSignal::ObserveRuntimeRetired {
                agent_runtime_id: runtime_id.clone(),
                fence_token,
            })
            .expect("retire old runtime while preserving topology");
        authority
            .apply_signal(MobMachineSignal::ObserveMemberRetirementArchived {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_id.clone(),
                fence_token,
                generation: Generation(0),
                session_id: Some(session_id),
                disposal: MemberSessionDisposal::Archived,
                preserve_machine_topology: true,
            })
            .expect("terminal retirement retains topology for successor");

        let successor_runtime = AgentRuntimeId::from("abandoning-member:1");
        authority
            .apply_signal(MobMachineSignal::RecoverRosterMember {
                agent_identity: identity.clone(),
                agent_runtime_id: successor_runtime.clone(),
                fence_token: FenceToken(8),
                generation: Generation(1),
                profile_name: "worker".to_string(),
                runtime_mode: SpawnPolicyRuntimeMode::AutonomousHost,
                external_addressable: true,
            })
            .expect("recover exact successor membership");
        let successor_epoch = authority.state().topology_epoch;

        let stale = authority
            .apply_signal(MobMachineSignal::ObserveRespawnTopologyAbandoned {
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_id,
                fence_token,
                generation: Generation(0),
            })
            .expect("old-generation marker after successor is a no-op");
        assert!(stale.effects().is_empty());
        assert_eq!(authority.state().topology_epoch, successor_epoch);
        assert_eq!(
            authority.state().identity_to_runtime.get(&identity),
            Some(&successor_runtime)
        );
        assert!(authority.state().wiring_edges.contains(&member_edge));
        assert!(
            authority
                .state()
                .external_peer_edges
                .contains(&external_edge)
        );
        assert_eq!(
            authority
                .state()
                .external_peer_edges_by_key
                .get(&external_key),
            Some(&external_edge)
        );
    }

    fn seed_autonomous_placed_kickoff()
    -> (MobMachineAuthority, AgentIdentity, PlacedKickoffObligation) {
        let identity = AgentIdentity::from("placed-kickoff-worker");
        let runtime_id = AgentRuntimeId("placed-kickoff-worker:0".to_string());
        let host_id = HostId("placed-kickoff-host".to_string());
        let member_session_id = SessionId("placed-kickoff-session".to_string());
        let owner_session_id = SessionId("placed-kickoff-owner".to_string());
        let objective_id = "00000000-0000-4000-8000-000000000001".to_string();
        let mut authority = MobMachineAuthority::new();
        authority
            .apply_signal(MobMachineSignal::RecoverOwnerBridgeSession {
                bridge_session_id: owner_session_id.clone(),
                destroy_on_owner_archive: false,
                implicit_delegation_mob: false,
            })
            .expect("recover placed kickoff owner");
        authority
            .apply_signal(MobMachineSignal::RecoverHostBinding {
                host_id: host_id.clone(),
                pubkey: PeerSigningKey([8; 32]),
                endpoint: PeerAddress("tcp://placed-kickoff-host.test:4100".to_string()),
                epoch: 1,
                binding_generation: 1,
                protocol_min: 4,
                protocol_max: 4,
                engine_version: "placed-kickoff-test-engine".to_string(),
                durable_sessions: true,
                autonomous_members: true,
                hard_cancel_member: true,
                tracked_input_cancel: true,
                memory_store: true,
                mcp: true,
                resolvable_providers: std::collections::BTreeSet::from(["anthropic".to_string()]),
                approval_forwarding: false,
                live_endpoint: None,
            })
            .expect("recover placed kickoff host");
        authority
            .apply_signal(MobMachineSignal::RecoverCommittedPlacedSpawn {
                spawn_id: PlacedSpawnId("placed-kickoff-spawn".to_string()),
                agent_identity: identity.clone(),
                agent_runtime_id: runtime_id,
                generation: Generation(0),
                fence_token: FenceToken(51),
                host_id: host_id.clone(),
                host_binding_generation: 1,
                member_session_id: member_session_id.clone(),
                member_peer_endpoint: MemberPeerEndpoint {
                    name: PeerName("mob/worker/placed-kickoff-worker".to_string()),
                    peer_id: PeerId("placed-kickoff-peer".to_string()),
                    address: PeerAddress("tcp://placed-kickoff-member.test:4200".to_string()),
                    signing_key: PeerSigningKey([3; 32]),
                },
                profile_name: "worker".to_string(),
                runtime_mode: SpawnPolicyRuntimeMode::AutonomousHost,
                external_addressable: true,
                provision_operation_id: "placed-kickoff-operation".to_string(),
                operation_owner_session_id: owner_session_id,
            })
            .expect("recover autonomous placed member");
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::KickoffMarkPending {
                member_id: identity.clone(),
                objective_id: objective_id.clone(),
            },
        )
        .expect("mark placed kickoff pending");

        let obligation = PlacedKickoffObligation {
            agent_identity: identity.clone(),
            host_id,
            host_binding_generation: 1,
            member_session_id,
            generation: Generation(0),
            fence_token: FenceToken(51),
            input_id: InputId("00000000-0000-4000-8000-000000000002".to_string()),
            objective_id,
        };
        (authority, identity, obligation)
    }

    #[test]
    fn placed_kickoff_start_resolve_and_ack_preserve_exact_custody() {
        let (mut authority, identity, obligation) = seed_autonomous_placed_kickoff();
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::StartPlacedKickoff {
                obligation: obligation.clone(),
            },
        )
        .expect("record placed kickoff before send");
        assert!(
            authority
                .state()
                .pending_placed_kickoff_outcomes
                .contains(&obligation)
        );
        assert!(
            authority
                .state()
                .member_kickoff_starting
                .contains(&identity)
        );
        assert_eq!(
            authority.state().member_kickoff_input_ids.get(&identity),
            Some(&obligation.input_id)
        );
        assert!(
            MobMachineMutator::apply(
                &mut authority,
                MobMachineInput::KickoffResolveStarted {
                    member_id: identity.clone(),
                },
            )
            .is_err(),
            "ordinary kickoff resolution must not bypass placed custody"
        );

        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ResolvePlacedKickoffStarted {
                obligation: obligation.clone(),
            },
        )
        .expect("resolve exact retained host terminal");
        assert!(authority.state().member_kickoff_started.contains(&identity));
        assert!(
            !authority
                .state()
                .pending_placed_kickoff_outcomes
                .contains(&obligation)
                && authority
                    .state()
                    .resolved_placed_kickoff_outcomes
                    .contains(&obligation)
        );
        assert!(
            MobMachineMutator::apply(
                &mut authority,
                MobMachineInput::ResolvePlacedKickoffFailed {
                    obligation: obligation.clone(),
                    error: "different terminal".to_string(),
                },
            )
            .is_err(),
            "a different host terminal kind is not a replay"
        );
        let mut forged_residency = obligation.clone();
        forged_residency.host_id = HostId("forged-host".to_string());
        assert!(
            MobMachineMutator::apply(
                &mut authority,
                MobMachineInput::AcknowledgePlacedKickoffOutcome {
                    obligation: forged_residency,
                },
            )
            .is_err(),
            "ACK must match the retained full residency tuple"
        );

        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::AcknowledgePlacedKickoffOutcome {
                obligation: obligation.clone(),
            },
        )
        .expect("ack exact resolved host outcome");
        assert!(
            !authority
                .state()
                .resolved_placed_kickoff_outcomes
                .contains(&obligation)
        );
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::AcknowledgePlacedKickoffOutcome {
                obligation: obligation.clone(),
            },
        )
        .expect("exact ACK replay is idempotent");
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::ResolvePlacedKickoffStarted {
                obligation: obligation.clone(),
            },
        )
        .expect("exact host terminal replay remains total after ACK");
        assert!(
            MobMachineMutator::apply(
                &mut authority,
                MobMachineInput::DisposePlacedKickoffObligation {
                    obligation: obligation.clone(),
                },
            )
            .is_err(),
            "ACK closure cannot be laundered into disposal replay"
        );

        let mut wrong = obligation;
        wrong.input_id = InputId("00000000-0000-4000-8000-000000000003".to_string());
        assert!(
            MobMachineMutator::apply(
                &mut authority,
                MobMachineInput::AcknowledgePlacedKickoffOutcome { obligation: wrong },
            )
            .is_err(),
            "an absent but differently correlated ACK is not an exact replay"
        );
    }

    #[test]
    fn placed_kickoff_certified_no_effect_rejection_has_no_ack_custody() {
        let (mut authority, identity, obligation) = seed_autonomous_placed_kickoff();
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::StartPlacedKickoff {
                obligation: obligation.clone(),
            },
        )
        .expect("record placed kickoff before send");
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RejectPlacedKickoffBeforeAdmission {
                obligation: obligation.clone(),
                error: "host certified no admission".to_string(),
            },
        )
        .expect("terminalize certified no-effect rejection");
        assert!(authority.state().member_kickoff_failed.contains(&identity));
        assert_eq!(
            authority
                .state()
                .member_kickoff_error
                .get(&identity)
                .map(String::as_str),
            Some("host certified no admission")
        );
        assert!(
            authority.state().pending_placed_kickoff_outcomes.is_empty()
                && authority
                    .state()
                    .resolved_placed_kickoff_outcomes
                    .is_empty(),
            "certified no-effect rejection must not enter host ACK custody"
        );
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RejectPlacedKickoffBeforeAdmission {
                obligation: obligation.clone(),
                error: "host certified no admission".to_string(),
            },
        )
        .expect("exact rejection replay is idempotent");
        assert!(
            MobMachineMutator::apply(
                &mut authority,
                MobMachineInput::RejectPlacedKickoffBeforeAdmission {
                    obligation,
                    error: "different rejection".to_string(),
                },
            )
            .is_err(),
            "replay must match the exact retained failure"
        );
    }

    #[test]
    fn placed_kickoff_cancellation_and_cross_family_aliases_stay_fenced() {
        let (mut authority, identity, obligation) = seed_autonomous_placed_kickoff();
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::StartPlacedKickoff {
                obligation: obligation.clone(),
            },
        )
        .expect("record placed kickoff before send");
        let aliased_flow = RemoteTurnObligation {
            agent_identity: obligation.agent_identity.clone(),
            host_id: obligation.host_id.clone(),
            host_binding_generation: obligation.host_binding_generation,
            member_session_id: obligation.member_session_id.clone(),
            generation: obligation.generation,
            fence_token: obligation.fence_token,
            dispatch_sequence: 1,
            input_id: obligation.input_id.clone(),
            run_id: RunId("placed-kickoff-alias-run".to_string()),
            step_id: StepId("placed-kickoff-alias-step".to_string()),
        };
        assert!(
            MobMachineMutator::apply(
                &mut authority,
                MobMachineInput::RecordRemoteTurnObligation {
                    obligation: aliased_flow,
                },
            )
            .is_err(),
            "flow and kickoff custody must never share an input correlation"
        );
        begin_completion_lifecycle_quiesce(
            &mut authority,
            PlacedCompletionLifecycleIntentKind::Reset,
        );
        assert!(
            MobMachineMutator::apply(&mut authority, MobMachineInput::Reset).is_err(),
            "reset must wait for placed kickoff custody to drain"
        );
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::KickoffCancelRequested {
                member_id: identity.clone(),
            },
        )
        .expect("request local cancellation while the host terminal is pending");
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RejectPlacedKickoffBeforeAdmission {
                obligation: obligation.clone(),
                error: "late no-effect claim".to_string(),
            },
        )
        .expect("no-effect proof closes custody while preserving cancellation");
        assert!(
            authority
                .state()
                .member_kickoff_cancelled
                .contains(&identity)
        );
        assert!(
            authority.state().pending_placed_kickoff_outcomes.is_empty()
                && authority
                    .state()
                    .resolved_placed_kickoff_outcomes
                    .is_empty()
        );
        MobMachineMutator::apply(
            &mut authority,
            MobMachineInput::RejectPlacedKickoffBeforeAdmission {
                obligation,
                error: "late no-effect claim".to_string(),
            },
        )
        .expect("cancelled no-effect replay is idempotent");
        MobMachineMutator::apply(&mut authority, MobMachineInput::Reset)
            .expect("reset is admitted after certified no-effect custody closure");
    }
}
