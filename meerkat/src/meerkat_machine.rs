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
    DestroyedRuntimeStillHasCompletionWaiters {
        waiter_count: usize,
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

    if control.current_run_id.is_some()
        && !matches!(control.phase, RuntimeState::Running | RuntimeState::Retired)
    {
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

    if control.phase == RuntimeState::Destroyed && completion_waiters.waiter_count > 0 {
        violations.push(
            MeerkatMachineInvariantViolation::DestroyedRuntimeStillHasCompletionWaiters {
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
    use futures::stream;
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest};
    #[cfg(feature = "comms")]
    use meerkat_comms::Keypair;
    use meerkat_core::DeferredPromptPolicy;
    #[cfg(feature = "comms")]
    use meerkat_core::PlainEventSource;
    use meerkat_core::agent::CommsRuntime;
    use meerkat_core::comms::TrustedPeerSpec;
    use meerkat_core::comms_drain_lifecycle_authority::CommsDrainPhase;
    use meerkat_core::lifecycle::{InputId, RunId, WaitRequestId};
    use meerkat_core::ops::OperationId;
    use meerkat_core::ops_lifecycle::{
        OperationKind, OperationLifecycleSnapshot, OperationPeerHandle, OperationStatus,
    };
    use meerkat_core::service::{CreateSessionRequest, InitialTurnPolicy, SessionBuildOptions};
    #[cfg(feature = "comms")]
    use meerkat_core::types::HandlingMode;
    #[cfg(feature = "mcp")]
    use meerkat_core::{
        ExternalToolSurfaceBaseState, ExternalToolSurfaceDeltaOperation,
        ExternalToolSurfaceDeltaPhase, ExternalToolSurfaceGlobalPhase,
        ExternalToolSurfacePendingOp, ExternalToolSurfaceStagedOp, McpServerConfig,
    };
    use meerkat_runtime::runtime_state::RuntimeState;
    use meerkat_runtime::{
        Input, InputLifecycleState, InputTerminalOutcome, MeerkatAdmittedInputSnapshot,
        MeerkatCompletionWaiterSnapshot, PromptInput,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
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
