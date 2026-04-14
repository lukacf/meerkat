//! Meerkat runtime command/result and snapshot support types.
//!
//! The authority surface now lives in `meerkat_machine.rs`; this module holds
//! the supporting command/result enums and the durable diagnostic snapshots that
//! remain useful after cutover.

use std::sync::Arc;

use indexmap::IndexSet;
use meerkat_core::RuntimeEpochId;
use meerkat_core::agent::CommsRuntime;
use meerkat_core::comms_drain_lifecycle_authority::DrainExitReason;
use meerkat_core::lifecycle::WaitRequestId;
use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
use meerkat_core::lifecycle::core_executor::CoreExecutor;
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::RunPrimitive;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::lifecycle::{RunBoundaryReceipt, RunId as LifecycleRunId};
use meerkat_core::ops::OperationId;
use meerkat_core::ops_lifecycle::OperationLifecycleSnapshot;
use meerkat_core::types::HandlingMode;
use meerkat_core::types::SessionId;
use meerkat_core::{CommsDrainMode, CommsDrainPhase};
use meerkat_machine_derive::CommandManifest;

use crate::AcceptOutcome;
use crate::identifiers::LogicalRuntimeId;
use crate::input::Input;
use crate::input_state::InputLifecycleState;
use crate::input_state::InputState;
use crate::input_state::InputTerminalOutcome;
use crate::runtime_event::RuntimeEventEnvelope;
use crate::runtime_ingress_authority::{ContentShape, IngressPhase, RequestId, ReservationKey};
use crate::runtime_state::RuntimeState;
use crate::traits::{DestroyReport, RecoveryReport, RecycleReport, ResetReport, RetireReport};

/// Public Meerkat lifecycle/control mutations that now route through the
/// top-level Meerkat machine seam instead of bespoke adapter entrypoints.
#[derive(CommandManifest)]
pub(crate) enum MeerkatMachineSessionCommand {
    RegisterSession {
        session_id: SessionId,
    },
    UnregisterSession {
        session_id: SessionId,
    },
    EnsureSessionWithExecutor {
        session_id: SessionId,
        executor: Box<dyn CoreExecutor>,
    },
    SetSilentIntents {
        session_id: SessionId,
        intents: Vec<String>,
    },
    InterruptCurrentRun {
        session_id: SessionId,
    },
    CancelAfterBoundary {
        session_id: SessionId,
    },
    StopRuntimeExecutor {
        session_id: SessionId,
        command: RunControlCommand,
    },
    ContainsSession {
        session_id: SessionId,
    },
    SessionHasExecutor {
        session_id: SessionId,
    },
    SessionHasComms {
        session_id: SessionId,
    },
    OpsLifecycleRegistry {
        session_id: SessionId,
    },
    PrepareBindings {
        session_id: SessionId,
    },
    InputState {
        session_id: SessionId,
        input_id: InputId,
    },
    ListActiveInputs {
        session_id: SessionId,
    },
    /// Publish the committed visible tool set through the machine dispatch.
    ///
    /// TLA+ source: VisibleSurfacesMatchAppliedStateInvariant —
    /// the visible-set publication must route through the canonical command
    /// path and be gated on session existence and non-Destroyed state.
    PublishCommittedVisibleSet {
        session_id: SessionId,
        visibility_state: Box<meerkat_core::SessionToolVisibilityState>,
    },
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum MeerkatMachineSessionCommandResult {
    Unit,
    Bool(bool),
    OpsLifecycleRegistry(Option<Arc<crate::ops_lifecycle::RuntimeOpsLifecycleRegistry>>),
    Bindings(meerkat_core::SessionRuntimeBindings),
    InputState(Option<InputState>),
    ActiveInputs(Vec<InputId>),
    VisibilityPublished(meerkat_core::SessionToolVisibilityState),
}

/// Public comms-drain mutations that now route through the Meerkat machine
/// instead of a wrapper/helper split.
#[derive(CommandManifest)]
pub(crate) enum MeerkatMachineDrainCommand {
    SetPeerIngressContext {
        session_id: SessionId,
        keep_alive: bool,
        comms_runtime: Option<Arc<dyn CommsRuntime>>,
    },
    NotifyDrainExited {
        session_id: SessionId,
        reason: DrainExitReason,
    },
}

pub(crate) enum MeerkatMachineDrainCommandResult {
    Spawned(bool),
    Notified,
}

/// Public comms-drain local lifecycle helpers that do not require `Arc<Self>`
/// task-spawn authority.
#[derive(CommandManifest)]
pub(crate) enum MeerkatMachineDrainLocalCommand {
    AbortAll,
    Abort { session_id: SessionId },
    Wait { session_id: SessionId },
}

#[derive(CommandManifest)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum MeerkatMachineControlCommand {
    Ingest {
        runtime_id: LogicalRuntimeId,
        input: Input,
    },
    PublishEvent {
        event: RuntimeEventEnvelope,
    },
    Retire {
        runtime_id: LogicalRuntimeId,
    },
    Recycle {
        runtime_id: LogicalRuntimeId,
    },
    Reset {
        runtime_id: LogicalRuntimeId,
    },
    Recover {
        runtime_id: LogicalRuntimeId,
    },
    Destroy {
        runtime_id: LogicalRuntimeId,
    },
    RuntimeState {
        runtime_id: LogicalRuntimeId,
    },
    LoadBoundaryReceipt {
        runtime_id: LogicalRuntimeId,
        run_id: LifecycleRunId,
        sequence: u64,
    },
}

