//! Internal diagnostic projection for the evolving MeerkatMachine boundary.
//!
//! This module does not define the final MeerkatMachine reducer. It provides a
//! small, explicit state projection over the current runtime spine so the
//! architecture work can be verified against the existing codebase while the
//! refactor proceeds in safe slices.

#![allow(dead_code)]

use meerkat_core::RuntimeEpochId;
use meerkat_core::lifecycle::WaitRequestId;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::ops::OperationId;
use meerkat_core::ops_lifecycle::OperationLifecycleSnapshot;
use meerkat_core::types::HandlingMode;
use meerkat_core::types::SessionId;
use meerkat_core::{CommsDrainMode, CommsDrainPhase};

use crate::identifiers::LogicalRuntimeId;
use crate::input_state::InputLifecycleState;
use crate::input_state::InputTerminalOutcome;
use crate::runtime_ingress_authority::{ContentShape, IngressPhase, RequestId, ReservationKey};
use crate::runtime_state::RuntimeState;

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
