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
    Retire {
        meerkat_id: MeerkatId,
    },
    Respawn {
        meerkat_id: MeerkatId,
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
    ExternalTurn {
        meerkat_id: MeerkatId,
        content: meerkat_core::types::ContentInput,
        handling_mode: meerkat_core::types::HandlingMode,
        render_metadata: Option<meerkat_core::types::RenderMetadata>,
    },
    InternalTurn {
        meerkat_id: MeerkatId,
        content: meerkat_core::types::ContentInput,
    },
    /// Submit a unit of work to a mob member, validated by fence token.
    SubmitWork {
        runtime_id: AgentRuntimeId,
        fence_token: FenceToken,
        work_ref: WorkRef,
        spec: WorkSpec,
    },
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
        meerkat_id: MeerkatId,
    },
    SubscribeAgentEvents {
        meerkat_id: MeerkatId,
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
        meerkat_id: MeerkatId,
    },
    #[cfg(test)]
    FlowTrackerCounts,
    #[cfg(test)]
    OrchestratorSnapshot,
    #[cfg(test)]
    LifecycleSnapshot,
    SetSpawnPolicy {
        policy: Option<Arc<dyn crate::runtime::SpawnPolicy>>,
    },
    Shutdown,
    ForceCancel {
        meerkat_id: MeerkatId,
    },
}

pub(crate) enum MobMachineCommandResult {
    Unit,
    RunId(RunId),
    WorkReceipt {
        work_ref: WorkRef,
    },
    FlowStatus(Option<MobRun>),
    SpawnReceipt(crate::runtime::MemberSpawnReceipt),
    Respawn(Result<crate::MemberRespawnReceipt, crate::MobRespawnError>),
    BridgeSessionId(meerkat_core::types::SessionId),
    TaskId(crate::ids::TaskId),
    TaskList(Vec<MobTask>),
    TaskGet(Option<MobTask>),
    McpServerStates(BTreeMap<String, bool>),
    RosterSnapshot(Roster),
    ListMembers(Vec<MobMemberListEntry>),
    ListMembersIncludingRetiring(Vec<MobMemberListEntry>),
    ListAllMembers(Vec<RosterEntry>),
    MemberStatus(crate::runtime::MobMemberSnapshot),
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