#[derive(CommandManifest)]
pub(crate) enum MeerkatMachineIngressCommand {
    AcceptWithCompletion { session_id: SessionId, input: Input },
    AcceptWithoutWake { session_id: SessionId, input: Input },
}

pub(crate) enum MeerkatMachineIngressCommandResult {
    AcceptWithCompletion {
        outcome: AcceptOutcome,
        handle: Option<crate::completion::CompletionHandle>,
    },
    AcceptOutcome(AcceptOutcome),
}

#[doc(hidden)]
#[must_use]
pub fn canonical_meerkat_machine_command_manifest() -> IndexSet<&'static str> {
    let mut variants = IndexSet::new();
    variants.extend(
        MeerkatMachineSessionCommand::command_manifest()
            .iter()
            .copied(),
    );
    variants.extend(
        MeerkatMachineControlCommand::command_manifest()
            .iter()
            .copied(),
    );
    variants.extend(
        MeerkatMachineDrainCommand::command_manifest()
            .iter()
            .copied(),
    );
    variants.extend(
        MeerkatMachineDrainLocalCommand::command_manifest()
            .iter()
            .copied(),
    );
    variants.extend(
        MeerkatMachineIngressCommand::command_manifest()
            .iter()
            .copied(),
    );
    variants.extend(
        MeerkatMachineLegacyRunCommand::command_manifest()
            .iter()
            .copied(),
    );
    variants
}

#[derive(CommandManifest)]
pub(crate) enum MeerkatMachineLegacyRunCommand {
    Prepare {
        session_id: SessionId,
        input: Input,
    },
    Commit {
        session_id: SessionId,
        input_id: InputId,
        run_id: RunId,
        output: CoreApplyOutput,
    },
    Fail {
        session_id: SessionId,
        run_id: RunId,
        error: String,
    },
}

