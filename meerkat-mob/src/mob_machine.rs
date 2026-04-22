//! Diagnostic snapshot and command facade for the Mob runtime surface.
//!
//! The runtime actor owns Mob authority; this module keeps the top-level
//! command/result surface plus the durable diagnostic snapshot shapes that
//! remain useful for inspection and follow-up work.

use crate::ids::{
    AgentIdentity, AgentRuntimeId, FenceToken, FlowId, MeerkatId, RunId, WorkRef, WorkSpec,
};
use crate::roster::{Roster, RosterEntry};
use crate::run::MobRun;
#[cfg(test)]
use crate::runtime::MobLifecycleSnapshot;
use crate::runtime::MobMemberListEntry;
#[cfg(test)]
use crate::runtime::MobOrchestratorSnapshot;
use crate::tasks::MobTask;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use indexmap::IndexSet;
use meerkat_machine_derive::CommandManifest;
use std::collections::BTreeMap;
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
        owner_context: Option<crate::runtime::CanonicalOpsOwnerContext>,
    },
    /// Declarative spawn-if-absent. T4a seam; T5d wires the actor handler.
    #[allow(dead_code)]
    EnsureMember {
        spec: Box<crate::runtime::SpawnMemberSpec>,
    },
    /// Declarative drive-toward-desired roster. T4a seam; T5d wires the actor handler.
    #[allow(dead_code)]
    Reconcile {
        desired: Vec<crate::runtime::SpawnMemberSpec>,
        options: crate::runtime::ReconcileOptions,
    },
    /// Filtered roster listing. T4a seam; T5d wires the actor handler.
    #[allow(dead_code)]
    ListMembersMatching {
        filter: Box<crate::runtime::MemberFilter>,
    },
    Retire {
        agent_identity: MeerkatId,
    },
    Respawn {
        agent_identity: MeerkatId,
        initial_message: Option<meerkat_core::types::ContentInput>,
    },
    RetireAll,
    Wire {
        local: MeerkatId,
        target: crate::PeerTarget,
    },
    Unwire {
        local: MeerkatId,
        target: crate::PeerTarget,
    },
    /// Submit a unit of work to a mob member. Fence-token freshness is
    /// validated in the actor; work-origin legality (External vs Internal,
    /// external-addressability, live-runtime membership, phase gates) is
    /// owned by the `MobMachine` DSL — there is no shell-side branching on
    /// `spec.origin`. Boxed: `WorkSpec` already carries `ContentInput`, and
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
    TaskCreate {
        subject: String,
        description: String,
        blocked_by: Vec<crate::ids::TaskId>,
    },
    TaskUpdate {
        task_id: crate::ids::TaskId,
        status: crate::tasks::TaskStatus,
        owner: Option<AgentIdentity>,
    },
    TaskList,
    TaskGet {
        task_id: crate::ids::TaskId,
    },
    McpServerStates,
    RosterSnapshot,
    ListMembers,
    ListMembersIncludingRetiring,
    ListAllMembers,
    MemberStatus {
        agent_identity: MeerkatId,
    },
    SubscribeAgentEvents {
        agent_identity: MeerkatId,
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
        agent_identity: MeerkatId,
    },
    #[cfg(test)]
    FlowTrackerCounts,
    #[cfg(test)]
    OrchestratorSnapshot,
    #[cfg(test)]
    LifecycleSnapshot,
    #[cfg(test)]
    DslT2Snapshot,
    SetSpawnPolicy {
        policy: Option<Arc<dyn crate::runtime::SpawnPolicy>>,
    },
    Shutdown,
    ForceCancel {
        agent_identity: MeerkatId,
    },
}

/// Payload for [`MobMachineCommand::SubmitWork`].
pub(crate) struct SubmitWorkCommand {
    pub runtime_id: AgentRuntimeId,
    pub fence_token: FenceToken,
    pub work_ref: WorkRef,
    pub spec: WorkSpec,
    pub handling_mode: meerkat_core::types::HandlingMode,
    pub render_metadata: Option<meerkat_core::types::RenderMetadata>,
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum MobMachineCommandResult {
    Unit,
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
    TaskId(crate::ids::TaskId),
    TaskList(Vec<MobTask>),
    TaskGet(Option<MobTask>),
    McpServerStates(BTreeMap<String, bool>),
    RosterSnapshot(Roster),
    ListMembers(Vec<MobMemberListEntry>),
    ListMembersIncludingRetiring(Vec<MobMemberListEntry>),
    ListAllMembers(Vec<RosterEntry>),
    MemberStatus(crate::runtime::MobMemberSnapshot),
    #[allow(dead_code)]
    Bool(bool),
    EventStream(meerkat_core::EventStream),
    AllAgentEventStreams(Vec<(MeerkatId, meerkat_core::EventStream)>),
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
    DslT2Snapshot(crate::runtime::MobDslT2Snapshot),
}

#[doc(hidden)]
#[must_use]
pub fn canonical_mob_machine_command_manifest() -> IndexSet<&'static str> {
    let mut variants: IndexSet<&'static str> = MobMachineCommand::command_manifest()
        .iter()
        .copied()
        .collect();
    for excluded in [
        "FlowTrackerCounts",
        "OrchestratorSnapshot",
        "LifecycleSnapshot",
    ] {
        variants.shift_remove(excluded);
    }
    variants
}
