//! Internal integration helpers for the evolving MeerkatMachine boundary.
//!
//! The current refactor work keeps runtime-owned and turn-owned truth in their
//! real crates. This module composes those read paths without inventing a new
//! owner.

use async_trait::async_trait;
use meerkat_core::AgentExecutionSnapshot;
use meerkat_core::CommsCapabilityError;
use meerkat_core::CommsDrainMode;
use meerkat_core::CommsDrainPhase;
use meerkat_core::ExternalToolSurfaceBaseState;
use meerkat_core::ExternalToolSurfaceDeltaOperation;
use meerkat_core::ExternalToolSurfaceDeltaPhase;
use meerkat_core::ExternalToolSurfacePendingOp;
use meerkat_core::ExternalToolSurfaceSnapshot;
use meerkat_core::ExternalToolSurfaceStagedOp;
use meerkat_core::PeerIngressAuthorityPhase;
use meerkat_core::PeerIngressKind;
use meerkat_core::PeerIngressQueueSnapshot;
use meerkat_core::PeerIngressRuntimeSnapshot;
use meerkat_core::PeerInputClass;
use meerkat_core::ToolScopeSnapshot;
use meerkat_core::turn_execution_authority::TurnPhase;
use meerkat_core::types::SessionId;
use meerkat_runtime::runtime_state::RuntimeState;
use meerkat_runtime::{MeerkatMachineSpineSnapshot, RuntimeSessionAdapter};
use meerkat_session::{EphemeralSessionService, SessionAgentBuilder};

use crate::SessionError;
use std::collections::{HashMap, HashSet};

#[cfg(test)]
use meerkat_core::PeerIngressEntrySnapshot;