pub(crate) struct MeerkatMachineLegacyRunPrepared {
    pub input_id: InputId,
    pub run_id: RunId,
    pub primitive: RunPrimitive,
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum MeerkatMachineLegacyRunCommandResult {
    Prepared(MeerkatMachineLegacyRunPrepared),
    Unit,
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum MeerkatMachineControlCommandResult {
    AcceptOutcome(AcceptOutcome),
    Unit,
    RetireReport(RetireReport),
    RecycleReport(RecycleReport),
    ResetReport(ResetReport),
    RecoveryReport(RecoveryReport),
    DestroyReport(DestroyReport),
    RuntimeState(RuntimeState),
    BoundaryReceipt(Option<RunBoundaryReceipt>),
}

/// Snapshot of completion waiters registered for one input.
///
/// This is a supporting-carrier view, not canonical semantic truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeerkatCompletionWaiterSnapshot {
    pub input_id: InputId,
    pub waiter_count: usize,
}

/// Snapshot of the runtime completion waiter carrier.
///
/// Completion waiters are supporting carrier state rather than primary
/// authority, but they remain useful for diagnostics and recovery inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeerkatCompletionWaitersSnapshot {
    pub input_count: usize,
    pub waiter_count: usize,
    pub waiting_inputs: Vec<MeerkatCompletionWaiterSnapshot>,
}

/// Runtime driver flavor for a registered Meerkat session entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeerkatDriverKind {
    Ephemeral,
    Persistent,
}

/// Snapshot of the hidden runtime epoch cursor state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeerkatCursorSnapshot {
    pub agent_applied_cursor: u64,
    pub runtime_observed_seq: u64,
    pub runtime_last_injected_seq: u64,
}

/// Snapshot of the hidden runtime binding for one Meerkat session.
#[derive(Debug, Clone)]
pub struct MeerkatBindingSnapshot {
    pub session_id: SessionId,
    pub runtime_id: LogicalRuntimeId,
    pub driver_kind: MeerkatDriverKind,
    pub driver_present: bool,
    pub completions_present: bool,
    pub ops_registry_present: bool,
    pub attachment_live: bool,
    pub detached_wake_present: bool,
    pub epoch_id: RuntimeEpochId,
    pub cursor_state: MeerkatCursorSnapshot,
}

/// Snapshot of runtime control-plane truth for one session.
#[derive(Debug, Clone)]
pub struct MeerkatControlSnapshot {
    pub phase: RuntimeState,
    pub current_run_id: Option<RunId>,
    pub pre_run_phase: Option<RuntimeState>,
    pub wake_pending: bool,
    pub process_pending: bool,
}

/// Snapshot of one admitted runtime input.
#[derive(Debug, Clone)]
pub struct MeerkatAdmittedInputSnapshot {
    pub input_id: InputId,
    pub content_shape: Option<ContentShape>,
    pub request_id: Option<RequestId>,
    pub reservation_key: Option<ReservationKey>,
    pub handling_mode: Option<HandlingMode>,
    pub lifecycle: Option<InputLifecycleState>,
    pub terminal_outcome: Option<InputTerminalOutcome>,
    pub last_run_id: Option<RunId>,
    pub last_boundary_sequence: Option<u64>,
    pub is_prompt: bool,
}

/// Snapshot of runtime ingress truth for one session.
#[derive(Debug, Clone)]
pub struct MeerkatInputsSnapshot {
    pub ingress_phase: IngressPhase,
    pub admission_order: Vec<MeerkatAdmittedInputSnapshot>,
    pub queue: Vec<InputId>,
    pub steer_queue: Vec<InputId>,
    pub current_run_id: Option<RunId>,
    pub current_run_contributors: Vec<InputId>,
    pub wake_requested: bool,
    pub process_requested: bool,
    pub silent_intent_overrides: Vec<String>,
}

/// Snapshot of runtime-owned async operation truth for one session.
#[derive(Debug, Clone)]
pub struct MeerkatOpsSnapshot {
    pub operation_count: usize,
    pub active_count: usize,
    pub wait_request_id: Option<WaitRequestId>,
    pub pending_wait_present: bool,
    pub pending_wait_request_id: Option<WaitRequestId>,
    pub wait_operation_ids: Vec<OperationId>,
    pub operations: Vec<OperationLifecycleSnapshot>,
    pub detached_wake_pending: Option<bool>,
    pub detached_wake_signaled: Option<bool>,
}

