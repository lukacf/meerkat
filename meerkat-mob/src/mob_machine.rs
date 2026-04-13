//! Diagnostic snapshot and command facade for the current MobMachine boundary.
//!
//! The cutover removes the old shadow/taxonomy/validator regime from this
//! facade. The runtime actor owns Mob authority; this module keeps the
//! top-level command/result surface plus the durable diagnostic snapshot shapes
//! that remain useful for inspection and follow-up work.

use crate::definition::DependencyMode;
use crate::ids::{BranchId, FlowId, MeerkatId, RunId, StepId};
use crate::roster::{Roster, RosterEntry};
use crate::run::{MobRun, MobRunStatus, RunCollectionPolicyKind, StepRunStatus};
#[cfg(test)]
use crate::runtime::MobOrchestratorSnapshot;
use crate::runtime::{MobHandle, MobKernelDiagnosticSnapshot, MobMemberListEntry, MobState};
use crate::tasks::MobTask;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Joined diagnostic view over the current mob lifecycle state plus the live
/// roster and member-status projection.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct MobMachineSnapshot {
    pub phase: MobState,
    pub kernel: MobKernelDiagnosticSnapshot,
    pub roster: Roster,
    pub members: Vec<MobMemberListEntry>,
    pub restore_failures: BTreeMap<MeerkatId, RestoreFailureSnapshot>,
    pub tracked_runs: BTreeMap<RunId, TrackedRunSnapshot>,
}

/// Public Mob mutations route through this single top-level machine command
/// surface instead of each `MobHandle` method hand-sending actor commands.
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
        owner: Option<MeerkatId>,
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
    RestoreFailuresSnapshot,
    GetMember {
        meerkat_id: MeerkatId,
    },
    #[cfg(test)]
    FlowTrackerCounts,
    #[cfg(test)]
    OrchestratorSnapshot,
    DiagnosticKernelSnapshot,
    KickoffBarrierSnapshot {
        meerkat_ids: Vec<MeerkatId>,
    },
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
    RestoreFailuresSnapshot(BTreeMap<MeerkatId, crate::runtime::RestoreFailureDiagnostic>),
    GetMember(Option<RosterEntry>),
    #[cfg(test)]
    FlowTrackerCounts((usize, usize)),
    #[cfg(test)]
    OrchestratorSnapshot(MobOrchestratorSnapshot),
    DiagnosticKernelSnapshot(MobKernelDiagnosticSnapshot),
    KickoffBarrierSnapshot(Vec<(MeerkatId, tokio::sync::watch::Receiver<bool>)>),
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestoreFailureSnapshot {
    pub session_id: meerkat_core::types::SessionId,
    pub bridge_session_id: meerkat_core::types::SessionId,
    pub reason: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrackedRunStoreSnapshot {
    pub flow_id: FlowId,
    pub schema_version: u32,
    pub status: MobRunStatus,
    pub completed_at_present: bool,
    pub frame_count: usize,
    pub loop_count: usize,
    pub loop_iteration_count: usize,
    pub ordered_steps: Vec<StepId>,
    pub step_dependencies: BTreeMap<StepId, Vec<StepId>>,
    pub step_dependency_modes: BTreeMap<StepId, DependencyMode>,
    pub step_has_conditions: BTreeMap<StepId, bool>,
    pub step_branches: BTreeMap<StepId, Option<BranchId>>,
    pub step_collection_policy_kinds: BTreeMap<StepId, RunCollectionPolicyKind>,
    pub step_quorum_thresholds: BTreeMap<StepId, u32>,
    pub step_statuses: BTreeMap<StepId, StepRunStatus>,
    pub failure_count: u32,
    pub consecutive_failure_count: u32,
    pub max_step_retries: u32,
    pub escalation_threshold: u32,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrackedRunMalformedSnapshot {
    pub flow_id: FlowId,
    pub status: MobRunStatus,
    pub completed_at_present: bool,
    pub reason: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrackedRunSnapshot {
    Missing,
    Present(TrackedRunStoreSnapshot),
    Malformed(TrackedRunMalformedSnapshot),
}

#[allow(dead_code)]
fn tracked_run_snapshot_from_run(run: &MobRun) -> Result<TrackedRunStoreSnapshot, crate::MobError> {
    Ok(TrackedRunStoreSnapshot {
        flow_id: run.flow_id.clone(),
        schema_version: run.schema_version,
        status: run.status.clone(),
        completed_at_present: run.completed_at.is_some(),
        frame_count: run.frames.len(),
        loop_count: run.loops.len(),
        loop_iteration_count: run.loop_iteration_ledger.len(),
        ordered_steps: run.ordered_steps()?,
        step_dependencies: run.step_dependencies()?,
        step_dependency_modes: run.step_dependency_modes()?,
        step_has_conditions: run.step_has_conditions()?,
        step_branches: run.step_branches()?,
        step_collection_policy_kinds: run.step_collection_policy_kinds()?,
        step_quorum_thresholds: run.step_quorum_thresholds()?,
        step_statuses: run.step_status_snapshot()?,
        failure_count: run.failure_count()?,
        consecutive_failure_count: run.consecutive_failure_count()?,
        max_step_retries: run.max_step_retries()?,
        escalation_threshold: run.escalation_threshold()?,
    })
}

/// Capture the currently joined MobMachine snapshot from the real handle read
/// paths.
#[allow(dead_code)]
pub(crate) async fn capture_mob_machine_snapshot(handle: &MobHandle) -> MobMachineSnapshot {
    let phase = handle.status();
    let kernel = handle
        .diagnostic_kernel_snapshot()
        .await
        .expect("MobMachine diagnostic kernel snapshot should be available");
    let roster = handle.roster().await;
    let members = handle.list_members_including_retiring().await;
    let restore_failures = handle
        .diagnostic_restore_failures_snapshot()
        .await
        .iter()
        .map(|(meerkat_id, diag)| {
            (
                meerkat_id.clone(),
                RestoreFailureSnapshot {
                    session_id: diag.bridge_session_id.clone(),
                    bridge_session_id: diag.bridge_session_id.clone(),
                    reason: diag.reason.clone(),
                },
            )
        })
        .collect();
    let mut tracked_run_ids = kernel.flow_trackers.run_task_ids.clone();
    tracked_run_ids.extend(kernel.flow_trackers.cancel_token_ids.iter().cloned());
    tracked_run_ids.extend(kernel.flow_trackers.stream_ids.iter().cloned());
    let mut tracked_runs = BTreeMap::new();
    for run_id in tracked_run_ids {
        let projection = handle
            .flow_status(run_id.clone())
            .await
            .expect("MobMachine tracked run projection should be queryable");
        let projection = match projection {
            None => TrackedRunSnapshot::Missing,
            Some(run) => match tracked_run_snapshot_from_run(&run) {
                Ok(snapshot) => TrackedRunSnapshot::Present(snapshot),
                Err(error) => TrackedRunSnapshot::Malformed(TrackedRunMalformedSnapshot {
                    flow_id: run.flow_id,
                    status: run.status,
                    completed_at_present: run.completed_at.is_some(),
                    reason: error.to_string(),
                }),
            },
        };
        tracked_runs.insert(run_id, projection);
    }
    MobMachineSnapshot {
        phase,
        kernel,
        roster,
        members,
        restore_failures,
        tracked_runs,
    }
}
