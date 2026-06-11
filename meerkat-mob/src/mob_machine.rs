//! Diagnostic snapshot and command facade for the Mob runtime surface.
//!
//! The runtime actor owns Mob authority; this module keeps the top-level
//! command/result surface plus the durable diagnostic snapshot shapes that
//! remain useful for inspection and follow-up work.

use crate::ids::{AgentIdentity, AgentRuntimeId, FenceToken, FlowId, RunId, WorkRef, WorkSpec};
use crate::roster::{Roster, RosterEntry};
use crate::run::MobRun;
#[cfg(test)]
use crate::runtime::MobLifecycleSnapshot;
use crate::runtime::MobMemberListEntry;
#[cfg(test)]
use crate::runtime::MobOrchestratorSnapshot;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use indexmap::IndexSet;
use meerkat_machine_derive::CommandManifest;
use meerkat_machine_schema::catalog::dsl::mob_machine::MobMachineInputVariant;
use std::collections::BTreeMap;

// Keystone-A generalization: the MobMachineCatalogInput mirror (enum + ALL +
// input_variant + as_str) is now SCHEMA-DERIVED — emitted by `xtask
// protocol-codegen` from MobMachine's inputs.variants into the generated module
// below, replacing the former ~560-line hand-maintained enum. A mob input fold
// no longer touches this mirror.
pub use crate::generated::catalog_input::MobMachineCatalogInput;
use std::sync::Arc;

/// Public Mob mutations route through this single top-level machine command
/// surface instead of each `MobHandle` method hand-sending actor commands.
#[derive(CommandManifest)]
pub(crate) enum MobMachineCommand {
    RunFlow {
        flow_id: FlowId,
        activation_params: serde_json::Value,
        scoped_event_tx: Option<tokio::sync::mpsc::Sender<meerkat_core::ScopedAgentEvent>>,
    },
    CancelFlow {
        run_id: RunId,
    },
    FlowStatus {
        run_id: RunId,
    },
    Spawn {
        spec: Box<crate::runtime::SpawnMemberSpec>,
        spawn_source: crate::runtime::SpawnSource,
        owner_context: Option<crate::runtime::CanonicalOpsOwnerContext>,
    },
    /// Declarative spawn-if-absent. Constructed by
    /// `MobHandle::ensure_member` (runtime/handle.rs:2291) and matched in
    /// `MobHandle::execute_machine_command` (runtime/handle.rs:864);
    /// surfaced on the RPC `mob.ensure_member` verb
    /// (meerkat-rpc/src/handlers/mob.rs:1707).
    EnsureMember {
        spec: Box<crate::runtime::SpawnMemberSpec>,
    },
    /// Declarative drive-toward-desired roster. Constructed by
    /// `MobHandle::reconcile` (runtime/handle.rs:2317) and matched at
    /// runtime/handle.rs:868.
    Reconcile {
        desired: Vec<crate::runtime::SpawnMemberSpec>,
        options: crate::runtime::ReconcileOptions,
    },
    /// Filtered roster listing. Constructed by
    /// `MobHandle::list_members_matching` (runtime/handle.rs:2336) and
    /// matched at runtime/handle.rs:872.
    ListMembersMatching {
        filter: Box<crate::runtime::MemberFilter>,
    },
    Retire {
        agent_identity: AgentIdentity,
    },
    Respawn {
        agent_identity: AgentIdentity,
        initial_message: Option<meerkat_core::types::ContentInput>,
    },
    RetireAll,
    /// Submit a unit of work to a mob member. Fence-token freshness,
    /// work-origin legality (External vs Internal), external-addressability,
    /// live-runtime membership, and phase gates are owned by the `MobMachine`
    /// DSL — there is no shell-side branching on `spec.origin`. Boxed:
    /// `WorkSpec` already carries `ContentInput`, and
    /// adding render/handling metadata directly in the enum would widen the
    /// `MobMachineCommand` size for every other variant (every
    /// `MobHandle::execute_machine_command` call site captures this enum in
    /// a future).
    SubmitWork(Box<SubmitWorkCommand>),
    /// Cancel a previously submitted unit of work.
    CancelWork {
        work_ref: WorkRef,
    },
    /// Cancel all in-flight work for a mob member, validated by fence token.
    CancelAllWork {
        runtime_id: AgentRuntimeId,
        fence_token: FenceToken,
    },
    Stop,
    Resume,
    Complete,
    Reset,
    Destroy,
    RosterSnapshot,
    ListMembers,
    ListMembersIncludingRetiring,
    ListAllMembers,
    MemberStatus {
        agent_identity: AgentIdentity,
    },
    SubscribeAgentEvents {
        agent_identity: AgentIdentity,
    },
    SubscribeAllAgentEvents,
    SubscribeMobEvents {
        config: crate::runtime::MobEventRouterConfig,
    },
    PollEvents {
        after_cursor: u64,
        limit: usize,
    },
    ReplayAllEvents,
    RecordOperatorActionProvenance {
        tool_name: String,
        authority_context: meerkat_core::service::MobToolAuthorityContext,
    },
    GetMember {
        agent_identity: AgentIdentity,
    },
    #[cfg(test)]
    FlowTrackerCounts,
    #[cfg(test)]
    OrchestratorSnapshot,
    #[cfg(test)]
    LifecycleSnapshot,
    #[cfg(test)]
    LifecycleNotificationBurst {
        count: usize,
        message: String,
    },
    #[cfg(test)]
    DslT2Snapshot,
    SetSpawnPolicy {
        policy: Option<Arc<dyn crate::runtime::SpawnPolicy>>,
    },
    Shutdown,
    ForceCancel {
        agent_identity: AgentIdentity,
    },
    /// Wire a local member to a peer target. D-track-b (#14) lands the
    /// producer-wiring handler that authorizes and applies this command;
    /// until then the handler returns `MobError::Internal`. Carried in
    /// the command surface so the public `MobHandle::wire` method stays
    /// on the one top-level machine-command seam.
    Wire {
        local: AgentIdentity,
        target: crate::runtime::PeerTarget,
    },
    /// Materialize many local-member wiring edges through one actor command.
    ///
    /// This is shell batching for dense topology reconciliation. Each edge
    /// still lowers into the MobMachine-owned `WireMembers` input; the command
    /// only coalesces validation, actor queuing, comms side effects, and
    /// projection/event fanout.
    WireMembersBatch {
        edges: Vec<(AgentIdentity, AgentIdentity)>,
    },
    /// Unwire a local member from a peer target. Mirror of `Wire`.
    Unwire {
        local: AgentIdentity,
        target: crate::runtime::PeerTarget,
    },
}