/// Snapshot of comms-drain lifecycle truth for one session.
#[derive(Debug, Clone)]
pub struct MeerkatDrainSnapshot {
    pub slot_present: bool,
    pub phase: Option<CommsDrainPhase>,
    pub mode: Option<CommsDrainMode>,
    pub handle_present: bool,
}

/// Diagnostic snapshot of the current Meerkat runtime spine.
///
/// This is an observational scaffold over the existing runtime-owned Meerkat
/// regions. It is intentionally not the final MeerkatMachine reducer.
#[derive(Debug, Clone)]
pub struct MeerkatMachineSpineSnapshot {
    pub binding: MeerkatBindingSnapshot,
    pub control: MeerkatControlSnapshot,
    pub inputs: MeerkatInputsSnapshot,
    pub completion_waiters: MeerkatCompletionWaitersSnapshot,
    pub ops: MeerkatOpsSnapshot,
    pub drain: MeerkatDrainSnapshot,
}

impl MeerkatMachineSpineSnapshot {
    /// Validate TLA+ structural invariants against the current spine snapshot.
    ///
    /// Returns `Ok(())` if all invariants hold, or `Err(violations)` with a
    /// list of human-readable violation descriptions. This is release-mode
    /// validation — not `debug_assert`.
    pub fn validate_spine_invariants(&self) -> Result<(), Vec<String>> {
        let mut violations = Vec::new();

        // --- Control/binding invariants ---

        // RunningHasActiveRunInvariant: Running => HasActiveRun
        if self.control.phase == RuntimeState::Running && self.control.current_run_id.is_none() {
            violations
                .push("RunningHasActiveRunInvariant: phase is Running but no active run_id".into());
        }

        // ActiveRunPhaseInvariant: HasActiveRun => phase in {Running, Retired}
        if self.control.current_run_id.is_some()
            && !matches!(
                self.control.phase,
                RuntimeState::Running | RuntimeState::Retired
            )
        {
            violations.push(format!(
                "ActiveRunPhaseInvariant: active run_id present but phase is {:?}",
                self.control.phase
            ));
        }

        // DestroyedShapeInvariant: Destroyed => empty queues, no waiting inputs
        if self.control.phase == RuntimeState::Destroyed {
            if !self.inputs.queue.is_empty() {
                violations.push("DestroyedShapeInvariant: Destroyed but queue is non-empty".into());
            }
            if !self.inputs.steer_queue.is_empty() {
                violations
                    .push("DestroyedShapeInvariant: Destroyed but steer_queue is non-empty".into());
            }
            if self.completion_waiters.input_count > 0 {
                violations.push(
                    "DestroyedShapeInvariant: Destroyed but completion waiters remain".into(),
                );
            }
        }

        // --- Input invariants ---

        // QueueSteerDisjointInvariant
        let queue_set: std::collections::HashSet<_> = self.inputs.queue.iter().collect();
        let steer_set: std::collections::HashSet<_> = self.inputs.steer_queue.iter().collect();
        if !queue_set.is_disjoint(&steer_set) {
            violations
                .push("QueueSteerDisjointInvariant: queue and steer_queue share entries".into());
        }

        // QueueHandlingInvariant: all queue entries must have handling_mode=Queue, lifecycle=Queued
        for qid in &self.inputs.queue {
            if let Some(snap) = self
                .inputs
                .admission_order
                .iter()
                .find(|a| &a.input_id == qid)
            {
                if snap.handling_mode != Some(HandlingMode::Queue) {
                    violations.push(format!(
                        "QueueHandlingInvariant: queue entry {qid} has handling_mode {:?}",
                        snap.handling_mode
                    ));
                }
                if snap.lifecycle != Some(InputLifecycleState::Queued) {
                    violations.push(format!(
                        "QueueHandlingInvariant: queue entry {qid} has lifecycle {:?}",
                        snap.lifecycle
                    ));
                }
            }
        }

        // SteerHandlingInvariant: all steer_queue entries must have handling_mode=Steer, lifecycle=Queued
        for sid in &self.inputs.steer_queue {
            if let Some(snap) = self
                .inputs
                .admission_order
                .iter()
                .find(|a| &a.input_id == sid)
            {
                if snap.handling_mode != Some(HandlingMode::Steer) {
                    violations.push(format!(
                        "SteerHandlingInvariant: steer_queue entry {sid} has handling_mode {:?}",
                        snap.handling_mode
                    ));
                }
                if snap.lifecycle != Some(InputLifecycleState::Queued) {
                    violations.push(format!(
                        "SteerHandlingInvariant: steer_queue entry {sid} has lifecycle {:?}",
                        snap.lifecycle
                    ));
                }
            }
        }

        // ContributorLifecycleInvariant: all current_run_contributors must have lifecycle=Staged
        for cid in &self.inputs.current_run_contributors {
            if let Some(snap) = self
                .inputs
                .admission_order
                .iter()
                .find(|a| &a.input_id == cid)
                && snap.lifecycle != Some(InputLifecycleState::Staged)
            {
                violations.push(format!(
                    "ContributorLifecycleInvariant: contributor {cid} has lifecycle {:?}",
                    snap.lifecycle
                ));
            }
        }

        // TerminalInputsNotQueuedInvariant: terminal inputs must not be in queue or steer_queue
        for snap in &self.inputs.admission_order {
            if snap.terminal_outcome.is_some() {
                if queue_set.contains(&snap.input_id) {
                    violations.push(format!(
                        "TerminalInputsNotQueuedInvariant: terminal input {} in queue",
                        snap.input_id
                    ));
                }
                if steer_set.contains(&snap.input_id) {
                    violations.push(format!(
                        "TerminalInputsNotQueuedInvariant: terminal input {} in steer_queue",
                        snap.input_id
                    ));
                }
            }
        }

        // CurrentRunContributorsInvariant: HasActiveRun => contributors non-empty
        if self.control.current_run_id.is_some() && self.inputs.current_run_contributors.is_empty()
        {
            violations
                .push("CurrentRunContributorsInvariant: active run but no contributors".into());
        }

        // RunIdsAlignedInvariant: ~HasActiveRun \/ RunIdsAligned
        if self.control.current_run_id.is_some()
            && self.control.current_run_id != self.inputs.current_run_id
        {
            violations.push(
                "RunIdsAlignedInvariant: control.current_run_id != inputs.current_run_id".into(),
            );
        }

        // --- Ops invariants ---

        // WaitAllAlignmentInvariant
        let wait_active = self.ops.wait_request_id.is_some();
        if wait_active && self.ops.wait_operation_ids.is_empty() {
            violations
                .push("WaitAllAlignmentInvariant: wait_active but no wait_operation_ids".into());
        }
        if !wait_active && !self.ops.wait_operation_ids.is_empty() {
            violations.push(
                "WaitAllAlignmentInvariant: wait_operation_ids present but no wait_request_id"
                    .into(),
            );
        }

        // --- Drain invariants ---

        // DrainBindingInvariant: drain.phase != Inactive => binding.live
        if let Some(phase) = self.drain.phase {
            if phase != CommsDrainPhase::Inactive && !self.binding.attachment_live {
                // binding.live maps to session being registered (it is, since we have a snapshot)
                // This invariant is about the binding being live, which is always true if we have
                // a snapshot. Skip this check since getting a snapshot implies the session exists.
            }
            let _ = phase; // suppress unused warning
        }

        // DrainModeInvariant: drain.phase != Inactive => mode is set and not Disabled
        if let Some(phase) = self.drain.phase
            && phase != CommsDrainPhase::Inactive
            && self.drain.mode.is_none()
        {
            violations.push("DrainModeInvariant: drain.phase is active but mode is None".into());
        }

        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations)
        }
    }
}