/// Joined diagnostic view over the current Meerkat runtime spine plus the live
/// core turn/execution snapshot.
#[derive(Debug, Clone)]
pub(crate) struct MeerkatMachineSnapshot {
    pub spine: MeerkatMachineSpineSnapshot,
    pub turn: Option<AgentExecutionSnapshot>,
    pub tools: Option<ToolScopeSnapshot>,
    pub tool_surface: Option<ExternalToolSurfaceSnapshot>,
    pub peer: Option<PeerIngressRuntimeSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum MeerkatShadowLane {
    LifecycleControl,
    Tools,
    TurnOpsBarrier,
    PeerDrain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[allow(dead_code)]
pub(crate) enum ShadowMismatchTriage {
    ImplementationDetail,
    SemanticGap,
    DogmaViolation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MeerkatShadowMismatch {
    pub lane: MeerkatShadowLane,
    pub region: &'static str,
    pub entity_id: Option<String>,
    pub expected_summary: String,
    pub observed_summary: String,
    pub triage: ShadowMismatchTriage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MeerkatShadowReport {
    pub lane: MeerkatShadowLane,
    pub mismatches: Vec<MeerkatShadowMismatch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MeerkatShadowSuiteReport {
    pub session_id: SessionId,
    pub reports: Vec<MeerkatShadowReport>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MeerkatShadowTaxonomyBucket {
    pub lane: MeerkatShadowLane,
    pub region: &'static str,
    pub triage: ShadowMismatchTriage,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowTaxonomySinkBucket {
    pub scope: &'static str,
    pub lane: String,
    pub region: &'static str,
    pub triage: &'static str,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwoKernelShadowScenarioSample {
    pub scenario_id: String,
    pub phase: String,
    pub timestamp: String,
    pub meerkat_buckets: Vec<ShadowTaxonomySinkBucket>,
    pub mob_buckets: Vec<ShadowTaxonomySinkBucket>,
    pub seam_buckets: Vec<ShadowTaxonomySinkBucket>,
}

fn shadow_mismatch_triage_label(triage: ShadowMismatchTriage) -> &'static str {
    match triage {
        ShadowMismatchTriage::ImplementationDetail => "implementation_detail",
        ShadowMismatchTriage::SemanticGap => "semantic_gap",
        ShadowMismatchTriage::DogmaViolation => "dogma_violation",
    }
}

fn sink_bucket_from_taxonomy_bucket(
    bucket: &MeerkatShadowTaxonomyBucket,
) -> ShadowTaxonomySinkBucket {
    ShadowTaxonomySinkBucket {
        scope: "meerkat",
        lane: format!("{:?}", bucket.lane),
        region: bucket.region,
        triage: shadow_mismatch_triage_label(bucket.triage),
        count: bucket.count as u64,
    }
}

pub(crate) fn export_meerkat_shadow_scenario_sample(
    scenario_id: impl Into<String>,
    phase: impl Into<String>,
    timestamp: impl Into<String>,
    taxonomy: &[MeerkatShadowTaxonomyBucket],
) -> TwoKernelShadowScenarioSample {
    TwoKernelShadowScenarioSample {
        scenario_id: scenario_id.into(),
        phase: phase.into(),
        timestamp: timestamp.into(),
        meerkat_buckets: taxonomy
            .iter()
            .map(sink_bucket_from_taxonomy_bucket)
            .collect(),
        mob_buckets: Vec::new(),
        seam_buckets: Vec::new(),
    }
}

pub(crate) fn summarize_meerkat_shadow_taxonomy_reports(
    reports: &[MeerkatShadowSuiteReport],
) -> Vec<MeerkatShadowTaxonomyBucket> {
    let mut counts: HashMap<(MeerkatShadowLane, &'static str, ShadowMismatchTriage), usize> =
        HashMap::new();

    for report in reports {
        for lane_report in &report.reports {
            for mismatch in &lane_report.mismatches {
                *counts
                    .entry((mismatch.lane, mismatch.region, mismatch.triage))
                    .or_insert(0) += 1;
            }
        }
    }

    let mut buckets = counts
        .into_iter()
        .map(
            |((lane, region, triage), count)| MeerkatShadowTaxonomyBucket {
                lane,
                region,
                triage,
                count,
            },
        )
        .collect::<Vec<_>>();
    buckets.sort_by_key(|bucket| (bucket.lane, bucket.region, bucket.triage));
    buckets
}

fn summarize_meerkat_shadow_taxonomy_snapshot(
    snapshot: &MeerkatMachineSnapshot,
) -> Vec<MeerkatShadowTaxonomyBucket> {
    let mut counts: HashMap<(MeerkatShadowLane, &'static str, ShadowMismatchTriage), usize> =
        HashMap::new();

    for mismatch in capture_lifecycle_control_shadow_mismatches(snapshot) {
        *counts
            .entry((mismatch.lane, mismatch.region, mismatch.triage))
            .or_insert(0) += 1;
    }
    for mismatch in capture_tools_shadow_mismatches(snapshot) {
        *counts
            .entry((mismatch.lane, mismatch.region, mismatch.triage))
            .or_insert(0) += 1;
    }
    for mismatch in capture_turn_ops_barrier_shadow_mismatches(snapshot) {
        *counts
            .entry((mismatch.lane, mismatch.region, mismatch.triage))
            .or_insert(0) += 1;
    }
    for mismatch in capture_peer_drain_shadow_mismatches(snapshot) {
        *counts
            .entry((mismatch.lane, mismatch.region, mismatch.triage))
            .or_insert(0) += 1;
    }

    let mut buckets = counts
        .into_iter()
        .map(
            |((lane, region, triage), count)| MeerkatShadowTaxonomyBucket {
                lane,
                region,
                triage,
                count,
            },
        )
        .collect::<Vec<_>>();
    buckets.sort_by_key(|bucket| (bucket.lane, bucket.region, bucket.triage));
    buckets
}

/// Export a Meerkat shadow scenario sample from already-captured diagnostic
/// inputs. This keeps the public seam on the snapshot side rather than
/// exposing the live session-service trait boundary cross-crate.
pub fn export_meerkat_shadow_scenario_sample_from_diagnostic_snapshot(
    scenario_id: impl Into<String>,
    phase: impl Into<String>,
    timestamp: impl Into<String>,
    spine: MeerkatMachineSpineSnapshot,
    execution: Option<AgentExecutionSnapshot>,
    tool_scope: Option<ToolScopeSnapshot>,
    tool_surface: Option<ExternalToolSurfaceSnapshot>,
    peer_ingress: Option<PeerIngressRuntimeSnapshot>,
) -> TwoKernelShadowScenarioSample {
    let snapshot = MeerkatMachineSnapshot {
        spine,
        turn: execution,
        tools: tool_scope,
        tool_surface,
        peer: peer_ingress,
    };
    let taxonomy = summarize_meerkat_shadow_taxonomy_snapshot(&snapshot);
    export_meerkat_shadow_scenario_sample(scenario_id, phase, timestamp, &taxonomy)
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum MeerkatMachineSnapshotError {
    #[error(transparent)]
    Session(#[from] SessionError),
}

/// Cross-region invariant violations in the joined MeerkatMachine snapshot.
///
/// This validator is intentionally conservative: it only encodes invariants
/// already justified by the live authorities and the current Meerkat kernel
/// research. The goal is to turn the diagnostic snapshot into an executable
/// architecture check without inventing new ownership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MeerkatMachineInvariantViolation {
    CurrentRunInIllegalControlPhase {
        phase: RuntimeState,
    },
    RunningWithoutCurrentRun,
    ControlInputsRunMismatch {
        control_run_id: Option<meerkat_core::lifecycle::RunId>,
        inputs_run_id: Option<meerkat_core::lifecycle::RunId>,
    },
    QueueContainsUnknownInput {
        queue: &'static str,
        input_id: meerkat_core::lifecycle::InputId,
    },
    QueueContainsNonQueuedInput {
        queue: &'static str,
        input_id: meerkat_core::lifecycle::InputId,
        lifecycle: Option<meerkat_runtime::InputLifecycleState>,
    },
    QueueContainsWrongHandlingMode {
        queue: &'static str,
        input_id: meerkat_core::lifecycle::InputId,
        handling_mode: Option<meerkat_core::types::HandlingMode>,
    },
    AdmittedInputMissingContentShape {
        input_id: meerkat_core::lifecycle::InputId,
    },
    AdmittedInputMissingHandlingMode {
        input_id: meerkat_core::lifecycle::InputId,
    },
    AdmittedInputMissingLifecycle {
        input_id: meerkat_core::lifecycle::InputId,
    },
    QueuedInputMissingOwningQueue {
        input_id: meerkat_core::lifecycle::InputId,
        handling_mode: meerkat_core::types::HandlingMode,
    },
    ProcessRequestedWithoutWake,
    ProcessRequestedWithoutSteerQueue,
    WakeRequestedWithoutQueuedWork,
    InputBoundarySequenceWithoutRun {
        input_id: meerkat_core::lifecycle::InputId,
        last_boundary_sequence: u64,
    },
    InputBoundarySequenceIllegalLifecycle {
        input_id: meerkat_core::lifecycle::InputId,
        lifecycle: meerkat_runtime::InputLifecycleState,
        last_boundary_sequence: u64,
    },
    InputLifecycleMissingRunBinding {
        input_id: meerkat_core::lifecycle::InputId,
        lifecycle: meerkat_runtime::InputLifecycleState,
    },
    AppliedPendingConsumptionMissingBoundarySequence {
        input_id: meerkat_core::lifecycle::InputId,
    },
    DestroyedIngressStillHasQueuedWork {
        queue: &'static str,
        count: usize,
    },
    DestroyedIngressStillHasCurrentRun {
        run_id: meerkat_core::lifecycle::RunId,
    },
    DestroyedIngressStillHasContributors {
        contributor_count: usize,
    },
    DestroyedIngressStillRequestsWake,
    DestroyedIngressStillRequestsProcessing,
    DestroyedIngressHasNonTerminalInput {
        input_id: meerkat_core::lifecycle::InputId,
        lifecycle: meerkat_runtime::InputLifecycleState,
    },
    QueueAppearsInBothQueues {
        input_id: meerkat_core::lifecycle::InputId,
    },
    CurrentRunWithoutContributors {
        run_id: meerkat_core::lifecycle::RunId,
    },
    ContributorsWithoutCurrentRun {
        contributor_count: usize,
    },
    CurrentRunContributorUnknownInput {
        input_id: meerkat_core::lifecycle::InputId,
    },
    CurrentRunContributorIllegalLifecycle {
        input_id: meerkat_core::lifecycle::InputId,
        lifecycle: Option<meerkat_runtime::InputLifecycleState>,
    },
    CurrentRunContributorRunMismatch {
        input_id: meerkat_core::lifecycle::InputId,
        current_run_id: meerkat_core::lifecycle::RunId,
        last_run_id: Option<meerkat_core::lifecycle::RunId>,
    },
    CurrentRunContributorPendingConsumptionMissingBoundary {
        input_id: meerkat_core::lifecycle::InputId,
        current_run_id: meerkat_core::lifecycle::RunId,
    },
    CurrentRunBoundInputMissingContributor {
        input_id: meerkat_core::lifecycle::InputId,
        current_run_id: meerkat_core::lifecycle::RunId,
        lifecycle: meerkat_runtime::InputLifecycleState,
    },
    TerminalInputWithoutOutcome {
        input_id: meerkat_core::lifecycle::InputId,
        lifecycle: meerkat_runtime::InputLifecycleState,
    },
    NonTerminalInputWithTerminalOutcome {
        input_id: meerkat_core::lifecycle::InputId,
        lifecycle: meerkat_runtime::InputLifecycleState,
        terminal_outcome: meerkat_runtime::InputTerminalOutcome,
    },
    TurnRunMismatch {
        turn_phase: TurnPhase,
        control_run_id: Option<meerkat_core::lifecycle::RunId>,
        turn_run_id: Option<meerkat_core::lifecycle::RunId>,
    },
    WaitingForOpsWithoutPendingOperations,
    WaitingForOpsWithoutToolCalls,
    PendingOperationsOutsideWaitingPhase {
        turn_phase: TurnPhase,
        pending_operation_count: usize,
    },
    BarrierFlagWithoutOperations,
    BarrierOperationsWithoutFlag {
        barrier_operation_count: usize,
    },
    UnsatisfiedBarrierWithoutBarrierOps,
    PendingOperationUnknown {
        operation_id: meerkat_core::ops::OperationId,
    },
    BarrierOperationUnknown {
        operation_id: meerkat_core::ops::OperationId,
    },
    BarrierOperationNotPending {
        operation_id: meerkat_core::ops::OperationId,
    },
    PeerAuthorityQueueMismatch {
        authority_phase: PeerIngressAuthorityPhase,
        submission_queue_len: usize,
    },
    PeerAuthorityTrackedEntryCountMismatch {
        submission_queue_len: usize,
        queued_authority_entries: usize,
    },
    PeerQueueCountMismatch {
        field: &'static str,
        recorded: usize,
        computed: usize,
    },
    PeerEntryKindClassMismatch {
        raw_item_id: String,
        kind: PeerIngressKind,
        class: PeerInputClass,
    },
    PeerEntryLifecyclePeerMismatch {
        raw_item_id: String,
        class: PeerInputClass,
        lifecycle_peer_present: bool,
    },
    PeerEntryRequestIdMismatch {
        raw_item_id: String,
        kind: PeerIngressKind,
        request_id_present: bool,
    },
    PeerEntryFromPeerMismatch {
        raw_item_id: String,
        kind: PeerIngressKind,
        from_peer_present: bool,
    },
    PeerEntryTrustedSnapshotPresenceMismatch {
        raw_item_id: String,
        kind: PeerIngressKind,
        trusted_snapshot_present: bool,
    },
    PeerEntryUntrustedWhileAuthRequired {
        raw_item_id: String,
        kind: PeerIngressKind,
    },
    ToolSurfaceVisibleBaseMismatch {
        surface_id: String,
        visible: bool,
        base_state: ExternalToolSurfaceBaseState,
    },
    ToolSurfacePendingBaseMismatch {
        surface_id: String,
        base_state: ExternalToolSurfaceBaseState,
        pending_op: ExternalToolSurfacePendingOp,
    },
    ToolSurfaceRemovalTimingMismatch {
        surface_id: String,
        base_state: ExternalToolSurfaceBaseState,
        has_removal_timing: bool,
    },
    ToolSurfaceInflightBaseMismatch {
        surface_id: String,
        base_state: ExternalToolSurfaceBaseState,
        inflight_call_count: u64,
    },
    ToolSurfaceForcedPhaseWithoutRemoveDelta {
        surface_id: String,
        last_delta_operation: ExternalToolSurfaceDeltaOperation,
    },
    ToolSurfaceStagedSequenceMismatch {
        surface_id: String,
        staged_op: ExternalToolSurfaceStagedOp,
        staged_intent_sequence: u64,
    },
    ToolSurfacePendingSequenceMismatch {
        surface_id: String,
        pending_op: ExternalToolSurfacePendingOp,
        pending_task_sequence: u64,
        pending_lineage_sequence: u64,
    },
    CompletionWaiterInputCountMismatch {
        recorded_input_count: usize,
        waiting_inputs_len: usize,
    },
    CompletionWaiterCountMismatch {
        recorded_waiter_count: usize,
        summed_waiter_count: usize,
    },
    CompletionWaiterUnknownInput {
        input_id: meerkat_core::lifecycle::InputId,
    },
    CompletionWaiterZeroCount {
        input_id: meerkat_core::lifecycle::InputId,
    },
    CompletionWaiterResolvedInput {
        input_id: meerkat_core::lifecycle::InputId,
        lifecycle: Option<meerkat_runtime::InputLifecycleState>,
        terminal_outcome_present: bool,
    },
    DestroyedRuntimeStillHasQueuedWork {
        queue: &'static str,
        count: usize,
    },
    DestroyedRuntimeStillHasCompletionWaiters {
        waiter_count: usize,
    },
    RetiredRuntimeStillHasCompletionWaiters {
        waiter_count: usize,
    },
    StoppedRuntimeStillHasQueuedWork {
        queue: &'static str,
        count: usize,
    },
    StoppedRuntimeStillHasCompletionWaiters {
        waiter_count: usize,
    },
    DrainSlotMissingPhase,
    DrainPhaseWithoutSlot {
        phase: CommsDrainPhase,
    },
    DrainModeWithoutSlot {
        mode: CommsDrainMode,
    },
    DrainHandleWithoutSlot,
    DrainPhaseMissingMode {
        phase: CommsDrainPhase,
    },
    DrainInactiveWithMode {
        mode: CommsDrainMode,
    },
    DrainInactiveWithHandle,
    DrainTerminalPhaseWithHandle {
        phase: CommsDrainPhase,
    },
    DetachedWakeBindingMismatch {
        binding_present: bool,
        ops_present: bool,
    },
    OpsOperationCountMismatch {
        operation_count: usize,
        operations_len: usize,
    },
    ActiveOpsExceedsOperationCount {
        active_count: usize,
        operation_count: usize,
    },
    ActiveOpsStatusCountMismatch {
        active_count: usize,
        nonterminal_count: usize,
    },
    WaitOperationIdsWithoutWaitRequest {
        wait_operation_count: usize,
    },
    PendingWaitCarrierShapeMismatch {
        pending_wait_present: bool,
        pending_wait_request_id_present: bool,
    },
    PendingWaitCarrierWithoutWaitRequest,
    WaitRequestWithoutTrackedOperations {
        wait_request_id: meerkat_core::lifecycle::WaitRequestId,
    },
    WaitRequestMissingPendingWaitCarrier {
        wait_request_id: meerkat_core::lifecycle::WaitRequestId,
    },
    PendingWaitCarrierRequestMismatch {
        wait_request_id: meerkat_core::lifecycle::WaitRequestId,
        pending_wait_request_id: meerkat_core::lifecycle::WaitRequestId,
    },
    WaitRequestAlreadySatisfied {
        wait_request_id: meerkat_core::lifecycle::WaitRequestId,
    },
    WaitTargetsUnknownOperation {
        operation_id: meerkat_core::ops::OperationId,
    },
    DetachedWakePresenceMismatch {
        pending_present: bool,
        signaled_present: bool,
    },
    DetachedWakeSignaledWithoutPending,
    OperationPeerReadyWithoutExpectedKind {
        operation_id: meerkat_core::ops::OperationId,
        kind: meerkat_core::ops_lifecycle::OperationKind,
    },
    OperationPeerReadyWithoutHandle {
        operation_id: meerkat_core::ops::OperationId,
    },
    OperationHandleWithoutPeerReady {
        operation_id: meerkat_core::ops::OperationId,
    },
    OperationTerminalOutcomeMismatch {
        operation_id: meerkat_core::ops::OperationId,
        status: meerkat_core::ops_lifecycle::OperationStatus,
        terminal_outcome_present: bool,
    },
    OperationCompletionTimestampMismatch {
        operation_id: meerkat_core::ops::OperationId,
        status: meerkat_core::ops_lifecycle::OperationStatus,
        completed_at_present: bool,
        elapsed_ms_present: bool,
    },
    OperationActiveWithoutStartTimestamp {
        operation_id: meerkat_core::ops::OperationId,
        status: meerkat_core::ops_lifecycle::OperationStatus,
    },
    OperationTerminalWithWatchers {
        operation_id: meerkat_core::ops::OperationId,
        watcher_count: u32,
    },
}

fn turn_requires_active_run(phase: TurnPhase) -> bool {
    !matches!(
        phase,
        TurnPhase::Ready | TurnPhase::Completed | TurnPhase::Failed | TurnPhase::Cancelled
    )
}

fn contributor_lifecycle_is_valid(lifecycle: Option<meerkat_runtime::InputLifecycleState>) -> bool {
    matches!(
        lifecycle,
        Some(
            meerkat_runtime::InputLifecycleState::Staged
                | meerkat_runtime::InputLifecycleState::Applied
                | meerkat_runtime::InputLifecycleState::AppliedPendingConsumption
        )
    )
}

fn lifecycle_control_shadow_mismatch_from_violation(
    violation: MeerkatMachineInvariantViolation,
) -> Option<MeerkatShadowMismatch> {
    match violation {
        MeerkatMachineInvariantViolation::CurrentRunInIllegalControlPhase { phase } => {
            Some(MeerkatShadowMismatch {
                lane: MeerkatShadowLane::LifecycleControl,
                region: "control",
                entity_id: None,
                expected_summary: "current_run_id is only present while the runtime is Running"
                    .into(),
                observed_summary: format!("control phase {phase:?} still carries current_run_id"),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MeerkatMachineInvariantViolation::RunningWithoutCurrentRun => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::LifecycleControl,
            region: "control",
            entity_id: None,
            expected_summary: "Running runtimes always carry a current run binding".into(),
            observed_summary: "runtime is Running with no current_run_id".into(),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::ControlInputsRunMismatch {
            control_run_id,
            inputs_run_id,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::LifecycleControl,
            region: "control.inputs",
            entity_id: None,
            expected_summary: "control and ingress agree on the active run binding".into(),
            observed_summary: format!(
                "control current_run_id = {control_run_id:?}, inputs current_run_id = {inputs_run_id:?}"
            ),
            triage: ShadowMismatchTriage::DogmaViolation,
        }),
        MeerkatMachineInvariantViolation::DestroyedIngressStillHasCurrentRun { run_id } => {
            Some(MeerkatShadowMismatch {
                lane: MeerkatShadowLane::LifecycleControl,
                region: "inputs",
                entity_id: Some(format!("{run_id:?}")),
                expected_summary: "destroyed ingress clears active run binding".into(),
                observed_summary: "destroyed ingress still has current_run_id".into(),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        MeerkatMachineInvariantViolation::DestroyedIngressStillHasContributors {
            contributor_count,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::LifecycleControl,
            region: "inputs",
            entity_id: None,
            expected_summary: "destroyed ingress clears contributor bindings".into(),
            observed_summary: format!(
                "destroyed ingress still carries {contributor_count} contributor bindings"
            ),
            triage: ShadowMismatchTriage::DogmaViolation,
        }),
        MeerkatMachineInvariantViolation::CurrentRunWithoutContributors { run_id } => {
            Some(MeerkatShadowMismatch {
                lane: MeerkatShadowLane::LifecycleControl,
                region: "inputs",
                entity_id: Some(format!("{run_id:?}")),
                expected_summary: "active runs retain contributor bindings".into(),
                observed_summary: "current_run_id is present with no contributors".into(),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        MeerkatMachineInvariantViolation::ContributorsWithoutCurrentRun { contributor_count } => {
            Some(MeerkatShadowMismatch {
                lane: MeerkatShadowLane::LifecycleControl,
                region: "inputs",
                entity_id: None,
                expected_summary: "contributor bindings imply an active run".into(),
                observed_summary: format!(
                    "{contributor_count} contributor bindings remain without current_run_id"
                ),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        MeerkatMachineInvariantViolation::CurrentRunContributorRunMismatch {
            input_id,
            current_run_id,
            last_run_id,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::LifecycleControl,
            region: "inputs",
            entity_id: Some(format!("{input_id:?}")),
            expected_summary: "current-run contributors are bound to the active run".into(),
            observed_summary: format!(
                "input last_run_id = {last_run_id:?}, active current_run_id = {current_run_id:?}"
            ),
            triage: ShadowMismatchTriage::DogmaViolation,
        }),
        MeerkatMachineInvariantViolation::CurrentRunBoundInputMissingContributor {
            input_id,
            current_run_id,
            lifecycle,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::LifecycleControl,
            region: "inputs",
            entity_id: Some(format!("{input_id:?}")),
            expected_summary:
                "run-bound staged/applied inputs remain represented in current_run_contributors"
                    .into(),
            observed_summary: format!(
                "input with lifecycle {lifecycle:?} is still bound to current_run_id {current_run_id:?} but missing from contributors"
            ),
            triage: ShadowMismatchTriage::DogmaViolation,
        }),
        MeerkatMachineInvariantViolation::DestroyedRuntimeStillHasQueuedWork { queue, count } => {
            Some(MeerkatShadowMismatch {
                lane: MeerkatShadowLane::LifecycleControl,
                region: "inputs",
                entity_id: Some(queue.to_string()),
                expected_summary: "destroyed runtimes clear queued work immediately".into(),
                observed_summary: format!("destroyed runtime still has {count} queued inputs"),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MeerkatMachineInvariantViolation::DestroyedRuntimeStillHasCompletionWaiters {
            waiter_count,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::LifecycleControl,
            region: "completion_waiters",
            entity_id: None,
            expected_summary: "destroyed runtimes clear input completion waiters".into(),
            observed_summary: format!(
                "destroyed runtime still has {waiter_count} completion waiters"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::RetiredRuntimeStillHasCompletionWaiters {
            waiter_count,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::LifecycleControl,
            region: "completion_waiters",
            entity_id: None,
            expected_summary: "settled retired runtimes clear input completion waiters".into(),
            observed_summary: format!(
                "retired runtime still has {waiter_count} completion waiters"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::StoppedRuntimeStillHasQueuedWork { queue, count } => {
            Some(MeerkatShadowMismatch {
                lane: MeerkatShadowLane::LifecycleControl,
                region: "inputs",
                entity_id: Some(queue.to_string()),
                expected_summary: "stopped runtimes clear queued work immediately".into(),
                observed_summary: format!("stopped runtime still has {count} queued inputs"),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MeerkatMachineInvariantViolation::StoppedRuntimeStillHasCompletionWaiters {
            waiter_count,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::LifecycleControl,
            region: "completion_waiters",
            entity_id: None,
            expected_summary: "stopped runtimes clear input completion waiters".into(),
            observed_summary: format!(
                "stopped runtime still has {waiter_count} completion waiters"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        _ => None,
    }
}

fn capture_lifecycle_control_shadow_mismatches(
    snapshot: &MeerkatMachineSnapshot,
) -> Vec<MeerkatShadowMismatch> {
    validate_meerkat_machine_snapshot(snapshot)
        .into_iter()
        .filter_map(lifecycle_control_shadow_mismatch_from_violation)
        .collect()
}

fn tools_shadow_mismatch_from_violation(
    violation: MeerkatMachineInvariantViolation,
) -> Option<MeerkatShadowMismatch> {
    match violation {
        MeerkatMachineInvariantViolation::ToolSurfaceVisibleBaseMismatch {
            surface_id,
            visible,
            base_state,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::Tools,
            region: "tool_surface",
            entity_id: Some(surface_id),
            expected_summary: "tool surface visibility matches whether the surface is Active"
                .into(),
            observed_summary: format!("visible = {visible}, base_state = {base_state:?}"),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::ToolSurfacePendingBaseMismatch {
            surface_id,
            base_state,
            pending_op,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::Tools,
            region: "tool_surface",
            entity_id: Some(surface_id),
            expected_summary:
                "pending tool-surface operations stay compatible with the current base state".into(),
            observed_summary: format!("base_state = {base_state:?}, pending_op = {pending_op:?}"),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::ToolSurfaceRemovalTimingMismatch {
            surface_id,
            base_state,
            has_removal_timing,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::Tools,
            region: "tool_surface",
            entity_id: Some(surface_id),
            expected_summary: "removal timing is present iff the tool surface is Removing".into(),
            observed_summary: format!(
                "base_state = {base_state:?}, has_removal_timing = {has_removal_timing}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::ToolSurfaceInflightBaseMismatch {
            surface_id,
            base_state,
            inflight_call_count,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::Tools,
            region: "tool_surface",
            entity_id: Some(surface_id),
            expected_summary: "inflight tool calls only exist for Active or Removing tool surfaces"
                .into(),
            observed_summary: format!(
                "base_state = {base_state:?}, inflight_call_count = {inflight_call_count}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::ToolSurfaceForcedPhaseWithoutRemoveDelta {
            surface_id,
            last_delta_operation,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::Tools,
            region: "tool_surface",
            entity_id: Some(surface_id),
            expected_summary: "forced tool-surface delta phases only arise from remove lineage"
                .into(),
            observed_summary: format!("last_delta_operation = {last_delta_operation:?}"),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::ToolSurfaceStagedSequenceMismatch {
            surface_id,
            staged_op,
            staged_intent_sequence,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::Tools,
            region: "tool_surface",
            entity_id: Some(surface_id),
            expected_summary:
                "staged tool-surface operations carry a matching staged intent sequence".into(),
            observed_summary: format!(
                "staged_op = {staged_op:?}, staged_intent_sequence = {staged_intent_sequence}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::ToolSurfacePendingSequenceMismatch {
            surface_id,
            pending_op,
            pending_task_sequence,
            pending_lineage_sequence,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::Tools,
            region: "tool_surface",
            entity_id: Some(surface_id),
            expected_summary:
                "pending tool-surface operations publish both task and lineage sequences together"
                    .into(),
            observed_summary: format!(
                "pending_op = {pending_op:?}, pending_task_sequence = {pending_task_sequence}, pending_lineage_sequence = {pending_lineage_sequence}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        _ => None,
    }
}

fn capture_tools_shadow_mismatches(
    snapshot: &MeerkatMachineSnapshot,
) -> Vec<MeerkatShadowMismatch> {
    validate_meerkat_machine_snapshot(snapshot)
        .into_iter()
        .filter_map(tools_shadow_mismatch_from_violation)
        .collect()
}

fn turn_ops_barrier_shadow_mismatch_from_violation(
    violation: MeerkatMachineInvariantViolation,
) -> Option<MeerkatShadowMismatch> {
    match violation {
        MeerkatMachineInvariantViolation::TurnRunMismatch {
            turn_phase,
            control_run_id,
            turn_run_id,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::TurnOpsBarrier,
            region: "turn",
            entity_id: None,
            expected_summary: "turn state and control state agree on the active run binding".into(),
            observed_summary: format!(
                "turn_phase = {turn_phase:?}, control_run_id = {control_run_id:?}, turn_run_id = {turn_run_id:?}"
            ),
            triage: ShadowMismatchTriage::DogmaViolation,
        }),
        MeerkatMachineInvariantViolation::WaitingForOpsWithoutPendingOperations => {
            Some(MeerkatShadowMismatch {
                lane: MeerkatShadowLane::TurnOpsBarrier,
                region: "turn",
                entity_id: None,
                expected_summary:
                    "WaitingForOps turns always retain at least one pending operation".into(),
                observed_summary: "turn is WaitingForOps with no pending operations".into(),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MeerkatMachineInvariantViolation::WaitingForOpsWithoutToolCalls => {
            Some(MeerkatShadowMismatch {
                lane: MeerkatShadowLane::TurnOpsBarrier,
                region: "turn",
                entity_id: None,
                expected_summary: "WaitingForOps turns retain at least one pending tool call"
                    .into(),
                observed_summary: "turn is WaitingForOps with tool_calls_pending = 0".into(),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MeerkatMachineInvariantViolation::PendingOperationsOutsideWaitingPhase {
            turn_phase,
            pending_operation_count,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::TurnOpsBarrier,
            region: "turn",
            entity_id: None,
            expected_summary:
                "pending operation ids are only exposed while the turn is WaitingForOps".into(),
            observed_summary: format!(
                "turn_phase = {turn_phase:?}, pending_operation_count = {pending_operation_count}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::BarrierFlagWithoutOperations => {
            Some(MeerkatShadowMismatch {
                lane: MeerkatShadowLane::TurnOpsBarrier,
                region: "turn.barrier",
                entity_id: None,
                expected_summary:
                    "barrier flags only appear when barrier operation ids are present".into(),
                observed_summary: "turn exposes has_barrier_ops with no barrier operations".into(),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MeerkatMachineInvariantViolation::BarrierOperationsWithoutFlag {
            barrier_operation_count,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::TurnOpsBarrier,
            region: "turn.barrier",
            entity_id: None,
            expected_summary:
                "barrier operation ids imply the turn is marked as having barrier ops".into(),
            observed_summary: format!(
                "barrier_operation_count = {barrier_operation_count} with has_barrier_ops = false"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::UnsatisfiedBarrierWithoutBarrierOps => {
            Some(MeerkatShadowMismatch {
                lane: MeerkatShadowLane::TurnOpsBarrier,
                region: "turn.barrier",
                entity_id: None,
                expected_summary:
                    "unsatisfied barrier state only appears when barrier ops are present".into(),
                observed_summary:
                    "turn reports barrier_satisfied = false with no barrier operations".into(),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MeerkatMachineInvariantViolation::PendingOperationUnknown { operation_id } => {
            Some(MeerkatShadowMismatch {
                lane: MeerkatShadowLane::TurnOpsBarrier,
                region: "turn.ops",
                entity_id: Some(format!("{operation_id:?}")),
                expected_summary:
                    "pending operation ids resolve to known operation-lifecycle records".into(),
                observed_summary: "pending operation id is missing from ops registry".into(),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        MeerkatMachineInvariantViolation::BarrierOperationUnknown { operation_id } => {
            Some(MeerkatShadowMismatch {
                lane: MeerkatShadowLane::TurnOpsBarrier,
                region: "turn.barrier",
                entity_id: Some(format!("{operation_id:?}")),
                expected_summary:
                    "barrier operation ids resolve to known operation-lifecycle records".into(),
                observed_summary: "barrier operation id is missing from ops registry".into(),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        MeerkatMachineInvariantViolation::BarrierOperationNotPending { operation_id } => {
            Some(MeerkatShadowMismatch {
                lane: MeerkatShadowLane::TurnOpsBarrier,
                region: "turn.barrier",
                entity_id: Some(format!("{operation_id:?}")),
                expected_summary: "barrier operation ids are a subset of pending operation ids"
                    .into(),
                observed_summary: "barrier operation id is not listed in pending_operation_ids"
                    .into(),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        MeerkatMachineInvariantViolation::OpsOperationCountMismatch {
            operation_count,
            operations_len,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::TurnOpsBarrier,
            region: "ops",
            entity_id: None,
            expected_summary:
                "ops.operation_count matches the number of surfaced operation records".into(),
            observed_summary: format!(
                "operation_count = {operation_count}, operations_len = {operations_len}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::ActiveOpsExceedsOperationCount {
            active_count,
            operation_count,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::TurnOpsBarrier,
            region: "ops",
            entity_id: None,
            expected_summary: "active ops count never exceeds total operation count".into(),
            observed_summary: format!(
                "active_count = {active_count}, operation_count = {operation_count}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::ActiveOpsStatusCountMismatch {
            active_count,
            nonterminal_count,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::TurnOpsBarrier,
            region: "ops",
            entity_id: None,
            expected_summary: "active ops count matches the number of nonterminal operations"
                .into(),
            observed_summary: format!(
                "active_count = {active_count}, nonterminal_count = {nonterminal_count}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::WaitOperationIdsWithoutWaitRequest {
            wait_operation_count,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::TurnOpsBarrier,
            region: "ops.wait_all",
            entity_id: None,
            expected_summary:
                "wait-all target operation ids only appear with an active wait request".into(),
            observed_summary: format!(
                "wait_operation_count = {wait_operation_count} with no wait_request_id"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::PendingWaitCarrierShapeMismatch {
            pending_wait_present,
            pending_wait_request_id_present,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::TurnOpsBarrier,
            region: "ops.wait_all",
            entity_id: None,
            expected_summary:
                "pending wait carrier flag and pending wait request id appear together".into(),
            observed_summary: format!(
                "pending_wait_present = {pending_wait_present}, pending_wait_request_id_present = {pending_wait_request_id_present}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::PendingWaitCarrierWithoutWaitRequest => {
            Some(MeerkatShadowMismatch {
                lane: MeerkatShadowLane::TurnOpsBarrier,
                region: "ops.wait_all",
                entity_id: None,
                expected_summary: "pending wait carriers only exist while a wait request is active"
                    .into(),
                observed_summary: "pending wait carrier is present with no wait_request_id".into(),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MeerkatMachineInvariantViolation::WaitRequestWithoutTrackedOperations {
            wait_request_id,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::TurnOpsBarrier,
            region: "ops.wait_all",
            entity_id: Some(format!("{wait_request_id:?}")),
            expected_summary: "active wait requests retain at least one tracked target operation"
                .into(),
            observed_summary: "wait_request_id is present with no wait_operation_ids".into(),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::WaitRequestMissingPendingWaitCarrier {
            wait_request_id,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::TurnOpsBarrier,
            region: "ops.wait_all",
            entity_id: Some(format!("{wait_request_id:?}")),
            expected_summary:
                "active wait requests retain the pending wait carrier until settlement".into(),
            observed_summary: "wait_request_id is present with no pending wait carrier".into(),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::PendingWaitCarrierRequestMismatch {
            wait_request_id,
            pending_wait_request_id,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::TurnOpsBarrier,
            region: "ops.wait_all",
            entity_id: Some(format!("{wait_request_id:?}")),
            expected_summary: "pending wait carrier request id matches the active wait request id"
                .into(),
            observed_summary: format!(
                "wait_request_id = {wait_request_id:?}, pending_wait_request_id = {pending_wait_request_id:?}"
            ),
            triage: ShadowMismatchTriage::DogmaViolation,
        }),
        MeerkatMachineInvariantViolation::WaitRequestAlreadySatisfied { wait_request_id } => {
            Some(MeerkatShadowMismatch {
                lane: MeerkatShadowLane::TurnOpsBarrier,
                region: "ops.wait_all",
                entity_id: Some(format!("{wait_request_id:?}")),
                expected_summary:
                    "wait requests clear once all tracked target operations are terminal".into(),
                observed_summary:
                    "wait_request_id still present though all targets are already satisfied".into(),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MeerkatMachineInvariantViolation::WaitTargetsUnknownOperation { operation_id } => {
            Some(MeerkatShadowMismatch {
                lane: MeerkatShadowLane::TurnOpsBarrier,
                region: "ops.wait_all",
                entity_id: Some(format!("{operation_id:?}")),
                expected_summary:
                    "wait-all target operation ids resolve to known operation records".into(),
                observed_summary: "wait target operation id is missing from ops registry".into(),
                triage: ShadowMismatchTriage::DogmaViolation,
            })
        }
        _ => None,
    }
}

fn capture_turn_ops_barrier_shadow_mismatches(
    snapshot: &MeerkatMachineSnapshot,
) -> Vec<MeerkatShadowMismatch> {
    validate_meerkat_machine_snapshot(snapshot)
        .into_iter()
        .filter_map(turn_ops_barrier_shadow_mismatch_from_violation)
        .collect()
}

fn peer_drain_shadow_mismatch_from_violation(
    violation: MeerkatMachineInvariantViolation,
) -> Option<MeerkatShadowMismatch> {
    match violation {
        MeerkatMachineInvariantViolation::PeerAuthorityQueueMismatch {
            authority_phase,
            submission_queue_len,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::PeerDrain,
            region: "peer.authority",
            entity_id: None,
            expected_summary: "peer authority phase and submission queue length stay coherent"
                .into(),
            observed_summary: format!(
                "authority_phase = {authority_phase:?}, submission_queue_len = {submission_queue_len}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::PeerAuthorityTrackedEntryCountMismatch {
            submission_queue_len,
            queued_authority_entries,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::PeerDrain,
            region: "peer.authority",
            entity_id: None,
            expected_summary: "peer authority tracked-entry count matches queued authority entries"
                .into(),
            observed_summary: format!(
                "submission_queue_len = {submission_queue_len}, queued_authority_entries = {queued_authority_entries}"
            ),
            triage: ShadowMismatchTriage::DogmaViolation,
        }),
        MeerkatMachineInvariantViolation::PeerQueueCountMismatch {
            field,
            recorded,
            computed,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::PeerDrain,
            region: "peer.queue",
            entity_id: Some(field.to_string()),
            expected_summary: "peer queue counters match the queued entry projection".into(),
            observed_summary: format!("recorded = {recorded}, computed = {computed}"),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::PeerEntryKindClassMismatch {
            raw_item_id,
            kind,
            class,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::PeerDrain,
            region: "peer.entry",
            entity_id: Some(raw_item_id),
            expected_summary: "peer entry kind and class stay coherent".into(),
            observed_summary: format!("kind = {kind:?}, class = {class:?}"),
            triage: ShadowMismatchTriage::DogmaViolation,
        }),
        MeerkatMachineInvariantViolation::PeerEntryLifecyclePeerMismatch {
            raw_item_id,
            class,
            lifecycle_peer_present,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::PeerDrain,
            region: "peer.entry",
            entity_id: Some(raw_item_id),
            expected_summary:
                "peer lifecycle entries expose lifecycle_peer only for lifecycle classes".into(),
            observed_summary: format!(
                "class = {class:?}, lifecycle_peer_present = {lifecycle_peer_present}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::PeerEntryRequestIdMismatch {
            raw_item_id,
            kind,
            request_id_present,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::PeerDrain,
            region: "peer.entry",
            entity_id: Some(raw_item_id),
            expected_summary:
                "request/response/ack peer entries expose request ids exactly when required".into(),
            observed_summary: format!("kind = {kind:?}, request_id_present = {request_id_present}"),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::PeerEntryFromPeerMismatch {
            raw_item_id,
            kind,
            from_peer_present,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::PeerDrain,
            region: "peer.entry",
            entity_id: Some(raw_item_id),
            expected_summary: "non-plain peer ingress entries always retain from_peer identity"
                .into(),
            observed_summary: format!("kind = {kind:?}, from_peer_present = {from_peer_present}"),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::PeerEntryTrustedSnapshotPresenceMismatch {
            raw_item_id,
            kind,
            trusted_snapshot_present,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::PeerDrain,
            region: "peer.entry",
            entity_id: Some(raw_item_id),
            expected_summary: "trusted snapshots appear only for non-plain peer ingress entries"
                .into(),
            observed_summary: format!(
                "kind = {kind:?}, trusted_snapshot_present = {trusted_snapshot_present}"
            ),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::PeerEntryUntrustedWhileAuthRequired {
            raw_item_id,
            kind,
        } => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::PeerDrain,
            region: "peer.entry",
            entity_id: Some(raw_item_id),
            expected_summary:
                "auth-required peer ingress does not admit untrusted non-plain entries".into(),
            observed_summary: format!("kind = {kind:?} admitted with trusted_snapshot = false"),
            triage: ShadowMismatchTriage::DogmaViolation,
        }),
        MeerkatMachineInvariantViolation::DrainSlotMissingPhase => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::PeerDrain,
            region: "drain",
            entity_id: None,
            expected_summary: "drain slots always expose a phase when present".into(),
            observed_summary: "slot_present = true with phase = None".into(),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::DrainPhaseWithoutSlot { phase } => {
            Some(MeerkatShadowMismatch {
                lane: MeerkatShadowLane::PeerDrain,
                region: "drain",
                entity_id: None,
                expected_summary: "drain phase only appears when a drain slot is present".into(),
                observed_summary: format!("slot_present = false with phase = {phase:?}"),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MeerkatMachineInvariantViolation::DrainModeWithoutSlot { mode } => {
            Some(MeerkatShadowMismatch {
                lane: MeerkatShadowLane::PeerDrain,
                region: "drain",
                entity_id: None,
                expected_summary: "drain mode only appears when a drain slot is present".into(),
                observed_summary: format!("slot_present = false with mode = {mode:?}"),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MeerkatMachineInvariantViolation::DrainHandleWithoutSlot => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::PeerDrain,
            region: "drain",
            entity_id: None,
            expected_summary: "drain handles only appear when a drain slot is present".into(),
            observed_summary: "slot_present = false with handle_present = true".into(),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::DrainPhaseMissingMode { phase } => {
            Some(MeerkatShadowMismatch {
                lane: MeerkatShadowLane::PeerDrain,
                region: "drain",
                entity_id: None,
                expected_summary: "non-inactive drain phases carry an explicit drain mode".into(),
                observed_summary: format!("phase = {phase:?}, mode = None"),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MeerkatMachineInvariantViolation::DrainInactiveWithMode { mode } => {
            Some(MeerkatShadowMismatch {
                lane: MeerkatShadowLane::PeerDrain,
                region: "drain",
                entity_id: None,
                expected_summary: "inactive drain phases do not retain a drain mode".into(),
                observed_summary: format!("phase = Inactive, mode = {mode:?}"),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        MeerkatMachineInvariantViolation::DrainInactiveWithHandle => Some(MeerkatShadowMismatch {
            lane: MeerkatShadowLane::PeerDrain,
            region: "drain",
            entity_id: None,
            expected_summary: "inactive drain phases do not retain a live handle".into(),
            observed_summary: "phase = Inactive with handle_present = true".into(),
            triage: ShadowMismatchTriage::ImplementationDetail,
        }),
        MeerkatMachineInvariantViolation::DrainTerminalPhaseWithHandle { phase } => {
            Some(MeerkatShadowMismatch {
                lane: MeerkatShadowLane::PeerDrain,
                region: "drain",
                entity_id: None,
                expected_summary: "terminal drain phases do not retain a live drain handle".into(),
                observed_summary: format!("phase = {phase:?} with handle_present = true"),
                triage: ShadowMismatchTriage::ImplementationDetail,
            })
        }
        _ => None,
    }
}

fn capture_peer_drain_shadow_mismatches(
    snapshot: &MeerkatMachineSnapshot,
) -> Vec<MeerkatShadowMismatch> {
    validate_meerkat_machine_snapshot(snapshot)
        .into_iter()
        .filter_map(peer_drain_shadow_mismatch_from_violation)
        .collect()
}

pub(crate) async fn capture_meerkat_shadow_report<S>(
    runtime_adapter: &RuntimeSessionAdapter,
    service: &S,
    session_id: &SessionId,
    lane: MeerkatShadowLane,
) -> Result<Option<MeerkatShadowReport>, MeerkatMachineSnapshotError>
where
    S: MeerkatExecutionSnapshotSource + ?Sized,
{
    let Some(snapshot) =
        capture_meerkat_machine_snapshot(runtime_adapter, service, session_id).await?
    else {
        return Ok(None);
    };

    let mismatches = match lane {
        MeerkatShadowLane::LifecycleControl => {
            capture_lifecycle_control_shadow_mismatches(&snapshot)
        }
        MeerkatShadowLane::Tools => capture_tools_shadow_mismatches(&snapshot),
        MeerkatShadowLane::TurnOpsBarrier => capture_turn_ops_barrier_shadow_mismatches(&snapshot),
        MeerkatShadowLane::PeerDrain => capture_peer_drain_shadow_mismatches(&snapshot),
    };

    Ok(Some(MeerkatShadowReport { lane, mismatches }))
}

pub(crate) async fn capture_all_meerkat_shadow_reports<S>(
    runtime_adapter: &RuntimeSessionAdapter,
    service: &S,
    session_id: &SessionId,
) -> Result<Option<MeerkatShadowSuiteReport>, MeerkatMachineSnapshotError>
where
    S: MeerkatExecutionSnapshotSource + ?Sized,
{
    let mut reports = Vec::new();
    for lane in [
        MeerkatShadowLane::LifecycleControl,
        MeerkatShadowLane::Tools,
        MeerkatShadowLane::TurnOpsBarrier,
        MeerkatShadowLane::PeerDrain,
    ] {
        let Some(report) =
            capture_meerkat_shadow_report(runtime_adapter, service, session_id, lane).await?
        else {
            return Ok(None);
        };
        reports.push(report);
    }

    Ok(Some(MeerkatShadowSuiteReport {
        session_id: session_id.clone(),
        reports,
    }))
}

/// Validate the currently joined MeerkatMachine snapshot against a small set
/// of live cross-region invariants.
pub(crate) fn validate_meerkat_machine_snapshot(
    snapshot: &MeerkatMachineSnapshot,
) -> Vec<MeerkatMachineInvariantViolation> {
    let mut violations = Vec::new();
    let control = &snapshot.spine.control;
    let inputs = &snapshot.spine.inputs;
    let ops = &snapshot.spine.ops;
    let admitted_inputs: HashMap<_, _> = inputs
        .admission_order
        .iter()
        .map(|input| (input.input_id.clone(), input))
        .collect();

    if control.current_run_id.is_some() && control.phase != RuntimeState::Running {
        violations.push(
            MeerkatMachineInvariantViolation::CurrentRunInIllegalControlPhase {
                phase: control.phase,
            },
        );
    }

    if control.phase == RuntimeState::Running && control.current_run_id.is_none() {
        violations.push(MeerkatMachineInvariantViolation::RunningWithoutCurrentRun);
    }

    if control.current_run_id != inputs.current_run_id {
        violations.push(MeerkatMachineInvariantViolation::ControlInputsRunMismatch {
            control_run_id: control.current_run_id.clone(),
            inputs_run_id: inputs.current_run_id.clone(),
        });
    }

    if inputs.ingress_phase == meerkat_runtime::IngressPhase::Destroyed {
        for (queue_name, queue) in [
            ("queue", &inputs.queue),
            ("steer_queue", &inputs.steer_queue),
        ] {
            if !queue.is_empty() {
                violations.push(
                    MeerkatMachineInvariantViolation::DestroyedIngressStillHasQueuedWork {
                        queue: queue_name,
                        count: queue.len(),
                    },
                );
            }
        }
        if let Some(run_id) = &inputs.current_run_id {
            violations.push(
                MeerkatMachineInvariantViolation::DestroyedIngressStillHasCurrentRun {
                    run_id: run_id.clone(),
                },
            );
        }
        if !inputs.current_run_contributors.is_empty() {
            violations.push(
                MeerkatMachineInvariantViolation::DestroyedIngressStillHasContributors {
                    contributor_count: inputs.current_run_contributors.len(),
                },
            );
        }
        if inputs.wake_requested {
            violations.push(MeerkatMachineInvariantViolation::DestroyedIngressStillRequestsWake);
        }
        if inputs.process_requested {
            violations
                .push(MeerkatMachineInvariantViolation::DestroyedIngressStillRequestsProcessing);
        }
    }

    match (
        &inputs.current_run_id,
        inputs.current_run_contributors.is_empty(),
    ) {
        (Some(run_id), true) => {
            violations.push(
                MeerkatMachineInvariantViolation::CurrentRunWithoutContributors {
                    run_id: run_id.clone(),
                },
            );
        }
        (None, false) => {
            violations.push(
                MeerkatMachineInvariantViolation::ContributorsWithoutCurrentRun {
                    contributor_count: inputs.current_run_contributors.len(),
                },
            );
        }
        _ => {}
    }

    for (queue_name, queue) in [
        ("queue", &inputs.queue),
        ("steer_queue", &inputs.steer_queue),
    ] {
        for input_id in queue {
            let Some(input) = admitted_inputs.get(input_id) else {
                violations.push(
                    MeerkatMachineInvariantViolation::QueueContainsUnknownInput {
                        queue: queue_name,
                        input_id: input_id.clone(),
                    },
                );
                continue;
            };

            if input.lifecycle != Some(meerkat_runtime::InputLifecycleState::Queued) {
                violations.push(
                    MeerkatMachineInvariantViolation::QueueContainsNonQueuedInput {
                        queue: queue_name,
                        input_id: input_id.clone(),
                        lifecycle: input.lifecycle,
                    },
                );
            }

            let expected_handling_mode = match queue_name {
                "queue" => Some(meerkat_core::types::HandlingMode::Queue),
                "steer_queue" => Some(meerkat_core::types::HandlingMode::Steer),
                _ => None,
            };
            if input.handling_mode != expected_handling_mode {
                violations.push(
                    MeerkatMachineInvariantViolation::QueueContainsWrongHandlingMode {
                        queue: queue_name,
                        input_id: input_id.clone(),
                        handling_mode: input.handling_mode,
                    },
                );
            }
        }
    }

    for input_id in &inputs.queue {
        if inputs.steer_queue.contains(input_id) {
            violations.push(MeerkatMachineInvariantViolation::QueueAppearsInBothQueues {
                input_id: input_id.clone(),
            });
        }
    }

    if inputs.process_requested && !inputs.wake_requested {
        violations.push(MeerkatMachineInvariantViolation::ProcessRequestedWithoutWake);
    }

    if inputs.process_requested && inputs.steer_queue.is_empty() {
        violations.push(MeerkatMachineInvariantViolation::ProcessRequestedWithoutSteerQueue);
    }

    if inputs.wake_requested && inputs.queue.is_empty() && inputs.steer_queue.is_empty() {
        violations.push(MeerkatMachineInvariantViolation::WakeRequestedWithoutQueuedWork);
    }

    for input in &inputs.admission_order {
        if input.content_shape.is_none() {
            violations.push(
                MeerkatMachineInvariantViolation::AdmittedInputMissingContentShape {
                    input_id: input.input_id.clone(),
                },
            );
        }

        if input.handling_mode.is_none() {
            violations.push(
                MeerkatMachineInvariantViolation::AdmittedInputMissingHandlingMode {
                    input_id: input.input_id.clone(),
                },
            );
        }

        if input.lifecycle.is_none() {
            violations.push(
                MeerkatMachineInvariantViolation::AdmittedInputMissingLifecycle {
                    input_id: input.input_id.clone(),
                },
            );
        }

        match (input.lifecycle, input.terminal_outcome.clone()) {
            (Some(lifecycle), None) if lifecycle.is_terminal() => {
                violations.push(
                    MeerkatMachineInvariantViolation::TerminalInputWithoutOutcome {
                        input_id: input.input_id.clone(),
                        lifecycle,
                    },
                );
            }
            (Some(lifecycle), Some(terminal_outcome)) if !lifecycle.is_terminal() => {
                violations.push(
                    MeerkatMachineInvariantViolation::NonTerminalInputWithTerminalOutcome {
                        input_id: input.input_id.clone(),
                        lifecycle,
                        terminal_outcome,
                    },
                );
            }
            _ => {}
        }

        if input.lifecycle == Some(meerkat_runtime::InputLifecycleState::Queued) {
            match input.handling_mode {
                Some(meerkat_core::types::HandlingMode::Queue)
                    if !inputs.queue.contains(&input.input_id) =>
                {
                    violations.push(
                        MeerkatMachineInvariantViolation::QueuedInputMissingOwningQueue {
                            input_id: input.input_id.clone(),
                            handling_mode: meerkat_core::types::HandlingMode::Queue,
                        },
                    );
                }
                Some(meerkat_core::types::HandlingMode::Steer)
                    if !inputs.steer_queue.contains(&input.input_id) =>
                {
                    violations.push(
                        MeerkatMachineInvariantViolation::QueuedInputMissingOwningQueue {
                            input_id: input.input_id.clone(),
                            handling_mode: meerkat_core::types::HandlingMode::Steer,
                        },
                    );
                }
                _ => {}
            }
        }

        if let Some(last_boundary_sequence) = input.last_boundary_sequence {
            if input.last_run_id.is_none() {
                violations.push(
                    MeerkatMachineInvariantViolation::InputBoundarySequenceWithoutRun {
                        input_id: input.input_id.clone(),
                        last_boundary_sequence,
                    },
                );
            }

            if matches!(
                input.lifecycle,
                Some(
                    meerkat_runtime::InputLifecycleState::Queued
                        | meerkat_runtime::InputLifecycleState::Staged
                )
            ) {
                violations.push(
                    MeerkatMachineInvariantViolation::InputBoundarySequenceIllegalLifecycle {
                        input_id: input.input_id.clone(),
                        lifecycle: input
                            .lifecycle
                            .expect("queued/staged lifecycle matched above"),
                        last_boundary_sequence,
                    },
                );
            }
        }

        if matches!(
            input.lifecycle,
            Some(
                meerkat_runtime::InputLifecycleState::Staged
                    | meerkat_runtime::InputLifecycleState::AppliedPendingConsumption
            )
        ) && input.last_run_id.is_none()
        {
            violations.push(
                MeerkatMachineInvariantViolation::InputLifecycleMissingRunBinding {
                    input_id: input.input_id.clone(),
                    lifecycle: input
                        .lifecycle
                        .expect("staged/applied-pending lifecycle matched above"),
                },
            );
        }

        if input.lifecycle == Some(meerkat_runtime::InputLifecycleState::AppliedPendingConsumption)
            && input.last_boundary_sequence.is_none()
        {
            violations.push(
                MeerkatMachineInvariantViolation::AppliedPendingConsumptionMissingBoundarySequence {
                    input_id: input.input_id.clone(),
                },
            );
        }

        if inputs.ingress_phase == meerkat_runtime::IngressPhase::Destroyed
            && let Some(lifecycle) = input.lifecycle
            && !lifecycle.is_terminal()
        {
            violations.push(
                MeerkatMachineInvariantViolation::DestroyedIngressHasNonTerminalInput {
                    input_id: input.input_id.clone(),
                    lifecycle,
                },
            );
        }
    }

    for input_id in &inputs.current_run_contributors {
        let Some(input) = admitted_inputs.get(input_id) else {
            violations.push(
                MeerkatMachineInvariantViolation::CurrentRunContributorUnknownInput {
                    input_id: input_id.clone(),
                },
            );
            continue;
        };

        if !contributor_lifecycle_is_valid(input.lifecycle) {
            violations.push(
                MeerkatMachineInvariantViolation::CurrentRunContributorIllegalLifecycle {
                    input_id: input_id.clone(),
                    lifecycle: input.lifecycle,
                },
            );
        }

        if let Some(current_run_id) = &inputs.current_run_id {
            if input.last_run_id.as_ref() != Some(current_run_id) {
                violations.push(
                    MeerkatMachineInvariantViolation::CurrentRunContributorRunMismatch {
                        input_id: input_id.clone(),
                        current_run_id: current_run_id.clone(),
                        last_run_id: input.last_run_id.clone(),
                    },
                );
            }
            if input.lifecycle
                == Some(meerkat_runtime::InputLifecycleState::AppliedPendingConsumption)
                && input.last_boundary_sequence.is_none()
            {
                violations.push(
                    MeerkatMachineInvariantViolation::CurrentRunContributorPendingConsumptionMissingBoundary {
                        input_id: input_id.clone(),
                        current_run_id: current_run_id.clone(),
                    },
                );
            }
        }
    }

    if let Some(current_run_id) = &inputs.current_run_id {
        let contributor_ids: HashSet<_> = inputs.current_run_contributors.iter().cloned().collect();
        for input in admitted_inputs.values() {
            if input.last_run_id.as_ref() == Some(current_run_id)
                && contributor_lifecycle_is_valid(input.lifecycle)
                && !contributor_ids.contains(&input.input_id)
            {
                violations.push(
                    MeerkatMachineInvariantViolation::CurrentRunBoundInputMissingContributor {
                        input_id: input.input_id.clone(),
                        current_run_id: current_run_id.clone(),
                        lifecycle: input
                            .lifecycle
                            .expect("contributor-compatible lifecycle matched above"),
                    },
                );
            }
        }
    }

    if let Some(turn) = &snapshot.turn {
        if turn_requires_active_run(turn.turn_phase) && turn.active_run_id != control.current_run_id
        {
            violations.push(MeerkatMachineInvariantViolation::TurnRunMismatch {
                turn_phase: turn.turn_phase,
                control_run_id: control.current_run_id.clone(),
                turn_run_id: turn.active_run_id.clone(),
            });
        }

        if turn.turn_phase == TurnPhase::WaitingForOps
            && turn
                .pending_operation_ids
                .as_ref()
                .is_none_or(Vec::is_empty)
        {
            violations
                .push(MeerkatMachineInvariantViolation::WaitingForOpsWithoutPendingOperations);
        }

        if turn.turn_phase == TurnPhase::WaitingForOps && turn.tool_calls_pending == 0 {
            violations.push(MeerkatMachineInvariantViolation::WaitingForOpsWithoutToolCalls);
        }

        if turn.turn_phase != TurnPhase::WaitingForOps
            && let Some(pending_operation_ids) = &turn.pending_operation_ids
            && !pending_operation_ids.is_empty()
        {
            violations.push(
                MeerkatMachineInvariantViolation::PendingOperationsOutsideWaitingPhase {
                    turn_phase: turn.turn_phase,
                    pending_operation_count: pending_operation_ids.len(),
                },
            );
        }

        if turn.has_barrier_ops && turn.barrier_operation_ids.is_empty() {
            violations.push(MeerkatMachineInvariantViolation::BarrierFlagWithoutOperations);
        }
        if !turn.has_barrier_ops && !turn.barrier_operation_ids.is_empty() {
            violations.push(
                MeerkatMachineInvariantViolation::BarrierOperationsWithoutFlag {
                    barrier_operation_count: turn.barrier_operation_ids.len(),
                },
            );
        }
        if !turn.has_barrier_ops && !turn.barrier_satisfied {
            violations.push(MeerkatMachineInvariantViolation::UnsatisfiedBarrierWithoutBarrierOps);
        }

        let ops_operation_ids: HashSet<_> = ops
            .operations
            .iter()
            .map(|operation| operation.id.clone())
            .collect();
        let pending_operation_id_set: HashSet<_> = turn
            .pending_operation_ids
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect();

        for operation_id in &pending_operation_id_set {
            if !ops_operation_ids.contains(operation_id) {
                violations.push(MeerkatMachineInvariantViolation::PendingOperationUnknown {
                    operation_id: operation_id.clone(),
                });
            }
        }

        for operation_id in &turn.barrier_operation_ids {
            if !ops_operation_ids.contains(operation_id) {
                violations.push(MeerkatMachineInvariantViolation::BarrierOperationUnknown {
                    operation_id: operation_id.clone(),
                });
            }
            if !pending_operation_id_set.contains(operation_id) {
                violations.push(
                    MeerkatMachineInvariantViolation::BarrierOperationNotPending {
                        operation_id: operation_id.clone(),
                    },
                );
            }
        }
    }

    let completion_waiters = &snapshot.spine.completion_waiters;
    if completion_waiters.input_count != completion_waiters.waiting_inputs.len() {
        violations.push(
            MeerkatMachineInvariantViolation::CompletionWaiterInputCountMismatch {
                recorded_input_count: completion_waiters.input_count,
                waiting_inputs_len: completion_waiters.waiting_inputs.len(),
            },
        );
    }

    let summed_waiter_count: usize = completion_waiters
        .waiting_inputs
        .iter()
        .map(|entry| entry.waiter_count)
        .sum();
    if completion_waiters.waiter_count != summed_waiter_count {
        violations.push(
            MeerkatMachineInvariantViolation::CompletionWaiterCountMismatch {
                recorded_waiter_count: completion_waiters.waiter_count,
                summed_waiter_count,
            },
        );
    }

    if matches!(
        control.phase,
        RuntimeState::Destroyed | RuntimeState::Stopped
    ) {
        for (queue_name, queue) in [
            ("queue", &inputs.queue),
            ("steer_queue", &inputs.steer_queue),
        ] {
            if queue.is_empty() {
                continue;
            }

            match control.phase {
                RuntimeState::Destroyed => violations.push(
                    MeerkatMachineInvariantViolation::DestroyedRuntimeStillHasQueuedWork {
                        queue: queue_name,
                        count: queue.len(),
                    },
                ),
                RuntimeState::Stopped => violations.push(
                    MeerkatMachineInvariantViolation::StoppedRuntimeStillHasQueuedWork {
                        queue: queue_name,
                        count: queue.len(),
                    },
                ),
                _ => {}
            }
        }
    }

    if control.phase == RuntimeState::Destroyed && completion_waiters.waiter_count > 0 {
        violations.push(
            MeerkatMachineInvariantViolation::DestroyedRuntimeStillHasCompletionWaiters {
                waiter_count: completion_waiters.waiter_count,
            },
        );
    }
    if control.phase == RuntimeState::Retired && completion_waiters.waiter_count > 0 {
        violations.push(
            MeerkatMachineInvariantViolation::RetiredRuntimeStillHasCompletionWaiters {
                waiter_count: completion_waiters.waiter_count,
            },
        );
    }
    if control.phase == RuntimeState::Stopped && completion_waiters.waiter_count > 0 {
        violations.push(
            MeerkatMachineInvariantViolation::StoppedRuntimeStillHasCompletionWaiters {
                waiter_count: completion_waiters.waiter_count,
            },
        );
    }

    for entry in &completion_waiters.waiting_inputs {
        let Some(input) = admitted_inputs.get(&entry.input_id) else {
            violations.push(
                MeerkatMachineInvariantViolation::CompletionWaiterUnknownInput {
                    input_id: entry.input_id.clone(),
                },
            );
            continue;
        };

        if entry.waiter_count == 0 {
            violations.push(
                MeerkatMachineInvariantViolation::CompletionWaiterZeroCount {
                    input_id: entry.input_id.clone(),
                },
            );
        }

        let terminal_outcome_present = input.terminal_outcome.is_some();
        if input
            .lifecycle
            .is_some_and(|lifecycle| lifecycle.is_terminal())
            || terminal_outcome_present
        {
            violations.push(
                MeerkatMachineInvariantViolation::CompletionWaiterResolvedInput {
                    input_id: entry.input_id.clone(),
                    lifecycle: input.lifecycle,
                    terminal_outcome_present,
                },
            );
        }
    }

    push_drain_mismatches(&mut violations, &snapshot.spine.drain);

    if let Some(peer) = &snapshot.peer {
        let queue_mismatch = match peer.authority_phase {
            PeerIngressAuthorityPhase::Absent
            | PeerIngressAuthorityPhase::Dropped
            | PeerIngressAuthorityPhase::Delivered => peer.submission_queue_len != 0,
            PeerIngressAuthorityPhase::Received => peer.submission_queue_len == 0,
        };

        if queue_mismatch {
            violations.push(
                MeerkatMachineInvariantViolation::PeerAuthorityQueueMismatch {
                    authority_phase: peer.authority_phase,
                    submission_queue_len: peer.submission_queue_len,
                },
            );
        }

        push_peer_authority_mismatches(&mut violations, peer);
        push_peer_queue_mismatches(&mut violations, &peer.queue);
    }

    if let Some(tool_surface) = &snapshot.tool_surface {
        for entry in &tool_surface.entries {
            if entry.visible != (entry.base_state == ExternalToolSurfaceBaseState::Active) {
                violations.push(
                    MeerkatMachineInvariantViolation::ToolSurfaceVisibleBaseMismatch {
                        surface_id: entry.surface_id.clone(),
                        visible: entry.visible,
                        base_state: entry.base_state,
                    },
                );
            }

            let pending_base_mismatch = match entry.base_state {
                ExternalToolSurfaceBaseState::Removing => {
                    entry.pending_op != ExternalToolSurfacePendingOp::None
                }
                ExternalToolSurfaceBaseState::Removed => !matches!(
                    entry.pending_op,
                    ExternalToolSurfacePendingOp::None | ExternalToolSurfacePendingOp::Add
                ),
                ExternalToolSurfaceBaseState::Active | ExternalToolSurfaceBaseState::Absent => {
                    entry.pending_op == ExternalToolSurfacePendingOp::Reload
                        && entry.base_state != ExternalToolSurfaceBaseState::Active
                }
            };
            if pending_base_mismatch {
                violations.push(
                    MeerkatMachineInvariantViolation::ToolSurfacePendingBaseMismatch {
                        surface_id: entry.surface_id.clone(),
                        base_state: entry.base_state,
                        pending_op: entry.pending_op,
                    },
                );
            }

            let removal_timing_mismatch = entry.has_removal_timing
                != (entry.base_state == ExternalToolSurfaceBaseState::Removing);
            if removal_timing_mismatch {
                violations.push(
                    MeerkatMachineInvariantViolation::ToolSurfaceRemovalTimingMismatch {
                        surface_id: entry.surface_id.clone(),
                        base_state: entry.base_state,
                        has_removal_timing: entry.has_removal_timing,
                    },
                );
            }

            if entry.inflight_call_count > 0
                && !matches!(
                    entry.base_state,
                    ExternalToolSurfaceBaseState::Active | ExternalToolSurfaceBaseState::Removing
                )
            {
                violations.push(
                    MeerkatMachineInvariantViolation::ToolSurfaceInflightBaseMismatch {
                        surface_id: entry.surface_id.clone(),
                        base_state: entry.base_state,
                        inflight_call_count: entry.inflight_call_count,
                    },
                );
            }

            if entry.last_delta_phase == ExternalToolSurfaceDeltaPhase::Forced
                && entry.last_delta_operation != ExternalToolSurfaceDeltaOperation::Remove
            {
                violations.push(
                    MeerkatMachineInvariantViolation::ToolSurfaceForcedPhaseWithoutRemoveDelta {
                        surface_id: entry.surface_id.clone(),
                        last_delta_operation: entry.last_delta_operation,
                    },
                );
            }

            let staged_sequence_mismatch = if entry.staged_op == ExternalToolSurfaceStagedOp::None {
                entry.staged_intent_sequence != 0
            } else {
                entry.staged_intent_sequence == 0
            };
            if staged_sequence_mismatch {
                violations.push(
                    MeerkatMachineInvariantViolation::ToolSurfaceStagedSequenceMismatch {
                        surface_id: entry.surface_id.clone(),
                        staged_op: entry.staged_op,
                        staged_intent_sequence: entry.staged_intent_sequence,
                    },
                );
            }

            let pending_sequence_mismatch =
                if entry.pending_op == ExternalToolSurfacePendingOp::None {
                    entry.pending_task_sequence != 0 || entry.pending_lineage_sequence != 0
                } else {
                    entry.pending_task_sequence == 0 || entry.pending_lineage_sequence == 0
                };
            if pending_sequence_mismatch {
                violations.push(
                    MeerkatMachineInvariantViolation::ToolSurfacePendingSequenceMismatch {
                        surface_id: entry.surface_id.clone(),
                        pending_op: entry.pending_op,
                        pending_task_sequence: entry.pending_task_sequence,
                        pending_lineage_sequence: entry.pending_lineage_sequence,
                    },
                );
            }
        }
    }

    push_ops_mismatches(&mut violations, &snapshot.spine.binding, ops);

    violations
}

fn push_drain_mismatches(
    violations: &mut Vec<MeerkatMachineInvariantViolation>,
    drain: &meerkat_runtime::MeerkatDrainSnapshot,
) {
    if !drain.slot_present {
        if let Some(phase) = drain.phase {
            violations.push(MeerkatMachineInvariantViolation::DrainPhaseWithoutSlot { phase });
        }
        if let Some(mode) = drain.mode {
            violations.push(MeerkatMachineInvariantViolation::DrainModeWithoutSlot { mode });
        }
        if drain.handle_present {
            violations.push(MeerkatMachineInvariantViolation::DrainHandleWithoutSlot);
        }
        return;
    }

    let Some(phase) = drain.phase else {
        violations.push(MeerkatMachineInvariantViolation::DrainSlotMissingPhase);
        return;
    };

    match phase {
        CommsDrainPhase::Inactive => {
            if let Some(mode) = drain.mode {
                violations.push(MeerkatMachineInvariantViolation::DrainInactiveWithMode { mode });
            }
            if drain.handle_present {
                violations.push(MeerkatMachineInvariantViolation::DrainInactiveWithHandle);
            }
        }
        CommsDrainPhase::Starting
        | CommsDrainPhase::Running
        | CommsDrainPhase::ExitedRespawnable
        | CommsDrainPhase::Stopped => {
            if drain.mode.is_none() {
                violations.push(MeerkatMachineInvariantViolation::DrainPhaseMissingMode { phase });
            }
        }
    }

    if drain.handle_present
        && matches!(
            phase,
            CommsDrainPhase::Inactive
                | CommsDrainPhase::ExitedRespawnable
                | CommsDrainPhase::Stopped
        )
    {
        violations.push(MeerkatMachineInvariantViolation::DrainTerminalPhaseWithHandle { phase });
    }
}

fn push_ops_mismatches(
    violations: &mut Vec<MeerkatMachineInvariantViolation>,
    binding: &meerkat_runtime::MeerkatBindingSnapshot,
    ops: &meerkat_runtime::MeerkatOpsSnapshot,
) {
    if ops.operation_count != ops.operations.len() {
        violations.push(
            MeerkatMachineInvariantViolation::OpsOperationCountMismatch {
                operation_count: ops.operation_count,
                operations_len: ops.operations.len(),
            },
        );
    }

    if ops.active_count > ops.operation_count {
        violations.push(
            MeerkatMachineInvariantViolation::ActiveOpsExceedsOperationCount {
                active_count: ops.active_count,
                operation_count: ops.operation_count,
            },
        );
    }

    let nonterminal_count = ops
        .operations
        .iter()
        .filter(|operation| !operation.status.is_terminal())
        .count();
    if ops.operation_count == ops.operations.len() && ops.active_count != nonterminal_count {
        violations.push(
            MeerkatMachineInvariantViolation::ActiveOpsStatusCountMismatch {
                active_count: ops.active_count,
                nonterminal_count,
            },
        );
    }

    match (&ops.wait_request_id, ops.wait_operation_ids.is_empty()) {
        (None, false) => violations.push(
            MeerkatMachineInvariantViolation::WaitOperationIdsWithoutWaitRequest {
                wait_operation_count: ops.wait_operation_ids.len(),
            },
        ),
        (Some(wait_request_id), true) => violations.push(
            MeerkatMachineInvariantViolation::WaitRequestWithoutTrackedOperations {
                wait_request_id: wait_request_id.clone(),
            },
        ),
        _ => {}
    }
    if ops.pending_wait_present != ops.pending_wait_request_id.is_some() {
        violations.push(
            MeerkatMachineInvariantViolation::PendingWaitCarrierShapeMismatch {
                pending_wait_present: ops.pending_wait_present,
                pending_wait_request_id_present: ops.pending_wait_request_id.is_some(),
            },
        );
    }
    match (&ops.wait_request_id, ops.pending_wait_present) {
        (None, true) => {
            violations.push(MeerkatMachineInvariantViolation::PendingWaitCarrierWithoutWaitRequest)
        }
        (Some(wait_request_id), false) => violations.push(
            MeerkatMachineInvariantViolation::WaitRequestMissingPendingWaitCarrier {
                wait_request_id: wait_request_id.clone(),
            },
        ),
        _ => {}
    }
    if let (Some(wait_request_id), Some(pending_wait_request_id)) =
        (&ops.wait_request_id, &ops.pending_wait_request_id)
        && wait_request_id != pending_wait_request_id
    {
        violations.push(
            MeerkatMachineInvariantViolation::PendingWaitCarrierRequestMismatch {
                wait_request_id: wait_request_id.clone(),
                pending_wait_request_id: pending_wait_request_id.clone(),
            },
        );
    }

    let operation_ids: HashSet<_> = ops
        .operations
        .iter()
        .map(|operation| operation.id.clone())
        .collect();
    let operations_by_id: HashMap<_, _> = ops
        .operations
        .iter()
        .map(|operation| (operation.id.clone(), operation))
        .collect();
    for operation_id in &ops.wait_operation_ids {
        if !operation_ids.contains(operation_id) {
            violations.push(
                MeerkatMachineInvariantViolation::WaitTargetsUnknownOperation {
                    operation_id: operation_id.clone(),
                },
            );
        }
    }
    if let Some(wait_request_id) = &ops.wait_request_id
        && !ops.wait_operation_ids.is_empty()
        && ops.wait_operation_ids.iter().all(|operation_id| {
            operations_by_id
                .get(operation_id)
                .is_some_and(|operation| operation.status.is_terminal())
        })
    {
        violations.push(
            MeerkatMachineInvariantViolation::WaitRequestAlreadySatisfied {
                wait_request_id: wait_request_id.clone(),
            },
        );
    }

    let pending_present = ops.detached_wake_pending.is_some();
    let signaled_present = ops.detached_wake_signaled.is_some();
    if pending_present != signaled_present {
        violations.push(
            MeerkatMachineInvariantViolation::DetachedWakePresenceMismatch {
                pending_present,
                signaled_present,
            },
        );
    }
    if pending_present == signaled_present && binding.detached_wake_present != pending_present {
        violations.push(
            MeerkatMachineInvariantViolation::DetachedWakeBindingMismatch {
                binding_present: binding.detached_wake_present,
                ops_present: pending_present,
            },
        );
    }
    if ops.detached_wake_pending == Some(false) && ops.detached_wake_signaled == Some(true) {
        violations.push(MeerkatMachineInvariantViolation::DetachedWakeSignaledWithoutPending);
    }

    for operation in &ops.operations {
        if operation.peer_ready && !operation.kind.expects_peer_channel() {
            violations.push(
                MeerkatMachineInvariantViolation::OperationPeerReadyWithoutExpectedKind {
                    operation_id: operation.id.clone(),
                    kind: operation.kind,
                },
            );
        }
        if operation.peer_ready && operation.peer_handle.is_none() {
            violations.push(
                MeerkatMachineInvariantViolation::OperationPeerReadyWithoutHandle {
                    operation_id: operation.id.clone(),
                },
            );
        }
        if !operation.peer_ready && operation.peer_handle.is_some() {
            violations.push(
                MeerkatMachineInvariantViolation::OperationHandleWithoutPeerReady {
                    operation_id: operation.id.clone(),
                },
            );
        }

        let terminal_outcome_present = operation.terminal_outcome.is_some();
        if operation.status.is_terminal() != terminal_outcome_present {
            violations.push(
                MeerkatMachineInvariantViolation::OperationTerminalOutcomeMismatch {
                    operation_id: operation.id.clone(),
                    status: operation.status,
                    terminal_outcome_present,
                },
            );
        }

        let completed_at_present = operation.completed_at_ms.is_some();
        let elapsed_ms_present = operation.elapsed_ms.is_some();
        if operation.status.is_terminal() != completed_at_present
            || operation.status.is_terminal() != elapsed_ms_present
        {
            violations.push(
                MeerkatMachineInvariantViolation::OperationCompletionTimestampMismatch {
                    operation_id: operation.id.clone(),
                    status: operation.status,
                    completed_at_present,
                    elapsed_ms_present,
                },
            );
        }

        if matches!(
            operation.status,
            meerkat_core::ops_lifecycle::OperationStatus::Running
                | meerkat_core::ops_lifecycle::OperationStatus::Retiring
        ) && operation.started_at_ms.is_none()
        {
            violations.push(
                MeerkatMachineInvariantViolation::OperationActiveWithoutStartTimestamp {
                    operation_id: operation.id.clone(),
                    status: operation.status,
                },
            );
        }

        if operation.status.is_terminal() && operation.watcher_count != 0 {
            violations.push(
                MeerkatMachineInvariantViolation::OperationTerminalWithWatchers {
                    operation_id: operation.id.clone(),
                    watcher_count: operation.watcher_count,
                },
            );
        }
    }
}

fn push_peer_authority_mismatches(
    violations: &mut Vec<MeerkatMachineInvariantViolation>,
    peer: &PeerIngressRuntimeSnapshot,
) {
    let queued_authority_entries = peer
        .queue
        .queued_entries
        .iter()
        .filter(|entry| entry.trusted_snapshot.is_some())
        .count();

    if peer.submission_queue_len != queued_authority_entries {
        violations.push(
            MeerkatMachineInvariantViolation::PeerAuthorityTrackedEntryCountMismatch {
                submission_queue_len: peer.submission_queue_len,
                queued_authority_entries,
            },
        );
    }

    for entry in &peer.queue.queued_entries {
        let trusted_snapshot_present = entry.trusted_snapshot.is_some();
        let expected_trusted_snapshot = entry.kind != PeerIngressKind::PlainEvent;
        if trusted_snapshot_present != expected_trusted_snapshot {
            violations.push(
                MeerkatMachineInvariantViolation::PeerEntryTrustedSnapshotPresenceMismatch {
                    raw_item_id: entry.raw_item_id.clone(),
                    kind: entry.kind,
                    trusted_snapshot_present,
                },
            );
        }

        if peer.auth_required
            && entry.kind != PeerIngressKind::PlainEvent
            && entry.trusted_snapshot == Some(false)
        {
            violations.push(
                MeerkatMachineInvariantViolation::PeerEntryUntrustedWhileAuthRequired {
                    raw_item_id: entry.raw_item_id.clone(),
                    kind: entry.kind,
                },
            );
        }
    }
}

fn push_peer_queue_mismatches(
    violations: &mut Vec<MeerkatMachineInvariantViolation>,
    queue: &PeerIngressQueueSnapshot,
) {
    let computed_total = queue.queued_entries.len();
    if queue.total_count != computed_total {
        violations.push(MeerkatMachineInvariantViolation::PeerQueueCountMismatch {
            field: "total_count",
            recorded: queue.total_count,
            computed: computed_total,
        });
    }

    for (field, recorded, computed) in [
        (
            "actionable_count",
            queue.actionable_count,
            queue
                .queued_entries
                .iter()
                .filter(|entry| entry.class.is_actionable())
                .count(),
        ),
        (
            "response_count",
            queue.response_count,
            queue
                .queued_entries
                .iter()
                .filter(|entry| entry.class == PeerInputClass::Response)
                .count(),
        ),
        (
            "lifecycle_count",
            queue.lifecycle_count,
            queue
                .queued_entries
                .iter()
                .filter(|entry| {
                    matches!(
                        entry.class,
                        PeerInputClass::PeerLifecycleAdded | PeerInputClass::PeerLifecycleRetired
                    )
                })
                .count(),
        ),
        (
            "silent_request_count",
            queue.silent_request_count,
            queue
                .queued_entries
                .iter()
                .filter(|entry| entry.class == PeerInputClass::SilentRequest)
                .count(),
        ),
        (
            "ack_count",
            queue.ack_count,
            queue
                .queued_entries
                .iter()
                .filter(|entry| entry.class == PeerInputClass::Ack)
                .count(),
        ),
        (
            "plain_event_count",
            queue.plain_event_count,
            queue
                .queued_entries
                .iter()
                .filter(|entry| entry.class == PeerInputClass::PlainEvent)
                .count(),
        ),
    ] {
        if recorded != computed {
            violations.push(MeerkatMachineInvariantViolation::PeerQueueCountMismatch {
                field,
                recorded,
                computed,
            });
        }
    }

    for entry in &queue.queued_entries {
        let class_matches_kind = match entry.kind {
            PeerIngressKind::Message => entry.class == PeerInputClass::ActionableMessage,
            PeerIngressKind::Request => matches!(
                entry.class,
                PeerInputClass::ActionableRequest
                    | PeerInputClass::PeerLifecycleAdded
                    | PeerInputClass::PeerLifecycleRetired
                    | PeerInputClass::SilentRequest
            ),
            PeerIngressKind::Response => entry.class == PeerInputClass::Response,
            PeerIngressKind::Ack => entry.class == PeerInputClass::Ack,
            PeerIngressKind::PlainEvent => entry.class == PeerInputClass::PlainEvent,
        };
        if !class_matches_kind {
            violations.push(
                MeerkatMachineInvariantViolation::PeerEntryKindClassMismatch {
                    raw_item_id: entry.raw_item_id.clone(),
                    kind: entry.kind,
                    class: entry.class,
                },
            );
        }

        let lifecycle_peer_present = entry.lifecycle_peer.is_some();
        let lifecycle_peer_expected = matches!(
            entry.class,
            PeerInputClass::PeerLifecycleAdded | PeerInputClass::PeerLifecycleRetired
        );
        if lifecycle_peer_present != lifecycle_peer_expected {
            violations.push(
                MeerkatMachineInvariantViolation::PeerEntryLifecyclePeerMismatch {
                    raw_item_id: entry.raw_item_id.clone(),
                    class: entry.class,
                    lifecycle_peer_present,
                },
            );
        }

        let request_id_present = entry.request_id.is_some();
        let request_id_expected = matches!(
            entry.kind,
            PeerIngressKind::Request | PeerIngressKind::Response | PeerIngressKind::Ack
        );
        if request_id_present != request_id_expected {
            violations.push(
                MeerkatMachineInvariantViolation::PeerEntryRequestIdMismatch {
                    raw_item_id: entry.raw_item_id.clone(),
                    kind: entry.kind,
                    request_id_present,
                },
            );
        }

        let from_peer_present = entry.from_peer.is_some();
        let from_peer_expected = entry.kind != PeerIngressKind::PlainEvent;
        if from_peer_present != from_peer_expected {
            violations.push(
                MeerkatMachineInvariantViolation::PeerEntryFromPeerMismatch {
                    raw_item_id: entry.raw_item_id.clone(),
                    kind: entry.kind,
                    from_peer_present,
                },
            );
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub(crate) trait MeerkatExecutionSnapshotSource: Send + Sync {
    async fn execution_snapshot_for_meerkat_machine(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<AgentExecutionSnapshot>, SessionError>;

    async fn tool_scope_snapshot_for_meerkat_machine(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ToolScopeSnapshot>, SessionError>;

    async fn external_tool_surface_snapshot_for_meerkat_machine(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ExternalToolSurfaceSnapshot>, SessionError>;

    async fn peer_ingress_snapshot_for_meerkat_machine(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<PeerIngressRuntimeSnapshot>, SessionError>;
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<B> MeerkatExecutionSnapshotSource for EphemeralSessionService<B>
where
    B: SessionAgentBuilder + Send + Sync + 'static,
{
    async fn execution_snapshot_for_meerkat_machine(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<AgentExecutionSnapshot>, SessionError> {
        self.execution_snapshot(session_id).await
    }

    async fn tool_scope_snapshot_for_meerkat_machine(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ToolScopeSnapshot>, SessionError> {
        self.tool_scope_snapshot(session_id).await
    }

    async fn external_tool_surface_snapshot_for_meerkat_machine(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ExternalToolSurfaceSnapshot>, SessionError> {
        self.external_tool_surface_snapshot(session_id).await
    }

    async fn peer_ingress_snapshot_for_meerkat_machine(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<PeerIngressRuntimeSnapshot>, SessionError> {
        let Some(runtime) = self.comms_runtime(session_id).await else {
            return Ok(None);
        };

        match runtime.peer_ingress_runtime_snapshot().await {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(CommsCapabilityError::Unsupported(_)) => Ok(None),
        }
    }
}

#[cfg(feature = "session-store")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<B> MeerkatExecutionSnapshotSource for meerkat_session::PersistentSessionService<B>
where
    B: SessionAgentBuilder + Send + Sync + 'static,
{
    async fn execution_snapshot_for_meerkat_machine(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<AgentExecutionSnapshot>, SessionError> {
        self.execution_snapshot(session_id).await
    }

    async fn tool_scope_snapshot_for_meerkat_machine(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ToolScopeSnapshot>, SessionError> {
        self.tool_scope_snapshot(session_id).await
    }

    async fn external_tool_surface_snapshot_for_meerkat_machine(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ExternalToolSurfaceSnapshot>, SessionError> {
        self.external_tool_surface_snapshot(session_id).await
    }

    async fn peer_ingress_snapshot_for_meerkat_machine(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<PeerIngressRuntimeSnapshot>, SessionError> {
        let Some(runtime) = self.comms_runtime(session_id).await else {
            return Ok(None);
        };

        match runtime.peer_ingress_runtime_snapshot().await {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(CommsCapabilityError::Unsupported(_)) => Ok(None),
        }
    }
}

/// Capture the current joined MeerkatMachine view for one session.
///
/// `RuntimeSessionAdapter` still owns runtime binding/control/ingress/ops/drain
/// truth, while the live session task owns turn execution truth. This helper
/// composes those read paths without treating either side as derived state.
pub(crate) async fn capture_meerkat_machine_snapshot<S>(
    runtime_adapter: &RuntimeSessionAdapter,
    execution_source: &S,
    session_id: &SessionId,
) -> Result<Option<MeerkatMachineSnapshot>, MeerkatMachineSnapshotError>
where
    S: MeerkatExecutionSnapshotSource + ?Sized,
{
    let Some(spine) = runtime_adapter
        .meerkat_machine_spine_snapshot(session_id)
        .await
    else {
        return Ok(None);
    };

    // Runtime registration can exist before a live session task is built or
    // reattached, so "session not found" here is an honest Meerkat state, not
    // necessarily an error.
    let turn = match execution_source
        .execution_snapshot_for_meerkat_machine(session_id)
        .await
    {
        Ok(snapshot) => snapshot,
        Err(SessionError::NotFound { .. }) => None,
        Err(err) => return Err(err.into()),
    };

    let tools = match execution_source
        .tool_scope_snapshot_for_meerkat_machine(session_id)
        .await
    {
        Ok(snapshot) => snapshot,
        Err(SessionError::NotFound { .. }) => None,
        Err(err) => return Err(err.into()),
    };

    let tool_surface = match execution_source
        .external_tool_surface_snapshot_for_meerkat_machine(session_id)
        .await
    {
        Ok(snapshot) => snapshot,
        Err(SessionError::NotFound { .. }) => None,
        Err(err) => return Err(err.into()),
    };

    let peer = match execution_source
        .peer_ingress_snapshot_for_meerkat_machine(session_id)
        .await
    {
        Ok(snapshot) => snapshot,
        Err(SessionError::NotFound { .. }) => None,
        Err(err) => return Err(err.into()),
    };

    Ok(Some(MeerkatMachineSnapshot {
        spine,
        turn,
        tools,
        tool_surface,
        peer,
    }))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::service_factory::FactoryAgentBuilder;
    use crate::{AgentFactory, Config, Session, SessionService};
    use async_trait::async_trait;
    use chrono::Utc;
    use futures::stream;
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest};
    #[cfg(feature = "comms")]
    use meerkat_comms::Keypair;
    use meerkat_core::DeferredPromptPolicy;
    use meerkat_core::OpsLifecycleRegistry;
    #[cfg(feature = "comms")]
    use meerkat_core::PlainEventSource;
    #[cfg(feature = "mcp")]
    use meerkat_core::agent::AgentToolDispatcher;
    use meerkat_core::agent::CommsRuntime;
    use meerkat_core::comms::TrustedPeerSpec;
    use meerkat_core::comms_drain_lifecycle_authority::CommsDrainPhase;
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::{InputId, RunId, WaitRequestId};
    use meerkat_core::lifecycle::{
        RunApplyBoundary, RunBoundaryReceipt, RunControlCommand, RunPrimitive,
    };
    use meerkat_core::ops::{OperationId, OperationResult};
    use meerkat_core::ops_lifecycle::{
        OperationKind, OperationLifecycleSnapshot, OperationPeerHandle, OperationSpec,
        OperationStatus, OperationTerminalOutcome,
    };
    use meerkat_core::service::{CreateSessionRequest, InitialTurnPolicy, SessionBuildOptions};
    #[cfg(feature = "comms")]
    use meerkat_core::types::HandlingMode;
    #[cfg(feature = "mcp")]
    use meerkat_core::{
        ExternalToolSurfaceBaseState, ExternalToolSurfaceDeltaOperation,
        ExternalToolSurfaceDeltaPhase, ExternalToolSurfaceGlobalPhase,
        ExternalToolSurfacePendingOp, ExternalToolSurfaceStagedOp, McpServerConfig, ToolDef,
        ToolError,
    };
    use meerkat_runtime::runtime_state::RuntimeState;
    use meerkat_runtime::{
        CompletionOutcome, Input, InputLifecycleState, InputTerminalOutcome,
        MeerkatAdmittedInputSnapshot, MeerkatCompletionWaiterSnapshot, PromptInput,
    };
    use meerkat_runtime::{RuntimeControlPlane, SessionServiceRuntimeExt};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::sync::Notify;

    fn invalid_tool_surface_entry(
        surface_id: &str,
        visible: bool,
        base_state: ExternalToolSurfaceBaseState,
    ) -> meerkat_core::ExternalToolSurfaceEntrySnapshot {
        meerkat_core::ExternalToolSurfaceEntrySnapshot {
            surface_id: surface_id.to_string(),
            visible,
            base_state,
            has_removal_timing: base_state == ExternalToolSurfaceBaseState::Removing,
            pending_op: meerkat_core::ExternalToolSurfacePendingOp::None,
            staged_op: meerkat_core::ExternalToolSurfaceStagedOp::None,
            staged_intent_sequence: 0,
            pending_task_sequence: 0,
            pending_lineage_sequence: 0,
            inflight_call_count: 0,
            last_delta_operation: meerkat_core::ExternalToolSurfaceDeltaOperation::None,
            last_delta_phase: meerkat_core::ExternalToolSurfaceDeltaPhase::None,
        }
    }

    fn make_shadow_progress_input(label: &str) -> Input {
        Input::Peer(meerkat_runtime::PeerInput {
            header: meerkat_runtime::InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: meerkat_runtime::InputOrigin::Peer {
                    peer_id: "peer-1".into(),
                    runtime_id: None,
                },
                durability: meerkat_runtime::InputDurability::Ephemeral,
                visibility: meerkat_runtime::InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(meerkat_runtime::PeerConvention::ResponseProgress {
                request_id: format!("req-{label}"),
                phase: meerkat_runtime::ResponseProgressPhase::InProgress,
            }),
            body: format!("progress-{label}"),
            blocks: None,
            handling_mode: None,
        })
    }

    fn invalid_input_snapshot(
        input_id: InputId,
        lifecycle: Option<InputLifecycleState>,
        terminal_outcome: Option<InputTerminalOutcome>,
    ) -> MeerkatAdmittedInputSnapshot {
        MeerkatAdmittedInputSnapshot {
            input_id,
            content_shape: None,
            request_id: None,
            reservation_key: None,
            handling_mode: None,
            lifecycle,
            terminal_outcome,
            last_run_id: None,
            last_boundary_sequence: None,
            is_prompt: true,
        }
    }

    fn invalid_operation_snapshot(
        operation_id: OperationId,
        kind: OperationKind,
        status: OperationStatus,
    ) -> OperationLifecycleSnapshot {
        OperationLifecycleSnapshot {
            id: operation_id,
            kind,
            display_name: "invalid-op".into(),
            status,
            peer_ready: false,
            progress_count: 0,
            watcher_count: 0,
            terminal_outcome: None,
            child_session_id: None,
            peer_handle: None,
            created_at_ms: 1,
            started_at_ms: None,
            completed_at_ms: None,
            elapsed_ms: None,
        }
    }

    struct MockLlmClient;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl LlmClient for MockLlmClient {
        fn stream<'a>(
            &'a self,
            _request: &'a LlmRequest,
        ) -> std::pin::Pin<
            Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>,
        > {
            Box::pin(stream::iter(vec![Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                },
            })]))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
            Ok(())
        }
    }

    fn build_runtime_backed_ephemeral_service(
        temp: &TempDir,
    ) -> EphemeralSessionService<FactoryAgentBuilder> {
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(std::sync::Arc::new(MockLlmClient));
        EphemeralSessionService::new(builder, 4)
    }

    #[cfg(feature = "mcp")]
    fn build_runtime_backed_ephemeral_service_with_dispatcher(
        temp: &TempDir,
        dispatcher: std::sync::Arc<dyn meerkat_core::AgentToolDispatcher>,
    ) -> EphemeralSessionService<FactoryAgentBuilder> {
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(std::sync::Arc::new(MockLlmClient));
        builder.default_tool_dispatcher = Some(dispatcher);
        EphemeralSessionService::new(builder, 4)
    }

    #[cfg(feature = "mcp")]
    struct ToolPlusMcpDispatcher {
        local_tool: Arc<ToolDef>,
        inner: Arc<meerkat_mcp::McpRouterAdapter>,
        tools: Arc<[Arc<ToolDef>]>,
    }

    #[cfg(feature = "mcp")]
    impl ToolPlusMcpDispatcher {
        fn new(local_tool: Arc<ToolDef>, inner: Arc<meerkat_mcp::McpRouterAdapter>) -> Self {
            let mut tools = vec![Arc::clone(&local_tool)];
            tools.extend(inner.tools().iter().cloned());
            Self {
                local_tool,
                inner,
                tools: tools.into(),
            }
        }
    }

    #[cfg(feature = "mcp")]
    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl meerkat_core::AgentToolDispatcher for ToolPlusMcpDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::clone(&self.tools)
        }

        async fn dispatch(
            &self,
            call: meerkat_core::ToolCallView<'_>,
        ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
            if call.name == self.local_tool.name {
                return Err(ToolError::NotFound {
                    name: call.name.to_string(),
                });
            }
            self.inner.dispatch(call).await
        }

        async fn poll_external_updates(&self) -> meerkat_core::ExternalToolUpdate {
            self.inner.poll_external_updates().await
        }

        fn external_tool_surface_snapshot(
            &self,
        ) -> Option<meerkat_core::ExternalToolSurfaceSnapshot> {
            self.inner.external_tool_surface_snapshot()
        }
    }

    fn runtime_backed_request(
        session: Session,
        bindings: meerkat_core::SessionRuntimeBindings,
    ) -> CreateSessionRequest {
        runtime_backed_request_with_comms(session, bindings, None)
    }

    fn runtime_backed_request_with_comms(
        session: Session,
        bindings: meerkat_core::SessionRuntimeBindings,
        comms_name: Option<String>,
    ) -> CreateSessionRequest {
        CreateSessionRequest {
            model: "claude-sonnet-4-5".to_string(),
            prompt: "hello".to_string().into(),
            render_metadata: None,
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            skill_references: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                comms_name,
                resume_session: Some(session),
                runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                ..SessionBuildOptions::default()
            }),
            labels: None,
        }
    }

    fn assert_snapshot_is_valid(snapshot: &MeerkatMachineSnapshot) {
        let violations = validate_meerkat_machine_snapshot(snapshot);
        assert!(
            violations.is_empty(),
            "expected valid MeerkatMachine snapshot, found violations: {violations:#?}"
        );
    }

    #[tokio::test]
    async fn capture_meerkat_shadow_report_returns_empty_for_live_lifecycle_control()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let report = capture_meerkat_shadow_report(
            &runtime_adapter,
            &service,
            &session_id,
            MeerkatShadowLane::LifecycleControl,
        )
        .await
        .map_err(|err| err.to_string())?
        .ok_or_else(|| {
            "live session should produce a lifecycle/control shadow report".to_string()
        })?;

        assert_eq!(report.lane, MeerkatShadowLane::LifecycleControl);
        assert!(
            report.mismatches.is_empty(),
            "expected no lifecycle/control mismatches for healthy live session, got: {:#?}",
            report.mismatches
        );

        Ok(())
    }

    #[tokio::test]
    async fn capture_meerkat_shadow_report_reports_lifecycle_control_mismatches()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        let run_id = RunId::new();
        let input_id = InputId::new();
        let mut queued_input =
            invalid_input_snapshot(input_id.clone(), Some(InputLifecycleState::Queued), None);
        queued_input.content_shape = Some(
            meerkat_runtime::runtime_ingress_authority::ContentShape("text".into()),
        );
        queued_input.handling_mode = Some(meerkat_core::types::HandlingMode::Queue);
        queued_input.last_run_id = Some(run_id.clone());

        snapshot.spine.control.phase = RuntimeState::Retired;
        snapshot.spine.control.current_run_id = Some(run_id.clone());
        snapshot.spine.inputs.current_run_id = Some(run_id.clone());
        snapshot.spine.inputs.current_run_contributors.clear();
        snapshot.spine.inputs.queue = vec![input_id];
        snapshot.spine.inputs.steer_queue.clear();
        snapshot.spine.inputs.admission_order = vec![queued_input];
        snapshot.spine.completion_waiters.input_count = 1;
        snapshot.spine.completion_waiters.waiter_count = 1;
        snapshot.spine.completion_waiters.waiting_inputs = vec![MeerkatCompletionWaiterSnapshot {
            input_id: InputId::new(),
            waiter_count: 1,
        }];

        let mismatches = capture_lifecycle_control_shadow_mismatches(&snapshot);

        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MeerkatShadowMismatch {
                lane: MeerkatShadowLane::LifecycleControl,
                region: "control",
                triage: ShadowMismatchTriage::ImplementationDetail,
                ..
            } if mismatch.observed_summary.contains("Retired")
        )));
        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MeerkatShadowMismatch {
                lane: MeerkatShadowLane::LifecycleControl,
                region: "inputs",
                triage: ShadowMismatchTriage::DogmaViolation,
                ..
            } if mismatch.expected_summary.contains("active runs retain contributor bindings")
        )));
        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MeerkatShadowMismatch {
                lane: MeerkatShadowLane::LifecycleControl,
                region: "completion_waiters",
                triage: ShadowMismatchTriage::ImplementationDetail,
                ..
            } if mismatch.observed_summary.contains("retired runtime still has 1 completion waiters")
        )));

        Ok(())
    }

    #[tokio::test]
    async fn capture_meerkat_shadow_report_returns_empty_for_live_tools() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let report = capture_meerkat_shadow_report(
            &runtime_adapter,
            &service,
            &session_id,
            MeerkatShadowLane::Tools,
        )
        .await
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "live session should produce a tools shadow report".to_string())?;

        assert_eq!(report.lane, MeerkatShadowLane::Tools);
        assert!(
            report.mismatches.is_empty(),
            "expected no tools mismatches for healthy live session, got: {:#?}",
            report.mismatches
        );

        Ok(())
    }

    #[tokio::test]
    async fn capture_meerkat_shadow_report_reports_tool_mismatches() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        let mut removed_reload = invalid_tool_surface_entry(
            "shadow-invalid-removed-reload",
            false,
            ExternalToolSurfaceBaseState::Removed,
        );
        removed_reload.pending_op = ExternalToolSurfacePendingOp::Reload;
        removed_reload.pending_task_sequence = 1;
        removed_reload.pending_lineage_sequence = 1;

        let mut removing_without_timing = invalid_tool_surface_entry(
            "shadow-invalid-removing-timing",
            false,
            ExternalToolSurfaceBaseState::Removing,
        );
        removing_without_timing.has_removal_timing = false;

        snapshot.tool_surface = Some(ExternalToolSurfaceSnapshot {
            phase: meerkat_core::ExternalToolSurfaceGlobalPhase::Operating,
            snapshot_epoch: 0,
            snapshot_aligned_epoch: 0,
            entries: vec![removed_reload, removing_without_timing],
        });

        let mismatches = capture_tools_shadow_mismatches(&snapshot);

        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MeerkatShadowMismatch {
                lane: MeerkatShadowLane::Tools,
                region: "tool_surface",
                entity_id: Some(surface_id),
                triage: ShadowMismatchTriage::ImplementationDetail,
                ..
            } if surface_id == "shadow-invalid-removed-reload"
                && mismatch
                    .expected_summary
                    .contains("pending tool-surface operations stay compatible")
        )));
        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MeerkatShadowMismatch {
                lane: MeerkatShadowLane::Tools,
                region: "tool_surface",
                entity_id: Some(surface_id),
                triage: ShadowMismatchTriage::ImplementationDetail,
                ..
            } if surface_id == "shadow-invalid-removing-timing"
                && mismatch
                    .observed_summary
                    .contains("has_removal_timing = false")
        )));

        Ok(())
    }

    #[tokio::test]
    async fn capture_meerkat_shadow_report_returns_empty_for_live_turn_ops_barrier()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let report = capture_meerkat_shadow_report(
            &runtime_adapter,
            &service,
            &session_id,
            MeerkatShadowLane::TurnOpsBarrier,
        )
        .await
        .map_err(|err| err.to_string())?
        .ok_or_else(|| {
            "live session should produce a turn/ops/barrier shadow report".to_string()
        })?;

        assert_eq!(report.lane, MeerkatShadowLane::TurnOpsBarrier);
        assert!(
            report.mismatches.is_empty(),
            "expected no turn/ops/barrier mismatches for healthy live session, got: {:#?}",
            report.mismatches
        );

        Ok(())
    }

    #[tokio::test]
    async fn capture_meerkat_shadow_report_reports_turn_ops_barrier_mismatches()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        let pending_only_id = OperationId::new();
        let barrier_only_id = OperationId::new();
        let completed_op_id = OperationId::new();
        let wait_request_id = WaitRequestId::new();
        let mismatched_pending_wait_request_id = WaitRequestId::new();

        let turn = snapshot
            .turn
            .as_mut()
            .ok_or_else(|| "live session should expose execution snapshot".to_string())?;
        turn.turn_phase = TurnPhase::WaitingForOps;
        turn.tool_calls_pending = 0;
        turn.pending_operation_ids = Some(vec![pending_only_id.clone()]);
        turn.barrier_operation_ids = vec![barrier_only_id.clone()];
        turn.has_barrier_ops = false;
        turn.barrier_satisfied = false;

        snapshot.spine.ops.operations = vec![OperationLifecycleSnapshot {
            completed_at_ms: Some(2),
            terminal_outcome: Some(OperationTerminalOutcome::Completed(OperationResult {
                id: completed_op_id.clone(),
                content: String::new(),
                is_error: false,
                duration_ms: 0,
                tokens_used: 0,
            })),
            ..invalid_operation_snapshot(
                completed_op_id.clone(),
                OperationKind::BackgroundToolOp,
                OperationStatus::Completed,
            )
        }];
        snapshot.spine.ops.operation_count = 1;
        snapshot.spine.ops.active_count = 0;
        snapshot.spine.ops.wait_request_id = Some(wait_request_id);
        snapshot.spine.ops.pending_wait_present = true;
        snapshot.spine.ops.pending_wait_request_id = Some(mismatched_pending_wait_request_id);
        snapshot.spine.ops.wait_operation_ids = vec![barrier_only_id.clone()];

        let mismatches = capture_turn_ops_barrier_shadow_mismatches(&snapshot);

        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MeerkatShadowMismatch {
                lane: MeerkatShadowLane::TurnOpsBarrier,
                region: "turn",
                triage: ShadowMismatchTriage::ImplementationDetail,
                ..
            } if mismatch
                .expected_summary
                .contains("WaitingForOps turns retain at least one pending tool call")
        )));
        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MeerkatShadowMismatch {
                lane: MeerkatShadowLane::TurnOpsBarrier,
                region: "turn.barrier",
                triage: ShadowMismatchTriage::ImplementationDetail,
                ..
            } if mismatch
                .expected_summary
                .contains("barrier operation ids imply the turn is marked as having barrier ops")
        )));
        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MeerkatShadowMismatch {
                lane: MeerkatShadowLane::TurnOpsBarrier,
                region: "turn.ops",
                entity_id: Some(operation_id),
                triage: ShadowMismatchTriage::DogmaViolation,
                ..
            } if operation_id == &format!("{pending_only_id:?}")
                && mismatch.observed_summary.contains("missing from ops registry")
        )));
        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MeerkatShadowMismatch {
                lane: MeerkatShadowLane::TurnOpsBarrier,
                region: "ops.wait_all",
                triage: ShadowMismatchTriage::DogmaViolation,
                ..
            } if mismatch
                .observed_summary
                .contains("pending_wait_request_id")
        )));
        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MeerkatShadowMismatch {
                lane: MeerkatShadowLane::TurnOpsBarrier,
                region: "ops.wait_all",
                entity_id: Some(operation_id),
                triage: ShadowMismatchTriage::DogmaViolation,
                ..
            } if operation_id == &format!("{barrier_only_id:?}")
                && mismatch
                    .observed_summary
                    .contains("missing from ops registry")
        )));

        Ok(())
    }

    #[tokio::test]
    async fn capture_meerkat_shadow_report_returns_empty_for_live_peer_drain() -> Result<(), String>
    {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let report = capture_meerkat_shadow_report(
            &runtime_adapter,
            &service,
            &session_id,
            MeerkatShadowLane::PeerDrain,
        )
        .await
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "live session should produce a peer/drain shadow report".to_string())?;

        assert_eq!(report.lane, MeerkatShadowLane::PeerDrain);
        assert!(
            report.mismatches.is_empty(),
            "expected no peer/drain mismatches for healthy live session, got: {:#?}",
            report.mismatches
        );

        Ok(())
    }

    #[tokio::test]
    async fn capture_meerkat_shadow_report_reports_peer_drain_mismatches() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        snapshot.peer = Some(PeerIngressRuntimeSnapshot {
            self_peer_id: "self-peer".into(),
            auth_required: true,
            authority_phase: PeerIngressAuthorityPhase::Received,
            trusted_peers: Vec::new(),
            submission_queue_len: 2,
            queue: PeerIngressQueueSnapshot {
                total_count: 1,
                actionable_count: 0,
                response_count: 1,
                lifecycle_count: 0,
                silent_request_count: 0,
                ack_count: 0,
                plain_event_count: 1,
                queued_entries: vec![
                    PeerIngressEntrySnapshot {
                        raw_item_id: "shadow-invalid-peer-message".into(),
                        interaction_id: None,
                        class: PeerInputClass::Response,
                        kind: PeerIngressKind::Message,
                        from_peer: Some("ally".into()),
                        lifecycle_peer: None,
                        request_id: None,
                        trusted_snapshot: None,
                    },
                    PeerIngressEntrySnapshot {
                        raw_item_id: "shadow-invalid-peer-plain".into(),
                        interaction_id: None,
                        class: PeerInputClass::PlainEvent,
                        kind: PeerIngressKind::PlainEvent,
                        from_peer: Some("tcp".into()),
                        lifecycle_peer: None,
                        request_id: None,
                        trusted_snapshot: Some(false),
                    },
                ],
            },
        });

        snapshot.spine.drain.slot_present = false;
        snapshot.spine.drain.phase = Some(CommsDrainPhase::Running);
        snapshot.spine.drain.mode = Some(CommsDrainMode::PersistentHost);
        snapshot.spine.drain.handle_present = true;

        let mismatches = capture_peer_drain_shadow_mismatches(&snapshot);

        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MeerkatShadowMismatch {
                lane: MeerkatShadowLane::PeerDrain,
                region: "peer.authority",
                triage: ShadowMismatchTriage::DogmaViolation,
                ..
            } if mismatch
                .expected_summary
                .contains("tracked-entry count matches queued authority entries")
        )));
        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MeerkatShadowMismatch {
                lane: MeerkatShadowLane::PeerDrain,
                region: "peer.entry",
                entity_id: Some(raw_item_id),
                triage: ShadowMismatchTriage::DogmaViolation,
                ..
            } if raw_item_id == "shadow-invalid-peer-message"
                && mismatch.expected_summary.contains("kind and class stay coherent")
        )));
        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MeerkatShadowMismatch {
                lane: MeerkatShadowLane::PeerDrain,
                region: "peer.entry",
                entity_id: Some(raw_item_id),
                triage: ShadowMismatchTriage::ImplementationDetail,
                ..
            } if raw_item_id == "shadow-invalid-peer-plain"
                && mismatch.observed_summary.contains("from_peer_present = true")
        )));
        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MeerkatShadowMismatch {
                lane: MeerkatShadowLane::PeerDrain,
                region: "drain",
                triage: ShadowMismatchTriage::ImplementationDetail,
                ..
            } if mismatch
                .observed_summary
                .contains("slot_present = false with phase = Running")
        )));
        assert!(mismatches.iter().any(|mismatch| matches!(
            mismatch,
            MeerkatShadowMismatch {
                lane: MeerkatShadowLane::PeerDrain,
                region: "drain",
                triage: ShadowMismatchTriage::ImplementationDetail,
                ..
            } if mismatch
                .observed_summary
                .contains("slot_present = false with handle_present = true")
        )));

        Ok(())
    }

    #[tokio::test]
    async fn capture_all_meerkat_shadow_reports_returns_empty_for_healthy_live_session()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let report = capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &session_id)
            .await
            .map_err(|err| err.to_string())?
            .expect("live session should produce joined shadow report");

        assert_eq!(report.session_id, session_id);
        assert_eq!(report.reports.len(), 4);
        assert_eq!(
            report
                .reports
                .iter()
                .map(|entry| entry.lane)
                .collect::<Vec<_>>(),
            vec![
                MeerkatShadowLane::LifecycleControl,
                MeerkatShadowLane::Tools,
                MeerkatShadowLane::TurnOpsBarrier,
                MeerkatShadowLane::PeerDrain,
            ]
        );
        assert!(
            report
                .reports
                .iter()
                .all(|entry| entry.mismatches.is_empty()),
            "healthy live session should emit no aggregate Meerkat shadow mismatches: {report:#?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn summarize_meerkat_shadow_taxonomy_reports_collapses_seeded_lifecycle_control_drift()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        let run_id = RunId::new();
        let input_id = InputId::new();
        let mut queued_input =
            invalid_input_snapshot(input_id.clone(), Some(InputLifecycleState::Queued), None);
        queued_input.content_shape = Some(
            meerkat_runtime::runtime_ingress_authority::ContentShape("text".into()),
        );
        queued_input.handling_mode = Some(meerkat_core::types::HandlingMode::Queue);
        queued_input.last_run_id = Some(run_id.clone());

        snapshot.spine.control.phase = RuntimeState::Retired;
        snapshot.spine.control.current_run_id = Some(run_id.clone());
        snapshot.spine.inputs.current_run_id = Some(run_id);
        snapshot.spine.inputs.current_run_contributors.clear();
        snapshot.spine.inputs.queue = vec![input_id];
        snapshot.spine.inputs.steer_queue.clear();
        snapshot.spine.inputs.admission_order = vec![queued_input];
        snapshot.spine.completion_waiters.input_count = 1;
        snapshot.spine.completion_waiters.waiter_count = 1;
        snapshot.spine.completion_waiters.waiting_inputs = vec![MeerkatCompletionWaiterSnapshot {
            input_id: InputId::new(),
            waiter_count: 1,
        }];

        let taxonomy = summarize_meerkat_shadow_taxonomy_reports(&[MeerkatShadowSuiteReport {
            session_id,
            reports: vec![MeerkatShadowReport {
                lane: MeerkatShadowLane::LifecycleControl,
                mismatches: capture_lifecycle_control_shadow_mismatches(&snapshot),
            }],
        }]);

        assert_eq!(taxonomy.len(), 3);
        assert!(taxonomy.iter().any(|bucket| {
            bucket.lane == MeerkatShadowLane::LifecycleControl
                && bucket.region == "control"
                && bucket.triage == ShadowMismatchTriage::ImplementationDetail
                && bucket.count == 1
        }));
        assert!(taxonomy.iter().any(|bucket| {
            bucket.lane == MeerkatShadowLane::LifecycleControl
                && bucket.region == "inputs"
                && bucket.triage == ShadowMismatchTriage::DogmaViolation
                && bucket.count == 1
        }));
        assert!(taxonomy.iter().any(|bucket| {
            bucket.lane == MeerkatShadowLane::LifecycleControl
                && bucket.region == "completion_waiters"
                && bucket.triage == ShadowMismatchTriage::ImplementationDetail
                && bucket.count == 1
        }));

        let exported = export_meerkat_shadow_scenario_sample(
            "meerkat.seeded.lifecycle_control",
            "seeded",
            "2026-04-12T00:00:00Z",
            &taxonomy,
        );
        assert_eq!(exported.scenario_id, "meerkat.seeded.lifecycle_control");
        assert_eq!(exported.phase, "seeded");
        assert_eq!(exported.timestamp, "2026-04-12T00:00:00Z");
        assert!(
            exported.mob_buckets.is_empty() && exported.seam_buckets.is_empty(),
            "Meerkat-side exporter should not synthesize Mob/seam buckets: {exported:#?}"
        );
        assert_eq!(exported.meerkat_buckets.len(), 3);
        assert!(exported.meerkat_buckets.iter().any(|bucket| {
            bucket.scope == "meerkat"
                && bucket.lane == "LifecycleControl"
                && bucket.region == "control"
                && bucket.triage == "implementation_detail"
                && bucket.count == 1
        }));
        assert!(exported.meerkat_buckets.iter().any(|bucket| {
            bucket.scope == "meerkat"
                && bucket.lane == "LifecycleControl"
                && bucket.region == "inputs"
                && bucket.triage == "dogma_violation"
                && bucket.count == 1
        }));
        assert!(exported.meerkat_buckets.iter().any(|bucket| {
            bucket.scope == "meerkat"
                && bucket.lane == "LifecycleControl"
                && bucket.region == "completion_waiters"
                && bucket.triage == "implementation_detail"
                && bucket.count == 1
        }));

        Ok(())
    }

    #[tokio::test]
    async fn capture_all_meerkat_shadow_reports_stay_empty_across_plain_reset_with_pending_ops()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let input = Input::Prompt(PromptInput::new("shadow reset pending ops", None));
        let (_outcome, completion_handle) = runtime_adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .map_err(|err| err.to_string())?;
        let completion_handle =
            completion_handle.expect("queued prompt should register a completion waiter");

        let registry = runtime_adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for registered session");

        let operation_id = OperationId::new();
        registry
            .register_operation(meerkat_core::ops_lifecycle::OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "shadow-reset-wait-target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_reset =
            capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .expect("healthy live session should produce aggregate shadow report before reset");
        assert!(
            before_reset
                .reports
                .iter()
                .all(|entry| entry.mismatches.is_empty()),
            "plain reset scenario should start mismatch-free before reset: {before_reset:#?}"
        );

        let report = SessionServiceRuntimeExt::reset_runtime(&runtime_adapter, &session_id)
            .await
            .map_err(|err| err.to_string())?;
        assert_eq!(report.inputs_abandoned, 1);

        match completion_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime reset");
            }
            other => {
                return Err(format!(
                    "expected runtime reset completion termination, got {other:?}"
                ));
            }
        }

        let after_reset =
            capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .expect("healthy live session should produce aggregate shadow report after reset");
        assert!(
            after_reset
                .reports
                .iter()
                .all(|entry| entry.mismatches.is_empty()),
            "plain reset scenario should remain mismatch-free after reset while wait_all is live: {after_reset:#?}"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after reset");

        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &session_id)
            .await
            .map_err(|err| err.to_string())?
            .expect("healthy live session should produce aggregate shadow report after settle");
        assert!(
            settled
                .reports
                .iter()
                .all(|entry| entry.mismatches.is_empty()),
            "plain reset scenario should remain mismatch-free after wait_all settles: {settled:#?}"
        );

        Ok(())
    }

    struct FakeDrainRuntime {
        notify: Arc<Notify>,
        dismiss: AtomicBool,
    }

    impl FakeDrainRuntime {
        fn idle() -> Self {
            Self {
                notify: Arc::new(Notify::new()),
                dismiss: AtomicBool::new(false),
            }
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl CommsRuntime for FakeDrainRuntime {
        async fn drain_messages(&self) -> Vec<String> {
            Vec::new()
        }

        fn inbox_notify(&self) -> Arc<Notify> {
            Arc::clone(&self.notify)
        }

        fn dismiss_received(&self) -> bool {
            self.dismiss.load(Ordering::Acquire)
        }

        async fn drain_classified_inbox_interactions(
            &self,
        ) -> Result<Vec<meerkat_core::interaction::ClassifiedInboxInteraction>, CommsCapabilityError>
        {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn capture_meerkat_machine_snapshot_reports_registered_runtime_without_live_turn()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let _bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        let snapshot = capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
            .await
            .map_err(|err| err.to_string())?
            .ok_or_else(|| {
                "runtime-registered session should produce a Meerkat snapshot".to_string()
            })?;

        assert_snapshot_is_valid(&snapshot);
        assert_eq!(snapshot.spine.binding.session_id, session_id);
        assert!(!snapshot.spine.binding.attachment_live);
        assert!(snapshot.turn.is_none());
        assert!(snapshot.tools.is_none());
        assert!(snapshot.tool_surface.is_none());
        assert!(snapshot.peer.is_none());
        assert!(!snapshot.spine.drain.slot_present);
        assert_eq!(snapshot.spine.drain.phase, None);
        assert_eq!(snapshot.spine.drain.mode, None);
        assert!(!snapshot.spine.drain.handle_present);

        Ok(())
    }

    #[tokio::test]
    async fn capture_meerkat_machine_snapshot_joins_stopped_comms_drain_state() -> Result<(), String>
    {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = Arc::new(RuntimeSessionAdapter::ephemeral());
        let session = Session::new();
        let session_id = session.id().clone();

        let _bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        let spawned = runtime_adapter
            .maybe_spawn_comms_drain(
                &session_id,
                true,
                Some(Arc::new(FakeDrainRuntime::idle()) as Arc<dyn CommsRuntime>),
            )
            .await;
        assert!(spawned, "registered session should spawn a comms drain");

        runtime_adapter.abort_comms_drain(&session_id).await;

        let snapshot = capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
            .await
            .map_err(|err| err.to_string())?
            .ok_or_else(|| {
                "runtime-registered session should produce a Meerkat snapshot".to_string()
            })?;

        assert_snapshot_is_valid(&snapshot);
        assert!(snapshot.spine.drain.slot_present);
        assert_eq!(snapshot.spine.drain.phase, Some(CommsDrainPhase::Stopped));
        assert_eq!(
            snapshot.spine.drain.mode,
            Some(CommsDrainMode::PersistentHost)
        );
        assert!(!snapshot.spine.drain.handle_present);

        Ok(())
    }

    #[tokio::test]
    async fn capture_meerkat_machine_snapshot_joins_runtime_spine_with_live_turn_state()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let input = Input::Prompt(PromptInput::new("queued from runtime adapter", None));
        let input_id = input.id().clone();
        runtime_adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .map_err(|err| err.to_string())?;

        let snapshot = capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
            .await
            .map_err(|err| err.to_string())?
            .ok_or_else(|| {
                "live runtime-backed session should produce a Meerkat snapshot".to_string()
            })?;

        assert_snapshot_is_valid(&snapshot);
        let turn = snapshot
            .turn
            .ok_or_else(|| "live session should expose execution snapshot".to_string())?;
        let tools = snapshot
            .tools
            .ok_or_else(|| "live session should expose tool-scope snapshot".to_string())?;

        assert_eq!(snapshot.spine.binding.session_id, session_id);
        assert_eq!(snapshot.spine.inputs.queue, vec![input_id]);
        assert_eq!(snapshot.spine.inputs.admission_order.len(), 1);
        assert_eq!(snapshot.spine.completion_waiters.input_count, 1);
        assert_eq!(snapshot.spine.completion_waiters.waiter_count, 1);
        assert_eq!(snapshot.spine.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            snapshot.spine.completion_waiters.waiting_inputs[0].input_id,
            snapshot.spine.inputs.admission_order[0].input_id
        );
        assert_eq!(turn.active_run_id, snapshot.spine.control.current_run_id);
        assert_eq!(
            turn.applied_cursor,
            snapshot.spine.binding.cursor_state.agent_applied_cursor
        );
        assert_eq!(
            turn.turn_phase,
            meerkat_core::turn_execution_authority::TurnPhase::Ready
        );
        assert_eq!(tools.base_filter, meerkat_core::ToolFilter::All);
        assert_eq!(tools.active_external_filter, meerkat_core::ToolFilter::All);
        assert_eq!(tools.staged_external_filter, meerkat_core::ToolFilter::All);
        assert_eq!(tools.active_revision, meerkat_core::ToolScopeRevision(0));
        assert_eq!(tools.staged_revision, meerkat_core::ToolScopeRevision(0));
        assert_eq!(tools.known_base_names, tools.visible_names);
        assert!(snapshot.tool_surface.is_none());
        assert!(snapshot.peer.is_none());

        Ok(())
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn capture_all_meerkat_shadow_reports_stay_empty_across_tool_visibility_mutation_and_mcp_apply()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let router_adapter = Arc::new(meerkat_mcp::McpRouterAdapter::new(
            meerkat_mcp::McpRouter::new(),
        ));
        router_adapter
            .stage_add(McpServerConfig::stdio(
                "planner",
                "/bin/echo",
                Vec::<String>::new(),
                std::collections::HashMap::new(),
            ))
            .await
            .map_err(|err| format!("stage initial planner add: {err}"))?;
        router_adapter
            .apply_staged()
            .await
            .map_err(|err| format!("apply initial planner add: {err}"))?;
        let _ = router_adapter
            .wait_until_ready(std::time::Duration::from_secs(1))
            .await;

        let local_tool = Arc::new(ToolDef::new(
            "visible_local",
            "Stable filterable session-plane tool for Meerkat shadow testing",
            serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        ));
        let dispatcher = Arc::new(ToolPlusMcpDispatcher::new(
            Arc::clone(&local_tool),
            Arc::clone(&router_adapter),
        )) as Arc<dyn meerkat_core::AgentToolDispatcher>;
        let service = build_runtime_backed_ephemeral_service_with_dispatcher(&temp, dispatcher);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let before_mutation =
            capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .expect("live session should produce joined shadow report before tool mutation");
        assert!(
            before_mutation
                .reports
                .iter()
                .all(|entry| entry.mismatches.is_empty()),
            "healthy live session should emit no aggregate Meerkat mismatches before tool mutation: {before_mutation:#?}"
        );

        SessionService::set_session_tool_filter(
            &service,
            &session_id,
            meerkat_core::ToolFilter::Deny([local_tool.name.clone()].into_iter().collect()),
        )
        .await
        .map_err(|err| err.to_string())?;

        let staged_snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "runtime-backed session should produce a Meerkat snapshot after tool mutation"
                        .to_string()
                })?;
        let staged_tools = staged_snapshot.tools.ok_or_else(|| {
            "runtime-backed session should expose tool-scope snapshot after tool mutation"
                .to_string()
        })?;
        assert_eq!(
            staged_tools.active_external_filter,
            meerkat_core::ToolFilter::All
        );
        assert_eq!(
            staged_tools.staged_external_filter,
            meerkat_core::ToolFilter::Deny([local_tool.name.clone()].into_iter().collect())
        );
        assert!(
            staged_tools.staged_revision.0 > staged_tools.active_revision.0,
            "staged tool visibility mutation should advance the staged revision"
        );

        let after_mutation =
            capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .expect("live session should produce joined shadow report after tool mutation");
        assert!(
            after_mutation
                .reports
                .iter()
                .all(|entry| entry.mismatches.is_empty()),
            "staged tool visibility mutation should keep the aggregate Meerkat shadow suite clean: {after_mutation:#?}"
        );

        router_adapter
            .stage_reload("planner")
            .await
            .map_err(|err| format!("stage planner reload: {err}"))?;
        router_adapter
            .apply_staged()
            .await
            .map_err(|err| format!("apply planner reload: {err}"))?;

        let after_apply =
            capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .expect("live session should produce joined shadow report after MCP apply");
        assert!(
            after_apply
                .reports
                .iter()
                .all(|entry| entry.mismatches.is_empty()),
            "tool visibility mutation + MCP apply should keep the aggregate Meerkat shadow suite clean: {after_apply:#?}"
        );

        let _ = router_adapter
            .wait_until_ready(std::time::Duration::from_secs(1))
            .await;

        let after_settle =
            capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .expect("live session should produce joined shadow report after MCP settle");
        assert!(
            after_settle
                .reports
                .iter()
                .all(|entry| entry.mismatches.is_empty()),
            "tool visibility mutation + settled MCP reload should keep the aggregate Meerkat shadow suite clean: {after_settle:#?}"
        );

        Ok(())
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn capture_all_meerkat_shadow_reports_stay_empty_across_peer_ingress_trust_and_drain()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = Arc::new(RuntimeSessionAdapter::ephemeral());
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request_with_comms(
                session,
                bindings,
                Some("peer-shadow-session".to_string()),
            ))
            .await
            .map_err(|err| err.to_string())?;

        let before_ingress =
            capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .expect("live comms session should produce joined shadow report before ingress");
        assert!(
            before_ingress
                .reports
                .iter()
                .all(|entry| entry.mismatches.is_empty()),
            "healthy live comms session should emit no aggregate Meerkat mismatches before peer ingress: {before_ingress:#?}"
        );

        let runtime = service
            .comms_runtime(&session_id)
            .await
            .ok_or_else(|| "runtime-backed session should expose comms runtime".to_string())?;

        let trusted_peer_key = Keypair::generate();
        let trusted_peer = TrustedPeerSpec::new(
            "ally",
            trusted_peer_key.public_key().to_peer_id(),
            "inproc://ally",
        )
        .map_err(|err| format!("trusted peer spec: {err}"))?;

        runtime
            .add_trusted_peer(trusted_peer)
            .await
            .map_err(|err| err.to_string())?;

        let spawned = runtime_adapter
            .update_peer_ingress_context(
                &session_id,
                true,
                Some(Arc::clone(&runtime) as Arc<dyn CommsRuntime>),
            )
            .await;
        assert!(spawned, "keep_alive=true should spawn a live comms drain");

        runtime
            .event_injector()
            .ok_or_else(|| "comms runtime should expose event injector".to_string())?
            .inject(
                "peer shadow wake".to_string().into(),
                PlainEventSource::Tcp,
                HandlingMode::Queue,
                None,
            )
            .map_err(|err| err.to_string())?;

        let after_ingress =
            capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .expect("live comms session should produce joined shadow report after ingress");
        assert!(
            after_ingress
                .reports
                .iter()
                .all(|entry| entry.mismatches.is_empty()),
            "peer ingress + trust mutation + live drain should keep aggregate Meerkat shadow suite clean: {after_ingress:#?}"
        );

        runtime_adapter
            .update_peer_ingress_context(&session_id, false, Some(runtime))
            .await;

        let after_stop =
            capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .expect("live comms session should produce joined shadow report after drain stop");
        assert!(
            after_stop
                .reports
                .iter()
                .all(|entry| entry.mismatches.is_empty()),
            "stopped drain state should keep aggregate Meerkat shadow suite clean: {after_stop:#?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn capture_all_meerkat_shadow_reports_stay_empty_across_attached_recover_replay()
    -> Result<(), String> {
        struct BlockingExecutor {
            apply_started: Arc<Notify>,
            apply_finished: Arc<Notify>,
            allow_finish: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for BlockingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_started.notify_waiters();
                self.allow_finish.notified().await;
                self.apply_finished.notify_waiters();

                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                _command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                Ok(())
            }
        }

        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let apply_started = Arc::new(Notify::new());
        let apply_finished = Arc::new(Notify::new());
        let allow_finish = Arc::new(Notify::new());

        runtime_adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(BlockingExecutor {
                    apply_started: Arc::clone(&apply_started),
                    apply_finished: Arc::clone(&apply_finished),
                    allow_finish: Arc::clone(&allow_finish),
                }),
            )
            .await;

        let input = make_shadow_progress_input("shadow-attached-recover-replay");
        let (_outcome, completion_handle) = runtime_adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .map_err(|err| err.to_string())?;
        let completion_handle = completion_handle
            .expect("queued attached replay input should register a completion waiter");

        let registry = runtime_adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for attached session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "shadow attached recover replay wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_recover =
            capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .expect("attached runtime should produce aggregate shadow report before recover");
        assert!(
            before_recover
                .reports
                .iter()
                .all(|entry| entry.mismatches.is_empty()),
            "attached recover replay scenario should start mismatch-free before recover: {before_recover:#?}"
        );

        let runtime_id =
            meerkat_runtime::identifiers::LogicalRuntimeId::new(session_id.to_string());
        let report = RuntimeControlPlane::recover(&runtime_adapter, &runtime_id)
            .await
            .map_err(|err| err.to_string())?;
        assert_eq!(report.inputs_recovered, 1);

        tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
            .await
            .map_err(|_| {
                "recover should wake the attached runtime loop to replay preserved queued work"
                    .to_string()
            })?;

        let during_recover =
            capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .expect(
                    "attached runtime should produce aggregate shadow report during recover replay",
                );
        assert!(
            during_recover
                .reports
                .iter()
                .all(|entry| entry.mismatches.is_empty()),
            "attached recover replay should remain mismatch-free while replay is in flight: {during_recover:#?}"
        );

        allow_finish.notify_waiters();
        tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
            .await
            .map_err(|_| "attached replay should finish recovered work".to_string())?;

        match completion_handle.wait().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => {
                return Err(format!(
                    "expected recover+replay to complete queued work, got {other:?}"
                ));
            }
        }

        let after_replay =
            capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .expect("attached runtime should produce aggregate shadow report after replay");
        assert!(
            after_replay
                .reports
                .iter()
                .all(|entry| entry.mismatches.is_empty()),
            "attached recover replay should remain mismatch-free after replay while wait_all is still live: {after_replay:#?}"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after attached recover replay");

        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &session_id)
            .await
            .map_err(|err| err.to_string())?
            .expect("attached runtime should produce aggregate shadow report after settle");
        assert!(
            settled
                .reports
                .iter()
                .all(|entry| entry.mismatches.is_empty()),
            "attached recover replay should remain mismatch-free after wait_all settles: {settled:#?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn capture_all_meerkat_shadow_reports_stay_empty_across_attached_recycle_replay()
    -> Result<(), String> {
        struct BlockingExecutor {
            apply_started: Arc<Notify>,
            apply_finished: Arc<Notify>,
            allow_finish: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for BlockingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_started.notify_waiters();
                self.allow_finish.notified().await;
                self.apply_finished.notify_waiters();

                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                _command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                Ok(())
            }
        }

        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let apply_started = Arc::new(Notify::new());
        let apply_finished = Arc::new(Notify::new());
        let allow_finish = Arc::new(Notify::new());

        runtime_adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(BlockingExecutor {
                    apply_started: Arc::clone(&apply_started),
                    apply_finished: Arc::clone(&apply_finished),
                    allow_finish: Arc::clone(&allow_finish),
                }),
            )
            .await;

        let input = make_shadow_progress_input("shadow-attached-recycle-replay");
        let (_outcome, completion_handle) = runtime_adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .map_err(|err| err.to_string())?;
        let completion_handle = completion_handle
            .expect("queued attached replay input should register a completion waiter");

        let registry = runtime_adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for attached session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "shadow attached recycle replay wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_recycle =
            capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .expect("attached runtime should produce aggregate shadow report before recycle");
        assert!(
            before_recycle
                .reports
                .iter()
                .all(|entry| entry.mismatches.is_empty()),
            "attached recycle replay scenario should start mismatch-free before recycle: {before_recycle:#?}"
        );

        let runtime_id =
            meerkat_runtime::identifiers::LogicalRuntimeId::new(session_id.to_string());
        let report = RuntimeControlPlane::recycle(&runtime_adapter, &runtime_id)
            .await
            .map_err(|err| err.to_string())?;
        assert_eq!(report.inputs_transferred, 1);

        tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
            .await
            .expect("recycle should wake the attached runtime loop to replay preserved work");

        let during_recycle =
            capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .expect(
                    "attached runtime should produce aggregate shadow report during recycle replay",
                );
        assert!(
            during_recycle
                .reports
                .iter()
                .all(|entry| entry.mismatches.is_empty()),
            "attached recycle replay scenario should stay mismatch-free during replay: {during_recycle:#?}"
        );

        allow_finish.notify_waiters();
        tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
            .await
            .expect("attached loop should finish replaying preserved work after recycle");

        let after_replay =
            capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .expect(
                    "attached runtime should produce aggregate shadow report after recycle replay",
                );
        assert!(
            after_replay
                .reports
                .iter()
                .all(|entry| entry.mismatches.is_empty()),
            "attached recycle replay scenario should stay mismatch-free after replay while wait_all remains live: {after_replay:#?}"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after recycle replay");

        match completion_handle.wait().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => {
                return Err(format!(
                    "expected recycle+replay to complete queued work, got {other:?}"
                ));
            }
        }
        let wait_result = wait_future.await.map_err(|err| err.to_string())?;
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &session_id)
            .await
            .map_err(|err| err.to_string())?
            .expect(
                "attached runtime should produce aggregate shadow report after recycle settles",
            );
        assert!(
            settled
                .reports
                .iter()
                .all(|entry| entry.mismatches.is_empty()),
            "attached recycle replay scenario should settle mismatch-free: {settled:#?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn capture_meerkat_shadow_taxonomy_stays_empty_across_broader_smoke_run()
    -> Result<(), String> {
        struct BlockingExecutor {
            apply_started: Arc<Notify>,
            apply_finished: Arc<Notify>,
            allow_finish: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for BlockingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_started.notify_waiters();
                self.allow_finish.notified().await;
                self.apply_finished.notify_waiters();

                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                _command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                Ok(())
            }
        }

        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let mut reports = Vec::new();

        let plain_session = Session::new();
        let plain_session_id = plain_session.id().clone();
        let plain_bindings = runtime_adapter
            .prepare_bindings(plain_session_id.clone())
            .await
            .map_err(|err| err.to_string())?;
        service
            .create_session(runtime_backed_request(plain_session, plain_bindings))
            .await
            .map_err(|err| err.to_string())?;

        reports.push(
            capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &plain_session_id)
                .await
                .map_err(|err| err.to_string())?
                .expect("healthy plain session should produce aggregate shadow report"),
        );

        let input = Input::Prompt(PromptInput::new("shadow broader smoke reset", None));
        let (_outcome, completion_handle) = runtime_adapter
            .accept_input_with_completion(&plain_session_id, input)
            .await
            .map_err(|err| err.to_string())?;
        let completion_handle =
            completion_handle.expect("queued prompt should register a completion waiter");

        let registry = runtime_adapter
            .ops_lifecycle_registry(&plain_session_id)
            .await
            .expect("ops registry should exist for registered session");

        let operation_id = OperationId::new();
        registry
            .register_operation(meerkat_core::ops_lifecycle::OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: plain_session_id.clone(),
                display_name: "shadow-broader-smoke-reset-target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");
        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let report = SessionServiceRuntimeExt::reset_runtime(&runtime_adapter, &plain_session_id)
            .await
            .map_err(|err| err.to_string())?;
        assert_eq!(report.inputs_abandoned, 1);

        match completion_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime reset");
            }
            other => {
                return Err(format!(
                    "expected runtime reset completion termination in broader smoke run, got {other:?}"
                ));
            }
        }

        reports.push(
            capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &plain_session_id)
                .await
                .map_err(|err| err.to_string())?
                .expect("plain session should produce aggregate shadow report after reset"),
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after reset in broader smoke run");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        reports.push(
            capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &plain_session_id)
                .await
                .map_err(|err| err.to_string())?
                .expect("plain session should produce aggregate shadow report after reset settle"),
        );

        let attached_session = Session::new();
        let attached_session_id = attached_session.id().clone();
        let attached_bindings = runtime_adapter
            .prepare_bindings(attached_session_id.clone())
            .await
            .map_err(|err| err.to_string())?;
        service
            .create_session(runtime_backed_request(attached_session, attached_bindings))
            .await
            .map_err(|err| err.to_string())?;

        let apply_started = Arc::new(Notify::new());
        let apply_finished = Arc::new(Notify::new());
        let allow_finish = Arc::new(Notify::new());
        runtime_adapter
            .register_session_with_executor(
                attached_session_id.clone(),
                Box::new(BlockingExecutor {
                    apply_started: Arc::clone(&apply_started),
                    apply_finished: Arc::clone(&apply_finished),
                    allow_finish: Arc::clone(&allow_finish),
                }),
            )
            .await;

        let attached_input = make_shadow_progress_input("shadow-broader-smoke-attached-recover");
        let (_outcome, completion_handle) = runtime_adapter
            .accept_input_with_completion(&attached_session_id, attached_input)
            .await
            .map_err(|err| err.to_string())?;
        let completion_handle = completion_handle
            .expect("queued attached replay input should register a completion waiter");

        let attached_registry = runtime_adapter
            .ops_lifecycle_registry(&attached_session_id)
            .await
            .expect("ops registry should exist for attached session");
        let attached_operation_id = OperationId::new();
        attached_registry
            .register_operation(OperationSpec {
                id: attached_operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: attached_session_id.clone(),
                display_name: "shadow broader smoke attached recover target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        attached_registry
            .provisioning_succeeded(&attached_operation_id)
            .expect("operation should enter running");
        let attached_wait_future =
            attached_registry.wait_all(&RunId::new(), std::slice::from_ref(&attached_operation_id));

        reports.push(
            capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &attached_session_id)
                .await
                .map_err(|err| err.to_string())?
                .expect("attached session should produce aggregate shadow report before recover"),
        );

        let runtime_id =
            meerkat_runtime::identifiers::LogicalRuntimeId::new(attached_session_id.to_string());
        let report = RuntimeControlPlane::recover(&runtime_adapter, &runtime_id)
            .await
            .map_err(|err| err.to_string())?;
        assert_eq!(report.inputs_recovered, 1);

        tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
            .await
            .map_err(|_| {
                "recover should wake the attached runtime loop in broader smoke run".to_string()
            })?;

        reports.push(
            capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &attached_session_id)
                .await
                .map_err(|err| err.to_string())?
                .expect("attached session should produce aggregate shadow report during recover"),
        );

        allow_finish.notify_waiters();
        tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
            .await
            .map_err(|_| "attached replay should finish in broader smoke run".to_string())?;

        match completion_handle.wait().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => {
                return Err(format!(
                    "expected recover+replay to complete queued work in broader smoke run, got {other:?}"
                ));
            }
        }

        reports.push(
            capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &attached_session_id)
                .await
                .map_err(|err| err.to_string())?
                .expect(
                    "attached session should produce aggregate shadow report after recover replay",
                ),
        );

        attached_registry
            .complete_operation(
                &attached_operation_id,
                OperationResult {
                    id: attached_operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after attached recover replay");
        let attached_wait_result = attached_wait_future
            .await
            .expect("wait_all should still resolve after attached recover replay");
        assert_eq!(
            attached_wait_result.satisfied.operation_ids,
            vec![attached_operation_id.clone()]
        );

        reports.push(
            capture_all_meerkat_shadow_reports(&runtime_adapter, &service, &attached_session_id)
                .await
                .map_err(|err| err.to_string())?
                .expect("attached session should produce aggregate shadow report after settle"),
        );

        let taxonomy = summarize_meerkat_shadow_taxonomy_reports(&reports);
        assert!(
            taxonomy.is_empty(),
            "broader Meerkat shadow smoke run should keep mismatch taxonomy empty: {taxonomy:#?}"
        );

        let exported = export_meerkat_shadow_scenario_sample(
            "meerkat.broader_smoke",
            "settled",
            "2026-04-12T00:00:00Z",
            &taxonomy,
        );
        assert_eq!(exported.scenario_id, "meerkat.broader_smoke");
        assert_eq!(exported.phase, "settled");
        assert_eq!(exported.timestamp, "2026-04-12T00:00:00Z");
        assert!(
            exported.meerkat_buckets.is_empty()
                && exported.mob_buckets.is_empty()
                && exported.seam_buckets.is_empty(),
            "empty Meerkat taxonomy should export as an empty sample: {exported:#?}"
        );

        Ok(())
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn capture_meerkat_machine_snapshot_joins_live_external_tool_surface_state()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let mut router = meerkat_mcp::McpRouter::new();
        router.stage_add(McpServerConfig::stdio(
            "planner",
            "/bin/echo",
            Vec::<String>::new(),
            std::collections::HashMap::new(),
        ));
        let dispatcher = std::sync::Arc::new(meerkat_mcp::McpRouterAdapter::new(router))
            as std::sync::Arc<dyn meerkat_core::AgentToolDispatcher>;
        let service = build_runtime_backed_ephemeral_service_with_dispatcher(&temp, dispatcher);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let snapshot = capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
            .await
            .map_err(|err| err.to_string())?
            .ok_or_else(|| {
                "runtime-backed session with staged MCP surface should produce a Meerkat snapshot"
                    .to_string()
            })?;

        assert_snapshot_is_valid(&snapshot);
        let tool_surface = snapshot.tool_surface.ok_or_else(|| {
            "live session should expose external tool surface snapshot".to_string()
        })?;

        assert_eq!(
            tool_surface.phase,
            ExternalToolSurfaceGlobalPhase::Operating
        );
        assert_eq!(tool_surface.snapshot_epoch, 0);
        assert_eq!(tool_surface.snapshot_aligned_epoch, 0);
        assert_eq!(tool_surface.entries.len(), 1);

        let entry = &tool_surface.entries[0];
        assert_eq!(entry.surface_id, "planner");
        assert!(!entry.visible);
        assert_eq!(entry.base_state, ExternalToolSurfaceBaseState::Absent);
        assert_eq!(entry.pending_op, ExternalToolSurfacePendingOp::None);
        assert_eq!(entry.staged_op, ExternalToolSurfaceStagedOp::Add);
        assert_eq!(entry.staged_intent_sequence, 1);
        assert_eq!(entry.pending_task_sequence, 0);
        assert_eq!(entry.pending_lineage_sequence, 0);
        assert_eq!(entry.inflight_call_count, 0);
        assert_eq!(
            entry.last_delta_operation,
            ExternalToolSurfaceDeltaOperation::None
        );
        assert_eq!(entry.last_delta_phase, ExternalToolSurfaceDeltaPhase::None);

        Ok(())
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn capture_meerkat_machine_snapshot_joins_live_peer_runtime_state() -> Result<(), String>
    {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request_with_comms(
                session,
                bindings,
                Some("peer-snapshot-session".to_string()),
            ))
            .await
            .map_err(|err| err.to_string())?;

        let runtime = service
            .comms_runtime(&session_id)
            .await
            .ok_or_else(|| "runtime-backed session should expose comms runtime".to_string())?;

        let peer_key = Keypair::generate();
        let trusted_peer =
            TrustedPeerSpec::new("ally", peer_key.public_key().to_peer_id(), "inproc://ally")
                .map_err(|err| format!("trusted peer spec: {err}"))?;

        runtime
            .add_trusted_peer(trusted_peer.clone())
            .await
            .map_err(|err| err.to_string())?;

        runtime
            .event_injector()
            .ok_or_else(|| "comms runtime should expose event injector".to_string())?
            .inject(
                "external wake".to_string().into(),
                PlainEventSource::Tcp,
                HandlingMode::Queue,
                None,
            )
            .map_err(|err| err.to_string())?;

        let snapshot = capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
            .await
            .map_err(|err| err.to_string())?
            .ok_or_else(|| {
                "runtime-registered session with comms should produce a Meerkat snapshot"
                    .to_string()
            })?;

        assert_snapshot_is_valid(&snapshot);
        let peer = snapshot
            .peer
            .ok_or_else(|| "live comms session should expose peer snapshot".to_string())?;
        let self_peer_id = runtime
            .public_key()
            .ok_or_else(|| "comms runtime should expose public key".to_string())?;

        assert_eq!(peer.self_peer_id, self_peer_id);
        assert_eq!(
            peer.authority_phase,
            meerkat_core::PeerIngressAuthorityPhase::Absent
        );
        assert_eq!(peer.trusted_peers, vec![trusted_peer]);
        assert_eq!(peer.submission_queue_len, 0);
        assert_eq!(peer.queue.total_count, 1);
        assert_eq!(peer.queue.plain_event_count, 1);
        assert_eq!(peer.queue.queued_entries.len(), 1);
        assert_eq!(
            peer.queue.queued_entries[0].kind,
            meerkat_core::PeerIngressKind::PlainEvent
        );
        assert_eq!(peer.queue.queued_entries[0].trusted_snapshot, None);

        Ok(())
    }

    #[tokio::test]
    async fn validate_meerkat_machine_snapshot_reports_cross_region_violations()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        snapshot.spine.control.phase = RuntimeState::Running;
        snapshot.spine.control.current_run_id = None;
        let current_run_id = RunId::new();
        snapshot.spine.inputs.current_run_id = Some(current_run_id.clone());
        snapshot.spine.ops.active_count = snapshot.spine.ops.operation_count + 1;

        let turn = snapshot
            .turn
            .as_mut()
            .ok_or_else(|| "live session should expose execution snapshot".to_string())?;
        turn.turn_phase = TurnPhase::WaitingForOps;
        turn.pending_operation_ids = None;
        turn.active_run_id = Some(RunId::new());

        let queue_input_id = InputId::new();
        let contributor_input_id = InputId::new();
        let boundaryless_contributor_input_id = InputId::new();
        let missing_metadata_input_id = InputId::new();
        let missing_queue_membership_input_id = InputId::new();
        let waiter_input_id = InputId::new();
        let resolved_waiter_input_id = InputId::new();
        let mut queue_input = invalid_input_snapshot(
            queue_input_id.clone(),
            Some(InputLifecycleState::Staged),
            None,
        );
        queue_input.handling_mode = Some(meerkat_core::types::HandlingMode::Steer);

        let mut contributor_input = invalid_input_snapshot(
            contributor_input_id.clone(),
            Some(InputLifecycleState::Queued),
            None,
        );
        contributor_input.handling_mode = Some(meerkat_core::types::HandlingMode::Queue);
        contributor_input.last_run_id = Some(current_run_id.clone());

        let mut boundaryless_contributor_input = invalid_input_snapshot(
            boundaryless_contributor_input_id.clone(),
            Some(InputLifecycleState::AppliedPendingConsumption),
            None,
        );
        boundaryless_contributor_input.handling_mode =
            Some(meerkat_core::types::HandlingMode::Queue);

        let mut missing_metadata_input =
            invalid_input_snapshot(missing_metadata_input_id.clone(), None, None);
        missing_metadata_input.content_shape = None;
        missing_metadata_input.handling_mode = None;

        let mut missing_queue_membership_input = invalid_input_snapshot(
            missing_queue_membership_input_id.clone(),
            Some(InputLifecycleState::Queued),
            None,
        );
        missing_queue_membership_input.handling_mode =
            Some(meerkat_core::types::HandlingMode::Queue);

        let resolved_waiter_input = invalid_input_snapshot(
            resolved_waiter_input_id.clone(),
            Some(InputLifecycleState::Consumed),
            Some(InputTerminalOutcome::Consumed),
        );

        snapshot.spine.inputs.admission_order = vec![
            queue_input,
            contributor_input,
            boundaryless_contributor_input,
            missing_metadata_input,
            missing_queue_membership_input,
            resolved_waiter_input,
        ];
        snapshot.spine.inputs.queue = vec![queue_input_id.clone()];
        snapshot.spine.inputs.steer_queue = vec![queue_input_id.clone()];
        snapshot.spine.inputs.current_run_contributors = vec![
            contributor_input_id.clone(),
            boundaryless_contributor_input_id.clone(),
        ];
        snapshot.spine.completion_waiters.input_count = 4;
        snapshot.spine.completion_waiters.waiter_count = 4;
        snapshot.spine.completion_waiters.waiting_inputs = vec![
            MeerkatCompletionWaiterSnapshot {
                input_id: waiter_input_id.clone(),
                waiter_count: 2,
            },
            MeerkatCompletionWaiterSnapshot {
                input_id: resolved_waiter_input_id.clone(),
                waiter_count: 0,
            },
        ];

        snapshot.tool_surface = Some(ExternalToolSurfaceSnapshot {
            phase: meerkat_core::ExternalToolSurfaceGlobalPhase::Operating,
            snapshot_epoch: 0,
            snapshot_aligned_epoch: 0,
            entries: vec![invalid_tool_surface_entry(
                "invalid-visible-removed-surface",
                true,
                ExternalToolSurfaceBaseState::Removed,
            )],
        });

        let violations = validate_meerkat_machine_snapshot(&snapshot);

        assert!(violations.contains(&MeerkatMachineInvariantViolation::RunningWithoutCurrentRun));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::ControlInputsRunMismatch {
                control_run_id: None,
                inputs_run_id: Some(_),
            }
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::QueueContainsNonQueuedInput {
                queue: "queue",
                input_id,
                lifecycle: Some(InputLifecycleState::Staged),
            } if input_id == &queue_input_id
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::QueueContainsWrongHandlingMode {
                queue: "queue",
                input_id,
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
            } if input_id == &queue_input_id
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::QueueAppearsInBothQueues { input_id }
                if input_id == &queue_input_id
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::AdmittedInputMissingContentShape { input_id }
                if input_id == &missing_metadata_input_id
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::AdmittedInputMissingHandlingMode { input_id }
                if input_id == &missing_metadata_input_id
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::AdmittedInputMissingLifecycle { input_id }
                if input_id == &missing_metadata_input_id
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::QueuedInputMissingOwningQueue {
                input_id,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
            } if input_id == &missing_queue_membership_input_id
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::CurrentRunContributorIllegalLifecycle {
                input_id,
                lifecycle: Some(InputLifecycleState::Queued),
            } if input_id == &contributor_input_id
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::CurrentRunContributorRunMismatch {
                input_id,
                current_run_id: run_id,
                last_run_id: None,
            } if input_id == &boundaryless_contributor_input_id && run_id == &current_run_id
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::CurrentRunContributorPendingConsumptionMissingBoundary {
                input_id,
                current_run_id: run_id,
            } if input_id == &boundaryless_contributor_input_id && run_id == &current_run_id
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::CompletionWaiterInputCountMismatch {
                recorded_input_count: 4,
                waiting_inputs_len: 2,
            }
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::CompletionWaiterCountMismatch {
                recorded_waiter_count: 4,
                summed_waiter_count: 2,
            }
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::CompletionWaiterUnknownInput { input_id }
                if input_id == &waiter_input_id
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::CompletionWaiterZeroCount { input_id }
                if input_id == &resolved_waiter_input_id
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::CompletionWaiterResolvedInput {
                input_id,
                lifecycle: Some(InputLifecycleState::Consumed),
                terminal_outcome_present: true,
            } if input_id == &resolved_waiter_input_id
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::TurnRunMismatch {
                turn_phase: TurnPhase::WaitingForOps,
                control_run_id: None,
                turn_run_id: Some(_),
            }
        )));
        assert!(
            violations
                .contains(&MeerkatMachineInvariantViolation::WaitingForOpsWithoutPendingOperations)
        );
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::ActiveOpsExceedsOperationCount {
                active_count,
                operation_count,
            } if active_count > operation_count
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::ToolSurfaceVisibleBaseMismatch {
                surface_id,
                visible: true,
                base_state: ExternalToolSurfaceBaseState::Removed,
            } if surface_id == "invalid-visible-removed-surface"
        )));

        Ok(())
    }

    #[tokio::test]
    async fn validate_meerkat_machine_snapshot_reports_turn_ops_barrier_violations()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let base_snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        let pending_only_id = OperationId::new();
        let barrier_only_id = OperationId::new();
        let mut waiting_snapshot = base_snapshot.clone();
        let turn = waiting_snapshot
            .turn
            .as_mut()
            .ok_or_else(|| "live session should expose execution snapshot".to_string())?;
        turn.turn_phase = TurnPhase::WaitingForOps;
        turn.tool_calls_pending = 0;
        turn.pending_operation_ids = Some(vec![pending_only_id.clone()]);
        turn.barrier_operation_ids = vec![barrier_only_id.clone()];
        turn.has_barrier_ops = false;
        turn.barrier_satisfied = false;
        waiting_snapshot.spine.ops.operations.clear();
        waiting_snapshot.spine.ops.operation_count = 0;
        waiting_snapshot.spine.ops.active_count = 0;

        let waiting_violations = validate_meerkat_machine_snapshot(&waiting_snapshot);

        assert!(waiting_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::WaitingForOpsWithoutToolCalls
        )));
        assert!(waiting_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::BarrierOperationsWithoutFlag {
                barrier_operation_count: 1,
            }
        )));
        assert!(waiting_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::UnsatisfiedBarrierWithoutBarrierOps
        )));
        assert!(waiting_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::PendingOperationUnknown { operation_id }
                if operation_id == &pending_only_id
        )));
        assert!(waiting_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::BarrierOperationUnknown { operation_id }
                if operation_id == &barrier_only_id
        )));
        assert!(waiting_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::BarrierOperationNotPending { operation_id }
                if operation_id == &barrier_only_id
        )));

        let mut outside_waiting_snapshot = base_snapshot.clone();
        let known_operation_id = OperationId::new();
        let turn = outside_waiting_snapshot
            .turn
            .as_mut()
            .ok_or_else(|| "live session should expose execution snapshot".to_string())?;
        turn.turn_phase = TurnPhase::CallingLlm;
        turn.tool_calls_pending = 1;
        turn.pending_operation_ids = Some(vec![known_operation_id.clone()]);
        turn.barrier_operation_ids.clear();
        turn.has_barrier_ops = false;
        turn.barrier_satisfied = true;
        outside_waiting_snapshot.spine.ops.operations = vec![OperationLifecycleSnapshot {
            started_at_ms: Some(1),
            ..invalid_operation_snapshot(
                known_operation_id.clone(),
                OperationKind::BackgroundToolOp,
                OperationStatus::Running,
            )
        }];
        outside_waiting_snapshot.spine.ops.operation_count = 1;
        outside_waiting_snapshot.spine.ops.active_count = 1;

        let outside_waiting_violations =
            validate_meerkat_machine_snapshot(&outside_waiting_snapshot);

        assert!(outside_waiting_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::PendingOperationsOutsideWaitingPhase {
                turn_phase: TurnPhase::CallingLlm,
                pending_operation_count: 1,
            }
        )));

        let mut missing_barrier_snapshot = base_snapshot;
        let turn = missing_barrier_snapshot
            .turn
            .as_mut()
            .ok_or_else(|| "live session should expose execution snapshot".to_string())?;
        turn.turn_phase = TurnPhase::WaitingForOps;
        turn.tool_calls_pending = 1;
        turn.pending_operation_ids = Some(vec![known_operation_id]);
        turn.barrier_operation_ids.clear();
        turn.has_barrier_ops = true;
        turn.barrier_satisfied = false;
        missing_barrier_snapshot.spine.ops.operations = vec![OperationLifecycleSnapshot {
            started_at_ms: Some(1),
            ..invalid_operation_snapshot(
                OperationId::new(),
                OperationKind::BackgroundToolOp,
                OperationStatus::Running,
            )
        }];
        missing_barrier_snapshot.spine.ops.operation_count = 1;
        missing_barrier_snapshot.spine.ops.active_count = 1;

        let missing_barrier_violations =
            validate_meerkat_machine_snapshot(&missing_barrier_snapshot);

        assert!(missing_barrier_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::BarrierFlagWithoutOperations
        )));

        Ok(())
    }

    #[tokio::test]
    async fn validate_meerkat_machine_snapshot_reports_ops_shape_violations() -> Result<(), String>
    {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let base_snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        let missing_wait_target = OperationId::new();
        let orphaned_pending_wait_request_id = WaitRequestId::new();
        let mut malformed_snapshot = base_snapshot.clone();
        malformed_snapshot.spine.ops.operation_count = 1;
        malformed_snapshot.spine.ops.wait_request_id = None;
        malformed_snapshot.spine.ops.pending_wait_present = true;
        malformed_snapshot.spine.ops.pending_wait_request_id =
            Some(orphaned_pending_wait_request_id);
        malformed_snapshot.spine.ops.wait_operation_ids = vec![missing_wait_target.clone()];
        malformed_snapshot.spine.ops.detached_wake_pending = Some(true);
        malformed_snapshot.spine.ops.detached_wake_signaled = None;

        let malformed_violations = validate_meerkat_machine_snapshot(&malformed_snapshot);

        assert!(malformed_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::OpsOperationCountMismatch {
                operation_count: 1,
                operations_len: 0,
            }
        )));
        assert!(malformed_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::WaitOperationIdsWithoutWaitRequest {
                wait_operation_count: 1,
            }
        )));
        assert!(malformed_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::PendingWaitCarrierWithoutWaitRequest
        )));
        assert!(malformed_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::WaitTargetsUnknownOperation { operation_id }
                if operation_id == &missing_wait_target
        )));
        assert!(malformed_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::DetachedWakePresenceMismatch {
                pending_present: true,
                signaled_present: false,
            }
        )));