/// Runtime-shell acknowledgement policy for an admitted SubmitWork command.
///
/// This does not decide whether the mob machine accepts the transition. It only
/// selects how long the runtime actor waits while realizing the admitted effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubmitWorkAckMode {
    /// Return once the turn has been accepted by the runtime/session ingress.
    IngressAccepted,
    /// Return after the member turn completes.
    TurnCompleted,
}

/// Payload for [`MobMachineCommand::SubmitWork`].
pub(crate) struct SubmitWorkCommand {
    pub runtime_id: AgentRuntimeId,
    pub fence_token: FenceToken,
    pub work_ref: WorkRef,
    pub spec: WorkSpec,
    pub handling_mode: meerkat_core::types::HandlingMode,
    pub render_metadata: Option<meerkat_core::types::RenderMetadata>,
    pub ack_mode: SubmitWorkAckMode,
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum MobMachineCommandResult {
    Unit,
    WireMembersBatchReport(crate::runtime::MobWireMembersBatchReport),
    RunId(RunId),
    WorkReceipt {
        work_ref: WorkRef,
    },
    FlowStatus(Option<MobRun>),
    SpawnReceipt(crate::runtime::MemberSpawnReceipt),
    /// Result for `EnsureMember`. T4a seam.
    #[allow(dead_code)]
    EnsureMember(crate::runtime::EnsureMemberOutcome),
    /// Result for `Reconcile`. Boxed to keep the enum compact. T4a seam.
    #[allow(dead_code)]
    Reconcile(Box<crate::runtime::ReconcileReport>),
    Respawn(Result<crate::MemberRespawnReceipt, crate::MobRespawnError>),
    DestroyReport(crate::runtime::MobDestroyReport),
    RosterSnapshot(Roster),
    ListMembers(Vec<MobMemberListEntry>),
    ListMembersIncludingRetiring(Vec<MobMemberListEntry>),
    ListAllMembers(Vec<RosterEntry>),
    MemberStatus(crate::runtime::MobMemberSnapshot),
    #[allow(dead_code)]
    Bool(bool),
    EventStream(meerkat_core::EventStream),
    AllAgentEventStreams(Vec<(AgentIdentity, meerkat_core::EventStream)>),
    MobEventRouter(crate::runtime::MobEventRouterHandle),
    MobEvents(Vec<crate::event::MobEvent>),
    GetMember(Option<RosterEntry>),
    #[cfg(test)]
    FlowTrackerCounts((usize, usize)),
    #[cfg(test)]
    OrchestratorSnapshot(MobOrchestratorSnapshot),
    #[cfg(test)]
    LifecycleSnapshot(MobLifecycleSnapshot),
    #[cfg(test)]
    LifecycleNotificationBurst,
    #[cfg(test)]
    DslT2Snapshot(crate::runtime::MobDslT2Snapshot),
}

#[doc(hidden)]
#[must_use]
pub fn canonical_mob_machine_command_manifest() -> IndexSet<&'static str> {
    canonical_mob_machine_command_input_variant_manifest()
        .into_iter()
        .map(|variant| variant.as_str())
        .collect()
}

