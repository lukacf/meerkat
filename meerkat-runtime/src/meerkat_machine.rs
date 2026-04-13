//! Internal diagnostic projection for the evolving MeerkatMachine boundary.
//!
//! This module does not define the final MeerkatMachine reducer. It provides a
//! small, explicit state projection over the current runtime spine so the
//! architecture work can be verified against the existing codebase while the
//! refactor proceeds in safe slices.

#![allow(dead_code)]

use std::sync::Arc;

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
}

#[derive(Debug)]
pub(crate) enum MeerkatMachineSessionCommandResult {
    Unit,
    Bool(bool),
    OpsLifecycleRegistry(Option<Arc<crate::ops_lifecycle::RuntimeOpsLifecycleRegistry>>),
    Bindings(meerkat_core::SessionRuntimeBindings),
    InputState(Option<InputState>),
    ActiveInputs(Vec<InputId>),
}

/// Public comms-drain mutations that now route through the Meerkat machine
/// instead of a wrapper/helper split.
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
pub(crate) enum MeerkatMachineDrainLocalCommand {
    AbortAll,
    Abort { session_id: SessionId },
    Wait { session_id: SessionId },
}

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

pub(crate) enum MeerkatMachineLegacyRunCommandResult {
    Prepared(MeerkatMachineLegacyRunPrepared),
    Unit,
}

#[derive(Debug)]
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
/// This keeps the supporting carrier explicit while the MeerkatMachine mapping
/// work verifies that admission and terminalization stay aligned with waiter
/// plumbing.
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
