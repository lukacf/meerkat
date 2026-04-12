//! Internal integration helpers for the evolving MobMachine boundary.
//!
//! The current refactor work keeps lifecycle, roster, and runtime-derived
//! member status in their real owners. This module composes those read paths
//! without inventing a new canonical authority.

use crate::definition::DependencyMode;
use crate::ids::{BranchId, FlowId, MeerkatId, RunId, StepId};
use crate::roster::{MemberState, Roster, RosterEntry};
use crate::run::{MobRun, MobRunStatus, RunCollectionPolicyKind, StepRunStatus};
use crate::runtime::{
    MobHandle, MobKernelDiagnosticSnapshot, MobMemberListEntry, MobMemberStatus,
    MobOrchestratorSnapshot, MobState,
};
use meerkat_runtime::{MeerkatMachineSpineSnapshot, RuntimeState};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Joined diagnostic view over the current mob lifecycle state plus the live
/// roster and member-status projection.
#[derive(Debug, Clone)]
pub(crate) struct MobMachineSnapshot {
    pub phase: MobState,
    pub kernel: MobKernelDiagnosticSnapshot,
    pub roster: Roster,
    pub members: Vec<MobMemberListEntry>,
    pub restore_failures: BTreeMap<MeerkatId, RestoreFailureSnapshot>,
    pub tracked_runs: BTreeMap<RunId, TrackedRunSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum MobShadowLane {
    ProvisioningLifecycle,
    FlowFrameLoop,
    TaskHistoryRecovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[allow(dead_code)]
pub(crate) enum ShadowMismatchTriage {
    ImplementationDetail,
    SemanticGap,
    DogmaViolation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MobShadowMismatch {
    pub lane: MobShadowLane,
    pub region: &'static str,
    pub entity_id: Option<String>,
    pub expected_summary: String,
    pub observed_summary: String,
    pub triage: ShadowMismatchTriage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MobShadowReport {
    pub lane: MobShadowLane,
    pub mismatches: Vec<MobShadowMismatch>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum CompositionShadowLane {
    LifecycleSupersession,
    WorkBridge,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompositionShadowMismatch {
    pub lane: CompositionShadowLane,
    pub region: &'static str,
    pub entity_id: Option<String>,
    pub expected_summary: String,
    pub observed_summary: String,
    pub triage: ShadowMismatchTriage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompositionShadowReport {
    pub lane: CompositionShadowLane,
    pub mismatches: Vec<CompositionShadowMismatch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MobShadowSuiteReport {
    pub mob: Vec<MobShadowReport>,
    pub composition: Vec<CompositionShadowReport>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MobShadowTaxonomyBucket {
    pub scope: &'static str,
    pub lane: String,
    pub region: &'static str,
    pub triage: ShadowMismatchTriage,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ShadowTaxonomySinkBucket {
    pub scope: &'static str,
    pub lane: String,
    pub region: &'static str,
    pub triage: &'static str,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TwoKernelShadowScenarioSample {
    pub scenario_id: String,
    pub phase: String,
    pub timestamp: String,
    pub meerkat_buckets: Vec<ShadowTaxonomySinkBucket>,
    pub mob_buckets: Vec<ShadowTaxonomySinkBucket>,
    pub seam_buckets: Vec<ShadowTaxonomySinkBucket>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TwoKernelShadowScenarioBatch {
    pub run_id: String,
    pub samples: Vec<TwoKernelShadowScenarioSample>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TwoKernelShadowRunBatch {
    pub run_id: String,
    pub scenarios: Vec<TwoKernelShadowScenarioBatch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TwoKernelShadowReportSession {
    pub session_id: String,
    pub runs: Vec<TwoKernelShadowRunBatch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TwoKernelShadowReportSessionSummary {
    pub session_id: String,
    pub run_count: u64,
    pub scenario_count: u64,
    pub sample_count: u64,
    pub green_sample_count: u64,
    pub mismatch_sample_count: u64,
    pub meerkat_bucket_count: u64,
    pub mob_bucket_count: u64,
    pub seam_bucket_count: u64,
}

fn shadow_mismatch_triage_label(triage: ShadowMismatchTriage) -> &'static str {
    match triage {
        ShadowMismatchTriage::ImplementationDetail => "implementation_detail",
        ShadowMismatchTriage::SemanticGap => "semantic_gap",
        ShadowMismatchTriage::DogmaViolation => "dogma_violation",
    }
}

fn sink_bucket_from_taxonomy_bucket(bucket: &MobShadowTaxonomyBucket) -> ShadowTaxonomySinkBucket {
    ShadowTaxonomySinkBucket {
        scope: bucket.scope,
        lane: bucket.lane.clone(),
        region: bucket.region,
        triage: shadow_mismatch_triage_label(bucket.triage),
        count: bucket.count as u64,
    }
}

pub(crate) fn export_mob_shadow_scenario_sample(
    scenario_id: impl Into<String>,
    phase: impl Into<String>,
    timestamp: impl Into<String>,
    taxonomy: &[MobShadowTaxonomyBucket],
) -> TwoKernelShadowScenarioSample {
    let mut mob_buckets = Vec::new();
    let mut seam_buckets = Vec::new();

    for bucket in taxonomy {
        let sink_bucket = sink_bucket_from_taxonomy_bucket(bucket);
        match bucket.scope {
            "mob" => mob_buckets.push(sink_bucket),
            "composition" => seam_buckets.push(sink_bucket),
            _ => {}
        }
    }

    TwoKernelShadowScenarioSample {
        scenario_id: scenario_id.into(),
        phase: phase.into(),
        timestamp: timestamp.into(),
        meerkat_buckets: Vec::new(),
        mob_buckets,
        seam_buckets,
    }
}

pub(crate) fn merge_two_kernel_shadow_scenario_samples(
    meerkat: Option<meerkat::MeerkatShadowScenarioSample>,
    mob: TwoKernelShadowScenarioSample,
) -> TwoKernelShadowScenarioSample {
    if let Some(meerkat) = meerkat {
        assert_eq!(
            meerkat.scenario_id, mob.scenario_id,
            "paired Meerkat and Mob shadow samples must agree on scenario_id"
        );
        assert_eq!(
            meerkat.phase, mob.phase,
            "paired Meerkat and Mob shadow samples must agree on phase"
        );
        assert_eq!(
            meerkat.timestamp, mob.timestamp,
            "paired Meerkat and Mob shadow samples must agree on timestamp"
        );
        TwoKernelShadowScenarioSample {
            scenario_id: mob.scenario_id,
            phase: mob.phase,
            timestamp: mob.timestamp,
            meerkat_buckets: meerkat
                .meerkat_buckets
                .into_iter()
                .map(|bucket| ShadowTaxonomySinkBucket {
                    scope: bucket.scope,
                    lane: bucket.lane,
                    region: bucket.region,
                    triage: bucket.triage,
                    count: bucket.count,
                })
                .collect(),
            mob_buckets: mob.mob_buckets,
            seam_buckets: mob.seam_buckets,
        }
    } else {
        mob
    }
}

pub(crate) fn export_two_kernel_shadow_batch(
    run_id: impl Into<String>,
    samples: impl IntoIterator<Item = TwoKernelShadowScenarioSample>,
) -> TwoKernelShadowScenarioBatch {
    TwoKernelShadowScenarioBatch {
        run_id: run_id.into(),
        samples: samples.into_iter().collect(),
    }
}

pub(crate) fn export_two_kernel_shadow_run_batch(
    run_id: impl Into<String>,
    scenarios: impl IntoIterator<Item = TwoKernelShadowScenarioBatch>,
) -> TwoKernelShadowRunBatch {
    TwoKernelShadowRunBatch {
        run_id: run_id.into(),
        scenarios: scenarios.into_iter().collect(),
    }
}

pub(crate) fn export_two_kernel_shadow_report_session(
    session_id: impl Into<String>,
    runs: impl IntoIterator<Item = TwoKernelShadowRunBatch>,
) -> TwoKernelShadowReportSession {
    TwoKernelShadowReportSession {
        session_id: session_id.into(),
        runs: runs.into_iter().collect(),
    }
}

pub(crate) fn export_two_kernel_shadow_report_session_pretty_json(
    session: &TwoKernelShadowReportSession,
) -> serde_json::Result<String> {
    serde_json::to_string_pretty(session)
}

pub(crate) fn summarize_two_kernel_shadow_report_session(
    session: &TwoKernelShadowReportSession,
) -> TwoKernelShadowReportSessionSummary {
    let mut scenario_count = 0_u64;
    let mut sample_count = 0_u64;
    let mut green_sample_count = 0_u64;
    let mut mismatch_sample_count = 0_u64;
    let mut meerkat_bucket_count = 0_u64;
    let mut mob_bucket_count = 0_u64;
    let mut seam_bucket_count = 0_u64;

    for run in &session.runs {
        scenario_count += run.scenarios.len() as u64;
        for scenario in &run.scenarios {
            sample_count += scenario.samples.len() as u64;
            for sample in &scenario.samples {
                let bucket_total = sample.meerkat_buckets.len()
                    + sample.mob_buckets.len()
                    + sample.seam_buckets.len();
                if bucket_total == 0 {
                    green_sample_count += 1;
                } else {
                    mismatch_sample_count += 1;
                }
                meerkat_bucket_count += sample.meerkat_buckets.len() as u64;
                mob_bucket_count += sample.mob_buckets.len() as u64;
                seam_bucket_count += sample.seam_buckets.len() as u64;
            }
        }
    }

    TwoKernelShadowReportSessionSummary {
        session_id: session.session_id.clone(),
        run_count: session.runs.len() as u64,
        scenario_count,
        sample_count,
        green_sample_count,
        mismatch_sample_count,
        meerkat_bucket_count,
        mob_bucket_count,
        seam_bucket_count,
    }
}

pub(crate) fn summarize_mob_shadow_taxonomy_reports(
    reports: &[MobShadowSuiteReport],
) -> Vec<MobShadowTaxonomyBucket> {
    let mut counts: HashMap<(&'static str, String, &'static str, ShadowMismatchTriage), usize> =
        HashMap::new();

    for report in reports {
        for lane_report in &report.mob {
            for mismatch in &lane_report.mismatches {
                *counts
                    .entry((
                        "mob",
                        format!("{:?}", mismatch.lane),
                        mismatch.region,
                        mismatch.triage,
                    ))
                    .or_insert(0) += 1;
            }
        }
        for lane_report in &report.composition {
            for mismatch in &lane_report.mismatches {
                *counts
                    .entry((
                        "composition",
                        format!("{:?}", mismatch.lane),
                        mismatch.region,
                        mismatch.triage,
                    ))
                    .or_insert(0) += 1;
            }
        }
    }

    let mut buckets = counts
        .into_iter()
        .map(
            |((scope, lane, region, triage), count)| MobShadowTaxonomyBucket {
                scope,
                lane,
                region,
                triage,
                count,
            },
        )
        .collect::<Vec<_>>();
    buckets.sort_by(|a, b| {
        (a.scope, a.lane.as_str(), a.region, a.triage).cmp(&(
            b.scope,
            b.lane.as_str(),
            b.region,
            b.triage,
        ))
    });
    buckets
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestoreFailureSnapshot {
    pub session_id: meerkat_core::types::SessionId,
    pub reason: String,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrackedRunMalformedSnapshot {
    pub flow_id: FlowId,
    pub status: MobRunStatus,
    pub completed_at_present: bool,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrackedRunSnapshot {
    Missing,
    Present(TrackedRunStoreSnapshot),
    Malformed(TrackedRunMalformedSnapshot),
}

/// Cross-region invariant violations in the joined MobMachine snapshot.
///
/// This validator is intentionally conservative: it only encodes invariants
/// already justified by the current roster authority and member-status
/// projection logic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MobMachineInvariantViolation {
    WiringProjectionInconsistent {
        a: MeerkatId,
        b: MeerkatId,
    },
    ProjectedMemberMissingRosterEntry {
        meerkat_id: MeerkatId,
    },
    RosterMemberMissingProjectedEntry {
        meerkat_id: MeerkatId,
    },
    ProjectedStructuralMismatch {
        meerkat_id: MeerkatId,
        field: &'static str,
    },
    ListedMemberUnknownStatus {
        meerkat_id: MeerkatId,
    },
    ActiveMemberMarkedRetiring {
        meerkat_id: MeerkatId,
    },
    RetiringMemberHasIllegalStatus {
        meerkat_id: MeerkatId,
        status: MobMemberStatus,
    },
    ActiveMemberMissingSessionBinding {
        meerkat_id: MeerkatId,
    },
    BrokenMemberMissingError {
        meerkat_id: MeerkatId,
    },
    LifecyclePhaseMismatch {
        public_phase: MobState,
        lifecycle_phase: MobState,
    },
    LifecycleTerminalPhaseHasActiveRuns {
        phase: MobState,
        active_run_count: u32,
    },
    LifecycleCleanupPendingInIllegalPhase {
        phase: MobState,
    },
    OrchestratorPhaseMismatch {
        public_phase: MobState,
        orchestrator_phase: MobState,
    },
    OrchestratorTerminalPhaseHasInFlightWork {
        phase: MobState,
        pending_spawn_count: u32,
        active_flow_count: u32,
    },
    DestroyedOrchestratorStillBound {
        coordinator_bound: bool,
        supervisor_active: bool,
    },
    TopologyCoordinatorBindingMismatch {
        orchestrator_bound: bool,
        topology_bound: bool,
    },
    TopologyRevisionMismatch {
        orchestrator_revision: u32,
        topology_revision: u32,
    },
    LifecycleRunTrackerCountMismatch {
        active_run_count: u32,
        run_task_count: usize,
    },
    OrchestratorFlowTrackerCountMismatch {
        active_flow_count: u32,
        run_task_count: usize,
    },
    PendingSpawnCountMismatch {
        pending_spawn_count: u32,
        lineage_count: usize,
    },
    PendingSpawnTicketAlignmentMismatch {
        metadata_ticket_ids: Vec<u64>,
        task_ticket_ids: Vec<u64>,
    },
    PendingSpawnDuplicateMember {
        meerkat_id: MeerkatId,
    },
    PendingSpawnMemberAlreadyInRoster {
        meerkat_id: MeerkatId,
    },
    PendingSpawnMemberAlreadyProjected {
        meerkat_id: MeerkatId,
    },
    PendingSpawnPartialProvisionBinding {
        ticket: u64,
    },
    KickoffPendingMemberOverlapsPendingSpawn {
        meerkat_id: MeerkatId,
    },
    KickoffPendingMemberMissingRosterEntry {
        meerkat_id: MeerkatId,
    },
    KickoffPendingMemberMissingProjectedEntry {
        meerkat_id: MeerkatId,
    },
    KickoffPendingMemberMissingSessionBinding {
        meerkat_id: MeerkatId,
    },
    KickoffPendingMemberBroken {
        meerkat_id: MeerkatId,
    },
    RestoreFailureMissingRosterEntry {
        meerkat_id: MeerkatId,
    },
    RestoreFailureMissingProjectedMember {
        meerkat_id: MeerkatId,
    },
    RestoreFailureProjectedStatusMismatch {
        meerkat_id: MeerkatId,
        status: MobMemberStatus,
    },
    RestoreFailureProjectedSessionMismatch {
        meerkat_id: MeerkatId,
    },
    RestoreFailureProjectedReasonMismatch {
        meerkat_id: MeerkatId,
    },
    ProjectedBrokenMemberMissingRestoreFailure {
        meerkat_id: MeerkatId,
    },
    FlowTaskTokenMismatch {
        run_task_ids: Vec<crate::ids::RunId>,
        cancel_token_ids: Vec<crate::ids::RunId>,
    },
    FlowStreamWithoutTrackedRun {
        run_id: crate::ids::RunId,
    },
    TrackedRunMissingStoreEntry {
        run_id: RunId,
    },
    TrackedRunFlowIdMismatch {
        run_id: RunId,
        tracker_flow_id: FlowId,
        store_flow_id: FlowId,
    },
    TrackedRunMalformed {
        run_id: RunId,
        reason: String,
    },
    TrackedRunCompletionTimestampMismatch {
        run_id: RunId,
        status: MobRunStatus,
        completed_at_present: bool,
    },
    TrackedRunMissingOrderedSteps {
        run_id: RunId,
    },
    TrackedRunOrderedStepsDuplicate {
        run_id: RunId,
        step_id: StepId,
    },
    TrackedRunDependencyMapUnknownStep {
        run_id: RunId,
        step_id: StepId,
    },
    TrackedRunDependencyMapMissingOrderedStep {
        run_id: RunId,
        step_id: StepId,
    },
    TrackedRunDependencyModeMapUnknownStep {
        run_id: RunId,
        step_id: StepId,
    },
    TrackedRunDependencyModeMissingOrderedStep {
        run_id: RunId,
        step_id: StepId,
    },
    TrackedRunAnyDependencyModeWithoutBranchDependency {
        run_id: RunId,
        step_id: StepId,
    },
    TrackedRunConditionFlagUnknownStep {
        run_id: RunId,
        step_id: StepId,
    },
    TrackedRunConditionFlagMissingOrderedStep {
        run_id: RunId,
        step_id: StepId,
    },
    TrackedRunBranchUnknownStep {
        run_id: RunId,
        step_id: StepId,
    },
    TrackedRunBranchMissingOrderedStep {
        run_id: RunId,
        step_id: StepId,
    },
    TrackedRunBranchWithoutCondition {
        run_id: RunId,
        step_id: StepId,
        branch_id: BranchId,
    },
    TrackedRunBranchConflictingDependencies {
        run_id: RunId,
        branch_id: BranchId,
        reference_step_id: StepId,
        conflicting_step_id: StepId,
    },
    TrackedRunBranchGroupTooSmall {
        run_id: RunId,
        branch_id: BranchId,
        member_count: usize,
    },
    TrackedRunCollectionPolicyUnknownStep {
        run_id: RunId,
        step_id: StepId,
    },
    TrackedRunCollectionPolicyMissingOrderedStep {
        run_id: RunId,
        step_id: StepId,
    },
    TrackedRunQuorumThresholdUnknownStep {
        run_id: RunId,
        step_id: StepId,
    },
    TrackedRunQuorumThresholdMissingOrderedStep {
        run_id: RunId,
        step_id: StepId,
    },
    TrackedRunQuorumThresholdShapeMismatch {
        run_id: RunId,
        step_id: StepId,
        policy_kind: RunCollectionPolicyKind,
        quorum_threshold: u32,
    },
    TrackedRunDependencyUnknownStep {
        run_id: RunId,
        step_id: StepId,
        dependency_step_id: StepId,
    },
    TrackedRunDuplicateDependency {
        run_id: RunId,
        step_id: StepId,
        dependency_step_id: StepId,
    },
    TrackedRunSelfDependency {
        run_id: RunId,
        step_id: StepId,
    },
    TrackedRunStepStatusUnknownStep {
        run_id: RunId,
        step_id: StepId,
    },
    TrackedRunTerminalRunHasDispatchedStep {
        run_id: RunId,
        step_id: StepId,
        status: MobRunStatus,
    },
    TrackedRunCompletedRunHasNonCompletedStep {
        run_id: RunId,
        step_id: StepId,
        step_status: StepRunStatus,
    },
    TrackedRunConsecutiveFailureCountExceedsFailureCount {
        run_id: RunId,
        failure_count: u32,
        consecutive_failure_count: u32,
    },
    TrackedRunLoopIterationWithoutLoopStructure {
        run_id: RunId,
        loop_iteration_count: usize,
        loop_count: usize,
        frame_count: usize,
    },
    TrackedRunLoopWithoutFrameStructure {
        run_id: RunId,
        loop_count: usize,
        frame_count: usize,
    },
    TrackedRunStructuredStateWithLegacySchemaVersion {
        run_id: RunId,
        schema_version: u32,
        frame_count: usize,
        loop_count: usize,
        loop_iteration_count: usize,
    },
    MemberFinalityMismatch {
        meerkat_id: MeerkatId,
        status: MobMemberStatus,
        is_final: bool,
    },
}

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
                    session_id: diag.session_id.clone(),
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

/// Validate the currently joined MobMachine snapshot against a small set of
/// live cross-region invariants.
pub(crate) fn validate_mob_machine_snapshot(
    snapshot: &MobMachineSnapshot,
) -> Vec<MobMachineInvariantViolation> {
    let mut violations = Vec::new();
    let lifecycle = &snapshot.kernel.lifecycle;

    if snapshot.phase != lifecycle.phase {
        violations.push(MobMachineInvariantViolation::LifecyclePhaseMismatch {
            public_phase: snapshot.phase,
            lifecycle_phase: lifecycle.phase,
        });
    }

    if matches!(lifecycle.phase, MobState::Completed | MobState::Destroyed)
        && lifecycle.active_run_count != 0
    {
        violations.push(
            MobMachineInvariantViolation::LifecycleTerminalPhaseHasActiveRuns {
                phase: lifecycle.phase,
                active_run_count: lifecycle.active_run_count,
            },
        );
    }

    if lifecycle.cleanup_pending
        && !matches!(lifecycle.phase, MobState::Stopped | MobState::Completed)
    {
        violations.push(
            MobMachineInvariantViolation::LifecycleCleanupPendingInIllegalPhase {
                phase: lifecycle.phase,
            },
        );
    }

    if let Some(orchestrator) = &snapshot.kernel.orchestrator {
        push_orchestrator_mismatches(&mut violations, snapshot.phase, orchestrator);
        push_topology_mismatches(&mut violations, orchestrator, snapshot.kernel.topology);
    }
    push_pending_spawn_mismatches(&mut violations, snapshot);
    push_kickoff_barrier_mismatches(&mut violations, snapshot);
    push_flow_tracker_mismatches(&mut violations, &snapshot.kernel);
    push_tracked_run_store_mismatches(&mut violations, snapshot);

    for (a, b) in snapshot.roster.wiring_projection_inconsistencies() {
        violations.push(MobMachineInvariantViolation::WiringProjectionInconsistent { a, b });
    }

    let projected_members: HashMap<_, _> = snapshot
        .members
        .iter()
        .map(|entry| (entry.meerkat_id.clone(), entry))
        .collect();

    for roster_entry in snapshot.roster.list_all() {
        if !projected_members.contains_key(&roster_entry.meerkat_id) {
            violations.push(
                MobMachineInvariantViolation::RosterMemberMissingProjectedEntry {
                    meerkat_id: roster_entry.meerkat_id.clone(),
                },
            );
        }
    }

    push_restore_failure_mismatches(&mut violations, snapshot, &projected_members);

    for projected in &snapshot.members {
        let Some(roster_entry) = snapshot.roster.get(&projected.meerkat_id) else {
            violations.push(
                MobMachineInvariantViolation::ProjectedMemberMissingRosterEntry {
                    meerkat_id: projected.meerkat_id.clone(),
                },
            );
            continue;
        };

        push_structural_mismatches(&mut violations, projected, roster_entry);

        match projected.state {
            MemberState::Active => {
                if projected.status == MobMemberStatus::Retiring {
                    violations.push(MobMachineInvariantViolation::ActiveMemberMarkedRetiring {
                        meerkat_id: projected.meerkat_id.clone(),
                    });
                }
            }
            MemberState::Retiring => {
                if !matches!(
                    projected.status,
                    MobMemberStatus::Retiring | MobMemberStatus::Broken
                ) {
                    violations.push(
                        MobMachineInvariantViolation::RetiringMemberHasIllegalStatus {
                            meerkat_id: projected.meerkat_id.clone(),
                            status: projected.status,
                        },
                    );
                }
            }
        }

        match projected.status {
            MobMemberStatus::Unknown => {
                violations.push(MobMachineInvariantViolation::ListedMemberUnknownStatus {
                    meerkat_id: projected.meerkat_id.clone(),
                });
            }
            MobMemberStatus::Active => {
                if projected.current_session_id.is_none() {
                    violations.push(
                        MobMachineInvariantViolation::ActiveMemberMissingSessionBinding {
                            meerkat_id: projected.meerkat_id.clone(),
                        },
                    );
                }
            }
            MobMemberStatus::Broken => {
                if projected.error.is_none() {
                    violations.push(MobMachineInvariantViolation::BrokenMemberMissingError {
                        meerkat_id: projected.meerkat_id.clone(),
                    });
                }
            }
            MobMemberStatus::Retiring | MobMemberStatus::Completed => {}
        }

        let expected_final = matches!(
            projected.status,
            MobMemberStatus::Broken | MobMemberStatus::Completed | MobMemberStatus::Unknown
        );
        if projected.is_final != expected_final {
            violations.push(MobMachineInvariantViolation::MemberFinalityMismatch {
                meerkat_id: projected.meerkat_id.clone(),
                status: projected.status,
                is_final: projected.is_final,
            });
        }
    }

    violations
}

fn provisioning_lifecycle_shadow_mismatch_from_violation(
    violation: MobMachineInvariantViolation,
) -> Option<MobShadowMismatch> {
    match violation {
        MobMachineInvariantViolation::LifecyclePhaseMismatch {
            public_phase,
            lifecycle_phase,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::ProvisioningLifecycle,
            region: "lifecycle",
            entity_id: None,
            expected_summary: "public mob phase matches lifecycle authority phase".into(),
            observed_summary: format!(
                "public phase = {public_phase:?}, lifecycle phase = {lifecycle_phase:?}"
            ),
            triage: ShadowMismatchTriage::DogmaViolation,
        }),
        MobMachineInvariantViolation::LifecycleTerminalPhaseHasActiveRuns {
            phase,
            active_run_count,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::ProvisioningLifecycle,
            region: "lifecycle",
            entity_id: None,
            expected_summary: "terminal lifecycle phases have no active runs".into(),
            observed_summary: format!(
                "phase = {phase:?}, active_run_count = {active_run_count}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MobMachineInvariantViolation::LifecycleCleanupPendingInIllegalPhase { phase } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::ProvisioningLifecycle,
                region: "lifecycle",
                entity_id: None,
                expected_summary:
                    "cleanup_pending is only visible while the mob is Stopped or Completed"
                        .into(),
                observed_summary: format!("phase = {phase:?} with cleanup_pending = true"),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MobMachineInvariantViolation::OrchestratorPhaseMismatch {
            public_phase,
            orchestrator_phase,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::ProvisioningLifecycle,
            region: "orchestrator",
            entity_id: None,
            expected_summary: "public mob phase matches orchestrator phase".into(),
            observed_summary: format!(
                "public phase = {public_phase:?}, orchestrator phase = {orchestrator_phase:?}"
            ),
            triage: ShadowMismatchTriage::DogmaViolation,
        }),
        MobMachineInvariantViolation::OrchestratorTerminalPhaseHasInFlightWork {
            phase,
            pending_spawn_count,
            active_flow_count,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::ProvisioningLifecycle,
            region: "orchestrator",
            entity_id: None,
            expected_summary: "terminal orchestrator phases have no in-flight spawn or flow work"
                .into(),
            observed_summary: format!(
                "phase = {phase:?}, pending_spawn_count = {pending_spawn_count}, active_flow_count = {active_flow_count}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MobMachineInvariantViolation::DestroyedOrchestratorStillBound {
            coordinator_bound,
            supervisor_active,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::ProvisioningLifecycle,
            region: "orchestrator",
            entity_id: None,
            expected_summary:
                "destroyed orchestrators are unbound and have no active supervisor".into(),
            observed_summary: format!(
                "coordinator_bound = {coordinator_bound}, supervisor_active = {supervisor_active}"
            ),
            triage: ShadowMismatchTriage::DogmaViolation,
        }),
        MobMachineInvariantViolation::TopologyCoordinatorBindingMismatch {
            orchestrator_bound,
            topology_bound,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::ProvisioningLifecycle,
            region: "topology",
            entity_id: None,
            expected_summary:
                "topology coordinator binding matches orchestrator coordinator binding".into(),
            observed_summary: format!(
                "orchestrator_bound = {orchestrator_bound}, topology_bound = {topology_bound}"
            ),
            triage: ShadowMismatchTriage::DogmaViolation,
        }),
        MobMachineInvariantViolation::TopologyRevisionMismatch {
            orchestrator_revision,
            topology_revision,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::ProvisioningLifecycle,
            region: "topology",
            entity_id: None,
            expected_summary: "topology revision matches orchestrator topology revision".into(),
            observed_summary: format!(
                "orchestrator_revision = {orchestrator_revision}, topology_revision = {topology_revision}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MobMachineInvariantViolation::PendingSpawnCountMismatch {
            pending_spawn_count,
            lineage_count,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::ProvisioningLifecycle,
            region: "pending_spawns",
            entity_id: None,
            expected_summary:
                "orchestrator pending_spawn_count matches visible pending lineage size".into(),
            observed_summary: format!(
                "pending_spawn_count = {pending_spawn_count}, lineage_count = {lineage_count}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MobMachineInvariantViolation::PendingSpawnTicketAlignmentMismatch {
            metadata_ticket_ids,
            task_ticket_ids,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::ProvisioningLifecycle,
            region: "pending_spawns",
            entity_id: None,
            expected_summary: "pending spawn metadata and task tickets stay aligned".into(),
            observed_summary: format!(
                "metadata_ticket_ids = {metadata_ticket_ids:?}, task_ticket_ids = {task_ticket_ids:?}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MobMachineInvariantViolation::PendingSpawnDuplicateMember { meerkat_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::ProvisioningLifecycle,
                region: "pending_spawns",
                entity_id: Some(meerkat_id.to_string()),
                expected_summary: "pending spawn lineage stages each member identity at most once"
                    .into(),
                observed_summary: "duplicate pending spawn member identity".into(),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        MobMachineInvariantViolation::PendingSpawnMemberAlreadyInRoster { meerkat_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::ProvisioningLifecycle,
                region: "pending_spawns",
                entity_id: Some(meerkat_id.to_string()),
                expected_summary:
                    "pending spawn members do not materialize in the roster before finalization"
                        .into(),
                observed_summary: "pending spawn member already present in roster".into(),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        MobMachineInvariantViolation::PendingSpawnMemberAlreadyProjected { meerkat_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::ProvisioningLifecycle,
                region: "pending_spawns",
                entity_id: Some(meerkat_id.to_string()),
                expected_summary:
                    "pending spawn members do not materialize in projected member status before finalization"
                        .into(),
                observed_summary: "pending spawn member already present in projection".into(),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        MobMachineInvariantViolation::PendingSpawnPartialProvisionBinding { ticket } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::ProvisioningLifecycle,
                region: "pending_spawns",
                entity_id: Some(ticket.to_string()),
                expected_summary:
                    "pending spawn progress never exposes partial session or operation binding"
                        .into(),
                observed_summary: "ticket still has partial provision binding".into(),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MobMachineInvariantViolation::KickoffPendingMemberOverlapsPendingSpawn { meerkat_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::ProvisioningLifecycle,
                region: "kickoff_barrier",
                entity_id: Some(meerkat_id.to_string()),
                expected_summary:
                    "kickoff-tracked members are already past pending-spawn lineage".into(),
                observed_summary: "kickoff pending member still overlaps pending-spawn lineage".into(),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        MobMachineInvariantViolation::KickoffPendingMemberMissingRosterEntry { meerkat_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::ProvisioningLifecycle,
                region: "kickoff_barrier",
                entity_id: Some(meerkat_id.to_string()),
                expected_summary:
                    "kickoff pending members already exist in the canonical roster".into(),
                observed_summary: "kickoff pending member missing roster entry".into(),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        MobMachineInvariantViolation::KickoffPendingMemberMissingProjectedEntry { meerkat_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::ProvisioningLifecycle,
                region: "kickoff_barrier",
                entity_id: Some(meerkat_id.to_string()),
                expected_summary:
                    "kickoff pending members are visible in projected member status".into(),
                observed_summary: "kickoff pending member missing projected entry".into(),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        MobMachineInvariantViolation::KickoffPendingMemberMissingSessionBinding { meerkat_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::ProvisioningLifecycle,
                region: "kickoff_barrier",
                entity_id: Some(meerkat_id.to_string()),
                expected_summary:
                    "kickoff pending members already carry a live session binding".into(),
                observed_summary: "kickoff pending member missing session binding".into(),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        MobMachineInvariantViolation::KickoffPendingMemberBroken { meerkat_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::ProvisioningLifecycle,
                region: "kickoff_barrier",
                entity_id: Some(meerkat_id.to_string()),
                expected_summary:
                    "kickoff pending members are not already projected as Broken".into(),
                observed_summary: "kickoff pending member projected as Broken".into(),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MobMachineInvariantViolation::RestoreFailureMissingRosterEntry { meerkat_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::ProvisioningLifecycle,
                region: "restore_failures",
                entity_id: Some(meerkat_id.to_string()),
                expected_summary:
                    "restore failures remain attached to canonical roster members".into(),
                observed_summary: "restore failure missing roster entry".into(),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        MobMachineInvariantViolation::RestoreFailureMissingProjectedMember { meerkat_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::ProvisioningLifecycle,
                region: "restore_failures",
                entity_id: Some(meerkat_id.to_string()),
                expected_summary:
                    "restore failures remain visible in projected member status".into(),
                observed_summary: "restore failure missing projected member".into(),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        MobMachineInvariantViolation::RestoreFailureProjectedStatusMismatch { meerkat_id, status } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::ProvisioningLifecycle,
                region: "restore_failures",
                entity_id: Some(meerkat_id.to_string()),
                expected_summary:
                    "restore failure projections surface as Broken members".into(),
                observed_summary: format!("projected status = {status:?}"),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MobMachineInvariantViolation::RestoreFailureProjectedSessionMismatch { meerkat_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::ProvisioningLifecycle,
                region: "restore_failures",
                entity_id: Some(meerkat_id.to_string()),
                expected_summary:
                    "restore failure projections keep the same session binding as the diagnostic record"
                        .into(),
                observed_summary: "projected session binding mismatches restore failure record".into(),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MobMachineInvariantViolation::RestoreFailureProjectedReasonMismatch { meerkat_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::ProvisioningLifecycle,
                region: "restore_failures",
                entity_id: Some(meerkat_id.to_string()),
                expected_summary:
                    "restore failure projections keep the same failure reason as the diagnostic record"
                        .into(),
                observed_summary: "projected failure reason mismatches restore failure record".into(),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MobMachineInvariantViolation::ProjectedBrokenMemberMissingRestoreFailure { meerkat_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::ProvisioningLifecycle,
                region: "restore_failures",
                entity_id: Some(meerkat_id.to_string()),
                expected_summary:
                    "projected Broken members always have a matching restore-failure record"
                        .into(),
                observed_summary: "Broken projected member missing restore-failure record".into(),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        _ => None,
    }
}

fn capture_provisioning_lifecycle_shadow_mismatches(
    snapshot: &MobMachineSnapshot,
) -> Vec<MobShadowMismatch> {
    validate_mob_machine_snapshot(snapshot)
        .into_iter()
        .filter_map(provisioning_lifecycle_shadow_mismatch_from_violation)
        .collect()
}

fn flow_frame_loop_shadow_mismatch_from_violation(
    violation: MobMachineInvariantViolation,
) -> Option<MobShadowMismatch> {
    match violation {
        MobMachineInvariantViolation::OrchestratorFlowTrackerCountMismatch {
            active_flow_count,
            run_task_count,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::FlowFrameLoop,
            region: "flow_trackers",
            entity_id: None,
            expected_summary:
                "orchestrator active_flow_count matches actor-owned tracked flow tasks".into(),
            observed_summary: format!(
                "active_flow_count = {active_flow_count}, run_task_count = {run_task_count}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MobMachineInvariantViolation::FlowTaskTokenMismatch {
            run_task_ids,
            cancel_token_ids,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::FlowFrameLoop,
            region: "flow_trackers",
            entity_id: None,
            expected_summary: "tracked flow tasks and cancel tokens cover the same active run ids"
                .into(),
            observed_summary: format!(
                "run_task_ids = {run_task_ids:?}, cancel_token_ids = {cancel_token_ids:?}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MobMachineInvariantViolation::FlowStreamWithoutTrackedRun { run_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::FlowFrameLoop,
                region: "flow_trackers",
                entity_id: Some(run_id.to_string()),
                expected_summary: "flow-scoped event streams only exist for actor-tracked runs"
                    .into(),
                observed_summary: "stream tracked without an actor-owned run task".into(),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MobMachineInvariantViolation::TrackedRunMissingStoreEntry { run_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::FlowFrameLoop,
                region: "tracked_runs",
                entity_id: Some(run_id.to_string()),
                expected_summary:
                    "every actor-tracked flow run resolves to durable tracked-run state".into(),
                observed_summary: "run tracker exists but durable tracked-run snapshot is missing"
                    .into(),
                triage: ShadowMismatchTriage::SemanticGap,
            })
        }
        MobMachineInvariantViolation::TrackedRunFlowIdMismatch {
            run_id,
            tracker_flow_id,
            store_flow_id,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::FlowFrameLoop,
            region: "tracked_runs",
            entity_id: Some(run_id.to_string()),
            expected_summary:
                "actor-owned run tracking and durable tracked-run store agree on flow identity"
                    .into(),
            observed_summary: format!(
                "tracker_flow_id = {tracker_flow_id}, store_flow_id = {store_flow_id}"
            ),
            triage: ShadowMismatchTriage::DogmaViolation,
        }),
        MobMachineInvariantViolation::TrackedRunMalformed { run_id, reason } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::FlowFrameLoop,
                region: "tracked_runs",
                entity_id: Some(run_id.to_string()),
                expected_summary:
                    "tracked runs deserialize into canonical durable run-kernel shape".into(),
                observed_summary: format!("malformed tracked run snapshot: {reason}"),
                triage: ShadowMismatchTriage::SemanticGap,
            })
        }
        MobMachineInvariantViolation::TrackedRunCompletionTimestampMismatch {
            run_id,
            status,
            completed_at_present,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::FlowFrameLoop,
            region: "tracked_runs",
            entity_id: Some(run_id.to_string()),
            expected_summary:
                "terminal tracked runs carry completed_at and non-terminal tracked runs do not"
                    .into(),
            observed_summary: format!(
                "status = {status:?}, completed_at_present = {completed_at_present}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MobMachineInvariantViolation::TrackedRunMissingOrderedSteps { run_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::FlowFrameLoop,
                region: "tracked_runs",
                entity_id: Some(run_id.to_string()),
                expected_summary: "tracked runs preserve a non-empty ordered step sequence".into(),
                observed_summary: "tracked run has no ordered steps".into(),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        MobMachineInvariantViolation::TrackedRunOrderedStepsDuplicate { run_id, step_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::FlowFrameLoop,
                region: "tracked_runs",
                entity_id: Some(run_id.to_string()),
                expected_summary:
                    "tracked run ordered steps contain each step identity at most once".into(),
                observed_summary: format!("duplicate ordered step id = {step_id}"),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        MobMachineInvariantViolation::TrackedRunDependencyMapUnknownStep { run_id, step_id }
        | MobMachineInvariantViolation::TrackedRunDependencyModeMapUnknownStep {
            run_id,
            step_id,
        }
        | MobMachineInvariantViolation::TrackedRunConditionFlagUnknownStep { run_id, step_id }
        | MobMachineInvariantViolation::TrackedRunBranchUnknownStep { run_id, step_id }
        | MobMachineInvariantViolation::TrackedRunCollectionPolicyUnknownStep { run_id, step_id }
        | MobMachineInvariantViolation::TrackedRunQuorumThresholdUnknownStep { run_id, step_id }
        | MobMachineInvariantViolation::TrackedRunStepStatusUnknownStep { run_id, step_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::FlowFrameLoop,
                region: "tracked_runs",
                entity_id: Some(run_id.to_string()),
                expected_summary: "tracked-run step maps only reference kernel-known ordered steps"
                    .into(),
                observed_summary: format!("unknown step id in tracked-run projection = {step_id}"),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        MobMachineInvariantViolation::TrackedRunDependencyMapMissingOrderedStep {
            run_id,
            step_id,
        }
        | MobMachineInvariantViolation::TrackedRunDependencyModeMissingOrderedStep {
            run_id,
            step_id,
        }
        | MobMachineInvariantViolation::TrackedRunConditionFlagMissingOrderedStep {
            run_id,
            step_id,
        }
        | MobMachineInvariantViolation::TrackedRunBranchMissingOrderedStep { run_id, step_id }
        | MobMachineInvariantViolation::TrackedRunCollectionPolicyMissingOrderedStep {
            run_id,
            step_id,
        }
        | MobMachineInvariantViolation::TrackedRunQuorumThresholdMissingOrderedStep {
            run_id,
            step_id,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::FlowFrameLoop,
            region: "tracked_runs",
            entity_id: Some(run_id.to_string()),
            expected_summary:
                "tracked-run maps cover every ordered step in the durable kernel projection".into(),
            observed_summary: format!("missing ordered step entry for {step_id}"),
            triage: ShadowMismatchTriage::DogmaViolation,
        }),
        MobMachineInvariantViolation::TrackedRunAnyDependencyModeWithoutBranchDependency {
            run_id,
            step_id,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::FlowFrameLoop,
            region: "tracked_runs",
            entity_id: Some(run_id.to_string()),
            expected_summary:
                "depends_on_mode=any steps depend on at least one branch-labelled step".into(),
            observed_summary: format!(
                "step {step_id} uses DependencyMode::Any without branch-backed dependencies"
            ),
            triage: ShadowMismatchTriage::SemanticGap,
        }),
        MobMachineInvariantViolation::TrackedRunBranchWithoutCondition {
            run_id,
            step_id,
            branch_id,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::FlowFrameLoop,
            region: "tracked_runs",
            entity_id: Some(run_id.to_string()),
            expected_summary: "branch-labelled steps remain condition-backed in tracked-run state"
                .into(),
            observed_summary: format!(
                "step {step_id} carries branch {branch_id} without condition state"
            ),
            triage: ShadowMismatchTriage::SemanticGap,
        }),
        MobMachineInvariantViolation::TrackedRunBranchConflictingDependencies {
            run_id,
            branch_id,
            ..
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::FlowFrameLoop,
            region: "tracked_runs",
            entity_id: Some(run_id.to_string()),
            expected_summary: "steps in the same branch group preserve matching dependency sets"
                .into(),
            observed_summary: format!("branch group {branch_id} has conflicting dependency sets"),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MobMachineInvariantViolation::TrackedRunBranchGroupTooSmall {
            run_id, branch_id, ..
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::FlowFrameLoop,
            region: "tracked_runs",
            entity_id: Some(run_id.to_string()),
            expected_summary: "branch groups contain at least two branch-labelled steps".into(),
            observed_summary: format!("branch group {branch_id} contains fewer than two steps"),
            triage: ShadowMismatchTriage::SemanticGap,
        }),
        MobMachineInvariantViolation::TrackedRunQuorumThresholdShapeMismatch {
            run_id,
            step_id,
            policy_kind,
            quorum_threshold,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::FlowFrameLoop,
            region: "tracked_runs",
            entity_id: Some(run_id.to_string()),
            expected_summary: "quorum threshold shape matches the tracked collection policy".into(),
            observed_summary: format!(
                "step {step_id} uses {policy_kind:?} with quorum threshold {quorum_threshold}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MobMachineInvariantViolation::TrackedRunDependencyUnknownStep {
            run_id,
            step_id,
            dependency_step_id,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::FlowFrameLoop,
            region: "tracked_runs",
            entity_id: Some(run_id.to_string()),
            expected_summary: "tracked-run dependencies only point at kernel-known ordered steps"
                .into(),
            observed_summary: format!(
                "step {step_id} depends on unknown step {dependency_step_id}"
            ),
            triage: ShadowMismatchTriage::DogmaViolation,
        }),
        MobMachineInvariantViolation::TrackedRunDuplicateDependency {
            run_id, step_id, ..
        }
        | MobMachineInvariantViolation::TrackedRunSelfDependency { run_id, step_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::FlowFrameLoop,
                region: "tracked_runs",
                entity_id: Some(run_id.to_string()),
                expected_summary:
                    "tracked-run dependency sets are de-duplicated and acyclic at self edges".into(),
                observed_summary: format!(
                    "step {step_id} carries duplicate or self dependency state"
                ),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        MobMachineInvariantViolation::TrackedRunTerminalRunHasDispatchedStep {
            run_id,
            step_id,
            status,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::FlowFrameLoop,
            region: "tracked_runs",
            entity_id: Some(run_id.to_string()),
            expected_summary: "terminal tracked runs do not retain dispatched step statuses".into(),
            observed_summary: format!(
                "terminal run status {status:?} still has dispatched step {step_id}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MobMachineInvariantViolation::TrackedRunCompletedRunHasNonCompletedStep {
            run_id,
            step_id,
            step_status,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::FlowFrameLoop,
            region: "tracked_runs",
            entity_id: Some(run_id.to_string()),
            expected_summary:
                "completed tracked runs only preserve Completed or Skipped step statuses".into(),
            observed_summary: format!(
                "completed tracked run still shows step {step_id} in status {step_status:?}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MobMachineInvariantViolation::TrackedRunConsecutiveFailureCountExceedsFailureCount {
            run_id,
            failure_count,
            consecutive_failure_count,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::FlowFrameLoop,
            region: "tracked_runs",
            entity_id: Some(run_id.to_string()),
            expected_summary: "consecutive failure count does not exceed total failure count"
                .into(),
            observed_summary: format!(
                "failure_count = {failure_count}, consecutive_failure_count = {consecutive_failure_count}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MobMachineInvariantViolation::TrackedRunLoopIterationWithoutLoopStructure {
            run_id,
            loop_iteration_count,
            ..
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::FlowFrameLoop,
            region: "tracked_runs",
            entity_id: Some(run_id.to_string()),
            expected_summary:
                "loop iteration ledger rows only exist when the run projects loop structure".into(),
            observed_summary: format!(
                "loop_iteration_count = {loop_iteration_count} without loop structure"
            ),
            triage: ShadowMismatchTriage::SemanticGap,
        }),
        MobMachineInvariantViolation::TrackedRunLoopWithoutFrameStructure {
            run_id,
            loop_count,
            ..
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::FlowFrameLoop,
            region: "tracked_runs",
            entity_id: Some(run_id.to_string()),
            expected_summary:
                "loop structure only exists when the run also projects frame structure".into(),
            observed_summary: format!("loop_count = {loop_count} without frame structure"),
            triage: ShadowMismatchTriage::SemanticGap,
        }),
        MobMachineInvariantViolation::TrackedRunStructuredStateWithLegacySchemaVersion {
            run_id,
            schema_version,
            ..
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::FlowFrameLoop,
            region: "tracked_runs",
            entity_id: Some(run_id.to_string()),
            expected_summary: "frame/loop-aware tracked runs use the modern durable schema version"
                .into(),
            observed_summary: format!("schema_version = {schema_version}"),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        _ => None,
    }
}

fn capture_flow_frame_loop_shadow_mismatches(
    snapshot: &MobMachineSnapshot,
) -> Vec<MobShadowMismatch> {
    validate_mob_machine_snapshot(snapshot)
        .into_iter()
        .filter_map(flow_frame_loop_shadow_mismatch_from_violation)
        .collect()
}

fn task_history_recovery_shadow_mismatch_from_violation(
    violation: MobMachineInvariantViolation,
) -> Option<MobShadowMismatch> {
    match violation {
        MobMachineInvariantViolation::RestoreFailureMissingRosterEntry { meerkat_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::TaskHistoryRecovery,
                region: "restore_failures",
                entity_id: Some(meerkat_id.to_string()),
                expected_summary:
                    "restore failures remain attached to canonical roster members".into(),
                observed_summary: "restore failure missing roster entry".into(),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        MobMachineInvariantViolation::RestoreFailureMissingProjectedMember { meerkat_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::TaskHistoryRecovery,
                region: "restore_failures",
                entity_id: Some(meerkat_id.to_string()),
                expected_summary:
                    "restore failures remain visible in projected member status".into(),
                observed_summary: "restore failure missing projected member".into(),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        MobMachineInvariantViolation::RestoreFailureProjectedStatusMismatch {
            meerkat_id,
            status,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::TaskHistoryRecovery,
            region: "restore_failures",
            entity_id: Some(meerkat_id.to_string()),
            expected_summary: "restore failure projections surface as Broken members".into(),
            observed_summary: format!("projected status = {status:?}"),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MobMachineInvariantViolation::RestoreFailureProjectedSessionMismatch { meerkat_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::TaskHistoryRecovery,
                region: "restore_failures",
                entity_id: Some(meerkat_id.to_string()),
                expected_summary:
                    "restore failure projections keep the same session binding as the diagnostic record"
                        .into(),
                observed_summary: "projected session binding mismatches restore failure record".into(),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MobMachineInvariantViolation::RestoreFailureProjectedReasonMismatch { meerkat_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::TaskHistoryRecovery,
                region: "restore_failures",
                entity_id: Some(meerkat_id.to_string()),
                expected_summary:
                    "restore failure projections keep the same failure reason as the diagnostic record"
                        .into(),
                observed_summary: "projected failure reason mismatches restore failure record".into(),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MobMachineInvariantViolation::ProjectedBrokenMemberMissingRestoreFailure { meerkat_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::TaskHistoryRecovery,
                region: "restore_failures",
                entity_id: Some(meerkat_id.to_string()),
                expected_summary:
                    "projected Broken members always have a matching restore-failure record"
                        .into(),
                observed_summary: "Broken projected member missing restore-failure record".into(),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        MobMachineInvariantViolation::FlowTaskTokenMismatch {
            run_task_ids,
            cancel_token_ids,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::TaskHistoryRecovery,
            region: "task_ledger",
            entity_id: None,
            expected_summary:
                "tracked flow tasks and cancel tokens cover the same active run ids".into(),
            observed_summary: format!(
                "run_task_ids = {run_task_ids:?}, cancel_token_ids = {cancel_token_ids:?}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MobMachineInvariantViolation::FlowStreamWithoutTrackedRun { run_id } => {
            Some(MobShadowMismatch {
                lane: MobShadowLane::TaskHistoryRecovery,
                region: "task_ledger",
                entity_id: Some(run_id.to_string()),
                expected_summary:
                    "flow-scoped event streams only exist for actor-tracked runs".into(),
                observed_summary: "stream tracked without an actor-owned run task".into(),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MobMachineInvariantViolation::MemberFinalityMismatch {
            meerkat_id,
            status,
            is_final,
        } => Some(MobShadowMismatch {
            lane: MobShadowLane::TaskHistoryRecovery,
            region: "history",
            entity_id: Some(meerkat_id.to_string()),
            expected_summary:
                "projected member finality matches the terminal/member-status surface".into(),
            observed_summary: format!("status = {status:?}, is_final = {is_final}"),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        _ => None,
    }
}

fn capture_task_history_recovery_shadow_mismatches(
    snapshot: &MobMachineSnapshot,
) -> Vec<MobShadowMismatch> {
    validate_mob_machine_snapshot(snapshot)
        .into_iter()
        .filter_map(task_history_recovery_shadow_mismatch_from_violation)
        .collect()
}

pub(crate) async fn capture_mob_shadow_report(
    handle: &MobHandle,
    lane: MobShadowLane,
) -> MobShadowReport {
    let snapshot = capture_mob_machine_snapshot(handle).await;
    let mismatches = match lane {
        MobShadowLane::ProvisioningLifecycle => {
            capture_provisioning_lifecycle_shadow_mismatches(&snapshot)
        }
        MobShadowLane::FlowFrameLoop => capture_flow_frame_loop_shadow_mismatches(&snapshot),
        MobShadowLane::TaskHistoryRecovery => {
            capture_task_history_recovery_shadow_mismatches(&snapshot)
        }
    };
    MobShadowReport { lane, mismatches }
}

fn push_orchestrator_mismatches(
    violations: &mut Vec<MobMachineInvariantViolation>,
    public_phase: MobState,
    orchestrator: &MobOrchestratorSnapshot,
) {
    if public_phase != orchestrator.phase {
        violations.push(MobMachineInvariantViolation::OrchestratorPhaseMismatch {
            public_phase,
            orchestrator_phase: orchestrator.phase,
        });
    }

    if matches!(
        orchestrator.phase,
        MobState::Completed | MobState::Destroyed
    ) && (orchestrator.pending_spawn_count != 0 || orchestrator.active_flow_count != 0)
    {
        violations.push(
            MobMachineInvariantViolation::OrchestratorTerminalPhaseHasInFlightWork {
                phase: orchestrator.phase,
                pending_spawn_count: orchestrator.pending_spawn_count,
                active_flow_count: orchestrator.active_flow_count,
            },
        );
    }

    if orchestrator.phase == MobState::Destroyed
        && (orchestrator.coordinator_bound || orchestrator.supervisor_active)
    {
        violations.push(
            MobMachineInvariantViolation::DestroyedOrchestratorStillBound {
                coordinator_bound: orchestrator.coordinator_bound,
                supervisor_active: orchestrator.supervisor_active,
            },
        );
    }
}

fn composition_lifecycle_supersession_mismatch(
    region: &'static str,
    entity_id: impl Into<Option<String>>,
    expected_summary: impl Into<String>,
    observed_summary: impl Into<String>,
    triage: ShadowMismatchTriage,
) -> CompositionShadowMismatch {
    CompositionShadowMismatch {
        lane: CompositionShadowLane::LifecycleSupersession,
        region,
        entity_id: entity_id.into(),
        expected_summary: expected_summary.into(),
        observed_summary: observed_summary.into(),
        triage,
    }
}

fn composition_work_bridge_mismatch(
    region: &'static str,
    entity_id: impl Into<Option<String>>,
    expected_summary: impl Into<String>,
    observed_summary: impl Into<String>,
    triage: ShadowMismatchTriage,
) -> CompositionShadowMismatch {
    CompositionShadowMismatch {
        lane: CompositionShadowLane::WorkBridge,
        region,
        entity_id: entity_id.into(),
        expected_summary: expected_summary.into(),
        observed_summary: observed_summary.into(),
        triage,
    }
}

fn spine_has_observable_work_posture(spine: &MeerkatMachineSpineSnapshot) -> bool {
    spine.control.current_run_id.is_some()
        || !spine.inputs.queue.is_empty()
        || !spine.inputs.steer_queue.is_empty()
        || spine.completion_waiters.waiter_count != 0
}

fn tracked_run_is_nonterminal(run: &TrackedRunSnapshot) -> bool {
    match run {
        TrackedRunSnapshot::Present(snapshot) => !snapshot.status.is_terminal(),
        TrackedRunSnapshot::Malformed(snapshot) => !snapshot.status.is_terminal(),
        TrackedRunSnapshot::Missing => false,
    }
}

fn capture_composition_work_bridge_shadow_mismatches(
    snapshot: &MobMachineSnapshot,
    runtime_spines: &HashMap<meerkat_core::types::SessionId, Option<MeerkatMachineSpineSnapshot>>,
) -> Vec<CompositionShadowMismatch> {
    let mut mismatches = Vec::new();
    let mut work_visible_members = Vec::new();

    for member in &snapshot.members {
        let Some(session_id) = &member.current_session_id else {
            continue;
        };
        let Some(spine) = runtime_spines
            .get(session_id)
            .and_then(|snapshot| snapshot.as_ref())
        else {
            continue;
        };

        if spine_has_observable_work_posture(spine) {
            work_visible_members.push(member.meerkat_id.clone());
            if matches!(
                member.status,
                MobMemberStatus::Completed | MobMemberStatus::Broken | MobMemberStatus::Unknown
            ) {
                mismatches.push(composition_work_bridge_mismatch(
                    "work",
                    Some(member.meerkat_id.to_string()),
                    "terminal or broken Mob members should not retain observable Meerkat work posture",
                    format!(
                        "status={:?}, current_run_id={:?}, queue_len={}, steer_queue_len={}, waiter_count={}",
                        member.status,
                        spine.control.current_run_id,
                        spine.inputs.queue.len(),
                        spine.inputs.steer_queue.len(),
                        spine.completion_waiters.waiter_count
                    ),
                    ShadowMismatchTriage::ImplementationDetail,
                ));
            }
        }
    }

    let nonterminal_tracked_runs: Vec<_> = snapshot
        .tracked_runs
        .iter()
        .filter_map(|(run_id, run)| tracked_run_is_nonterminal(run).then_some(run_id.clone()))
        .collect();

    if !nonterminal_tracked_runs.is_empty() && work_visible_members.is_empty() {
        mismatches.push(composition_work_bridge_mismatch(
            "work",
            None,
            "nonterminal Mob work should surface at least one observable Meerkat work posture",
            format!("nonterminal_runs={nonterminal_tracked_runs:?}, work_visible_members=[]"),
            ShadowMismatchTriage::SemanticGap,
        ));
    }

    mismatches
}

fn capture_composition_lifecycle_supersession_shadow_mismatches(
    snapshot: &MobMachineSnapshot,
    session_live: &HashMap<meerkat_core::types::SessionId, bool>,
    runtime_spines: &HashMap<meerkat_core::types::SessionId, Option<MeerkatMachineSpineSnapshot>>,
) -> Vec<CompositionShadowMismatch> {
    let mut mismatches = Vec::new();
    let mut session_owners: HashMap<meerkat_core::types::SessionId, Vec<MeerkatId>> =
        HashMap::new();

    for member in &snapshot.members {
        if let Some(session_id) = &member.current_session_id {
            session_owners
                .entry(session_id.clone())
                .or_default()
                .push(member.meerkat_id.clone());
        }
    }

    for (session_id, owners) in session_owners {
        if owners.len() > 1 {
            mismatches.push(composition_lifecycle_supersession_mismatch(
                "supersession",
                Some(session_id.to_string()),
                "one Mob member should own a bridge session at a time",
                format!("multiple members project the same bridge session: {owners:?}"),
                ShadowMismatchTriage::DogmaViolation,
            ));
        }
    }

    for member in &snapshot.members {
        let Some(session_id) = &member.current_session_id else {
            if member.state == MemberState::Active && member.status == MobMemberStatus::Active {
                mismatches.push(composition_lifecycle_supersession_mismatch(
                    "lifecycle",
                    Some(member.meerkat_id.to_string()),
                    "active members should project a current bridge session",
                    "no current bridge session projected".to_string(),
                    ShadowMismatchTriage::DogmaViolation,
                ));
            }
            continue;
        };

        if member.member_ref.session_id() != Some(session_id) {
            mismatches.push(composition_lifecycle_supersession_mismatch(
                "supersession",
                Some(member.meerkat_id.to_string()),
                "projected bridge session should match canonical roster member_ref",
                format!(
                    "member_ref session {:?} != projected current_session_id {}",
                    member.member_ref.session_id(),
                    session_id
                ),
                ShadowMismatchTriage::DogmaViolation,
            ));
        }

        let live = session_live.get(session_id).copied().unwrap_or(false);
        let spine = runtime_spines
            .get(session_id)
            .and_then(|snapshot| snapshot.as_ref());

        if matches!(
            member.status,
            MobMemberStatus::Completed | MobMemberStatus::Unknown
        ) && (live || spine.is_some())
        {
            mismatches.push(composition_lifecycle_supersession_mismatch(
                "lifecycle",
                Some(member.meerkat_id.to_string()),
                "terminal or unknown members should not retain a live Meerkat bridge",
                format!(
                    "status={:?}, live_session={}, runtime_snapshot_present={}",
                    member.status,
                    live,
                    spine.is_some()
                ),
                ShadowMismatchTriage::ImplementationDetail,
            ));
        }

        if member.state == MemberState::Active && member.status == MobMemberStatus::Active {
            if !live && spine.is_none() {
                mismatches.push(composition_lifecycle_supersession_mismatch(
                    "lifecycle",
                    Some(member.meerkat_id.to_string()),
                    "active members should retain an observable Meerkat runtime bridge",
                    format!("bridge session {session_id} is neither live nor snapshotted"),
                    ShadowMismatchTriage::DogmaViolation,
                ));
            }
            if let Some(spine) = spine {
                if spine.binding.session_id != *session_id {
                    mismatches.push(composition_lifecycle_supersession_mismatch(
                        "supersession",
                        Some(member.meerkat_id.to_string()),
                        "runtime snapshot should be keyed by the projected bridge session",
                        format!(
                            "runtime snapshot session {} != projected current_session_id {}",
                            spine.binding.session_id, session_id
                        ),
                        ShadowMismatchTriage::DogmaViolation,
                    ));
                }

                if matches!(
                    spine.control.phase,
                    RuntimeState::Retired | RuntimeState::Stopped | RuntimeState::Destroyed
                ) {
                    mismatches.push(composition_lifecycle_supersession_mismatch(
                        "lifecycle",
                        Some(member.meerkat_id.to_string()),
                        "active members should not point at a terminal or stopped Meerkat runtime",
                        format!("projected runtime phase is {:?}", spine.control.phase),
                        ShadowMismatchTriage::ImplementationDetail,
                    ));
                }
            }
        }
    }

    mismatches
}

pub(crate) async fn capture_composition_shadow_report(
    handle: &MobHandle,
    lane: CompositionShadowLane,
) -> CompositionShadowReport {
    let snapshot = capture_mob_machine_snapshot(handle).await;
    let session_ids: Vec<_> = snapshot
        .members
        .iter()
        .filter_map(|member| member.current_session_id.clone())
        .collect();
    let mut session_live = HashMap::new();
    let mut runtime_spines = HashMap::new();
    let runtime_adapter = handle.diagnostic_runtime_adapter();
    for session_id in session_ids {
        let live = handle.diagnostic_has_live_session(&session_id).await;
        session_live.insert(session_id.clone(), live);
        let spine = match runtime_adapter.as_ref() {
            Some(adapter) => adapter.meerkat_machine_spine_snapshot(&session_id).await,
            None => None,
        };
        runtime_spines.insert(session_id, spine);
    }

    match lane {
        CompositionShadowLane::LifecycleSupersession => CompositionShadowReport {
            lane,
            mismatches: capture_composition_lifecycle_supersession_shadow_mismatches(
                &snapshot,
                &session_live,
                &runtime_spines,
            ),
        },
        CompositionShadowLane::WorkBridge => CompositionShadowReport {
            lane,
            mismatches: capture_composition_work_bridge_shadow_mismatches(
                &snapshot,
                &runtime_spines,
            ),
        },
    }
}

pub(crate) async fn capture_mob_shadow_suite_report(handle: &MobHandle) -> MobShadowSuiteReport {
    let mut mob = Vec::new();
    for lane in [
        MobShadowLane::ProvisioningLifecycle,
        MobShadowLane::FlowFrameLoop,
        MobShadowLane::TaskHistoryRecovery,
    ] {
        mob.push(capture_mob_shadow_report(handle, lane).await);
    }

    let mut composition = Vec::new();
    for lane in [
        CompositionShadowLane::LifecycleSupersession,
        CompositionShadowLane::WorkBridge,
    ] {
        composition.push(capture_composition_shadow_report(handle, lane).await);
    }

    MobShadowSuiteReport { mob, composition }
}

fn push_topology_mismatches(
    violations: &mut Vec<MobMachineInvariantViolation>,
    orchestrator: &MobOrchestratorSnapshot,
    topology: crate::runtime::MobTopologySnapshot,
) {
    if orchestrator.coordinator_bound != topology.coordinator_bound {
        violations.push(
            MobMachineInvariantViolation::TopologyCoordinatorBindingMismatch {
                orchestrator_bound: orchestrator.coordinator_bound,
                topology_bound: topology.coordinator_bound,
            },
        );
    }

    if orchestrator.topology_revision != topology.revision {
        violations.push(MobMachineInvariantViolation::TopologyRevisionMismatch {
            orchestrator_revision: orchestrator.topology_revision,
            topology_revision: topology.revision,
        });
    }
}

fn push_flow_tracker_mismatches(
    violations: &mut Vec<MobMachineInvariantViolation>,
    kernel: &MobKernelDiagnosticSnapshot,
) {
    let run_task_count = kernel.flow_trackers.run_task_ids.len();
    if kernel.lifecycle.active_run_count as usize != run_task_count {
        violations.push(
            MobMachineInvariantViolation::LifecycleRunTrackerCountMismatch {
                active_run_count: kernel.lifecycle.active_run_count,
                run_task_count,
            },
        );
    }

    if let Some(orchestrator) = &kernel.orchestrator
        && orchestrator.active_flow_count as usize != run_task_count
    {
        violations.push(
            MobMachineInvariantViolation::OrchestratorFlowTrackerCountMismatch {
                active_flow_count: orchestrator.active_flow_count,
                run_task_count,
            },
        );
    }

    if kernel.flow_trackers.run_task_ids != kernel.flow_trackers.cancel_token_ids {
        violations.push(MobMachineInvariantViolation::FlowTaskTokenMismatch {
            run_task_ids: kernel.flow_trackers.run_task_ids.iter().cloned().collect(),
            cancel_token_ids: kernel
                .flow_trackers
                .cancel_token_ids
                .iter()
                .cloned()
                .collect(),
        });
    }

    for run_id in kernel
        .flow_trackers
        .stream_ids
        .difference(&kernel.flow_trackers.run_task_ids)
    {
        violations.push(MobMachineInvariantViolation::FlowStreamWithoutTrackedRun {
            run_id: run_id.clone(),
        });
    }
}

fn push_pending_spawn_mismatches(
    violations: &mut Vec<MobMachineInvariantViolation>,
    snapshot: &MobMachineSnapshot,
) {
    let pending = &snapshot.kernel.pending_spawns;
    let lineage_count = pending.ticket_members.len();

    if let Some(orchestrator) = &snapshot.kernel.orchestrator
        && orchestrator.pending_spawn_count as usize != lineage_count
    {
        violations.push(MobMachineInvariantViolation::PendingSpawnCountMismatch {
            pending_spawn_count: orchestrator.pending_spawn_count,
            lineage_count,
        });
    }

    if pending.metadata_ticket_ids != pending.task_ticket_ids {
        violations.push(
            MobMachineInvariantViolation::PendingSpawnTicketAlignmentMismatch {
                metadata_ticket_ids: pending.metadata_ticket_ids.iter().copied().collect(),
                task_ticket_ids: pending.task_ticket_ids.iter().copied().collect(),
            },
        );
    }

    for ticket in &pending.partial_progress_ticket_ids {
        violations.push(
            MobMachineInvariantViolation::PendingSpawnPartialProvisionBinding { ticket: *ticket },
        );
    }

    let mut seen_members = BTreeSet::new();
    let projected_member_ids = snapshot
        .members
        .iter()
        .map(|entry| entry.meerkat_id.clone())
        .collect::<BTreeSet<_>>();

    for meerkat_id in pending.ticket_members.values() {
        if !seen_members.insert(meerkat_id.clone()) {
            violations.push(MobMachineInvariantViolation::PendingSpawnDuplicateMember {
                meerkat_id: meerkat_id.clone(),
            });
        }
        if snapshot.roster.get(meerkat_id).is_some() {
            violations.push(
                MobMachineInvariantViolation::PendingSpawnMemberAlreadyInRoster {
                    meerkat_id: meerkat_id.clone(),
                },
            );
        }
        if projected_member_ids.contains(meerkat_id) {
            violations.push(
                MobMachineInvariantViolation::PendingSpawnMemberAlreadyProjected {
                    meerkat_id: meerkat_id.clone(),
                },
            );
        }
    }
}

fn push_kickoff_barrier_mismatches(
    violations: &mut Vec<MobMachineInvariantViolation>,
    snapshot: &MobMachineSnapshot,
) {
    let pending_spawn_members: BTreeSet<_> = snapshot
        .kernel
        .pending_spawns
        .ticket_members
        .values()
        .cloned()
        .collect();

    let projected_members: HashMap<_, _> = snapshot
        .members
        .iter()
        .map(|entry| (entry.meerkat_id.clone(), entry))
        .collect();

    for meerkat_id in &snapshot.kernel.kickoff_barrier.pending_member_ids {
        if pending_spawn_members.contains(meerkat_id) {
            violations.push(
                MobMachineInvariantViolation::KickoffPendingMemberOverlapsPendingSpawn {
                    meerkat_id: meerkat_id.clone(),
                },
            );
        }

        if snapshot.roster.get(meerkat_id).is_none() {
            violations.push(
                MobMachineInvariantViolation::KickoffPendingMemberMissingRosterEntry {
                    meerkat_id: meerkat_id.clone(),
                },
            );
            continue;
        }

        let Some(projected) = projected_members.get(meerkat_id) else {
            violations.push(
                MobMachineInvariantViolation::KickoffPendingMemberMissingProjectedEntry {
                    meerkat_id: meerkat_id.clone(),
                },
            );
            continue;
        };

        if projected.current_session_id.is_none() {
            violations.push(
                MobMachineInvariantViolation::KickoffPendingMemberMissingSessionBinding {
                    meerkat_id: meerkat_id.clone(),
                },
            );
        }

        if projected.status == MobMemberStatus::Broken {
            violations.push(MobMachineInvariantViolation::KickoffPendingMemberBroken {
                meerkat_id: meerkat_id.clone(),
            });
        }
    }
}

fn push_tracked_run_store_mismatches(
    violations: &mut Vec<MobMachineInvariantViolation>,
    snapshot: &MobMachineSnapshot,
) {
    let mut tracked_run_ids = BTreeSet::new();
    tracked_run_ids.extend(snapshot.kernel.flow_trackers.run_task_ids.iter().cloned());
    tracked_run_ids.extend(
        snapshot
            .kernel
            .flow_trackers
            .cancel_token_ids
            .iter()
            .cloned(),
    );
    tracked_run_ids.extend(snapshot.kernel.flow_trackers.stream_ids.iter().cloned());

    for run_id in &tracked_run_ids {
        match snapshot.tracked_runs.get(run_id) {
            Some(TrackedRunSnapshot::Missing) | None => {
                violations.push(MobMachineInvariantViolation::TrackedRunMissingStoreEntry {
                    run_id: run_id.clone(),
                });
            }
            Some(TrackedRunSnapshot::Malformed(malformed)) => {
                violations.push(MobMachineInvariantViolation::TrackedRunMalformed {
                    run_id: run_id.clone(),
                    reason: malformed.reason.clone(),
                });
            }
            Some(TrackedRunSnapshot::Present(_)) => {}
        }
    }

    for run_id in &tracked_run_ids {
        let Some(TrackedRunSnapshot::Present(run_projection)) = snapshot.tracked_runs.get(run_id)
        else {
            continue;
        };

        if run_projection.status.is_terminal() != run_projection.completed_at_present {
            violations.push(
                MobMachineInvariantViolation::TrackedRunCompletionTimestampMismatch {
                    run_id: run_id.clone(),
                    status: run_projection.status.clone(),
                    completed_at_present: run_projection.completed_at_present,
                },
            );
        }

        if run_projection.ordered_steps.is_empty() {
            violations.push(
                MobMachineInvariantViolation::TrackedRunMissingOrderedSteps {
                    run_id: run_id.clone(),
                },
            );
        }

        if run_projection.consecutive_failure_count > run_projection.failure_count {
            violations.push(
                MobMachineInvariantViolation::TrackedRunConsecutiveFailureCountExceedsFailureCount {
                    run_id: run_id.clone(),
                    failure_count: run_projection.failure_count,
                    consecutive_failure_count: run_projection.consecutive_failure_count,
                },
            );
        }

        if run_projection.loop_iteration_count > 0
            && (run_projection.loop_count == 0 || run_projection.frame_count == 0)
        {
            violations.push(
                MobMachineInvariantViolation::TrackedRunLoopIterationWithoutLoopStructure {
                    run_id: run_id.clone(),
                    loop_iteration_count: run_projection.loop_iteration_count,
                    loop_count: run_projection.loop_count,
                    frame_count: run_projection.frame_count,
                },
            );
        }

        if run_projection.loop_count > 0 && run_projection.frame_count == 0 {
            violations.push(
                MobMachineInvariantViolation::TrackedRunLoopWithoutFrameStructure {
                    run_id: run_id.clone(),
                    loop_count: run_projection.loop_count,
                    frame_count: run_projection.frame_count,
                },
            );
        }

        if run_projection.schema_version < 4
            && (run_projection.frame_count > 0
                || run_projection.loop_count > 0
                || run_projection.loop_iteration_count > 0)
        {
            violations.push(
                MobMachineInvariantViolation::TrackedRunStructuredStateWithLegacySchemaVersion {
                    run_id: run_id.clone(),
                    schema_version: run_projection.schema_version,
                    frame_count: run_projection.frame_count,
                    loop_count: run_projection.loop_count,
                    loop_iteration_count: run_projection.loop_iteration_count,
                },
            );
        }

        let mut ordered_membership = BTreeSet::new();
        for step_id in &run_projection.ordered_steps {
            if !ordered_membership.insert(step_id.clone()) {
                violations.push(
                    MobMachineInvariantViolation::TrackedRunOrderedStepsDuplicate {
                        run_id: run_id.clone(),
                        step_id: step_id.clone(),
                    },
                );
            }
            if !run_projection.step_dependencies.contains_key(step_id) {
                violations.push(
                    MobMachineInvariantViolation::TrackedRunDependencyMapMissingOrderedStep {
                        run_id: run_id.clone(),
                        step_id: step_id.clone(),
                    },
                );
            }
            if !run_projection.step_dependency_modes.contains_key(step_id) {
                violations.push(
                    MobMachineInvariantViolation::TrackedRunDependencyModeMissingOrderedStep {
                        run_id: run_id.clone(),
                        step_id: step_id.clone(),
                    },
                );
            }
            if !run_projection.step_has_conditions.contains_key(step_id) {
                violations.push(
                    MobMachineInvariantViolation::TrackedRunConditionFlagMissingOrderedStep {
                        run_id: run_id.clone(),
                        step_id: step_id.clone(),
                    },
                );
            }
            if !run_projection.step_branches.contains_key(step_id) {
                violations.push(
                    MobMachineInvariantViolation::TrackedRunBranchMissingOrderedStep {
                        run_id: run_id.clone(),
                        step_id: step_id.clone(),
                    },
                );
            }
            if !run_projection
                .step_collection_policy_kinds
                .contains_key(step_id)
            {
                violations.push(
                    MobMachineInvariantViolation::TrackedRunCollectionPolicyMissingOrderedStep {
                        run_id: run_id.clone(),
                        step_id: step_id.clone(),
                    },
                );
            }
            if !run_projection.step_quorum_thresholds.contains_key(step_id) {
                violations.push(
                    MobMachineInvariantViolation::TrackedRunQuorumThresholdMissingOrderedStep {
                        run_id: run_id.clone(),
                        step_id: step_id.clone(),
                    },
                );
            }
        }

        for (step_id, dependencies) in &run_projection.step_dependencies {
            if !ordered_membership.contains(step_id) {
                violations.push(
                    MobMachineInvariantViolation::TrackedRunDependencyMapUnknownStep {
                        run_id: run_id.clone(),
                        step_id: step_id.clone(),
                    },
                );
            }
            for dependency_step_id in dependencies {
                if !ordered_membership.contains(dependency_step_id) {
                    violations.push(
                        MobMachineInvariantViolation::TrackedRunDependencyUnknownStep {
                            run_id: run_id.clone(),
                            step_id: step_id.clone(),
                            dependency_step_id: dependency_step_id.clone(),
                        },
                    );
                }
                let duplicate_dependency_count = dependencies
                    .iter()
                    .filter(|candidate| *candidate == dependency_step_id)
                    .count();
                if duplicate_dependency_count > 1 {
                    violations.push(
                        MobMachineInvariantViolation::TrackedRunDuplicateDependency {
                            run_id: run_id.clone(),
                            step_id: step_id.clone(),
                            dependency_step_id: dependency_step_id.clone(),
                        },
                    );
                }
                if dependency_step_id == step_id {
                    violations.push(MobMachineInvariantViolation::TrackedRunSelfDependency {
                        run_id: run_id.clone(),
                        step_id: step_id.clone(),
                    });
                }
            }
        }

        for step_id in run_projection.step_dependency_modes.keys() {
            if !ordered_membership.contains(step_id) {
                violations.push(
                    MobMachineInvariantViolation::TrackedRunDependencyModeMapUnknownStep {
                        run_id: run_id.clone(),
                        step_id: step_id.clone(),
                    },
                );
            }
        }

        for (step_id, dependency_mode) in &run_projection.step_dependency_modes {
            if *dependency_mode != DependencyMode::Any {
                continue;
            }
            let has_branch_dependency = run_projection
                .step_dependencies
                .get(step_id)
                .map(|dependencies| {
                    dependencies.iter().any(|dependency_step_id| {
                        run_projection
                            .step_branches
                            .get(dependency_step_id)
                            .is_some_and(Option::is_some)
                    })
                })
                .unwrap_or(false);
            if !has_branch_dependency {
                violations.push(
                    MobMachineInvariantViolation::TrackedRunAnyDependencyModeWithoutBranchDependency {
                        run_id: run_id.clone(),
                        step_id: step_id.clone(),
                    },
                );
            }
        }

        for step_id in run_projection.step_has_conditions.keys() {
            if !ordered_membership.contains(step_id) {
                violations.push(
                    MobMachineInvariantViolation::TrackedRunConditionFlagUnknownStep {
                        run_id: run_id.clone(),
                        step_id: step_id.clone(),
                    },
                );
            }
        }

        for step_id in run_projection.step_branches.keys() {
            if !ordered_membership.contains(step_id) {
                violations.push(MobMachineInvariantViolation::TrackedRunBranchUnknownStep {
                    run_id: run_id.clone(),
                    step_id: step_id.clone(),
                });
            }
        }

        for (step_id, branch) in &run_projection.step_branches {
            let Some(branch_id) = branch else {
                continue;
            };
            if run_projection.step_has_conditions.get(step_id) == Some(&false) {
                violations.push(
                    MobMachineInvariantViolation::TrackedRunBranchWithoutCondition {
                        run_id: run_id.clone(),
                        step_id: step_id.clone(),
                        branch_id: branch_id.clone(),
                    },
                );
            }
        }

        let mut branch_reference_dependencies: BTreeMap<BranchId, (StepId, BTreeSet<StepId>)> =
            BTreeMap::new();
        let mut branch_member_counts: BTreeMap<BranchId, usize> = BTreeMap::new();
        for (step_id, branch) in &run_projection.step_branches {
            let Some(branch_id) = branch else {
                continue;
            };
            *branch_member_counts.entry(branch_id.clone()).or_insert(0) += 1;
            let Some(dependencies) = run_projection.step_dependencies.get(step_id) else {
                continue;
            };
            let dependency_set = dependencies.iter().cloned().collect::<BTreeSet<_>>();
            match branch_reference_dependencies.get(branch_id) {
                None => {
                    branch_reference_dependencies
                        .insert(branch_id.clone(), (step_id.clone(), dependency_set));
                }
                Some((reference_step_id, reference_dependencies))
                    if reference_dependencies != &dependency_set =>
                {
                    violations.push(
                        MobMachineInvariantViolation::TrackedRunBranchConflictingDependencies {
                            run_id: run_id.clone(),
                            branch_id: branch_id.clone(),
                            reference_step_id: reference_step_id.clone(),
                            conflicting_step_id: step_id.clone(),
                        },
                    );
                }
                Some(_) => {}
            }
        }
        for (branch_id, member_count) in branch_member_counts {
            if member_count < 2 {
                violations.push(
                    MobMachineInvariantViolation::TrackedRunBranchGroupTooSmall {
                        run_id: run_id.clone(),
                        branch_id,
                        member_count,
                    },
                );
            }
        }

        for step_id in run_projection.step_collection_policy_kinds.keys() {
            if !ordered_membership.contains(step_id) {
                violations.push(
                    MobMachineInvariantViolation::TrackedRunCollectionPolicyUnknownStep {
                        run_id: run_id.clone(),
                        step_id: step_id.clone(),
                    },
                );
            }
        }

        for (step_id, quorum_threshold) in &run_projection.step_quorum_thresholds {
            if !ordered_membership.contains(step_id) {
                violations.push(
                    MobMachineInvariantViolation::TrackedRunQuorumThresholdUnknownStep {
                        run_id: run_id.clone(),
                        step_id: step_id.clone(),
                    },
                );
                continue;
            }

            if let Some(policy_kind) = run_projection.step_collection_policy_kinds.get(step_id) {
                let threshold_matches = match policy_kind {
                    RunCollectionPolicyKind::Quorum => *quorum_threshold > 0,
                    RunCollectionPolicyKind::All | RunCollectionPolicyKind::Any => {
                        *quorum_threshold == 0
                    }
                };
                if !threshold_matches {
                    violations.push(
                        MobMachineInvariantViolation::TrackedRunQuorumThresholdShapeMismatch {
                            run_id: run_id.clone(),
                            step_id: step_id.clone(),
                            policy_kind: *policy_kind,
                            quorum_threshold: *quorum_threshold,
                        },
                    );
                }
            }
        }

        for (step_id, status) in &run_projection.step_statuses {
            if !ordered_membership.contains(step_id) {
                violations.push(
                    MobMachineInvariantViolation::TrackedRunStepStatusUnknownStep {
                        run_id: run_id.clone(),
                        step_id: step_id.clone(),
                    },
                );
            }

            if run_projection.status.is_terminal() && *status == StepRunStatus::Dispatched {
                violations.push(
                    MobMachineInvariantViolation::TrackedRunTerminalRunHasDispatchedStep {
                        run_id: run_id.clone(),
                        step_id: step_id.clone(),
                        status: run_projection.status.clone(),
                    },
                );
            }
            if run_projection.status == MobRunStatus::Completed
                && !matches!(status, StepRunStatus::Completed | StepRunStatus::Skipped)
            {
                violations.push(
                    MobMachineInvariantViolation::TrackedRunCompletedRunHasNonCompletedStep {
                        run_id: run_id.clone(),
                        step_id: step_id.clone(),
                        step_status: status.clone(),
                    },
                );
            }
        }
    }

    for (run_id, tracker_flow_id) in &snapshot.kernel.flow_trackers.tracked_flows {
        if let Some(TrackedRunSnapshot::Present(run_projection)) = snapshot.tracked_runs.get(run_id)
        {
            if run_projection.flow_id != *tracker_flow_id {
                violations.push(MobMachineInvariantViolation::TrackedRunFlowIdMismatch {
                    run_id: run_id.clone(),
                    tracker_flow_id: tracker_flow_id.clone(),
                    store_flow_id: run_projection.flow_id.clone(),
                });
            }
        }
    }
}

fn push_restore_failure_mismatches(
    violations: &mut Vec<MobMachineInvariantViolation>,
    snapshot: &MobMachineSnapshot,
    projected_members: &HashMap<MeerkatId, &MobMemberListEntry>,
) {
    for (meerkat_id, failure) in &snapshot.restore_failures {
        if snapshot.roster.get(meerkat_id).is_none() {
            violations.push(
                MobMachineInvariantViolation::RestoreFailureMissingRosterEntry {
                    meerkat_id: meerkat_id.clone(),
                },
            );
        }

        let Some(projected) = projected_members.get(meerkat_id) else {
            violations.push(
                MobMachineInvariantViolation::RestoreFailureMissingProjectedMember {
                    meerkat_id: meerkat_id.clone(),
                },
            );
            continue;
        };

        if projected.status != MobMemberStatus::Broken {
            violations.push(
                MobMachineInvariantViolation::RestoreFailureProjectedStatusMismatch {
                    meerkat_id: meerkat_id.clone(),
                    status: projected.status,
                },
            );
        }
        if projected.current_session_id.as_ref() != Some(&failure.session_id) {
            violations.push(
                MobMachineInvariantViolation::RestoreFailureProjectedSessionMismatch {
                    meerkat_id: meerkat_id.clone(),
                },
            );
        }
        if projected.error.as_deref() != Some(failure.reason.as_str()) {
            violations.push(
                MobMachineInvariantViolation::RestoreFailureProjectedReasonMismatch {
                    meerkat_id: meerkat_id.clone(),
                },
            );
        }
    }

    for projected in projected_members.values() {
        if projected.status == MobMemberStatus::Broken
            && !snapshot
                .restore_failures
                .contains_key(&projected.meerkat_id)
        {
            violations.push(
                MobMachineInvariantViolation::ProjectedBrokenMemberMissingRestoreFailure {
                    meerkat_id: projected.meerkat_id.clone(),
                },
            );
        }
    }
}

fn push_structural_mismatches(
    violations: &mut Vec<MobMachineInvariantViolation>,
    projected: &MobMemberListEntry,
    roster_entry: &RosterEntry,
) {
    let meerkat_id = projected.meerkat_id.clone();

    if projected.profile != roster_entry.profile {
        violations.push(MobMachineInvariantViolation::ProjectedStructuralMismatch {
            meerkat_id: meerkat_id.clone(),
            field: "profile",
        });
    }
    if projected.member_ref != roster_entry.member_ref {
        violations.push(MobMachineInvariantViolation::ProjectedStructuralMismatch {
            meerkat_id: meerkat_id.clone(),
            field: "member_ref",
        });
    }
    if projected.runtime_mode != roster_entry.runtime_mode {
        violations.push(MobMachineInvariantViolation::ProjectedStructuralMismatch {
            meerkat_id: meerkat_id.clone(),
            field: "runtime_mode",
        });
    }
    if projected.peer_id != roster_entry.peer_id {
        violations.push(MobMachineInvariantViolation::ProjectedStructuralMismatch {
            meerkat_id: meerkat_id.clone(),
            field: "peer_id",
        });
    }
    if projected.state != roster_entry.state {
        violations.push(MobMachineInvariantViolation::ProjectedStructuralMismatch {
            meerkat_id: meerkat_id.clone(),
            field: "state",
        });
    }
    if projected.wired_to != roster_entry.wired_to {
        violations.push(MobMachineInvariantViolation::ProjectedStructuralMismatch {
            meerkat_id: meerkat_id.clone(),
            field: "wired_to",
        });
    }
    if projected.external_peer_specs != roster_entry.external_peer_specs {
        violations.push(MobMachineInvariantViolation::ProjectedStructuralMismatch {
            meerkat_id: meerkat_id.clone(),
            field: "external_peer_specs",
        });
    }
    if projected.labels != roster_entry.labels {
        violations.push(MobMachineInvariantViolation::ProjectedStructuralMismatch {
            meerkat_id: meerkat_id.clone(),
            field: "labels",
        });
    }
    if projected.current_session_id != roster_entry.member_ref.session_id().cloned() {
        violations.push(MobMachineInvariantViolation::ProjectedStructuralMismatch {
            meerkat_id,
            field: "current_session_id",
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::MemberRef;
    use crate::ids::ProfileName;
    use crate::roster::{Roster, RosterAddEntry};
    use crate::runtime_mode::MobRuntimeMode;
    use meerkat_core::types::SessionId;
    use meerkat_runtime::LogicalRuntimeId;

    fn valid_kernel_snapshot() -> MobKernelDiagnosticSnapshot {
        MobKernelDiagnosticSnapshot {
            lifecycle: crate::runtime::MobLifecycleSnapshot {
                phase: MobState::Running,
                active_run_count: 0,
                cleanup_pending: false,
            },
            orchestrator: Some(MobOrchestratorSnapshot {
                phase: MobState::Running,
                coordinator_bound: true,
                pending_spawn_count: 0,
                active_flow_count: 0,
                topology_revision: 1,
                supervisor_active: true,
            }),
            topology: crate::runtime::MobTopologySnapshot {
                coordinator_bound: true,
                revision: 1,
            },
            pending_spawns: crate::runtime::MobPendingSpawnLineageSnapshot::default(),
            kickoff_barrier: crate::runtime::MobKickoffBarrierSnapshot::default(),
            flow_trackers: crate::runtime::MobFlowTrackerSnapshot::default(),
        }
    }

    fn add_member(roster: &mut Roster, name: &str) -> SessionId {
        let session_id = SessionId::new();
        let inserted = roster.add(RosterAddEntry {
            meerkat_id: MeerkatId::from(name),
            profile: ProfileName::from("worker"),
            effective_profile_override: None,
            runtime_mode: MobRuntimeMode::AutonomousHost,
            member_ref: MemberRef::from_session_id(session_id.clone()),
            peer_id: Some(format!("peer-{name}")),
            labels: Default::default(),
        });
        assert!(inserted);
        session_id
    }

    fn projected_entry(roster: &Roster, name: &str) -> MobMemberListEntry {
        let entry = roster
            .get(&MeerkatId::from(name))
            .expect("roster entry must exist")
            .clone();
        let current_session_id = entry.member_ref.session_id().cloned();
        MobMemberListEntry {
            meerkat_id: entry.meerkat_id,
            profile: entry.profile,
            member_ref: entry.member_ref,
            runtime_mode: entry.runtime_mode,
            peer_id: entry.peer_id,
            state: entry.state,
            wired_to: entry.wired_to,
            external_peer_specs: entry.external_peer_specs,
            labels: entry.labels,
            status: MobMemberStatus::Active,
            error: None,
            is_final: false,
            current_session_id,
            kickoff: entry.kickoff,
        }
    }

    fn active_meerkat_spine(
        session_id: &SessionId,
        phase: RuntimeState,
    ) -> MeerkatMachineSpineSnapshot {
        MeerkatMachineSpineSnapshot {
            binding: meerkat_runtime::MeerkatBindingSnapshot {
                session_id: session_id.clone(),
                runtime_id: LogicalRuntimeId::new(format!("runtime-{session_id}")),
                driver_kind: meerkat_runtime::MeerkatDriverKind::Ephemeral,
                driver_present: true,
                completions_present: true,
                ops_registry_present: true,
                attachment_live: false,
                detached_wake_present: false,
                epoch_id: meerkat_core::RuntimeEpochId::new(),
                cursor_state: meerkat_runtime::MeerkatCursorSnapshot {
                    agent_applied_cursor: 0,
                    runtime_observed_seq: 0,
                    runtime_last_injected_seq: 0,
                },
            },
            control: meerkat_runtime::MeerkatControlSnapshot {
                phase,
                current_run_id: None,
                pre_run_phase: None,
                wake_pending: false,
                process_pending: false,
            },
            inputs: meerkat_runtime::MeerkatInputsSnapshot {
                ingress_phase: meerkat_runtime::runtime_ingress_authority::IngressPhase::Active,
                admission_order: Vec::new(),
                queue: Vec::new(),
                steer_queue: Vec::new(),
                current_run_id: None,
                current_run_contributors: Vec::new(),
                wake_requested: false,
                process_requested: false,
                silent_intent_overrides: Vec::new(),
            },
            completion_waiters: meerkat_runtime::MeerkatCompletionWaitersSnapshot {
                input_count: 0,
                waiter_count: 0,
                waiting_inputs: Vec::new(),
            },
            ops: meerkat_runtime::MeerkatOpsSnapshot {
                operation_count: 0,
                active_count: 0,
                wait_request_id: None,
                pending_wait_present: false,
                pending_wait_request_id: None,
                wait_operation_ids: Vec::new(),
                operations: Vec::new(),
                detached_wake_pending: None,
                detached_wake_signaled: None,
            },
            drain: meerkat_runtime::MeerkatDrainSnapshot {
                slot_present: false,
                phase: None,
                mode: None,
                handle_present: false,
            },
        }
    }

    #[test]
    fn validate_mob_machine_snapshot_reports_projection_violations() {
        let mut roster = Roster::new();
        let worker_sid = add_member(&mut roster, "worker-1");
        let _other_sid = add_member(&mut roster, "worker-2");
        roster.wire(&MeerkatId::from("worker-1"), &MeerkatId::from("worker-2"));

        let mut projected = projected_entry(&roster, "worker-1");
        projected.status = MobMemberStatus::Retiring;
        projected.current_session_id = None;
        projected.is_final = true;
        projected.profile = ProfileName::from("lead");

        let mut kernel = valid_kernel_snapshot();
        kernel.lifecycle.phase = MobState::Completed;
        kernel.lifecycle.active_run_count = 1;
        kernel.lifecycle.cleanup_pending = true;
        let orchestrator = kernel
            .orchestrator
            .as_mut()
            .expect("synthetic kernel snapshot should include orchestrator");
        orchestrator.phase = MobState::Destroyed;
        orchestrator.pending_spawn_count = 1;
        orchestrator.active_flow_count = 1;
        orchestrator.coordinator_bound = true;
        orchestrator.supervisor_active = true;
        kernel.topology.coordinator_bound = false;
        kernel.topology.revision = 0;
        kernel
            .pending_spawns
            .metadata_ticket_ids
            .extend([11_u64, 12_u64]);
        kernel.pending_spawns.task_ticket_ids.insert(11_u64);
        kernel
            .pending_spawns
            .ticket_members
            .insert(11_u64, MeerkatId::from("worker-1"));
        kernel
            .pending_spawns
            .ticket_members
            .insert(12_u64, MeerkatId::from("worker-1"));
        kernel
            .pending_spawns
            .partial_progress_ticket_ids
            .insert(11_u64);
        kernel
            .kickoff_barrier
            .pending_member_ids
            .insert(MeerkatId::from("worker-1"));
        let flow_run_id = crate::ids::RunId::new();
        let second_flow_run_id = crate::ids::RunId::new();
        let stream_only_run_id = crate::ids::RunId::new();
        kernel
            .flow_trackers
            .run_task_ids
            .insert(flow_run_id.clone());
        kernel
            .flow_trackers
            .run_task_ids
            .insert(second_flow_run_id.clone());
        kernel
            .flow_trackers
            .cancel_token_ids
            .insert(flow_run_id.clone());
        kernel
            .flow_trackers
            .tracked_flows
            .insert(flow_run_id.clone(), FlowId::from("demo"));
        kernel
            .flow_trackers
            .stream_ids
            .insert(stream_only_run_id.clone());

        let snapshot = MobMachineSnapshot {
            phase: MobState::Running,
            kernel,
            roster,
            members: vec![projected],
            restore_failures: BTreeMap::from([(
                MeerkatId::from("worker-1"),
                RestoreFailureSnapshot {
                    session_id: worker_sid.clone(),
                    reason: "restore failed".into(),
                },
            )]),
            tracked_runs: BTreeMap::from([(
                flow_run_id.clone(),
                TrackedRunSnapshot::Present(TrackedRunStoreSnapshot {
                    flow_id: FlowId::from("other"),
                    schema_version: 4,
                    status: MobRunStatus::Running,
                    completed_at_present: false,
                    frame_count: 0,
                    loop_count: 0,
                    loop_iteration_count: 0,
                    ordered_steps: vec![StepId::from("start")],
                    step_dependencies: BTreeMap::from([(StepId::from("start"), Vec::new())]),
                    step_dependency_modes: BTreeMap::from([(
                        StepId::from("start"),
                        DependencyMode::All,
                    )]),
                    step_has_conditions: BTreeMap::from([(StepId::from("start"), false)]),
                    step_branches: BTreeMap::from([(StepId::from("start"), None)]),
                    step_collection_policy_kinds: BTreeMap::from([(
                        StepId::from("start"),
                        RunCollectionPolicyKind::All,
                    )]),
                    step_quorum_thresholds: BTreeMap::from([(StepId::from("start"), 0)]),
                    step_statuses: BTreeMap::new(),
                    failure_count: 0,
                    consecutive_failure_count: 0,
                    max_step_retries: 0,
                    escalation_threshold: 0,
                }),
            )]),
        };

        let violations = validate_mob_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::RosterMemberMissingProjectedEntry { meerkat_id }
                if meerkat_id == &MeerkatId::from("worker-2")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::ProjectedStructuralMismatch { meerkat_id, field }
                if meerkat_id == &MeerkatId::from("worker-1") && *field == "profile"
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::ProjectedStructuralMismatch { meerkat_id, field }
                if meerkat_id == &MeerkatId::from("worker-1") && *field == "current_session_id"
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::ActiveMemberMarkedRetiring { meerkat_id }
                if meerkat_id == &MeerkatId::from("worker-1")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::LifecyclePhaseMismatch {
                public_phase: MobState::Running,
                lifecycle_phase: MobState::Completed,
            }
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::LifecycleTerminalPhaseHasActiveRuns {
                phase: MobState::Completed,
                active_run_count: 1,
            }
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::OrchestratorPhaseMismatch {
                public_phase: MobState::Running,
                orchestrator_phase: MobState::Destroyed,
            }
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::OrchestratorTerminalPhaseHasInFlightWork {
                phase: MobState::Destroyed,
                pending_spawn_count: 1,
                active_flow_count: 1,
            }
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::DestroyedOrchestratorStillBound {
                coordinator_bound: true,
                supervisor_active: true,
            }
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TopologyCoordinatorBindingMismatch {
                orchestrator_bound: true,
                topology_bound: false,
            }
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TopologyRevisionMismatch {
                orchestrator_revision: 1,
                topology_revision: 0,
            }
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::LifecycleRunTrackerCountMismatch {
                active_run_count: 1,
                run_task_count: 2,
            }
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::OrchestratorFlowTrackerCountMismatch {
                active_flow_count: 1,
                run_task_count: 2,
            }
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::PendingSpawnCountMismatch {
                pending_spawn_count: 1,
                lineage_count: 2,
            }
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::PendingSpawnTicketAlignmentMismatch {
                metadata_ticket_ids,
                task_ticket_ids,
            } if metadata_ticket_ids == &vec![11, 12] && task_ticket_ids == &vec![11]
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::PendingSpawnDuplicateMember { meerkat_id }
                if meerkat_id == &MeerkatId::from("worker-1")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::PendingSpawnMemberAlreadyInRoster { meerkat_id }
                if meerkat_id == &MeerkatId::from("worker-1")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::PendingSpawnMemberAlreadyProjected { meerkat_id }
                if meerkat_id == &MeerkatId::from("worker-1")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::PendingSpawnPartialProvisionBinding { ticket }
                if ticket == &11
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::KickoffPendingMemberOverlapsPendingSpawn { meerkat_id }
                if meerkat_id == &MeerkatId::from("worker-1")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::RestoreFailureProjectedStatusMismatch {
                meerkat_id,
                status: MobMemberStatus::Retiring,
            } if meerkat_id == &MeerkatId::from("worker-1")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::RestoreFailureProjectedReasonMismatch { meerkat_id }
                if meerkat_id == &MeerkatId::from("worker-1")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::FlowTaskTokenMismatch { run_task_ids, cancel_token_ids }
                if run_task_ids.len() == 2
                    && run_task_ids.contains(&flow_run_id)
                    && run_task_ids.contains(&second_flow_run_id)
                    && cancel_token_ids == &vec![flow_run_id.clone()]
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::FlowStreamWithoutTrackedRun { run_id }
                if run_id == &stream_only_run_id
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunMissingStoreEntry { run_id }
                if run_id == &second_flow_run_id
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunMissingStoreEntry { run_id }
                if run_id == &stream_only_run_id
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunFlowIdMismatch {
                run_id,
                tracker_flow_id,
                store_flow_id,
            }
                if run_id == &flow_run_id
                    && tracker_flow_id == &FlowId::from("demo")
                    && store_flow_id == &FlowId::from("other")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::MemberFinalityMismatch { meerkat_id, status: MobMemberStatus::Retiring, is_final: true }
                if meerkat_id == &MeerkatId::from("worker-1")
        )));

        assert_eq!(
            snapshot
                .roster
                .get(&MeerkatId::from("worker-1"))
                .and_then(|entry| entry.member_ref.session_id()),
            Some(&worker_sid)
        );
    }

    #[test]
    fn capture_mob_shadow_report_reports_provisioning_lifecycle_mismatches() {
        let mut roster = Roster::new();
        let worker_sid = add_member(&mut roster, "worker-broken");
        let mut projected = projected_entry(&roster, "worker-broken");
        projected.status = MobMemberStatus::Retiring;
        projected.current_session_id = None;

        let mut kernel = valid_kernel_snapshot();
        kernel.lifecycle.phase = MobState::Completed;
        kernel.lifecycle.active_run_count = 1;
        let orchestrator = kernel
            .orchestrator
            .as_mut()
            .expect("synthetic kernel snapshot should include orchestrator");
        orchestrator.phase = MobState::Destroyed;
        orchestrator.pending_spawn_count = 1;
        orchestrator.active_flow_count = 1;
        orchestrator.coordinator_bound = true;
        orchestrator.supervisor_active = true;

        let snapshot = MobMachineSnapshot {
            phase: MobState::Running,
            kernel,
            roster,
            members: vec![projected],
            restore_failures: BTreeMap::from([(
                MeerkatId::from("worker-broken"),
                RestoreFailureSnapshot {
                    session_id: worker_sid,
                    reason: "restore failed".into(),
                },
            )]),
            tracked_runs: BTreeMap::new(),
        };

        let mismatches = capture_provisioning_lifecycle_shadow_mismatches(&snapshot);

        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MobShadowMismatch {
                lane: MobShadowLane::ProvisioningLifecycle,
                region: "lifecycle",
                triage: ShadowMismatchTriage::DogmaViolation,
                ..
            } if mismatch.expected_summary.contains("public mob phase matches lifecycle authority phase")
        )));
        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MobShadowMismatch {
                lane: MobShadowLane::ProvisioningLifecycle,
                region: "orchestrator",
                triage: ShadowMismatchTriage::ImplementationDetail,
                ..
            } if mismatch.observed_summary.contains("pending_spawn_count = 1")
        )));
        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MobShadowMismatch {
                lane: MobShadowLane::ProvisioningLifecycle,
                region: "restore_failures",
                triage: ShadowMismatchTriage::ImplementationDetail,
                ..
            } if mismatch.observed_summary.contains("projected status = Retiring")
        )));
    }

    #[test]
    fn capture_mob_shadow_report_reports_flow_frame_loop_mismatches() {
        let flow_run_id = crate::ids::RunId::new();
        let snapshot = MobMachineSnapshot {
            phase: MobState::Running,
            kernel: MobKernelDiagnosticSnapshot {
                lifecycle: crate::runtime::MobLifecycleSnapshot {
                    phase: MobState::Running,
                    active_run_count: 1,
                    cleanup_pending: false,
                },
                orchestrator: Some(MobOrchestratorSnapshot {
                    phase: MobState::Running,
                    coordinator_bound: true,
                    pending_spawn_count: 0,
                    active_flow_count: 1,
                    topology_revision: 1,
                    supervisor_active: true,
                }),
                topology: crate::runtime::MobTopologySnapshot {
                    coordinator_bound: true,
                    revision: 1,
                },
                pending_spawns: crate::runtime::MobPendingSpawnLineageSnapshot::default(),
                kickoff_barrier: crate::runtime::MobKickoffBarrierSnapshot::default(),
                flow_trackers: crate::runtime::MobFlowTrackerSnapshot {
                    run_task_ids: BTreeSet::from([flow_run_id.clone()]),
                    cancel_token_ids: BTreeSet::from([flow_run_id.clone()]),
                    stream_ids: BTreeSet::new(),
                    tracked_flows: BTreeMap::from([(flow_run_id.clone(), FlowId::from("demo"))]),
                },
            },
            roster: Roster::new(),
            members: Vec::new(),
            restore_failures: BTreeMap::new(),
            tracked_runs: BTreeMap::from([(
                flow_run_id.clone(),
                TrackedRunSnapshot::Present(TrackedRunStoreSnapshot {
                    flow_id: FlowId::from("demo"),
                    schema_version: 4,
                    status: MobRunStatus::Running,
                    completed_at_present: false,
                    frame_count: 0,
                    loop_count: 0,
                    loop_iteration_count: 0,
                    ordered_steps: vec![StepId::from("start"), StepId::from("start")],
                    step_dependencies: BTreeMap::from([(
                        StepId::from("start"),
                        vec![
                            StepId::from("missing"),
                            StepId::from("start"),
                            StepId::from("start"),
                        ],
                    )]),
                    step_dependency_modes: BTreeMap::from([(
                        StepId::from("start"),
                        DependencyMode::All,
                    )]),
                    step_has_conditions: BTreeMap::from([(StepId::from("start"), false)]),
                    step_branches: BTreeMap::from([(StepId::from("start"), None)]),
                    step_collection_policy_kinds: BTreeMap::from([(
                        StepId::from("start"),
                        RunCollectionPolicyKind::All,
                    )]),
                    step_quorum_thresholds: BTreeMap::from([(StepId::from("start"), 0)]),
                    step_statuses: BTreeMap::from([(
                        StepId::from("missing"),
                        StepRunStatus::Dispatched,
                    )]),
                    failure_count: 0,
                    consecutive_failure_count: 0,
                    max_step_retries: 0,
                    escalation_threshold: 0,
                }),
            )]),
        };

        let mismatches = capture_flow_frame_loop_shadow_mismatches(&snapshot);

        assert!(mismatches.iter().any(|mismatch| {
            mismatch.lane == MobShadowLane::FlowFrameLoop
                && mismatch.region == "tracked_runs"
                && mismatch.observed_summary.contains("duplicate ordered step")
        }));
        assert!(mismatches.iter().any(|mismatch| {
            mismatch.lane == MobShadowLane::FlowFrameLoop
                && mismatch.region == "tracked_runs"
                && mismatch
                    .observed_summary
                    .contains("depends on unknown step")
        }));
        assert!(mismatches.iter().any(|mismatch| {
            mismatch.lane == MobShadowLane::FlowFrameLoop
                && mismatch.region == "tracked_runs"
                && mismatch
                    .observed_summary
                    .contains("duplicate or self dependency state")
        }));
        assert!(mismatches.iter().any(|mismatch| {
            mismatch.lane == MobShadowLane::FlowFrameLoop
                && mismatch.region == "tracked_runs"
                && mismatch
                    .observed_summary
                    .contains("unknown step id in tracked-run projection")
        }));
    }

    #[test]
    fn capture_mob_shadow_report_reports_task_history_recovery_mismatches() {
        let mut roster = Roster::new();
        let broken_sid = add_member(&mut roster, "worker-broken");

        let mut broken_projected = projected_entry(&roster, "worker-broken");
        broken_projected.status = MobMemberStatus::Retiring;
        broken_projected.error = Some("projected reason".into());
        broken_projected.is_final = true;
        broken_projected.current_session_id = Some(meerkat_core::types::SessionId::new());

        let flow_run_id = RunId::new();
        let snapshot = MobMachineSnapshot {
            phase: MobState::Running,
            kernel: MobKernelDiagnosticSnapshot {
                flow_trackers: crate::runtime::MobFlowTrackerSnapshot {
                    run_task_ids: BTreeSet::from([flow_run_id.clone()]),
                    cancel_token_ids: BTreeSet::new(),
                    stream_ids: BTreeSet::from([RunId::new()]),
                    tracked_flows: BTreeMap::from([(flow_run_id.clone(), FlowId::from("demo"))]),
                },
                ..valid_kernel_snapshot()
            },
            roster,
            members: vec![broken_projected],
            restore_failures: BTreeMap::from([(
                MeerkatId::from("worker-broken"),
                RestoreFailureSnapshot {
                    session_id: broken_sid,
                    reason: "restore failed".into(),
                },
            )]),
            tracked_runs: BTreeMap::from([(flow_run_id, TrackedRunSnapshot::Missing)]),
        };

        let mismatches = capture_task_history_recovery_shadow_mismatches(&snapshot);

        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MobShadowMismatch {
                lane: MobShadowLane::TaskHistoryRecovery,
                region: "restore_failures",
                triage: ShadowMismatchTriage::ImplementationDetail,
                ..
            } if mismatch.observed_summary.contains("projected status = Retiring")
        )));
        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MobShadowMismatch {
                lane: MobShadowLane::TaskHistoryRecovery,
                region: "restore_failures",
                triage: ShadowMismatchTriage::ImplementationDetail,
                ..
            } if mismatch
                .observed_summary
                .contains("projected session binding mismatches restore failure record")
        )));
        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MobShadowMismatch {
                lane: MobShadowLane::TaskHistoryRecovery,
                region: "restore_failures",
                triage: ShadowMismatchTriage::ImplementationDetail,
                ..
            } if mismatch
                .observed_summary
                .contains("projected failure reason mismatches restore failure record")
        )));
        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MobShadowMismatch {
                lane: MobShadowLane::TaskHistoryRecovery,
                region: "task_ledger",
                triage: ShadowMismatchTriage::ImplementationDetail,
                ..
            } if mismatch.observed_summary.contains("cancel_token_ids = []")
        )));
        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MobShadowMismatch {
                lane: MobShadowLane::TaskHistoryRecovery,
                region: "task_ledger",
                triage: ShadowMismatchTriage::ImplementationDetail,
                ..
            } if mismatch
                .observed_summary
                .contains("stream tracked without an actor-owned run task")
        )));
        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MobShadowMismatch {
                lane: MobShadowLane::TaskHistoryRecovery,
                region: "history",
                triage: ShadowMismatchTriage::ImplementationDetail,
                ..
            } if mismatch.observed_summary.contains("status = Retiring, is_final = true")
        )));
    }

    #[test]
    fn capture_composition_shadow_report_reports_lifecycle_supersession_mismatches() {
        let mut roster = Roster::new();
        let worker_sid = add_member(&mut roster, "worker-1");
        let _other_sid = add_member(&mut roster, "worker-2");

        let projected = projected_entry(&roster, "worker-1");
        let mut duplicate = projected_entry(&roster, "worker-2");
        duplicate.current_session_id = Some(worker_sid.clone());

        let snapshot = MobMachineSnapshot {
            phase: MobState::Running,
            kernel: valid_kernel_snapshot(),
            roster,
            members: vec![projected, duplicate],
            restore_failures: BTreeMap::new(),
            tracked_runs: BTreeMap::new(),
        };

        let mut session_live = HashMap::new();
        session_live.insert(worker_sid.clone(), false);

        let mut runtime_spines = HashMap::new();
        runtime_spines.insert(
            worker_sid.clone(),
            Some(active_meerkat_spine(&worker_sid, RuntimeState::Destroyed)),
        );

        let mismatches = capture_composition_lifecycle_supersession_shadow_mismatches(
            &snapshot,
            &session_live,
            &runtime_spines,
        );

        assert!(mismatches.iter().any(|mismatch| {
            mismatch.region == "supersession"
                && mismatch
                    .expected_summary
                    .contains("one Mob member should own a bridge session")
        }));
        assert!(mismatches.iter().any(|mismatch| {
            mismatch.region == "lifecycle"
                && mismatch.expected_summary.contains(
                    "active members should not point at a terminal or stopped Meerkat runtime",
                )
        }));
    }

    #[test]
    fn capture_composition_shadow_report_reports_work_bridge_mismatches() {
        let mut roster = Roster::new();
        let worker_sid = add_member(&mut roster, "worker-1");
        let projected = projected_entry(&roster, "worker-1");
        let run_id = RunId::new();

        let snapshot = MobMachineSnapshot {
            phase: MobState::Running,
            kernel: MobKernelDiagnosticSnapshot {
                lifecycle: crate::runtime::MobLifecycleSnapshot {
                    phase: MobState::Running,
                    active_run_count: 1,
                    cleanup_pending: false,
                },
                orchestrator: Some(MobOrchestratorSnapshot {
                    phase: MobState::Running,
                    coordinator_bound: true,
                    pending_spawn_count: 0,
                    active_flow_count: 1,
                    topology_revision: 1,
                    supervisor_active: true,
                }),
                flow_trackers: crate::runtime::MobFlowTrackerSnapshot {
                    run_task_ids: BTreeSet::from([run_id.clone()]),
                    cancel_token_ids: BTreeSet::from([run_id.clone()]),
                    stream_ids: BTreeSet::new(),
                    tracked_flows: BTreeMap::from([(run_id.clone(), FlowId::from("demo"))]),
                },
                ..valid_kernel_snapshot()
            },
            roster,
            members: vec![projected],
            restore_failures: BTreeMap::new(),
            tracked_runs: BTreeMap::from([(
                run_id,
                TrackedRunSnapshot::Present(TrackedRunStoreSnapshot {
                    flow_id: FlowId::from("demo"),
                    schema_version: 4,
                    status: MobRunStatus::Running,
                    completed_at_present: false,
                    frame_count: 0,
                    loop_count: 0,
                    loop_iteration_count: 0,
                    ordered_steps: vec![StepId::from("start")],
                    step_dependencies: BTreeMap::from([(StepId::from("start"), Vec::new())]),
                    step_dependency_modes: BTreeMap::from([(
                        StepId::from("start"),
                        DependencyMode::All,
                    )]),
                    step_has_conditions: BTreeMap::from([(StepId::from("start"), false)]),
                    step_branches: BTreeMap::from([(StepId::from("start"), None)]),
                    step_collection_policy_kinds: BTreeMap::from([(
                        StepId::from("start"),
                        RunCollectionPolicyKind::All,
                    )]),
                    step_quorum_thresholds: BTreeMap::from([(StepId::from("start"), 0)]),
                    step_statuses: BTreeMap::from([(
                        StepId::from("start"),
                        StepRunStatus::Dispatched,
                    )]),
                    failure_count: 0,
                    consecutive_failure_count: 0,
                    max_step_retries: 0,
                    escalation_threshold: 0,
                }),
            )]),
        };

        let runtime_spines = HashMap::from([(
            worker_sid.clone(),
            Some(active_meerkat_spine(&worker_sid, RuntimeState::Idle)),
        )]);
        let mismatches =
            capture_composition_work_bridge_shadow_mismatches(&snapshot, &runtime_spines);

        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            CompositionShadowMismatch {
                lane: CompositionShadowLane::WorkBridge,
                region: "work",
                triage: ShadowMismatchTriage::SemanticGap,
                ..
            } if mismatch
                .expected_summary
                .contains("nonterminal Mob work should surface at least one observable Meerkat work posture")
        )));
    }

    #[test]
    fn summarize_mob_shadow_taxonomy_reports_collapses_seeded_mob_and_seam_drift() {
        let mut roster = Roster::new();
        let worker_sid = add_member(&mut roster, "worker-1");
        let projected = projected_entry(&roster, "worker-1");
        let run_id = RunId::new();

        let provisioning_snapshot = MobMachineSnapshot {
            phase: MobState::Running,
            kernel: {
                let mut kernel = valid_kernel_snapshot();
                kernel.lifecycle.phase = MobState::Completed;
                let orchestrator = kernel
                    .orchestrator
                    .as_mut()
                    .expect("synthetic kernel snapshot should include orchestrator");
                orchestrator.phase = MobState::Destroyed;
                orchestrator.pending_spawn_count = 1;
                orchestrator.active_flow_count = 1;
                orchestrator.coordinator_bound = true;
                orchestrator.supervisor_active = true;
                kernel
            },
            roster: roster.clone(),
            members: vec![projected.clone()],
            restore_failures: BTreeMap::new(),
            tracked_runs: BTreeMap::new(),
        };

        let work_snapshot = MobMachineSnapshot {
            phase: MobState::Running,
            kernel: MobKernelDiagnosticSnapshot {
                lifecycle: crate::runtime::MobLifecycleSnapshot {
                    phase: MobState::Running,
                    active_run_count: 1,
                    cleanup_pending: false,
                },
                orchestrator: Some(MobOrchestratorSnapshot {
                    phase: MobState::Running,
                    coordinator_bound: true,
                    pending_spawn_count: 0,
                    active_flow_count: 1,
                    topology_revision: 1,
                    supervisor_active: true,
                }),
                flow_trackers: crate::runtime::MobFlowTrackerSnapshot {
                    run_task_ids: BTreeSet::from([run_id.clone()]),
                    cancel_token_ids: BTreeSet::from([run_id.clone()]),
                    stream_ids: BTreeSet::new(),
                    tracked_flows: BTreeMap::from([(run_id.clone(), FlowId::from("demo"))]),
                },
                ..valid_kernel_snapshot()
            },
            roster,
            members: vec![projected],
            restore_failures: BTreeMap::new(),
            tracked_runs: BTreeMap::from([(
                run_id,
                TrackedRunSnapshot::Present(TrackedRunStoreSnapshot {
                    flow_id: FlowId::from("demo"),
                    schema_version: 4,
                    status: MobRunStatus::Running,
                    completed_at_present: false,
                    frame_count: 0,
                    loop_count: 0,
                    loop_iteration_count: 0,
                    ordered_steps: vec![StepId::from("start")],
                    step_dependencies: BTreeMap::from([(StepId::from("start"), Vec::new())]),
                    step_dependency_modes: BTreeMap::from([(
                        StepId::from("start"),
                        DependencyMode::All,
                    )]),
                    step_has_conditions: BTreeMap::from([(StepId::from("start"), false)]),
                    step_branches: BTreeMap::from([(StepId::from("start"), None)]),
                    step_collection_policy_kinds: BTreeMap::from([(
                        StepId::from("start"),
                        RunCollectionPolicyKind::All,
                    )]),
                    step_quorum_thresholds: BTreeMap::from([(StepId::from("start"), 0)]),
                    step_statuses: BTreeMap::new(),
                    failure_count: 0,
                    consecutive_failure_count: 0,
                    max_step_retries: 0,
                    escalation_threshold: 0,
                }),
            )]),
        };

        let taxonomy = summarize_mob_shadow_taxonomy_reports(&[MobShadowSuiteReport {
            mob: vec![MobShadowReport {
                lane: MobShadowLane::ProvisioningLifecycle,
                mismatches: capture_provisioning_lifecycle_shadow_mismatches(
                    &provisioning_snapshot,
                ),
            }],
            composition: vec![CompositionShadowReport {
                lane: CompositionShadowLane::WorkBridge,
                mismatches: capture_composition_work_bridge_shadow_mismatches(
                    &work_snapshot,
                    &HashMap::from([(
                        worker_sid.clone(),
                        Some(active_meerkat_spine(&worker_sid, RuntimeState::Idle)),
                    )]),
                ),
            }],
        }]);

        assert!(taxonomy.iter().any(|bucket| {
            bucket.scope == "mob"
                && bucket.lane == "ProvisioningLifecycle"
                && bucket.region == "lifecycle"
                && bucket.triage == ShadowMismatchTriage::DogmaViolation
                && bucket.count == 1
        }));
        assert!(taxonomy.iter().any(|bucket| {
            bucket.scope == "mob"
                && bucket.lane == "ProvisioningLifecycle"
                && bucket.region == "orchestrator"
                && bucket.triage == ShadowMismatchTriage::ImplementationDetail
                && bucket.count == 1
        }));
        assert!(taxonomy.iter().any(|bucket| {
            bucket.scope == "composition"
                && bucket.lane == "WorkBridge"
                && bucket.region == "work"
                && bucket.triage == ShadowMismatchTriage::SemanticGap
                && bucket.count == 1
        }));
    }

    #[test]
    fn validate_mob_machine_snapshot_reports_broken_projection_without_restore_failure() {
        let mut roster = Roster::new();
        let session_id = add_member(&mut roster, "worker-broken");
        let mut projected = projected_entry(&roster, "worker-broken");
        projected.status = MobMemberStatus::Broken;
        projected.error = Some("restore failed".into());
        projected.is_final = true;
        projected.current_session_id = Some(session_id);

        let snapshot = MobMachineSnapshot {
            phase: MobState::Running,
            kernel: valid_kernel_snapshot(),
            roster,
            members: vec![projected],
            restore_failures: BTreeMap::new(),
            tracked_runs: BTreeMap::new(),
        };

        let violations = validate_mob_machine_snapshot(&snapshot);
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::ProjectedBrokenMemberMissingRestoreFailure { meerkat_id }
                if meerkat_id == &MeerkatId::from("worker-broken")
        )));
    }

    #[test]
    fn validate_mob_machine_snapshot_reports_kickoff_barrier_projection_violations() {
        let mut roster = Roster::new();
        let _missing_projected_sid = add_member(&mut roster, "worker-missing-projected");
        let broken_session_id = add_member(&mut roster, "worker-broken");
        let _no_session_id = add_member(&mut roster, "worker-no-session");

        let mut broken_projected = projected_entry(&roster, "worker-broken");
        broken_projected.status = MobMemberStatus::Broken;
        broken_projected.error = Some("restore failed".into());
        broken_projected.is_final = true;
        broken_projected.current_session_id = Some(broken_session_id);

        let mut no_session_projected = projected_entry(&roster, "worker-no-session");
        no_session_projected.current_session_id = None;

        let mut kernel = valid_kernel_snapshot();
        kernel
            .kickoff_barrier
            .pending_member_ids
            .insert(MeerkatId::from("worker-missing-projected"));
        kernel
            .kickoff_barrier
            .pending_member_ids
            .insert(MeerkatId::from("worker-broken"));
        kernel
            .kickoff_barrier
            .pending_member_ids
            .insert(MeerkatId::from("worker-no-session"));
        kernel
            .kickoff_barrier
            .pending_member_ids
            .insert(MeerkatId::from("worker-missing"));

        let snapshot = MobMachineSnapshot {
            phase: MobState::Running,
            kernel,
            roster,
            members: vec![broken_projected, no_session_projected],
            restore_failures: BTreeMap::new(),
            tracked_runs: BTreeMap::new(),
        };

        let violations = validate_mob_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::KickoffPendingMemberMissingProjectedEntry { meerkat_id }
                if meerkat_id == &MeerkatId::from("worker-missing-projected")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::KickoffPendingMemberMissingSessionBinding { meerkat_id }
                if meerkat_id == &MeerkatId::from("worker-no-session")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::KickoffPendingMemberBroken { meerkat_id }
                if meerkat_id == &MeerkatId::from("worker-broken")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::KickoffPendingMemberMissingRosterEntry { meerkat_id }
                if meerkat_id == &MeerkatId::from("worker-missing")
        )));
    }

    #[test]
    fn validate_mob_machine_snapshot_reports_tracked_run_kernel_shape_violations() {
        let flow_run_id = RunId::new();
        let snapshot = MobMachineSnapshot {
            phase: MobState::Running,
            kernel: MobKernelDiagnosticSnapshot {
                flow_trackers: crate::runtime::MobFlowTrackerSnapshot {
                    run_task_ids: BTreeSet::from([flow_run_id.clone()]),
                    cancel_token_ids: BTreeSet::from([flow_run_id.clone()]),
                    stream_ids: BTreeSet::new(),
                    tracked_flows: BTreeMap::from([(flow_run_id.clone(), FlowId::from("demo"))]),
                },
                ..valid_kernel_snapshot()
            },
            roster: Roster::new(),
            members: Vec::new(),
            restore_failures: BTreeMap::new(),
            tracked_runs: BTreeMap::from([(
                flow_run_id.clone(),
                TrackedRunSnapshot::Present(TrackedRunStoreSnapshot {
                    flow_id: FlowId::from("demo"),
                    schema_version: 4,
                    status: MobRunStatus::Completed,
                    completed_at_present: false,
                    frame_count: 0,
                    loop_count: 0,
                    loop_iteration_count: 0,
                    ordered_steps: vec![
                        StepId::from("start"),
                        StepId::from("start"),
                        StepId::from("other"),
                    ],
                    step_dependencies: BTreeMap::from([
                        (StepId::from("start"), vec![StepId::from("ghost-dep")]),
                        (StepId::from("ghost"), vec![StepId::from("start")]),
                    ]),
                    step_dependency_modes: BTreeMap::from([
                        (StepId::from("start"), DependencyMode::All),
                        (StepId::from("ghost-mode"), DependencyMode::Any),
                    ]),
                    step_has_conditions: BTreeMap::from([
                        (StepId::from("start"), false),
                        (StepId::from("ghost-condition"), true),
                    ]),
                    step_branches: BTreeMap::from([
                        (StepId::from("start"), Some(BranchId::from("winner"))),
                        (StepId::from("ghost-branch"), Some(BranchId::from("winner"))),
                    ]),
                    step_collection_policy_kinds: BTreeMap::from([
                        (StepId::from("start"), RunCollectionPolicyKind::All),
                        (StepId::from("ghost-policy"), RunCollectionPolicyKind::Any),
                    ]),
                    step_quorum_thresholds: BTreeMap::from([
                        (StepId::from("start"), 1),
                        (StepId::from("ghost-policy"), 2),
                    ]),
                    step_statuses: BTreeMap::from([
                        (StepId::from("start"), StepRunStatus::Dispatched),
                        (StepId::from("other"), StepRunStatus::Failed),
                        (StepId::from("ghost"), StepRunStatus::Completed),
                    ]),
                    failure_count: 1,
                    consecutive_failure_count: 2,
                    max_step_retries: 0,
                    escalation_threshold: 0,
                }),
            )]),
        };

        let violations = validate_mob_machine_snapshot(&snapshot);
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunCompletionTimestampMismatch {
                run_id,
                status: MobRunStatus::Completed,
                completed_at_present: false,
            } if run_id == &flow_run_id
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunOrderedStepsDuplicate { run_id, step_id }
                if run_id == &flow_run_id && step_id == &StepId::from("start")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunDependencyMapUnknownStep { run_id, step_id }
                if run_id == &flow_run_id && step_id == &StepId::from("ghost")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunDependencyMapMissingOrderedStep { run_id, step_id }
                if run_id == &flow_run_id && step_id == &StepId::from("other")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunDependencyModeMissingOrderedStep { run_id, step_id }
                if run_id == &flow_run_id && step_id == &StepId::from("other")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunDependencyModeMapUnknownStep { run_id, step_id }
                if run_id == &flow_run_id && step_id == &StepId::from("ghost-mode")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunConditionFlagMissingOrderedStep { run_id, step_id }
                if run_id == &flow_run_id && step_id == &StepId::from("other")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunConditionFlagUnknownStep { run_id, step_id }
                if run_id == &flow_run_id && step_id == &StepId::from("ghost-condition")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunBranchMissingOrderedStep { run_id, step_id }
                if run_id == &flow_run_id && step_id == &StepId::from("other")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunBranchUnknownStep { run_id, step_id }
                if run_id == &flow_run_id && step_id == &StepId::from("ghost-branch")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunBranchWithoutCondition {
                run_id,
                step_id,
                branch_id,
            }
                if run_id == &flow_run_id
                    && step_id == &StepId::from("start")
                    && branch_id == &BranchId::from("winner")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunCollectionPolicyMissingOrderedStep { run_id, step_id }
                if run_id == &flow_run_id && step_id == &StepId::from("other")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunCollectionPolicyUnknownStep { run_id, step_id }
                if run_id == &flow_run_id && step_id == &StepId::from("ghost-policy")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunQuorumThresholdMissingOrderedStep { run_id, step_id }
                if run_id == &flow_run_id && step_id == &StepId::from("other")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunQuorumThresholdUnknownStep { run_id, step_id }
                if run_id == &flow_run_id && step_id == &StepId::from("ghost-policy")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunQuorumThresholdShapeMismatch {
                run_id,
                step_id,
                policy_kind: RunCollectionPolicyKind::All,
                quorum_threshold: 1,
            } if run_id == &flow_run_id && step_id == &StepId::from("start")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunDependencyUnknownStep { run_id, step_id, dependency_step_id }
                if run_id == &flow_run_id
                    && step_id == &StepId::from("start")
                    && dependency_step_id == &StepId::from("ghost-dep")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunStepStatusUnknownStep { run_id, step_id }
                if run_id == &flow_run_id && step_id == &StepId::from("ghost")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunTerminalRunHasDispatchedStep { run_id, step_id, status: MobRunStatus::Completed }
                if run_id == &flow_run_id && step_id == &StepId::from("start")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunCompletedRunHasNonCompletedStep {
                run_id,
                step_id,
                step_status: StepRunStatus::Failed,
            } if run_id == &flow_run_id && step_id == &StepId::from("other")
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunConsecutiveFailureCountExceedsFailureCount {
                run_id,
                failure_count: 1,
                consecutive_failure_count: 2,
            } if run_id == &flow_run_id
        )));
    }

    #[test]
    fn validate_mob_machine_snapshot_reports_tracked_run_without_ordered_steps() {
        let flow_run_id = RunId::new();
        let snapshot = MobMachineSnapshot {
            phase: MobState::Running,
            kernel: MobKernelDiagnosticSnapshot {
                flow_trackers: crate::runtime::MobFlowTrackerSnapshot {
                    run_task_ids: BTreeSet::from([flow_run_id.clone()]),
                    cancel_token_ids: BTreeSet::from([flow_run_id.clone()]),
                    stream_ids: BTreeSet::new(),
                    tracked_flows: BTreeMap::from([(flow_run_id.clone(), FlowId::from("demo"))]),
                },
                ..valid_kernel_snapshot()
            },
            roster: Roster::new(),
            members: Vec::new(),
            restore_failures: BTreeMap::new(),
            tracked_runs: BTreeMap::from([(
                flow_run_id.clone(),
                TrackedRunSnapshot::Present(TrackedRunStoreSnapshot {
                    flow_id: FlowId::from("demo"),
                    schema_version: 4,
                    status: MobRunStatus::Running,
                    completed_at_present: false,
                    frame_count: 0,
                    loop_count: 0,
                    loop_iteration_count: 0,
                    ordered_steps: Vec::new(),
                    step_dependencies: BTreeMap::new(),
                    step_dependency_modes: BTreeMap::new(),
                    step_has_conditions: BTreeMap::new(),
                    step_branches: BTreeMap::new(),
                    step_collection_policy_kinds: BTreeMap::new(),
                    step_quorum_thresholds: BTreeMap::new(),
                    step_statuses: BTreeMap::new(),
                    failure_count: 0,
                    consecutive_failure_count: 0,
                    max_step_retries: 0,
                    escalation_threshold: 0,
                }),
            )]),
        };

        let violations = validate_mob_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunMissingOrderedSteps { run_id }
                if run_id == &flow_run_id
        )));
    }

    #[test]
    fn validate_mob_machine_snapshot_reports_tracked_run_self_dependency() {
        let flow_run_id = RunId::new();
        let snapshot = MobMachineSnapshot {
            phase: MobState::Running,
            kernel: MobKernelDiagnosticSnapshot {
                flow_trackers: crate::runtime::MobFlowTrackerSnapshot {
                    run_task_ids: BTreeSet::from([flow_run_id.clone()]),
                    cancel_token_ids: BTreeSet::from([flow_run_id.clone()]),
                    stream_ids: BTreeSet::new(),
                    tracked_flows: BTreeMap::from([(flow_run_id.clone(), FlowId::from("demo"))]),
                },
                ..valid_kernel_snapshot()
            },
            roster: Roster::new(),
            members: Vec::new(),
            restore_failures: BTreeMap::new(),
            tracked_runs: BTreeMap::from([(
                flow_run_id.clone(),
                TrackedRunSnapshot::Present(TrackedRunStoreSnapshot {
                    flow_id: FlowId::from("demo"),
                    schema_version: 4,
                    status: MobRunStatus::Running,
                    completed_at_present: false,
                    frame_count: 0,
                    loop_count: 0,
                    loop_iteration_count: 0,
                    ordered_steps: vec![StepId::from("start")],
                    step_dependencies: BTreeMap::from([(
                        StepId::from("start"),
                        vec![StepId::from("start")],
                    )]),
                    step_dependency_modes: BTreeMap::from([(
                        StepId::from("start"),
                        DependencyMode::All,
                    )]),
                    step_has_conditions: BTreeMap::from([(StepId::from("start"), false)]),
                    step_branches: BTreeMap::from([(StepId::from("start"), None)]),
                    step_collection_policy_kinds: BTreeMap::from([(
                        StepId::from("start"),
                        RunCollectionPolicyKind::All,
                    )]),
                    step_quorum_thresholds: BTreeMap::from([(StepId::from("start"), 0)]),
                    step_statuses: BTreeMap::new(),
                    failure_count: 0,
                    consecutive_failure_count: 0,
                    max_step_retries: 0,
                    escalation_threshold: 0,
                }),
            )]),
        };

        let violations = validate_mob_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunSelfDependency { run_id, step_id }
                if run_id == &flow_run_id && step_id == &StepId::from("start")
        )));
    }

    #[test]
    fn validate_mob_machine_snapshot_reports_tracked_run_duplicate_dependency() {
        let flow_run_id = RunId::new();
        let snapshot = MobMachineSnapshot {
            phase: MobState::Running,
            kernel: MobKernelDiagnosticSnapshot {
                flow_trackers: crate::runtime::MobFlowTrackerSnapshot {
                    run_task_ids: BTreeSet::from([flow_run_id.clone()]),
                    cancel_token_ids: BTreeSet::from([flow_run_id.clone()]),
                    stream_ids: BTreeSet::new(),
                    tracked_flows: BTreeMap::from([(flow_run_id.clone(), FlowId::from("demo"))]),
                },
                ..valid_kernel_snapshot()
            },
            roster: Roster::new(),
            members: Vec::new(),
            restore_failures: BTreeMap::new(),
            tracked_runs: BTreeMap::from([(
                flow_run_id.clone(),
                TrackedRunSnapshot::Present(TrackedRunStoreSnapshot {
                    flow_id: FlowId::from("demo"),
                    schema_version: 4,
                    status: MobRunStatus::Running,
                    completed_at_present: false,
                    frame_count: 0,
                    loop_count: 0,
                    loop_iteration_count: 0,
                    ordered_steps: vec![StepId::from("start"), StepId::from("dep")],
                    step_dependencies: BTreeMap::from([
                        (
                            StepId::from("start"),
                            vec![StepId::from("dep"), StepId::from("dep")],
                        ),
                        (StepId::from("dep"), Vec::new()),
                    ]),
                    step_dependency_modes: BTreeMap::from([
                        (StepId::from("start"), DependencyMode::All),
                        (StepId::from("dep"), DependencyMode::All),
                    ]),
                    step_has_conditions: BTreeMap::from([
                        (StepId::from("start"), false),
                        (StepId::from("dep"), false),
                    ]),
                    step_branches: BTreeMap::from([
                        (StepId::from("start"), None),
                        (StepId::from("dep"), None),
                    ]),
                    step_collection_policy_kinds: BTreeMap::from([
                        (StepId::from("start"), RunCollectionPolicyKind::All),
                        (StepId::from("dep"), RunCollectionPolicyKind::All),
                    ]),
                    step_quorum_thresholds: BTreeMap::from([
                        (StepId::from("start"), 0),
                        (StepId::from("dep"), 0),
                    ]),
                    step_statuses: BTreeMap::new(),
                    failure_count: 0,
                    consecutive_failure_count: 0,
                    max_step_retries: 0,
                    escalation_threshold: 0,
                }),
            )]),
        };

        let violations = validate_mob_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunDuplicateDependency {
                run_id,
                step_id,
                dependency_step_id,
            }
                if run_id == &flow_run_id
                    && step_id == &StepId::from("start")
                    && dependency_step_id == &StepId::from("dep")
        )));
    }

    #[test]
    fn validate_mob_machine_snapshot_reports_branch_dependency_conflicts() {
        let flow_run_id = RunId::new();
        let snapshot = MobMachineSnapshot {
            phase: MobState::Running,
            kernel: MobKernelDiagnosticSnapshot {
                flow_trackers: crate::runtime::MobFlowTrackerSnapshot {
                    run_task_ids: BTreeSet::from([flow_run_id.clone()]),
                    cancel_token_ids: BTreeSet::from([flow_run_id.clone()]),
                    stream_ids: BTreeSet::new(),
                    tracked_flows: BTreeMap::from([(flow_run_id.clone(), FlowId::from("demo"))]),
                },
                ..valid_kernel_snapshot()
            },
            roster: Roster::new(),
            members: Vec::new(),
            restore_failures: BTreeMap::new(),
            tracked_runs: BTreeMap::from([(
                flow_run_id.clone(),
                TrackedRunSnapshot::Present(TrackedRunStoreSnapshot {
                    flow_id: FlowId::from("demo"),
                    schema_version: 4,
                    status: MobRunStatus::Running,
                    completed_at_present: false,
                    frame_count: 0,
                    loop_count: 0,
                    loop_iteration_count: 0,
                    ordered_steps: vec![
                        StepId::from("start"),
                        StepId::from("left"),
                        StepId::from("right"),
                    ],
                    step_dependencies: BTreeMap::from([
                        (StepId::from("start"), Vec::new()),
                        (StepId::from("left"), vec![StepId::from("start")]),
                        (StepId::from("right"), Vec::new()),
                    ]),
                    step_dependency_modes: BTreeMap::from([
                        (StepId::from("start"), DependencyMode::All),
                        (StepId::from("left"), DependencyMode::All),
                        (StepId::from("right"), DependencyMode::All),
                    ]),
                    step_has_conditions: BTreeMap::from([
                        (StepId::from("start"), false),
                        (StepId::from("left"), true),
                        (StepId::from("right"), true),
                    ]),
                    step_branches: BTreeMap::from([
                        (StepId::from("start"), None),
                        (StepId::from("left"), Some(BranchId::from("repair"))),
                        (StepId::from("right"), Some(BranchId::from("repair"))),
                    ]),
                    step_collection_policy_kinds: BTreeMap::from([
                        (StepId::from("start"), RunCollectionPolicyKind::All),
                        (StepId::from("left"), RunCollectionPolicyKind::All),
                        (StepId::from("right"), RunCollectionPolicyKind::All),
                    ]),
                    step_quorum_thresholds: BTreeMap::from([
                        (StepId::from("start"), 0),
                        (StepId::from("left"), 0),
                        (StepId::from("right"), 0),
                    ]),
                    step_statuses: BTreeMap::new(),
                    failure_count: 0,
                    consecutive_failure_count: 0,
                    max_step_retries: 0,
                    escalation_threshold: 0,
                }),
            )]),
        };

        let violations = validate_mob_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunBranchConflictingDependencies {
                run_id,
                branch_id,
                reference_step_id,
                conflicting_step_id,
            }
                if run_id == &flow_run_id
                    && branch_id == &BranchId::from("repair")
                    && reference_step_id == &StepId::from("left")
                    && conflicting_step_id == &StepId::from("right")
        )));
    }

    #[test]
    fn validate_mob_machine_snapshot_reports_any_dependency_mode_without_branch_dependency() {
        let flow_run_id = RunId::new();
        let snapshot = MobMachineSnapshot {
            phase: MobState::Running,
            kernel: MobKernelDiagnosticSnapshot {
                flow_trackers: crate::runtime::MobFlowTrackerSnapshot {
                    run_task_ids: BTreeSet::from([flow_run_id.clone()]),
                    cancel_token_ids: BTreeSet::from([flow_run_id.clone()]),
                    stream_ids: BTreeSet::new(),
                    tracked_flows: BTreeMap::from([(flow_run_id.clone(), FlowId::from("demo"))]),
                },
                ..valid_kernel_snapshot()
            },
            roster: Roster::new(),
            members: Vec::new(),
            restore_failures: BTreeMap::new(),
            tracked_runs: BTreeMap::from([(
                flow_run_id.clone(),
                TrackedRunSnapshot::Present(TrackedRunStoreSnapshot {
                    flow_id: FlowId::from("demo"),
                    schema_version: 4,
                    status: MobRunStatus::Running,
                    completed_at_present: false,
                    frame_count: 0,
                    loop_count: 0,
                    loop_iteration_count: 0,
                    ordered_steps: vec![
                        StepId::from("start"),
                        StepId::from("left"),
                        StepId::from("join"),
                    ],
                    step_dependencies: BTreeMap::from([
                        (StepId::from("start"), Vec::new()),
                        (StepId::from("left"), vec![StepId::from("start")]),
                        (StepId::from("join"), vec![StepId::from("start")]),
                    ]),
                    step_dependency_modes: BTreeMap::from([
                        (StepId::from("start"), DependencyMode::All),
                        (StepId::from("left"), DependencyMode::All),
                        (StepId::from("join"), DependencyMode::Any),
                    ]),
                    step_has_conditions: BTreeMap::from([
                        (StepId::from("start"), false),
                        (StepId::from("left"), true),
                        (StepId::from("join"), false),
                    ]),
                    step_branches: BTreeMap::from([
                        (StepId::from("start"), None),
                        (StepId::from("left"), Some(BranchId::from("repair"))),
                        (StepId::from("join"), None),
                    ]),
                    step_collection_policy_kinds: BTreeMap::from([
                        (StepId::from("start"), RunCollectionPolicyKind::All),
                        (StepId::from("left"), RunCollectionPolicyKind::All),
                        (StepId::from("join"), RunCollectionPolicyKind::All),
                    ]),
                    step_quorum_thresholds: BTreeMap::from([
                        (StepId::from("start"), 0),
                        (StepId::from("left"), 0),
                        (StepId::from("join"), 0),
                    ]),
                    step_statuses: BTreeMap::new(),
                    failure_count: 0,
                    consecutive_failure_count: 0,
                    max_step_retries: 0,
                    escalation_threshold: 0,
                }),
            )]),
        };

        let violations = validate_mob_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunAnyDependencyModeWithoutBranchDependency {
                run_id,
                step_id,
            } if run_id == &flow_run_id && step_id == &StepId::from("join")
        )));
    }

    #[test]
    fn validate_mob_machine_snapshot_reports_branch_group_too_small() {
        let flow_run_id = RunId::new();
        let snapshot = MobMachineSnapshot {
            phase: MobState::Running,
            kernel: MobKernelDiagnosticSnapshot {
                flow_trackers: crate::runtime::MobFlowTrackerSnapshot {
                    run_task_ids: BTreeSet::from([flow_run_id.clone()]),
                    cancel_token_ids: BTreeSet::from([flow_run_id.clone()]),
                    stream_ids: BTreeSet::new(),
                    tracked_flows: BTreeMap::from([(flow_run_id.clone(), FlowId::from("demo"))]),
                },
                ..valid_kernel_snapshot()
            },
            roster: Roster::new(),
            members: Vec::new(),
            restore_failures: BTreeMap::new(),
            tracked_runs: BTreeMap::from([(
                flow_run_id.clone(),
                TrackedRunSnapshot::Present(TrackedRunStoreSnapshot {
                    flow_id: FlowId::from("demo"),
                    schema_version: 4,
                    status: MobRunStatus::Running,
                    completed_at_present: false,
                    frame_count: 0,
                    loop_count: 0,
                    loop_iteration_count: 0,
                    ordered_steps: vec![StepId::from("start"), StepId::from("only_branch")],
                    step_dependencies: BTreeMap::from([
                        (StepId::from("start"), Vec::new()),
                        (StepId::from("only_branch"), vec![StepId::from("start")]),
                    ]),
                    step_dependency_modes: BTreeMap::from([
                        (StepId::from("start"), DependencyMode::All),
                        (StepId::from("only_branch"), DependencyMode::All),
                    ]),
                    step_has_conditions: BTreeMap::from([
                        (StepId::from("start"), false),
                        (StepId::from("only_branch"), true),
                    ]),
                    step_branches: BTreeMap::from([
                        (StepId::from("start"), None),
                        (StepId::from("only_branch"), Some(BranchId::from("repair"))),
                    ]),
                    step_collection_policy_kinds: BTreeMap::from([
                        (StepId::from("start"), RunCollectionPolicyKind::All),
                        (StepId::from("only_branch"), RunCollectionPolicyKind::All),
                    ]),
                    step_quorum_thresholds: BTreeMap::from([
                        (StepId::from("start"), 0),
                        (StepId::from("only_branch"), 0),
                    ]),
                    step_statuses: BTreeMap::new(),
                    failure_count: 0,
                    consecutive_failure_count: 0,
                    max_step_retries: 0,
                    escalation_threshold: 0,
                }),
            )]),
        };

        let violations = validate_mob_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunBranchGroupTooSmall {
                run_id,
                branch_id,
                member_count: 1,
            } if run_id == &flow_run_id && branch_id == &BranchId::from("repair")
        )));
    }

    #[test]
    fn validate_mob_machine_snapshot_reports_loop_iteration_without_loop_structure() {
        let flow_run_id = RunId::new();
        let snapshot = MobMachineSnapshot {
            phase: MobState::Running,
            kernel: MobKernelDiagnosticSnapshot {
                flow_trackers: crate::runtime::MobFlowTrackerSnapshot {
                    run_task_ids: BTreeSet::from([flow_run_id.clone()]),
                    cancel_token_ids: BTreeSet::from([flow_run_id.clone()]),
                    stream_ids: BTreeSet::new(),
                    tracked_flows: BTreeMap::from([(flow_run_id.clone(), FlowId::from("demo"))]),
                },
                ..valid_kernel_snapshot()
            },
            roster: Roster::new(),
            members: Vec::new(),
            restore_failures: BTreeMap::new(),
            tracked_runs: BTreeMap::from([(
                flow_run_id.clone(),
                TrackedRunSnapshot::Present(TrackedRunStoreSnapshot {
                    flow_id: FlowId::from("demo"),
                    schema_version: 4,
                    status: MobRunStatus::Running,
                    completed_at_present: false,
                    frame_count: 0,
                    loop_count: 0,
                    loop_iteration_count: 1,
                    ordered_steps: vec![StepId::from("body")],
                    step_dependencies: BTreeMap::from([(StepId::from("body"), Vec::new())]),
                    step_dependency_modes: BTreeMap::from([(
                        StepId::from("body"),
                        DependencyMode::All,
                    )]),
                    step_has_conditions: BTreeMap::from([(StepId::from("body"), false)]),
                    step_branches: BTreeMap::from([(StepId::from("body"), None)]),
                    step_collection_policy_kinds: BTreeMap::from([(
                        StepId::from("body"),
                        RunCollectionPolicyKind::All,
                    )]),
                    step_quorum_thresholds: BTreeMap::from([(StepId::from("body"), 0)]),
                    step_statuses: BTreeMap::new(),
                    failure_count: 0,
                    consecutive_failure_count: 0,
                    max_step_retries: 0,
                    escalation_threshold: 0,
                }),
            )]),
        };

        let violations = validate_mob_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunLoopIterationWithoutLoopStructure {
                run_id,
                loop_iteration_count: 1,
                loop_count: 0,
                frame_count: 0,
            } if run_id == &flow_run_id
        )));
    }

    #[test]
    fn validate_mob_machine_snapshot_reports_loop_without_frame_structure() {
        let flow_run_id = RunId::new();
        let snapshot = MobMachineSnapshot {
            phase: MobState::Running,
            kernel: MobKernelDiagnosticSnapshot {
                flow_trackers: crate::runtime::MobFlowTrackerSnapshot {
                    run_task_ids: BTreeSet::from([flow_run_id.clone()]),
                    cancel_token_ids: BTreeSet::from([flow_run_id.clone()]),
                    stream_ids: BTreeSet::new(),
                    tracked_flows: BTreeMap::from([(flow_run_id.clone(), FlowId::from("demo"))]),
                },
                ..valid_kernel_snapshot()
            },
            roster: Roster::new(),
            members: Vec::new(),
            restore_failures: BTreeMap::new(),
            tracked_runs: BTreeMap::from([(
                flow_run_id.clone(),
                TrackedRunSnapshot::Present(TrackedRunStoreSnapshot {
                    flow_id: FlowId::from("demo"),
                    schema_version: 4,
                    status: MobRunStatus::Running,
                    completed_at_present: false,
                    frame_count: 0,
                    loop_count: 1,
                    loop_iteration_count: 0,
                    ordered_steps: vec![StepId::from("body")],
                    step_dependencies: BTreeMap::from([(StepId::from("body"), Vec::new())]),
                    step_dependency_modes: BTreeMap::from([(
                        StepId::from("body"),
                        DependencyMode::All,
                    )]),
                    step_has_conditions: BTreeMap::from([(StepId::from("body"), false)]),
                    step_branches: BTreeMap::from([(StepId::from("body"), None)]),
                    step_collection_policy_kinds: BTreeMap::from([(
                        StepId::from("body"),
                        RunCollectionPolicyKind::All,
                    )]),
                    step_quorum_thresholds: BTreeMap::from([(StepId::from("body"), 0)]),
                    step_statuses: BTreeMap::new(),
                    failure_count: 0,
                    consecutive_failure_count: 0,
                    max_step_retries: 0,
                    escalation_threshold: 0,
                }),
            )]),
        };

        let violations = validate_mob_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunLoopWithoutFrameStructure {
                run_id,
                loop_count: 1,
                frame_count: 0,
            } if run_id == &flow_run_id
        )));
    }

    #[test]
    fn validate_mob_machine_snapshot_reports_structured_state_with_legacy_schema_version() {
        let flow_run_id = RunId::new();
        let snapshot = MobMachineSnapshot {
            phase: MobState::Running,
            kernel: MobKernelDiagnosticSnapshot {
                flow_trackers: crate::runtime::MobFlowTrackerSnapshot {
                    run_task_ids: BTreeSet::from([flow_run_id.clone()]),
                    cancel_token_ids: BTreeSet::from([flow_run_id.clone()]),
                    stream_ids: BTreeSet::new(),
                    tracked_flows: BTreeMap::from([(flow_run_id.clone(), FlowId::from("demo"))]),
                },
                ..valid_kernel_snapshot()
            },
            roster: Roster::new(),
            members: Vec::new(),
            restore_failures: BTreeMap::new(),
            tracked_runs: BTreeMap::from([(
                flow_run_id.clone(),
                TrackedRunSnapshot::Present(TrackedRunStoreSnapshot {
                    flow_id: FlowId::from("demo"),
                    schema_version: 0,
                    status: MobRunStatus::Running,
                    completed_at_present: false,
                    frame_count: 1,
                    loop_count: 1,
                    loop_iteration_count: 1,
                    ordered_steps: vec![StepId::from("body")],
                    step_dependencies: BTreeMap::from([(StepId::from("body"), Vec::new())]),
                    step_dependency_modes: BTreeMap::from([(
                        StepId::from("body"),
                        DependencyMode::All,
                    )]),
                    step_has_conditions: BTreeMap::from([(StepId::from("body"), false)]),
                    step_branches: BTreeMap::from([(StepId::from("body"), None)]),
                    step_collection_policy_kinds: BTreeMap::from([(
                        StepId::from("body"),
                        RunCollectionPolicyKind::All,
                    )]),
                    step_quorum_thresholds: BTreeMap::from([(StepId::from("body"), 0)]),
                    step_statuses: BTreeMap::new(),
                    failure_count: 0,
                    consecutive_failure_count: 0,
                    max_step_retries: 0,
                    escalation_threshold: 0,
                }),
            )]),
        };

        let violations = validate_mob_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MobMachineInvariantViolation::TrackedRunStructuredStateWithLegacySchemaVersion {
                run_id,
                schema_version: 0,
                frame_count: 1,
                loop_count: 1,
                loop_iteration_count: 1,
            } if run_id == &flow_run_id
        )));
    }

    #[test]
    fn export_two_kernel_shadow_report_session_pretty_json_roundtrips_structure() {
        let session = export_two_kernel_shadow_report_session(
            "shadow.cutover.session",
            [export_two_kernel_shadow_run_batch(
                "shadow.cutover.smoke",
                [export_two_kernel_shadow_batch(
                    "seam.live_bridge_loss",
                    [TwoKernelShadowScenarioSample {
                        scenario_id: "seam.live_bridge_loss".into(),
                        phase: "post_archive".into(),
                        timestamp: "2026-04-12T00:00:00Z".into(),
                        meerkat_buckets: Vec::new(),
                        mob_buckets: Vec::new(),
                        seam_buckets: vec![ShadowTaxonomySinkBucket {
                            scope: "composition",
                            lane: "WorkBridge".into(),
                            region: "work",
                            triage: "semantic_gap",
                            count: 1,
                        }],
                    }],
                )],
            )],
        );

        let json = export_two_kernel_shadow_report_session_pretty_json(&session)
            .expect("serialize shadow report session");
        let value: serde_json::Value =
            serde_json::from_str(&json).expect("roundtrip json should parse");

        assert_eq!(value["session_id"], "shadow.cutover.session");
        assert_eq!(value["runs"].as_array().map(Vec::len), Some(1));
        assert_eq!(value["runs"][0]["run_id"], "shadow.cutover.smoke");
        assert_eq!(
            value["runs"][0]["scenarios"][0]["run_id"],
            "seam.live_bridge_loss"
        );
        assert_eq!(
            value["runs"][0]["scenarios"][0]["samples"][0]["scenario_id"],
            "seam.live_bridge_loss"
        );
        assert_eq!(
            value["runs"][0]["scenarios"][0]["samples"][0]["seam_buckets"][0]["lane"],
            "WorkBridge"
        );
    }
}