        let wait_request_id = WaitRequestId::new();
        let mut empty_wait_snapshot = base_snapshot.clone();
        empty_wait_snapshot.spine.ops.wait_request_id = Some(wait_request_id.clone());
        empty_wait_snapshot.spine.ops.pending_wait_present = false;
        empty_wait_snapshot.spine.ops.pending_wait_request_id = None;
        empty_wait_snapshot.spine.ops.wait_operation_ids.clear();

        let empty_wait_violations = validate_meerkat_machine_snapshot(&empty_wait_snapshot);

        assert!(empty_wait_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::WaitRequestWithoutTrackedOperations {
                wait_request_id: active_wait_request_id,
            } if active_wait_request_id == &wait_request_id
        )));
        assert!(empty_wait_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::WaitRequestMissingPendingWaitCarrier {
                wait_request_id: active_wait_request_id,
            } if active_wait_request_id == &wait_request_id
        )));

        let mut stale_detached_wake_snapshot = base_snapshot.clone();
        stale_detached_wake_snapshot
            .spine
            .binding
            .detached_wake_present = true;
        stale_detached_wake_snapshot.spine.ops.detached_wake_pending = Some(false);
        stale_detached_wake_snapshot
            .spine
            .ops
            .detached_wake_signaled = Some(true);

        let stale_detached_wake_violations =
            validate_meerkat_machine_snapshot(&stale_detached_wake_snapshot);

        assert!(
            stale_detached_wake_violations
                .iter()
                .any(|violation| matches!(
                    violation,
                    MeerkatMachineInvariantViolation::DetachedWakeSignaledWithoutPending
                ))
        );

        let peer_handle = OperationPeerHandle {
            peer_name: "child".into(),
            trusted_peer: TrustedPeerSpec::new("child", "child-id", "inproc://child")
                .map_err(|err| err.to_string())?,
        };
        let running_operation_id = OperationId::new();
        let terminal_operation_id = OperationId::new();
        let wait_request_id = WaitRequestId::new();
        let mismatched_pending_wait_request_id = WaitRequestId::new();
        let mut operation_shape_snapshot = base_snapshot.clone();
        operation_shape_snapshot.spine.binding.detached_wake_present = false;
        operation_shape_snapshot.spine.ops.operation_count = 2;
        operation_shape_snapshot.spine.ops.active_count = 2;
        operation_shape_snapshot.spine.ops.wait_request_id = Some(wait_request_id.clone());
        operation_shape_snapshot.spine.ops.pending_wait_present = true;
        operation_shape_snapshot.spine.ops.pending_wait_request_id =
            Some(mismatched_pending_wait_request_id.clone());
        operation_shape_snapshot.spine.ops.wait_operation_ids = vec![terminal_operation_id.clone()];
        operation_shape_snapshot.spine.ops.detached_wake_pending = Some(false);
        operation_shape_snapshot.spine.ops.detached_wake_signaled = Some(false);
        operation_shape_snapshot.spine.ops.operations = vec![
            OperationLifecycleSnapshot {
                peer_ready: true,
                ..invalid_operation_snapshot(
                    running_operation_id.clone(),
                    OperationKind::BackgroundToolOp,
                    OperationStatus::Running,
                )
            },
            OperationLifecycleSnapshot {
                peer_handle: Some(peer_handle),
                watcher_count: 2,
                ..invalid_operation_snapshot(
                    terminal_operation_id.clone(),
                    OperationKind::MobMemberChild,
                    OperationStatus::Completed,
                )
            },
        ];

        let operation_shape_violations =
            validate_meerkat_machine_snapshot(&operation_shape_snapshot);

        assert!(operation_shape_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::ActiveOpsStatusCountMismatch {
                active_count: 2,
                nonterminal_count: 1,
            }
        )));
        assert!(operation_shape_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::WaitRequestAlreadySatisfied {
                wait_request_id: active_wait_request_id,
            } if active_wait_request_id == &wait_request_id
        )));
        assert!(operation_shape_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::PendingWaitCarrierRequestMismatch {
                wait_request_id: active_wait_request_id,
                pending_wait_request_id,
            } if active_wait_request_id == &wait_request_id
                && pending_wait_request_id == &mismatched_pending_wait_request_id
        )));
        assert!(operation_shape_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::DetachedWakeBindingMismatch {
                binding_present: false,
                ops_present: true,
            }
        )));
        assert!(operation_shape_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::OperationPeerReadyWithoutExpectedKind {
                operation_id,
                kind: OperationKind::BackgroundToolOp,
            } if operation_id == &running_operation_id
        )));
        assert!(operation_shape_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::OperationPeerReadyWithoutHandle { operation_id }
                if operation_id == &running_operation_id
        )));
        assert!(operation_shape_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::OperationActiveWithoutStartTimestamp {
                operation_id,
                status: OperationStatus::Running,
            } if operation_id == &running_operation_id
        )));
        assert!(operation_shape_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::OperationHandleWithoutPeerReady { operation_id }
                if operation_id == &terminal_operation_id
        )));
        assert!(operation_shape_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::OperationTerminalOutcomeMismatch {
                operation_id,
                status: OperationStatus::Completed,
                terminal_outcome_present: false,
            } if operation_id == &terminal_operation_id
        )));
        assert!(operation_shape_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::OperationCompletionTimestampMismatch {
                operation_id,
                status: OperationStatus::Completed,
                completed_at_present: false,
                elapsed_ms_present: false,
            } if operation_id == &terminal_operation_id
        )));
        assert!(operation_shape_violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::OperationTerminalWithWatchers {
                operation_id,
                watcher_count: 2,
            } if operation_id == &terminal_operation_id
        )));

        Ok(())
    }

    #[tokio::test]
    async fn validate_meerkat_machine_snapshot_reports_peer_shape_violations() -> Result<(), String>
    {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        snapshot.peer = Some(PeerIngressRuntimeSnapshot {
            self_peer_id: "self-peer".into(),
            auth_required: true,
            authority_phase: PeerIngressAuthorityPhase::Absent,
            trusted_peers: Vec::new(),
            submission_queue_len: 0,
            queue: PeerIngressQueueSnapshot {
                total_count: 3,
                actionable_count: 0,
                response_count: 1,
                lifecycle_count: 0,
                silent_request_count: 0,
                ack_count: 0,
                plain_event_count: 0,
                queued_entries: vec![
                    PeerIngressEntrySnapshot {
                        raw_item_id: "invalid-message".into(),
                        interaction_id: None,
                        class: PeerInputClass::Response,
                        kind: PeerIngressKind::Message,
                        from_peer: Some("ally".into()),
                        lifecycle_peer: None,
                        request_id: None,
                        trusted_snapshot: Some(true),
                    },
                    PeerIngressEntrySnapshot {
                        raw_item_id: "invalid-lifecycle".into(),
                        interaction_id: None,
                        class: PeerInputClass::PeerLifecycleAdded,
                        kind: PeerIngressKind::Request,
                        from_peer: Some("ally".into()),
                        lifecycle_peer: None,
                        request_id: Some("req-1".into()),
                        trusted_snapshot: Some(true),
                    },
                    PeerIngressEntrySnapshot {
                        raw_item_id: "invalid-plain".into(),
                        interaction_id: None,
                        class: PeerInputClass::PlainEvent,
                        kind: PeerIngressKind::PlainEvent,
                        from_peer: Some("tcp".into()),
                        lifecycle_peer: None,
                        request_id: Some("req-2".into()),
                        trusted_snapshot: None,
                    },
                ],
            },
        });

        let violations = validate_meerkat_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::PeerQueueCountMismatch {
                field: "actionable_count",
                recorded: 0,
                computed: 2,
            }
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::PeerQueueCountMismatch {
                field: "lifecycle_count",
                recorded: 0,
                computed: 1,
            }
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::PeerEntryKindClassMismatch { raw_item_id, kind: PeerIngressKind::Message, class: PeerInputClass::Response }
                if raw_item_id == "invalid-message"
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::PeerEntryLifecyclePeerMismatch {
                raw_item_id,
                class: PeerInputClass::PeerLifecycleAdded,
                lifecycle_peer_present: false,
            } if raw_item_id == "invalid-lifecycle"
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::PeerEntryFromPeerMismatch {
                raw_item_id,
                kind: PeerIngressKind::PlainEvent,
                from_peer_present: true,
            } if raw_item_id == "invalid-plain"
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::PeerEntryRequestIdMismatch {
                raw_item_id,
                kind: PeerIngressKind::PlainEvent,
                request_id_present: true,
            } if raw_item_id == "invalid-plain"
        )));

        Ok(())
    }

    #[tokio::test]
    async fn validate_meerkat_machine_snapshot_reports_peer_authority_count_violations()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        snapshot.peer = Some(PeerIngressRuntimeSnapshot {
            self_peer_id: "self-peer".into(),
            auth_required: false,
            authority_phase: PeerIngressAuthorityPhase::Received,
            trusted_peers: Vec::new(),
            submission_queue_len: 2,
            queue: PeerIngressQueueSnapshot {
                total_count: 2,
                actionable_count: 2,
                response_count: 0,
                lifecycle_count: 0,
                silent_request_count: 0,
                ack_count: 0,
                plain_event_count: 1,
                queued_entries: vec![
                    PeerIngressEntrySnapshot {
                        raw_item_id: "tracked-message".into(),
                        interaction_id: None,
                        class: PeerInputClass::ActionableMessage,
                        kind: PeerIngressKind::Message,
                        from_peer: Some("ally".into()),
                        lifecycle_peer: None,
                        request_id: None,
                        trusted_snapshot: Some(true),
                    },
                    PeerIngressEntrySnapshot {
                        raw_item_id: "plain-event".into(),
                        interaction_id: None,
                        class: PeerInputClass::PlainEvent,
                        kind: PeerIngressKind::PlainEvent,
                        from_peer: None,
                        lifecycle_peer: None,
                        request_id: None,
                        trusted_snapshot: None,
                    },
                ],
            },
        });

        let violations = validate_meerkat_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::PeerAuthorityTrackedEntryCountMismatch {
                submission_queue_len: 2,
                queued_authority_entries: 1,
            }
        )));

        Ok(())
    }

    #[tokio::test]
    async fn validate_meerkat_machine_snapshot_reports_peer_trust_shape_violations()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        snapshot.peer = Some(PeerIngressRuntimeSnapshot {
            self_peer_id: "self-peer".into(),
            auth_required: true,
            authority_phase: PeerIngressAuthorityPhase::Received,
            trusted_peers: Vec::new(),
            submission_queue_len: 2,
            queue: PeerIngressQueueSnapshot {
                total_count: 3,
                actionable_count: 3,
                response_count: 0,
                lifecycle_count: 0,
                silent_request_count: 0,
                ack_count: 0,
                plain_event_count: 1,
                queued_entries: vec![
                    PeerIngressEntrySnapshot {
                        raw_item_id: "tracked-missing".into(),
                        interaction_id: None,
                        class: PeerInputClass::ActionableMessage,
                        kind: PeerIngressKind::Message,
                        from_peer: Some("ally".into()),
                        lifecycle_peer: None,
                        request_id: None,
                        trusted_snapshot: None,
                    },
                    PeerIngressEntrySnapshot {
                        raw_item_id: "tracked-untrusted".into(),
                        interaction_id: None,
                        class: PeerInputClass::ActionableRequest,
                        kind: PeerIngressKind::Request,
                        from_peer: Some("ally".into()),
                        lifecycle_peer: None,
                        request_id: Some("req-1".into()),
                        trusted_snapshot: Some(false),
                    },
                    PeerIngressEntrySnapshot {
                        raw_item_id: "plain-with-trust".into(),
                        interaction_id: None,
                        class: PeerInputClass::PlainEvent,
                        kind: PeerIngressKind::PlainEvent,
                        from_peer: None,
                        lifecycle_peer: None,
                        request_id: None,
                        trusted_snapshot: Some(false),
                    },
                ],
            },
        });

        let violations = validate_meerkat_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::PeerEntryTrustedSnapshotPresenceMismatch {
                raw_item_id,
                kind: PeerIngressKind::Message,
                trusted_snapshot_present: false,
            } if raw_item_id == "tracked-missing"
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::PeerEntryTrustedSnapshotPresenceMismatch {
                raw_item_id,
                kind: PeerIngressKind::PlainEvent,
                trusted_snapshot_present: true,
            } if raw_item_id == "plain-with-trust"
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::PeerEntryUntrustedWhileAuthRequired {
                raw_item_id,
                kind: PeerIngressKind::Request,
            } if raw_item_id == "tracked-untrusted"
        )));

        Ok(())
    }

    #[tokio::test]
    async fn validate_meerkat_machine_snapshot_reports_runtime_spine_lifecycle_violations()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        let contributor_input_id = InputId::new();
        let terminal_missing_outcome_id = InputId::new();
        let nonterminal_with_outcome_id = InputId::new();

        snapshot.spine.inputs.current_run_id = None;
        snapshot.spine.inputs.current_run_contributors = vec![contributor_input_id.clone()];
        snapshot.spine.inputs.admission_order = vec![
            invalid_input_snapshot(
                contributor_input_id.clone(),
                Some(InputLifecycleState::Queued),
                None,
            ),
            invalid_input_snapshot(
                terminal_missing_outcome_id.clone(),
                Some(InputLifecycleState::Consumed),
                None,
            ),
            invalid_input_snapshot(
                nonterminal_with_outcome_id.clone(),
                Some(InputLifecycleState::Queued),
                Some(InputTerminalOutcome::Consumed),
            ),
        ];

        let violations = validate_meerkat_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::ContributorsWithoutCurrentRun {
                contributor_count: 1,
            }
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::CurrentRunContributorIllegalLifecycle {
                input_id,
                lifecycle: Some(InputLifecycleState::Queued),
            } if input_id == &contributor_input_id
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::TerminalInputWithoutOutcome {
                input_id,
                lifecycle: InputLifecycleState::Consumed,
            } if input_id == &terminal_missing_outcome_id
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::NonTerminalInputWithTerminalOutcome {
                input_id,
                lifecycle: InputLifecycleState::Queued,
                terminal_outcome: InputTerminalOutcome::Consumed,
            } if input_id == &nonterminal_with_outcome_id
        )));

        Ok(())
    }

    #[tokio::test]
    async fn validate_meerkat_machine_snapshot_reports_ingress_boundary_shape_violations()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        let queued_input_id = InputId::new();
        let applied_input_id = InputId::new();
        let current_run_id = RunId::new();

        let mut queued_input = invalid_input_snapshot(
            queued_input_id.clone(),
            Some(InputLifecycleState::Queued),
            None,
        );
        queued_input.content_shape = Some(
            meerkat_runtime::runtime_ingress_authority::ContentShape("text".into()),
        );
        queued_input.handling_mode = Some(meerkat_core::types::HandlingMode::Queue);
        queued_input.last_run_id = Some(current_run_id);
        queued_input.last_boundary_sequence = Some(7);

        let mut applied_input = invalid_input_snapshot(
            applied_input_id.clone(),
            Some(InputLifecycleState::AppliedPendingConsumption),
            None,
        );
        applied_input.content_shape = Some(
            meerkat_runtime::runtime_ingress_authority::ContentShape("text".into()),
        );
        applied_input.handling_mode = Some(meerkat_core::types::HandlingMode::Queue);
        applied_input.last_boundary_sequence = Some(9);

        snapshot.spine.inputs.admission_order = vec![queued_input, applied_input];
        snapshot.spine.inputs.queue = vec![queued_input_id.clone()];
        snapshot.spine.inputs.steer_queue.clear();
        snapshot.spine.inputs.current_run_id = None;
        snapshot.spine.inputs.current_run_contributors.clear();

        let violations = validate_meerkat_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::InputBoundarySequenceIllegalLifecycle {
                input_id,
                lifecycle: InputLifecycleState::Queued,
                last_boundary_sequence: 7,
            } if input_id == &queued_input_id
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::InputBoundarySequenceWithoutRun {
                input_id,
                last_boundary_sequence: 9,
            } if input_id == &applied_input_id
        )));

        Ok(())
    }

    #[tokio::test]
    async fn validate_meerkat_machine_snapshot_reports_process_request_shape_violations()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        snapshot.spine.inputs.process_requested = true;
        snapshot.spine.inputs.wake_requested = false;
        snapshot.spine.inputs.steer_queue.clear();

        let violations = validate_meerkat_machine_snapshot(&snapshot);

        assert!(
            violations.contains(&MeerkatMachineInvariantViolation::ProcessRequestedWithoutWake)
        );
        assert!(
            violations
                .contains(&MeerkatMachineInvariantViolation::ProcessRequestedWithoutSteerQueue)
        );

        Ok(())
    }

    #[tokio::test]
    async fn validate_meerkat_machine_snapshot_reports_wake_without_queued_work()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        snapshot.spine.inputs.wake_requested = true;
        snapshot.spine.inputs.queue.clear();
        snapshot.spine.inputs.steer_queue.clear();

        let violations = validate_meerkat_machine_snapshot(&snapshot);

        assert!(
            violations.contains(&MeerkatMachineInvariantViolation::WakeRequestedWithoutQueuedWork)
        );

        Ok(())
    }

    #[tokio::test]
    async fn validate_meerkat_machine_snapshot_reports_ingress_run_binding_shape_violations()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        let staged_input_id = InputId::new();
        let pending_input_id = InputId::new();
        let pending_run_id = RunId::new();

        let mut staged_input = invalid_input_snapshot(
            staged_input_id.clone(),
            Some(InputLifecycleState::Staged),
            None,
        );
        staged_input.content_shape = Some(
            meerkat_runtime::runtime_ingress_authority::ContentShape("text".into()),
        );
        staged_input.handling_mode = Some(meerkat_core::types::HandlingMode::Queue);

        let mut pending_input = invalid_input_snapshot(
            pending_input_id.clone(),
            Some(InputLifecycleState::AppliedPendingConsumption),
            None,
        );
        pending_input.content_shape = Some(
            meerkat_runtime::runtime_ingress_authority::ContentShape("text".into()),
        );
        pending_input.handling_mode = Some(meerkat_core::types::HandlingMode::Queue);
        pending_input.last_run_id = Some(pending_run_id);

        snapshot.spine.inputs.admission_order = vec![staged_input, pending_input];
        snapshot.spine.inputs.queue.clear();
        snapshot.spine.inputs.steer_queue.clear();
        snapshot.spine.inputs.current_run_id = None;
        snapshot.spine.inputs.current_run_contributors.clear();

        let violations = validate_meerkat_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::InputLifecycleMissingRunBinding {
                input_id,
                lifecycle: InputLifecycleState::Staged,
            } if input_id == &staged_input_id
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::AppliedPendingConsumptionMissingBoundarySequence {
                input_id,
            } if input_id == &pending_input_id
        )));

        Ok(())
    }

    #[tokio::test]
    async fn validate_meerkat_machine_snapshot_reports_missing_run_bound_contributor()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        let current_run_id = RunId::new();
        let present_input_id = InputId::new();
        let missing_input_id = InputId::new();

        let mut present_input = invalid_input_snapshot(
            present_input_id.clone(),
            Some(InputLifecycleState::Staged),
            None,
        );
        present_input.content_shape = Some(
            meerkat_runtime::runtime_ingress_authority::ContentShape("text".into()),
        );
        present_input.handling_mode = Some(meerkat_core::types::HandlingMode::Queue);
        present_input.last_run_id = Some(current_run_id.clone());

        let mut missing_input = invalid_input_snapshot(
            missing_input_id.clone(),
            Some(InputLifecycleState::AppliedPendingConsumption),
            None,
        );
        missing_input.content_shape = Some(
            meerkat_runtime::runtime_ingress_authority::ContentShape("text".into()),
        );
        missing_input.handling_mode = Some(meerkat_core::types::HandlingMode::Queue);
        missing_input.last_run_id = Some(current_run_id.clone());
        missing_input.last_boundary_sequence = Some(7);

        snapshot.spine.control.phase = RuntimeState::Running;
        snapshot.spine.control.current_run_id = Some(current_run_id.clone());
        snapshot.spine.inputs.current_run_id = Some(current_run_id.clone());
        snapshot.spine.inputs.current_run_contributors = vec![present_input_id];
        snapshot.spine.inputs.queue.clear();
        snapshot.spine.inputs.steer_queue.clear();
        snapshot.spine.inputs.admission_order = vec![present_input, missing_input];

        let violations = validate_meerkat_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::CurrentRunBoundInputMissingContributor {
                input_id,
                current_run_id: violation_run_id,
                lifecycle: InputLifecycleState::AppliedPendingConsumption,
            } if input_id == &missing_input_id && violation_run_id == &current_run_id
        )));

        Ok(())
    }

    #[tokio::test]
    async fn validate_meerkat_machine_snapshot_reports_destroyed_ingress_violations()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        let queued_input_id = InputId::new();
        let mut queued_input = invalid_input_snapshot(
            queued_input_id.clone(),
            Some(InputLifecycleState::Queued),
            None,
        );
        queued_input.handling_mode = Some(meerkat_core::types::HandlingMode::Queue);

        let active_input_id = InputId::new();
        let mut active_input = invalid_input_snapshot(
            active_input_id.clone(),
            Some(InputLifecycleState::Staged),
            None,
        );
        active_input.handling_mode = Some(meerkat_core::types::HandlingMode::Queue);

        snapshot.spine.inputs.ingress_phase = meerkat_runtime::IngressPhase::Destroyed;
        snapshot.spine.inputs.queue = vec![queued_input_id.clone()];
        snapshot.spine.inputs.steer_queue = vec![queued_input_id.clone()];
        snapshot.spine.inputs.current_run_id = Some(RunId::new());
        snapshot.spine.inputs.current_run_contributors = vec![active_input_id.clone()];
        snapshot.spine.inputs.wake_requested = true;
        snapshot.spine.inputs.process_requested = true;
        snapshot.spine.inputs.admission_order = vec![queued_input, active_input];

        let violations = validate_meerkat_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::DestroyedIngressStillHasQueuedWork {
                queue: "queue",
                count: 1,
            }
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::DestroyedIngressStillHasQueuedWork {
                queue: "steer_queue",
                count: 1,
            }
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::DestroyedIngressStillHasCurrentRun { .. }
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::DestroyedIngressStillHasContributors {
                contributor_count: 1,
            }
        )));
        assert!(
            violations
                .contains(&MeerkatMachineInvariantViolation::DestroyedIngressStillRequestsWake)
        );
        assert!(
            violations.contains(
                &MeerkatMachineInvariantViolation::DestroyedIngressStillRequestsProcessing
            )
        );
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::DestroyedIngressHasNonTerminalInput {
                input_id,
                lifecycle: InputLifecycleState::Queued,
            } if input_id == &queued_input_id
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::DestroyedIngressHasNonTerminalInput {
                input_id,
                lifecycle: InputLifecycleState::Staged,
            } if input_id == &active_input_id
        )));

        Ok(())
    }

    #[tokio::test]
    async fn validate_meerkat_machine_snapshot_reports_destroyed_completion_waiters()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        let input_id = InputId::new();
        let mut queued_input =
            invalid_input_snapshot(input_id.clone(), Some(InputLifecycleState::Queued), None);
        queued_input.content_shape = Some(
            meerkat_runtime::runtime_ingress_authority::ContentShape("text".into()),
        );
        queued_input.handling_mode = Some(meerkat_core::types::HandlingMode::Queue);

        snapshot.spine.control.phase = RuntimeState::Destroyed;
        snapshot.spine.control.current_run_id = None;
        snapshot.spine.inputs.current_run_id = None;
        snapshot.spine.inputs.current_run_contributors.clear();
        snapshot.spine.inputs.queue = vec![input_id.clone()];
        snapshot.spine.inputs.steer_queue.clear();
        snapshot.spine.inputs.wake_requested = false;
        snapshot.spine.inputs.process_requested = false;
        snapshot.spine.inputs.admission_order = vec![queued_input];
        snapshot.spine.completion_waiters.input_count = 1;
        snapshot.spine.completion_waiters.waiter_count = 1;
        snapshot.spine.completion_waiters.waiting_inputs = vec![MeerkatCompletionWaiterSnapshot {
            input_id,
            waiter_count: 1,
        }];

        let violations = validate_meerkat_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::DestroyedRuntimeStillHasCompletionWaiters {
                waiter_count: 1,
            }
        )));

        Ok(())
    }

    #[tokio::test]
    async fn validate_meerkat_machine_snapshot_reports_destroyed_queued_work() -> Result<(), String>
    {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        let input_id = InputId::new();
        let mut queued_input =
            invalid_input_snapshot(input_id.clone(), Some(InputLifecycleState::Queued), None);
        queued_input.content_shape = Some(
            meerkat_runtime::runtime_ingress_authority::ContentShape("text".into()),
        );
        queued_input.handling_mode = Some(meerkat_core::types::HandlingMode::Queue);

        snapshot.spine.control.phase = RuntimeState::Destroyed;
        snapshot.spine.control.current_run_id = None;
        snapshot.spine.inputs.current_run_id = None;
        snapshot.spine.inputs.current_run_contributors.clear();
        snapshot.spine.inputs.queue = vec![input_id];
        snapshot.spine.inputs.steer_queue.clear();
        snapshot.spine.inputs.wake_requested = false;
        snapshot.spine.inputs.process_requested = false;
        snapshot.spine.inputs.admission_order = vec![queued_input];

        let violations = validate_meerkat_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::DestroyedRuntimeStillHasQueuedWork {
                queue: "queue",
                count: 1,
            }
        )));

        Ok(())
    }

    #[tokio::test]
    async fn validate_meerkat_machine_snapshot_reports_stopped_completion_waiters()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        let input_id = InputId::new();
        let mut queued_input =
            invalid_input_snapshot(input_id.clone(), Some(InputLifecycleState::Queued), None);
        queued_input.content_shape = Some(
            meerkat_runtime::runtime_ingress_authority::ContentShape("text".into()),
        );
        queued_input.handling_mode = Some(meerkat_core::types::HandlingMode::Queue);

        snapshot.spine.control.phase = RuntimeState::Stopped;
        snapshot.spine.control.current_run_id = None;
        snapshot.spine.inputs.current_run_id = None;
        snapshot.spine.inputs.current_run_contributors.clear();
        snapshot.spine.inputs.queue = vec![input_id.clone()];
        snapshot.spine.inputs.steer_queue.clear();
        snapshot.spine.inputs.wake_requested = false;
        snapshot.spine.inputs.process_requested = false;
        snapshot.spine.inputs.admission_order = vec![queued_input];
        snapshot.spine.completion_waiters.input_count = 1;
        snapshot.spine.completion_waiters.waiter_count = 1;
        snapshot.spine.completion_waiters.waiting_inputs = vec![MeerkatCompletionWaiterSnapshot {
            input_id,
            waiter_count: 1,
        }];

        let violations = validate_meerkat_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::StoppedRuntimeStillHasCompletionWaiters {
                waiter_count: 1,
            }
        )));

        Ok(())
    }

    #[tokio::test]
    async fn validate_meerkat_machine_snapshot_reports_stopped_queued_work() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        let input_id = InputId::new();
        let mut queued_input =
            invalid_input_snapshot(input_id.clone(), Some(InputLifecycleState::Queued), None);
        queued_input.content_shape = Some(
            meerkat_runtime::runtime_ingress_authority::ContentShape("text".into()),
        );
        queued_input.handling_mode = Some(meerkat_core::types::HandlingMode::Queue);

        snapshot.spine.control.phase = RuntimeState::Stopped;
        snapshot.spine.control.current_run_id = None;
        snapshot.spine.inputs.current_run_id = None;
        snapshot.spine.inputs.current_run_contributors.clear();
        snapshot.spine.inputs.queue = vec![input_id];
        snapshot.spine.inputs.steer_queue.clear();
        snapshot.spine.inputs.wake_requested = false;
        snapshot.spine.inputs.process_requested = false;
        snapshot.spine.inputs.admission_order = vec![queued_input];

        let violations = validate_meerkat_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::StoppedRuntimeStillHasQueuedWork {
                queue: "queue",
                count: 1,
            }
        )));

        Ok(())
    }

    #[tokio::test]
    async fn validate_meerkat_machine_snapshot_reports_retired_completion_waiters()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        let input_id = InputId::new();
        let mut queued_input =
            invalid_input_snapshot(input_id.clone(), Some(InputLifecycleState::Queued), None);
        queued_input.content_shape = Some(
            meerkat_runtime::runtime_ingress_authority::ContentShape("text".into()),
        );
        queued_input.handling_mode = Some(meerkat_core::types::HandlingMode::Queue);

        snapshot.spine.control.phase = RuntimeState::Retired;
        snapshot.spine.control.current_run_id = None;
        snapshot.spine.inputs.current_run_id = None;
        snapshot.spine.inputs.current_run_contributors.clear();
        snapshot.spine.inputs.queue = vec![input_id.clone()];
        snapshot.spine.inputs.steer_queue.clear();
        snapshot.spine.inputs.wake_requested = false;
        snapshot.spine.inputs.process_requested = false;
        snapshot.spine.inputs.admission_order = vec![queued_input];
        snapshot.spine.completion_waiters.input_count = 1;
        snapshot.spine.completion_waiters.waiter_count = 1;
        snapshot.spine.completion_waiters.waiting_inputs = vec![MeerkatCompletionWaiterSnapshot {
            input_id,
            waiter_count: 1,
        }];

        let violations = validate_meerkat_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::RetiredRuntimeStillHasCompletionWaiters {
                waiter_count: 1,
            }
        )));

        Ok(())
    }

    #[tokio::test]
    async fn validate_meerkat_machine_snapshot_reports_retired_active_run() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        let run_id = RunId::new();
        snapshot.spine.control.phase = RuntimeState::Retired;
        snapshot.spine.control.current_run_id = Some(run_id.clone());
        snapshot.spine.inputs.current_run_id = Some(run_id);
        snapshot.spine.inputs.current_run_contributors.clear();
        snapshot.spine.inputs.queue.clear();
        snapshot.spine.inputs.steer_queue.clear();
        snapshot.spine.inputs.wake_requested = false;
        snapshot.spine.inputs.process_requested = false;

        let violations = validate_meerkat_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::CurrentRunInIllegalControlPhase {
                phase: RuntimeState::Retired,
            }
        )));

        Ok(())
    }

    #[tokio::test]
    async fn validate_meerkat_machine_snapshot_reports_attached_steer_queue() -> Result<(), String>
    {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        let queued_input_id = InputId::new();
        let mut queued_input = invalid_input_snapshot(
            queued_input_id.clone(),
            Some(InputLifecycleState::Queued),
            None,
        );
        queued_input.content_shape = Some(
            meerkat_runtime::runtime_ingress_authority::ContentShape("text".into()),
        );
        queued_input.handling_mode = Some(meerkat_core::types::HandlingMode::Steer);

        snapshot.spine.control.phase = RuntimeState::Attached;
        snapshot.spine.control.current_run_id = None;
        snapshot.spine.inputs.current_run_id = None;
        snapshot.spine.inputs.current_run_contributors.clear();
        snapshot.spine.inputs.admission_order = vec![queued_input];
        snapshot.spine.inputs.queue.clear();
        snapshot.spine.inputs.steer_queue = vec![queued_input_id];
        snapshot.spine.inputs.wake_requested = true;
        snapshot.spine.inputs.process_requested = true;
        snapshot.spine.completion_waiters.input_count = 0;
        snapshot.spine.completion_waiters.waiter_count = 0;
        snapshot.spine.completion_waiters.waiting_inputs.clear();

        let violations = validate_meerkat_machine_snapshot(&snapshot);

        assert!(violations.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn validate_meerkat_machine_snapshot_reports_drain_shape_violations() -> Result<(), String>
    {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let snapshot = capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
            .await
            .map_err(|err| err.to_string())?
            .ok_or_else(|| {
                "live runtime-backed session should produce a Meerkat snapshot".to_string()
            })?;

        let mut missing_slot = snapshot.clone();
        missing_slot.spine.drain.slot_present = false;
        missing_slot.spine.drain.phase = Some(CommsDrainPhase::Running);
        missing_slot.spine.drain.mode = Some(CommsDrainMode::PersistentHost);
        missing_slot.spine.drain.handle_present = true;
        let missing_slot_violations = validate_meerkat_machine_snapshot(&missing_slot);
        assert!(missing_slot_violations.contains(
            &MeerkatMachineInvariantViolation::DrainPhaseWithoutSlot {
                phase: CommsDrainPhase::Running,
            }
        ));
        assert!(missing_slot_violations.contains(
            &MeerkatMachineInvariantViolation::DrainModeWithoutSlot {
                mode: CommsDrainMode::PersistentHost,
            }
        ));
        assert!(
            missing_slot_violations
                .contains(&MeerkatMachineInvariantViolation::DrainHandleWithoutSlot)
        );

        let mut missing_phase = snapshot.clone();
        missing_phase.spine.drain.slot_present = true;
        missing_phase.spine.drain.phase = None;
        let missing_phase_violations = validate_meerkat_machine_snapshot(&missing_phase);
        assert!(
            missing_phase_violations
                .contains(&MeerkatMachineInvariantViolation::DrainSlotMissingPhase)
        );

        let mut inactive = snapshot.clone();
        inactive.spine.drain.slot_present = true;
        inactive.spine.drain.phase = Some(CommsDrainPhase::Inactive);
        inactive.spine.drain.mode = Some(CommsDrainMode::Timed);
        inactive.spine.drain.handle_present = true;
        let inactive_violations = validate_meerkat_machine_snapshot(&inactive);
        assert!(inactive_violations.contains(
            &MeerkatMachineInvariantViolation::DrainInactiveWithMode {
                mode: CommsDrainMode::Timed,
            }
        ));
        assert!(
            inactive_violations
                .contains(&MeerkatMachineInvariantViolation::DrainInactiveWithHandle)
        );
        assert!(inactive_violations.contains(
            &MeerkatMachineInvariantViolation::DrainTerminalPhaseWithHandle {
                phase: CommsDrainPhase::Inactive,
            }
        ));

        let mut stopped = snapshot;
        stopped.spine.drain.slot_present = true;
        stopped.spine.drain.phase = Some(CommsDrainPhase::Stopped);
        stopped.spine.drain.mode = None;
        stopped.spine.drain.handle_present = true;
        let stopped_violations = validate_meerkat_machine_snapshot(&stopped);
        assert!(stopped_violations.contains(
            &MeerkatMachineInvariantViolation::DrainPhaseMissingMode {
                phase: CommsDrainPhase::Stopped,
            }
        ));
        assert!(stopped_violations.contains(
            &MeerkatMachineInvariantViolation::DrainTerminalPhaseWithHandle {
                phase: CommsDrainPhase::Stopped,
            }
        ));

        Ok(())
    }

    #[tokio::test]
    async fn validate_meerkat_machine_snapshot_reports_tool_surface_shape_violations()
    -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let service = build_runtime_backed_ephemeral_service(&temp);
        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();

        let bindings = runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|err| err.to_string())?;

        service
            .create_session(runtime_backed_request(session, bindings))
            .await
            .map_err(|err| err.to_string())?;

        let mut snapshot =
            capture_meerkat_machine_snapshot(&runtime_adapter, &service, &session_id)
                .await
                .map_err(|err| err.to_string())?
                .ok_or_else(|| {
                    "live runtime-backed session should produce a Meerkat snapshot".to_string()
                })?;

        let mut removed_reload = invalid_tool_surface_entry(
            "invalid-removed-reload",
            false,
            ExternalToolSurfaceBaseState::Removed,
        );
        removed_reload.pending_op = ExternalToolSurfacePendingOp::Reload;
        removed_reload.pending_task_sequence = 1;
        removed_reload.pending_lineage_sequence = 1;

        let mut absent_inflight = invalid_tool_surface_entry(
            "invalid-absent-inflight",
            false,
            ExternalToolSurfaceBaseState::Absent,
        );
        absent_inflight.inflight_call_count = 1;

        let mut forced_wrong_op = invalid_tool_surface_entry(
            "invalid-forced-delta",
            true,
            ExternalToolSurfaceBaseState::Active,
        );
        forced_wrong_op.last_delta_phase = ExternalToolSurfaceDeltaPhase::Forced;
        forced_wrong_op.last_delta_operation = ExternalToolSurfaceDeltaOperation::Add;

        let mut staged_seq_zero = invalid_tool_surface_entry(
            "invalid-staged-seq",
            true,
            ExternalToolSurfaceBaseState::Active,
        );
        staged_seq_zero.staged_op = ExternalToolSurfaceStagedOp::Reload;
        staged_seq_zero.staged_intent_sequence = 0;

        let mut pending_seq_mismatch = invalid_tool_surface_entry(
            "invalid-pending-seq",
            true,
            ExternalToolSurfaceBaseState::Active,
        );
        pending_seq_mismatch.pending_op = ExternalToolSurfacePendingOp::Add;
        pending_seq_mismatch.pending_task_sequence = 0;
        pending_seq_mismatch.pending_lineage_sequence = 1;

        let mut removing_without_timing = invalid_tool_surface_entry(
            "invalid-removing-timing",
            false,
            ExternalToolSurfaceBaseState::Removing,
        );
        removing_without_timing.has_removal_timing = false;

        snapshot.tool_surface = Some(ExternalToolSurfaceSnapshot {
            phase: meerkat_core::ExternalToolSurfaceGlobalPhase::Operating,
            snapshot_epoch: 0,
            snapshot_aligned_epoch: 0,
            entries: vec![
                removed_reload,
                absent_inflight,
                forced_wrong_op,
                staged_seq_zero,
                pending_seq_mismatch,
                removing_without_timing,
            ],
        });

        let violations = validate_meerkat_machine_snapshot(&snapshot);

        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::ToolSurfacePendingBaseMismatch {
                surface_id,
                base_state: ExternalToolSurfaceBaseState::Removed,
                pending_op: ExternalToolSurfacePendingOp::Reload,
            } if surface_id == "invalid-removed-reload"
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::ToolSurfaceInflightBaseMismatch {
                surface_id,
                base_state: ExternalToolSurfaceBaseState::Absent,
                inflight_call_count: 1,
            } if surface_id == "invalid-absent-inflight"
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::ToolSurfaceForcedPhaseWithoutRemoveDelta {
                surface_id,
                last_delta_operation: ExternalToolSurfaceDeltaOperation::Add,
            } if surface_id == "invalid-forced-delta"
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::ToolSurfaceStagedSequenceMismatch {
                surface_id,
                staged_op: ExternalToolSurfaceStagedOp::Reload,
                staged_intent_sequence: 0,
            } if surface_id == "invalid-staged-seq"
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::ToolSurfacePendingSequenceMismatch {
                surface_id,
                pending_op: ExternalToolSurfacePendingOp::Add,
                pending_task_sequence: 0,
                pending_lineage_sequence: 1,
            } if surface_id == "invalid-pending-seq"
        )));
        assert!(violations.iter().any(|violation| matches!(
            violation,
            MeerkatMachineInvariantViolation::ToolSurfaceRemovalTimingMismatch {
                surface_id,
                base_state: ExternalToolSurfaceBaseState::Removing,
                has_removal_timing: false,
            } if surface_id == "invalid-removing-timing"
        )));

        Ok(())
    }
}