#[doc(hidden)]
#[must_use]
pub fn canonical_mob_machine_command_input_variant_manifest() -> IndexSet<MobMachineInputVariant> {
    canonical_mob_machine_command_classifications()
        .into_iter()
        .flat_map(|record| record.classification.catalog_input_variants())
        .collect()
}

#[doc(hidden)]
#[must_use]
pub fn canonical_mob_machine_runtime_internal_manifest() -> IndexSet<&'static str> {
    canonical_mob_machine_runtime_internal_input_variant_manifest()
        .into_iter()
        .map(|variant| variant.as_str())
        .collect()
}

#[doc(hidden)]
#[must_use]
pub fn canonical_mob_machine_runtime_internal_input_variant_manifest()
-> IndexSet<MobMachineInputVariant> {
    canonical_mob_machine_runtime_internal_classifications()
        .iter()
        .map(|record| record.input.input_variant())
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobMachineCommandClassification {
    CatalogInput(MobMachineCatalogInput),
    CatalogInputs(&'static [MobMachineCatalogInput]),
    ShellMechanic(MobMachineShellMechanicReason),
}

impl MobMachineCommandClassification {
    #[must_use]
    pub fn catalog_inputs(self) -> Vec<MobMachineCatalogInput> {
        match self {
            Self::CatalogInput(input) => vec![input],
            Self::CatalogInputs(inputs) => inputs.to_vec(),
            Self::ShellMechanic(_) => Vec::new(),
        }
    }

    #[must_use]
    pub fn catalog_input_variants(self) -> Vec<MobMachineInputVariant> {
        self.catalog_inputs()
            .into_iter()
            .map(MobMachineCatalogInput::input_variant)
            .collect()
    }
}

impl MobMachineCommandVariant {
    #[must_use]
    pub const fn catalog_input(self) -> Option<MobMachineCatalogInput> {
        match self {
            #[cfg(test)]
            Self::FlowTrackerCounts
            | Self::OrchestratorSnapshot
            | Self::LifecycleSnapshot
            | Self::LifecycleNotificationBurst
            | Self::DslT2Snapshot
            | Self::ListMembersMatching
            | Self::Wire
            | Self::WireMembersBatch
            | Self::Unwire => None,
            #[cfg(not(test))]
            Self::ListMembersMatching | Self::Wire | Self::WireMembersBatch | Self::Unwire => None,
            Self::RunFlow => Some(MobMachineCatalogInput::RunFlow),
            Self::CancelFlow => Some(MobMachineCatalogInput::CancelFlow),
            Self::FlowStatus => Some(MobMachineCatalogInput::FlowStatus),
            Self::Spawn => Some(MobMachineCatalogInput::Spawn),
            Self::EnsureMember => Some(MobMachineCatalogInput::EnsureMember),
            Self::Reconcile => Some(MobMachineCatalogInput::Reconcile),
            Self::Retire => Some(MobMachineCatalogInput::Retire),
            Self::Respawn => Some(MobMachineCatalogInput::Respawn),
            Self::RetireAll => Some(MobMachineCatalogInput::RetireAll),
            Self::SubmitWork => Some(MobMachineCatalogInput::SubmitWork),
            Self::CancelWork => Some(MobMachineCatalogInput::CancelWork),
            Self::CancelAllWork => Some(MobMachineCatalogInput::CancelAllWork),
            Self::Stop => Some(MobMachineCatalogInput::Stop),
            Self::Resume => Some(MobMachineCatalogInput::Resume),
            Self::Complete => Some(MobMachineCatalogInput::Complete),
            Self::Reset => Some(MobMachineCatalogInput::Reset),
            Self::Destroy => Some(MobMachineCatalogInput::Destroy),
            Self::RosterSnapshot => Some(MobMachineCatalogInput::RosterSnapshot),
            Self::ListMembers => Some(MobMachineCatalogInput::ListMembers),
            Self::ListMembersIncludingRetiring => {
                Some(MobMachineCatalogInput::ListMembersIncludingRetiring)
            }
            Self::ListAllMembers => Some(MobMachineCatalogInput::ListAllMembers),
            Self::MemberStatus => Some(MobMachineCatalogInput::MemberStatus),
            Self::SubscribeAgentEvents => Some(MobMachineCatalogInput::SubscribeAgentEvents),
            Self::SubscribeAllAgentEvents => Some(MobMachineCatalogInput::SubscribeAllAgentEvents),
            Self::SubscribeMobEvents => Some(MobMachineCatalogInput::SubscribeMobEvents),
            Self::PollEvents => Some(MobMachineCatalogInput::PollEvents),
            Self::ReplayAllEvents => Some(MobMachineCatalogInput::ReplayAllEvents),
            Self::RecordOperatorActionProvenance => {
                Some(MobMachineCatalogInput::RecordOperatorActionProvenance)
            }
            Self::GetMember => Some(MobMachineCatalogInput::GetMember),
            Self::SetSpawnPolicy => Some(MobMachineCatalogInput::SetSpawnPolicy),
            Self::Shutdown => Some(MobMachineCatalogInput::Shutdown),
            Self::ForceCancel => Some(MobMachineCatalogInput::ForceCancel),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobMachineShellMechanicReason {
    TestInspection,
    FilteredRosterProjection,
    ProducerWiringBridge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobMachineRuntimeInternalReason {
    EventObservationAuthority,
    FlowProjectionAuthority,
    RuntimeRejectionFeedback,
    SurfaceResultClassificationAuthority,
    SpawnProfileAuthority,
    OperatorScopeAdmissionAuthority,
    OwnerBridgeSessionAuthority,
    SpawnPolicyFeedbackAuthority,
    SessionIngressDetachFeedback,
    SessionIngressDetachRequest,
    StartupKickoffLifecycle,
    RetireIdempotencyAuthority,
    SupervisorAuthority,
    TrustHandoffAuthority,
    CoordinationBoardAuthority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MobMachineCommandClassificationRecord {
    pub command: MobMachineCommandVariant,
    pub classification: MobMachineCommandClassification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MobMachineRuntimeInternalClassificationRecord {
    pub input: MobMachineCatalogInput,
    pub reason: MobMachineRuntimeInternalReason,
}

const MOB_MACHINE_RUNTIME_INTERNAL_CLASSIFICATIONS:
    &[MobMachineRuntimeInternalClassificationRecord] = &[
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::SubscribeStructuralEvents,
        reason: MobMachineRuntimeInternalReason::EventObservationAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::AuthorizeMobEventRouterMemberSubscription,
        reason: MobMachineRuntimeInternalReason::EventObservationAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::AuthorizeMobEventRouterMemberRemoval,
        reason: MobMachineRuntimeInternalReason::EventObservationAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::PollEventsStrict,
        reason: MobMachineRuntimeInternalReason::EventObservationAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::AuthorizeFlowFrameReducerCommand,
        reason: MobMachineRuntimeInternalReason::FlowProjectionAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::AuthorizeFlowRunReducerCommand,
        reason: MobMachineRuntimeInternalReason::FlowProjectionAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::AuthorizeLoopIterationReducerCommand,
        reason: MobMachineRuntimeInternalReason::FlowProjectionAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::CreateFrameSeed,
        reason: MobMachineRuntimeInternalReason::FlowProjectionAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::CreateLoopSeed,
        reason: MobMachineRuntimeInternalReason::FlowProjectionAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::CreateRunSeed,
        reason: MobMachineRuntimeInternalReason::FlowProjectionAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::ClassifyFlowRunTerminality,
        reason: MobMachineRuntimeInternalReason::FlowProjectionAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::ClassifyFlowStepTerminality,
        reason: MobMachineRuntimeInternalReason::FlowProjectionAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::ClassifyFlowFrameTerminalStatus,
        reason: MobMachineRuntimeInternalReason::FlowProjectionAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::ClassifyFlowRunPublicResult,
        reason: MobMachineRuntimeInternalReason::SurfaceResultClassificationAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::RecordLoopBodyFrameCompleted,
        reason: MobMachineRuntimeInternalReason::FlowProjectionAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::RecordLoopUntilConditionFailed,
        reason: MobMachineRuntimeInternalReason::FlowProjectionAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::RecordLoopUntilConditionMet,
        reason: MobMachineRuntimeInternalReason::FlowProjectionAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::ResolveSubmitWorkRejection,
        reason: MobMachineRuntimeInternalReason::RuntimeRejectionFeedback,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::ResolveCancelAllWorkRejection,
        reason: MobMachineRuntimeInternalReason::RuntimeRejectionFeedback,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::ClassifySpawnManyFailure,
        reason: MobMachineRuntimeInternalReason::SurfaceResultClassificationAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        // Flow topology edge admission is decided by MobMachine from the
        // shell's pure rule-match witness; the flow engine drives this input
        // during step execution, so it is a runtime-internal flow-projection
        // authority input (like the loop until-condition feedback inputs),
        // not a surface command.
        input: MobMachineCatalogInput::ResolveFlowDelegationEdgeAdmission,
        reason: MobMachineRuntimeInternalReason::FlowProjectionAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        // Remote-member runtime observation terminality is decided by
        // MobMachine from the bridge consumer's pure wire-state observation
        // during respawn/destroy cleanup — a runtime-internal observation
        // classification authority, not a surface command.
        input: MobMachineCatalogInput::ClassifyRemoteMemberRuntimeObservation,
        reason: MobMachineRuntimeInternalReason::SurfaceResultClassificationAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        // Spawn-member operator admission is composed by MobMachine from the
        // tool surface's pure scope/privileged-arg observations; the tool
        // surface drives this as a spawn-profile authority input, not a
        // standalone surface command.
        input: MobMachineCatalogInput::ResolveSpawnMemberAdmission,
        reason: MobMachineRuntimeInternalReason::SpawnProfileAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        // Per-mob operator admission for current-mob tools is decided by
        // MobMachine from the tool surface's pure manage-scope observation;
        // the tool surface drives this as an operator-scope admission input,
        // not a standalone surface command.
        input: MobMachineCatalogInput::ResolveCurrentMobAdmission,
        reason: MobMachineRuntimeInternalReason::OperatorScopeAdmissionAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        // Coarse spawn-tool admission for the spawn-member tool surfaces is
        // decided by MobMachine, which composes the disjunction from the tool
        // surface's TWO raw observations (can_manage_mob,
        // spawn_profile_scope_present); the tool surface drives this as an
        // operator-scope admission input, not a standalone surface command.
        input: MobMachineCatalogInput::ResolveSpawnToolAdmission,
        reason: MobMachineRuntimeInternalReason::OperatorScopeAdmissionAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        // Operator create-mob admission is decided by MobMachine from the tool
        // surface's pure create-mobs capability observation; the tool surface
        // drives this as an operator-scope admission input, not a standalone
        // surface command.
        input: MobMachineCatalogInput::ResolveCreateMobAdmission,
        reason: MobMachineRuntimeInternalReason::OperatorScopeAdmissionAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        // Operator profile-mutation admission is decided by MobMachine from the
        // tool surface's pure mutate-profiles capability observation; the tool
        // surface drives this as an operator-scope admission input, not a
        // standalone surface command.
        input: MobMachineCatalogInput::ResolveProfileMutationAdmission,
        reason: MobMachineRuntimeInternalReason::OperatorScopeAdmissionAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        // Within-mob member-operation eligibility (spawn finalization, peer
        // messaging, respawn finalization) is decided by MobMachine from its
        // own lifecycle phase plus the destroy_admitted marker — a
        // runtime-internal eligibility classification authority driven by the
        // actor, not a surface command.
        input: MobMachineCatalogInput::ClassifyMemberOperationEligibility,
        reason: MobMachineRuntimeInternalReason::SurfaceResultClassificationAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        // Bridge-rejection recovery class is decided by MobMachine from the
        // bridge consumer's pure wire rejection-cause observation during
        // supervisor (re)authorization — a runtime-internal observation
        // classification authority, not a surface command.
        input: MobMachineCatalogInput::ClassifyBridgeRejectionRecovery,
        reason: MobMachineRuntimeInternalReason::SurfaceResultClassificationAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        // Pending-supervisor-acceptance class is decided by MobMachine from the
        // actor's pure wire rejection-cause observation while re-verifying an
        // already-accepted remote peer during supervisor rotation — a
        // runtime-internal observation classification authority, not a surface
        // command.
        input: MobMachineCatalogInput::ClassifyPendingSupervisorAcceptance,
        reason: MobMachineRuntimeInternalReason::SurfaceResultClassificationAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::AuthorizeSpawnProfile,
        reason: MobMachineRuntimeInternalReason::SpawnProfileAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::BindOwnerBridgeSession,
        reason: MobMachineRuntimeInternalReason::OwnerBridgeSessionAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::ClassifyMemberWait,
        reason: MobMachineRuntimeInternalReason::SpawnProfileAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::ResolveSpawnPolicy,
        reason: MobMachineRuntimeInternalReason::SpawnPolicyFeedbackAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::RetireAbsent,
        reason: MobMachineRuntimeInternalReason::RetireIdempotencyAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::RequestPendingSessionIngressDetachForMobDestroy,
        reason: MobMachineRuntimeInternalReason::SessionIngressDetachRequest,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::SessionIngressDetachedForMobDestroy,
        reason: MobMachineRuntimeInternalReason::SessionIngressDetachFeedback,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::SessionIngressDetachFailedForMobDestroy,
        reason: MobMachineRuntimeInternalReason::SessionIngressDetachFeedback,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::KickoffMarkPending,
        reason: MobMachineRuntimeInternalReason::StartupKickoffLifecycle,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::KickoffMarkStarting,
        reason: MobMachineRuntimeInternalReason::StartupKickoffLifecycle,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::StartupMarkReady,
        reason: MobMachineRuntimeInternalReason::StartupKickoffLifecycle,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::KickoffResolveStarted,
        reason: MobMachineRuntimeInternalReason::StartupKickoffLifecycle,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::KickoffResolveCallbackPending,
        reason: MobMachineRuntimeInternalReason::StartupKickoffLifecycle,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::KickoffResolveFailed,
        reason: MobMachineRuntimeInternalReason::StartupKickoffLifecycle,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::KickoffCancelRequested,
        reason: MobMachineRuntimeInternalReason::StartupKickoffLifecycle,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::KickoffClear,
        reason: MobMachineRuntimeInternalReason::StartupKickoffLifecycle,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::RegisterMemberPeer,
        reason: MobMachineRuntimeInternalReason::TrustHandoffAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::AuthorizeMemberPeerRebind,
        reason: MobMachineRuntimeInternalReason::TrustHandoffAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::AuthorizeMemberPeerOverlay,
        reason: MobMachineRuntimeInternalReason::TrustHandoffAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::AuthorizeMemberTrustWiring,
        reason: MobMachineRuntimeInternalReason::TrustHandoffAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::AuthorizeMemberTrustUnwiring,
        reason: MobMachineRuntimeInternalReason::TrustHandoffAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::AuthorizeMemberTrustCleanup,
        reason: MobMachineRuntimeInternalReason::TrustHandoffAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::AuthorizeMemberTrustCleanupObserved,
        reason: MobMachineRuntimeInternalReason::TrustHandoffAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::AuthorizeExternalPeerReciprocalTrust,
        reason: MobMachineRuntimeInternalReason::TrustHandoffAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::ProvisionSupervisorAuthority,
        reason: MobMachineRuntimeInternalReason::SupervisorAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::ClearSupervisorPendingRotation,
        reason: MobMachineRuntimeInternalReason::SupervisorAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::RecordSupervisorPendingRotation,
        reason: MobMachineRuntimeInternalReason::SupervisorAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::CommitSupervisorRotation,
        reason: MobMachineRuntimeInternalReason::SupervisorAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::ClearSupervisorAuthorityForDestroy,
        reason: MobMachineRuntimeInternalReason::SupervisorAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::RestoreSupervisorAuthorityAfterDestroyRollback,
        reason: MobMachineRuntimeInternalReason::SupervisorAuthority,
    },
    // Dogma row R044: machine-owned trust-install-before-authorization-
    // terminality obligation window for supervisor-bridge recipients.
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::RecordPendingRecipientTrust,
        reason: MobMachineRuntimeInternalReason::TrustHandoffAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::ResolvePendingRecipientTrust,
        reason: MobMachineRuntimeInternalReason::TrustHandoffAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::RollbackPendingRecipientTrust,
        reason: MobMachineRuntimeInternalReason::TrustHandoffAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::RecordCoordinationWorkIntent,
        reason: MobMachineRuntimeInternalReason::CoordinationBoardAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::RecordCoordinationResourceClaim,
        reason: MobMachineRuntimeInternalReason::CoordinationBoardAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::UpdateCoordinationWorkIntentStatus,
        reason: MobMachineRuntimeInternalReason::CoordinationBoardAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::UpdateCoordinationResourceClaimStatus,
        reason: MobMachineRuntimeInternalReason::CoordinationBoardAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        input: MobMachineCatalogInput::ObserveCoordinationResourceClaimOverlap,
        reason: MobMachineRuntimeInternalReason::CoordinationBoardAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        // Duplicate-member admission probe is decided by MobMachine from its own
        // roster/binding/pending-spawn authority; the actor drives it as a
        // read-only spawn-profile authority input before staging a spawn.
        input: MobMachineCatalogInput::ProbeMemberAdmission,
        reason: MobMachineRuntimeInternalReason::SpawnProfileAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        // Respawn replacement generation is computed by MobMachine from the
        // machine-owned per-identity generation counter; the actor drives this
        // read-only spawn-profile authority input during respawn handling.
        input: MobMachineCatalogInput::ComputeRespawnGeneration,
        reason: MobMachineRuntimeInternalReason::SpawnProfileAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        // Step-output fault retry-vs-terminal disposition is decided by
        // MobMachine from attempt/max-retries state; the flow engine drives this
        // runtime-internal flow-projection authority input during step execution.
        input: MobMachineCatalogInput::ClassifyStepOutputFault,
        reason: MobMachineRuntimeInternalReason::FlowProjectionAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        // Supervisor escalation request (with an eligible target) is owned by
        // MobMachine; the supervisor flow drives this as a supervisor-authority
        // input from its pure eligible-candidate projection.
        input: MobMachineCatalogInput::EscalateToSupervisor,
        reason: MobMachineRuntimeInternalReason::SupervisorAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        // No-eligible-target supervisor escalation is owned by MobMachine, which
        // emits the typed failure; the supervisor flow drives this as a
        // supervisor-authority input when no eligible candidate exists.
        input: MobMachineCatalogInput::EscalateToSupervisorNoEligibleTarget,
        reason: MobMachineRuntimeInternalReason::SupervisorAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        // Topology edge verdict is decided by MobMachine, which applies the
        // default-policy fallback to the shell's pure rule-match witness; the
        // flow engine drives this runtime-internal flow-projection authority
        // input during delegation-edge resolution.
        input: MobMachineCatalogInput::EvaluateTopologyEdge,
        reason: MobMachineRuntimeInternalReason::FlowProjectionAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        // External-member rebind capability is recorded by MobMachine at spawn
        // mint / recovery / peer-only-resume-rebind; the actor/builder drives
        // this read-only spawn-profile authority input so the external-member
        // projection reads machine state instead of deriving from a token.
        input: MobMachineCatalogInput::SetExternalMemberRebindCapability,
        reason: MobMachineRuntimeInternalReason::SpawnProfileAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        // Hard turn-timeout detach-vs-cancel disposition is decided by MobMachine
        // from the machine-owned orphan budget; the turn executor drives this
        // runtime-internal flow-projection authority input on timeout.
        input: MobMachineCatalogInput::ClassifyTurnTimeoutDisposition,
        reason: MobMachineRuntimeInternalReason::FlowProjectionAuthority,
    },
    MobMachineRuntimeInternalClassificationRecord {
        // Orphan budget is machine state seeded once at build time from the
        // definition limits; the builder drives this read-only spawn-profile
        // authority input to seed the budget before any timeout classification.
        input: MobMachineCatalogInput::SeedOrphanBudget,
        reason: MobMachineRuntimeInternalReason::SpawnProfileAuthority,
    },
];

#[doc(hidden)]
#[must_use]
pub fn canonical_mob_machine_command_classifications() -> Vec<MobMachineCommandClassificationRecord>
{
    MobMachineCommand::command_variant_manifest()
        .iter()
        .copied()
        .map(|variant| MobMachineCommandClassificationRecord {
            command: variant,
            classification: mob_machine_command_classification(variant),
        })
        .collect()
}

#[doc(hidden)]
#[must_use]
pub const fn canonical_mob_machine_runtime_internal_classifications()
-> &'static [MobMachineRuntimeInternalClassificationRecord] {
    MOB_MACHINE_RUNTIME_INTERNAL_CLASSIFICATIONS
}

const fn mob_machine_command_classification(
    variant: MobMachineCommandVariant,
) -> MobMachineCommandClassification {
    match variant {
        #[cfg(test)]
        MobMachineCommandVariant::FlowTrackerCounts
        | MobMachineCommandVariant::OrchestratorSnapshot
        | MobMachineCommandVariant::LifecycleSnapshot
        | MobMachineCommandVariant::LifecycleNotificationBurst
        | MobMachineCommandVariant::DslT2Snapshot => {
            MobMachineCommandClassification::ShellMechanic(
                MobMachineShellMechanicReason::TestInspection,
            )
        }
        MobMachineCommandVariant::ListMembersMatching => {
            MobMachineCommandClassification::ShellMechanic(
                MobMachineShellMechanicReason::FilteredRosterProjection,
            )
        }
        MobMachineCommandVariant::Wire => MobMachineCommandClassification::CatalogInputs(&[
            MobMachineCatalogInput::WireMembers,
            MobMachineCatalogInput::WireExternalPeer,
        ]),
        MobMachineCommandVariant::WireMembersBatch => {
            MobMachineCommandClassification::CatalogInput(
                MobMachineCatalogInput::WireMembersWithTrust,
            )
        }
        MobMachineCommandVariant::Unwire => MobMachineCommandClassification::CatalogInputs(&[
            MobMachineCatalogInput::UnwireMembers,
            MobMachineCatalogInput::UnwireExternalPeer,
        ]),
        MobMachineCommandVariant::RunFlow => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::RunFlow)
        }
        MobMachineCommandVariant::CancelFlow => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::CancelFlow)
        }
        MobMachineCommandVariant::FlowStatus => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::FlowStatus)
        }
        MobMachineCommandVariant::Spawn => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::Spawn)
        }
        MobMachineCommandVariant::EnsureMember => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::EnsureMember)
        }
        MobMachineCommandVariant::Reconcile => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::Reconcile)
        }
        MobMachineCommandVariant::Retire => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::Retire)
        }
        MobMachineCommandVariant::Respawn => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::Respawn)
        }
        MobMachineCommandVariant::RetireAll => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::RetireAll)
        }
        MobMachineCommandVariant::SubmitWork => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::SubmitWork)
        }
        MobMachineCommandVariant::CancelWork => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::CancelWork)
        }
        MobMachineCommandVariant::CancelAllWork => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::CancelAllWork)
        }
        MobMachineCommandVariant::Stop => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::Stop)
        }
        MobMachineCommandVariant::Resume => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::Resume)
        }
        MobMachineCommandVariant::Complete => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::Complete)
        }
        MobMachineCommandVariant::Reset => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::Reset)
        }
        MobMachineCommandVariant::Destroy => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::Destroy)
        }
        MobMachineCommandVariant::RosterSnapshot => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::RosterSnapshot)
        }
        MobMachineCommandVariant::ListMembers => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::ListMembers)
        }
        MobMachineCommandVariant::ListMembersIncludingRetiring => {
            MobMachineCommandClassification::CatalogInput(
                MobMachineCatalogInput::ListMembersIncludingRetiring,
            )
        }
        MobMachineCommandVariant::ListAllMembers => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::ListAllMembers)
        }
        MobMachineCommandVariant::MemberStatus => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::MemberStatus)
        }
        MobMachineCommandVariant::SubscribeAgentEvents => {
            MobMachineCommandClassification::CatalogInput(
                MobMachineCatalogInput::SubscribeAgentEvents,
            )
        }
        MobMachineCommandVariant::SubscribeAllAgentEvents => {
            MobMachineCommandClassification::CatalogInput(
                MobMachineCatalogInput::SubscribeAllAgentEvents,
            )
        }
        MobMachineCommandVariant::SubscribeMobEvents => {
            MobMachineCommandClassification::CatalogInput(
                MobMachineCatalogInput::SubscribeMobEvents,
            )
        }
        MobMachineCommandVariant::PollEvents => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::PollEvents)
        }
        MobMachineCommandVariant::ReplayAllEvents => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::ReplayAllEvents)
        }
        MobMachineCommandVariant::RecordOperatorActionProvenance => {
            MobMachineCommandClassification::CatalogInput(
                MobMachineCatalogInput::RecordOperatorActionProvenance,
            )
        }
        MobMachineCommandVariant::GetMember => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::GetMember)
        }
        MobMachineCommandVariant::SetSpawnPolicy => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::SetSpawnPolicy)
        }
        MobMachineCommandVariant::Shutdown => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::Shutdown)
        }
        MobMachineCommandVariant::ForceCancel => {
            MobMachineCommandClassification::CatalogInput(MobMachineCatalogInput::ForceCancel)
        }
    }
}
