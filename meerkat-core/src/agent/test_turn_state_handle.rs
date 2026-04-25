//! Test-only in-process `TurnStateHandle` implementation for `meerkat-core`
//! tests.
//!
//! ## Why this exists
//!
//! Wave-A deleted the standalone in-core turn-state fallback
//! (`LocalTurnExecutionState`) that previously backed every agent loop when
//! no runtime handle was attached. All production surfaces now obtain a real
//! DSL-backed [`RuntimeTurnStateHandle`] through
//! `MeerkatMachine::prepare_bindings`, so the `turn_state_handle` slot on
//! `Agent` is always `Some` in production.
//!
//! Tests inside `meerkat-core` cannot reach for the production
//! `RuntimeTurnStateHandle` because `meerkat-runtime` depends on
//! `meerkat-core` (circular dev-dependency instantiates a second copy of
//! `meerkat-core` and the trait impls do not unify across the two copies).
//!
//! This module ports the deleted `LocalTurnExecutionState` phase logic and
//! wraps it in a [`TurnStateHandle`] so core tests can exercise the full
//! agent loop against a phase-tracking handle. The adapter shape mirrors
//! `apply_turn_input_via_runtime_handle` in `agent::state`: each trait
//! method builds the corresponding `TurnExecutionInput` and drives the
//! internal state machine via `apply`.
//!
//! See #32 Class W1.
//!
//! The transition logic here is a literal port of the pre-wave-a
//! `meerkat-core/src/agent/turn_state.rs` (deleted in `fdb569aaf`); the
//! DSL-backed `meerkat-runtime` handle is the real authority in every
//! live surface.

use std::collections::{BTreeSet, HashSet};
use std::sync::{Mutex, MutexGuard};

use crate::handles::{DslTransitionError, TurnStateHandle, TurnStateSnapshot};
use crate::lifecycle::RunId;
use crate::ops::OperationId;
use crate::turn_execution_authority::{
    ContentShape, TurnExecutionInput, TurnPhase, TurnPrimitiveKind, TurnTerminalOutcome,
};

#[derive(Debug, Clone)]
struct LocalState {
    phase: TurnPhase,
    fields: LocalFields,
}

#[derive(Debug, Clone)]
struct LocalFields {
    active_run: Option<RunId>,
    primitive_kind: TurnPrimitiveKind,
    admitted_content_shape: Option<ContentShape>,
    vision_enabled: bool,
    image_tool_results_enabled: bool,
    tool_calls_pending: u32,
    pending_op_ids: Vec<OperationId>,
    barrier_operation_ids: Vec<OperationId>,
    has_barrier_ops: bool,
    barrier_satisfied: bool,
    boundary_count: u32,
    cancel_after_boundary: bool,
    terminal_outcome: TurnTerminalOutcome,
    extraction_attempts: u32,
    max_extraction_retries: u32,
}

impl LocalFields {
    fn init() -> Self {
        Self {
            active_run: None,
            primitive_kind: TurnPrimitiveKind::None,
            admitted_content_shape: None,
            vision_enabled: false,
            image_tool_results_enabled: false,
            tool_calls_pending: 0,
            pending_op_ids: Vec::new(),
            barrier_operation_ids: Vec::new(),
            has_barrier_ops: false,
            barrier_satisfied: true,
            boundary_count: 0,
            cancel_after_boundary: false,
            terminal_outcome: TurnTerminalOutcome::None,
            extraction_attempts: 0,
            max_extraction_retries: 0,
        }
    }

    fn reset(&mut self) {
        *self = Self::init();
    }
}

impl LocalState {
    fn new() -> Self {
        Self {
            phase: TurnPhase::Ready,
            fields: LocalFields::init(),
        }
    }

    fn guard_run_matches(&self, run_id: &RunId) -> bool {
        self.fields.active_run.as_ref() == Some(run_id)
    }

    fn barrier_operation_ids_match(&self, operation_ids: &[OperationId]) -> bool {
        let expected = self
            .fields
            .barrier_operation_ids
            .iter()
            .cloned()
            .collect::<HashSet<_>>();
        let actual = operation_ids.iter().cloned().collect::<HashSet<_>>();
        expected.len() == operation_ids.len() && expected == actual
    }

    fn apply(&mut self, input: TurnExecutionInput) -> Result<(), DslTransitionError> {
        use TurnExecutionInput::{
            AcknowledgeTerminal, BoundaryComplete, BoundaryContinue, BudgetExhausted,
            CancelAfterBoundary, CancelNow, CancellationObserved, EnterExtraction, ExtractionStart,
            ExtractionValidationFailed, ExtractionValidationPassed, FatalFailure, ForceCancelNoRun,
            LlmReturnedTerminal, LlmReturnedToolCalls, OpsBarrierSatisfied, PrimitiveApplied,
            RecoverableFailure, RegisterPendingOps, RetryRequested, StartConversationRun,
            StartImmediateAppend, StartImmediateContext, TimeBudgetExceeded, ToolCallsResolved,
            TurnLimitReached,
        };
        use TurnPhase::{
            ApplyingPrimitive, CallingLlm, Cancelled, Cancelling, Completed, DrainingBoundary,
            ErrorRecovery, Extracting, Failed, Ready, WaitingForOps,
        };

        let phase = self.phase;
        let mut fields = self.fields.clone();

        let next_phase = match (phase, &input) {
            (Ready | Completed | Failed | Cancelled, StartConversationRun { run_id }) => {
                fields = LocalFields::init();
                fields.active_run = Some(run_id.clone());
                fields.primitive_kind = TurnPrimitiveKind::ConversationTurn;
                ApplyingPrimitive
            }
            (Ready | Completed | Failed | Cancelled, StartImmediateAppend { run_id }) => {
                fields = LocalFields::init();
                fields.active_run = Some(run_id.clone());
                fields.primitive_kind = TurnPrimitiveKind::ImmediateAppend;
                ApplyingPrimitive
            }
            (Ready | Completed | Failed | Cancelled, StartImmediateContext { run_id }) => {
                fields = LocalFields::init();
                fields.active_run = Some(run_id.clone());
                fields.primitive_kind = TurnPrimitiveKind::ImmediateContextAppend;
                ApplyingPrimitive
            }
            (
                ApplyingPrimitive,
                PrimitiveApplied {
                    run_id,
                    admitted_content_shape,
                    vision_enabled,
                    image_tool_results_enabled,
                },
            ) => {
                if !self.guard_run_matches(run_id) {
                    return Err(invalid(phase, &input));
                }
                fields.admitted_content_shape = Some(admitted_content_shape.clone());
                fields.vision_enabled = *vision_enabled;
                fields.image_tool_results_enabled = *image_tool_results_enabled;
                match fields.primitive_kind {
                    TurnPrimitiveKind::ConversationTurn => CallingLlm,
                    TurnPrimitiveKind::ImmediateAppend
                    | TurnPrimitiveKind::ImmediateContextAppend => {
                        fields.boundary_count += 1;
                        if fields.cancel_after_boundary {
                            fields.cancel_after_boundary = false;
                            fields.terminal_outcome = TurnTerminalOutcome::Cancelled;
                            Cancelled
                        } else {
                            fields.terminal_outcome = TurnTerminalOutcome::Completed;
                            Completed
                        }
                    }
                    TurnPrimitiveKind::None => return Err(invalid(phase, &input)),
                }
            }
            (CallingLlm, LlmReturnedToolCalls { run_id, tool_count }) => {
                if !self.guard_run_matches(run_id) || *tool_count == 0 {
                    return Err(invalid(phase, &input));
                }
                fields.tool_calls_pending = *tool_count;
                fields.pending_op_ids.clear();
                fields.barrier_operation_ids.clear();
                fields.has_barrier_ops = false;
                fields.barrier_satisfied = true;
                WaitingForOps
            }
            (CallingLlm, LlmReturnedTerminal { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(invalid(phase, &input));
                }
                fields.boundary_count += 1;
                DrainingBoundary
            }
            (
                WaitingForOps,
                RegisterPendingOps {
                    run_id,
                    op_refs,
                    barrier_operation_ids,
                    has_barrier_ops,
                },
            ) => {
                if !self.guard_run_matches(run_id) || fields.tool_calls_pending == 0 {
                    return Err(invalid(phase, &input));
                }
                fields.pending_op_ids = op_refs.iter().map(|r| r.operation_id.clone()).collect();
                fields.barrier_operation_ids = barrier_operation_ids.clone();
                fields.has_barrier_ops = *has_barrier_ops;
                fields.barrier_satisfied = !*has_barrier_ops;
                WaitingForOps
            }
            (
                WaitingForOps,
                OpsBarrierSatisfied {
                    run_id,
                    operation_ids,
                },
            ) => {
                if !self.guard_run_matches(run_id)
                    || fields.barrier_satisfied
                    || !self.barrier_operation_ids_match(operation_ids)
                {
                    return Err(invalid(phase, &input));
                }
                fields.barrier_satisfied = true;
                WaitingForOps
            }
            (WaitingForOps, ToolCallsResolved { run_id }) => {
                // Relaxed vs. the deleted `LocalTurnExecutionState`: the test
                // stub does not require `pending_op_ids` to be non-empty —
                // agent-loop tests that exercise tool calls without going
                // through `RegisterPendingOps` should still advance.
                if !self.guard_run_matches(run_id) || !fields.barrier_satisfied {
                    return Err(invalid(phase, &input));
                }
                fields.tool_calls_pending = 0;
                fields.pending_op_ids.clear();
                fields.barrier_operation_ids.clear();
                fields.has_barrier_ops = false;
                fields.barrier_satisfied = true;
                fields.boundary_count += 1;
                DrainingBoundary
            }
            (DrainingBoundary, BoundaryContinue { run_id }) => {
                if !self.guard_run_matches(run_id)
                    || fields.primitive_kind != TurnPrimitiveKind::ConversationTurn
                {
                    return Err(invalid(phase, &input));
                }
                if fields.cancel_after_boundary {
                    fields.cancel_after_boundary = false;
                    fields.terminal_outcome = TurnTerminalOutcome::Cancelled;
                    Cancelled
                } else {
                    CallingLlm
                }
            }
            (DrainingBoundary, BoundaryComplete { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(invalid(phase, &input));
                }
                if fields.cancel_after_boundary {
                    fields.cancel_after_boundary = false;
                    fields.terminal_outcome = TurnTerminalOutcome::Cancelled;
                    Cancelled
                } else {
                    fields.terminal_outcome = TurnTerminalOutcome::Completed;
                    Completed
                }
            }
            (
                DrainingBoundary,
                EnterExtraction {
                    run_id,
                    max_retries,
                },
            ) => {
                if !self.guard_run_matches(run_id) {
                    return Err(invalid(phase, &input));
                }
                fields.max_extraction_retries = *max_retries;
                Extracting
            }
            (Extracting, ExtractionValidationPassed { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(invalid(phase, &input));
                }
                fields.terminal_outcome = TurnTerminalOutcome::Completed;
                Completed
            }
            (Extracting, ExtractionStart { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(invalid(phase, &input));
                }
                CallingLlm
            }
            (Extracting, ExtractionValidationFailed { run_id, .. }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(invalid(phase, &input));
                }
                fields.extraction_attempts += 1;
                if fields.extraction_attempts < fields.max_extraction_retries {
                    CallingLlm
                } else {
                    fields.terminal_outcome = TurnTerminalOutcome::StructuredOutputValidationFailed;
                    Failed
                }
            }
            (CallingLlm | WaitingForOps | DrainingBoundary, RecoverableFailure { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(invalid(phase, &input));
                }
                if matches!(phase, WaitingForOps) {
                    fields.pending_op_ids.clear();
                    fields.barrier_operation_ids.clear();
                    fields.has_barrier_ops = false;
                    fields.barrier_satisfied = true;
                }
                ErrorRecovery
            }
            (ErrorRecovery, RetryRequested { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(invalid(phase, &input));
                }
                CallingLlm
            }
            (
                ApplyingPrimitive | CallingLlm | WaitingForOps | DrainingBoundary | Extracting
                | ErrorRecovery,
                FatalFailure { run_id },
            ) => {
                if !self.guard_run_matches(run_id) {
                    return Err(invalid(phase, &input));
                }
                if matches!(phase, WaitingForOps) {
                    fields.pending_op_ids.clear();
                    fields.barrier_operation_ids.clear();
                    fields.has_barrier_ops = false;
                    fields.barrier_satisfied = true;
                }
                fields.terminal_outcome = TurnTerminalOutcome::Failed;
                Failed
            }
            (
                ApplyingPrimitive | CallingLlm | WaitingForOps | DrainingBoundary | Extracting
                | ErrorRecovery,
                CancelNow { run_id },
            ) => {
                if !self.guard_run_matches(run_id) {
                    return Err(invalid(phase, &input));
                }
                if matches!(phase, WaitingForOps) {
                    fields.pending_op_ids.clear();
                    fields.barrier_operation_ids.clear();
                    fields.has_barrier_ops = false;
                    fields.barrier_satisfied = true;
                }
                Cancelling
            }
            (
                ApplyingPrimitive | CallingLlm | WaitingForOps | DrainingBoundary | Extracting
                | ErrorRecovery,
                CancelAfterBoundary { run_id },
            ) => {
                if !self.guard_run_matches(run_id) {
                    return Err(invalid(phase, &input));
                }
                fields.cancel_after_boundary = true;
                phase
            }
            (Cancelling, CancellationObserved { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(invalid(phase, &input));
                }
                fields.terminal_outcome = TurnTerminalOutcome::Cancelled;
                fields.cancel_after_boundary = false;
                Cancelled
            }
            (
                ApplyingPrimitive | CallingLlm | WaitingForOps | DrainingBoundary | Extracting
                | ErrorRecovery,
                TurnLimitReached { run_id },
            ) => {
                if !self.guard_run_matches(run_id) {
                    return Err(invalid(phase, &input));
                }
                if matches!(phase, WaitingForOps) {
                    fields.pending_op_ids.clear();
                    fields.barrier_operation_ids.clear();
                    fields.has_barrier_ops = false;
                    fields.barrier_satisfied = true;
                }
                fields.boundary_count += 1;
                fields.terminal_outcome = TurnTerminalOutcome::Completed;
                Completed
            }
            (
                ApplyingPrimitive | CallingLlm | WaitingForOps | DrainingBoundary | Extracting
                | ErrorRecovery,
                BudgetExhausted { run_id },
            ) => {
                if !self.guard_run_matches(run_id) {
                    return Err(invalid(phase, &input));
                }
                if matches!(phase, WaitingForOps) {
                    fields.pending_op_ids.clear();
                    fields.barrier_operation_ids.clear();
                    fields.has_barrier_ops = false;
                    fields.barrier_satisfied = true;
                }
                fields.boundary_count += 1;
                fields.terminal_outcome = TurnTerminalOutcome::BudgetExhausted;
                Completed
            }
            (
                ApplyingPrimitive | CallingLlm | WaitingForOps | DrainingBoundary | Extracting
                | ErrorRecovery,
                TimeBudgetExceeded { run_id },
            ) => {
                if !self.guard_run_matches(run_id) {
                    return Err(invalid(phase, &input));
                }
                if matches!(phase, WaitingForOps) {
                    fields.pending_op_ids.clear();
                    fields.barrier_operation_ids.clear();
                    fields.has_barrier_ops = false;
                    fields.barrier_satisfied = true;
                }
                fields.boundary_count += 1;
                fields.terminal_outcome = TurnTerminalOutcome::TimeBudgetExceeded;
                Completed
            }
            (
                Ready | ApplyingPrimitive | CallingLlm | WaitingForOps | DrainingBoundary
                | Extracting | ErrorRecovery | Cancelling,
                ForceCancelNoRun,
            ) => {
                if matches!(phase, WaitingForOps) {
                    fields.pending_op_ids.clear();
                    fields.barrier_operation_ids.clear();
                    fields.has_barrier_ops = false;
                    fields.barrier_satisfied = true;
                }
                fields.terminal_outcome = TurnTerminalOutcome::Cancelled;
                Cancelled
            }
            (Completed | Failed | Cancelled, AcknowledgeTerminal { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(invalid(phase, &input));
                }
                fields.reset();
                Ready
            }
            _ => return Err(invalid(phase, &input)),
        };

        self.phase = next_phase;
        self.fields = fields;
        Ok(())
    }
}

fn invalid(from: TurnPhase, input: &TurnExecutionInput) -> DslTransitionError {
    DslTransitionError::guard_rejected(
        "test-turn-state-handle",
        format!("invalid transition from {from:?} for input {input:?}"),
    )
}

/// If no run is active (or the previous run reached a terminal phase),
/// synthesize a fresh `StartConversationRun` + transition to `CallingLlm`
/// so the caller can drive a terminal transition (BudgetExhausted,
/// TurnLimitReached, etc.) from the expected phase.
///
/// Tests that directly invoke `agent.run_loop(None)` with pre-staged
/// `agent.state = LoopState::CallingLlm` never call `primitive_applied`,
/// so the stub would otherwise panic on the first terminal-routing
/// transition. This helper keeps the stub permissive for such tests.
fn ensure_active_conversation_run(state: &mut LocalState) -> Result<(), DslTransitionError> {
    let is_terminal = matches!(
        state.phase,
        TurnPhase::Completed | TurnPhase::Failed | TurnPhase::Cancelled
    );
    // RMAT-ALLOW(NoGuardedApply): this is a test-only `LocalState` stub
    // reset, not an authority apply. The conditional resets the stub's
    // synthetic state to a fresh run; the subsequent `state.apply(...)`
    // calls in this function ARE unconditional authority submits against
    // the now-reset stub. The RMAT rule flags `is_terminal`-gated branches
    // that guard `.apply()` calls — here the guard gates the stub
    // bootstrap, not the authority, so the flag is a false positive for
    // test scaffolding.
    if state.fields.active_run.is_none() || is_terminal {
        *state = LocalState::new();
    }
    if state.fields.active_run.is_none() {
        let run_id = RunId(uuid::Uuid::new_v4());
        state
            .apply(TurnExecutionInput::StartConversationRun {
                run_id: run_id.clone(),
            })
            .map_err(|err| {
                DslTransitionError::guard_rejected(
                    "test-turn-state-handle",
                    format!("synthetic StartConversationRun rejected: {err:?}"),
                )
            })?;
        state
            .apply(TurnExecutionInput::PrimitiveApplied {
                run_id,
                admitted_content_shape: ContentShape("conversation".to_string()),
                vision_enabled: false,
                image_tool_results_enabled: false,
            })
            .map_err(|err| {
                DslTransitionError::guard_rejected(
                    "test-turn-state-handle",
                    format!("synthetic PrimitiveApplied rejected: {err:?}"),
                )
            })?;
    }
    Ok(())
}

fn active_run_or_err(state: &LocalState, context: &str) -> Result<RunId, DslTransitionError> {
    state.fields.active_run.clone().ok_or_else(|| {
        DslTransitionError::guard_rejected(
            "test-turn-state-handle",
            format!("{context} without active run"),
        )
    })
}

/// Test-only `TurnStateHandle` that tracks phase via the deleted-wave-a
/// `LocalTurnExecutionState` transition logic.
#[derive(Debug)]
pub struct TestTurnStateHandle {
    state: Mutex<LocalState>,
}

impl TestTurnStateHandle {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(LocalState::new()),
        }
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, LocalState>, DslTransitionError> {
        self.state.lock().map_err(|_| {
            DslTransitionError::guard_rejected(
                "test-turn-state-handle",
                "state mutex poisoned".to_string(),
            )
        })
    }
}

impl Default for TestTurnStateHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl TurnStateHandle for TestTurnStateHandle {
    fn start_conversation_run(
        &self,
        run_id: RunId,
        primitive_kind: crate::turn_execution_authority::TurnPrimitiveKind,
        _admitted_content_shape: String,
        _vision_enabled: bool,
        _image_tool_results_enabled: bool,
        _max_extraction_retries: u64,
    ) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        match primitive_kind {
            TurnPrimitiveKind::ConversationTurn => {
                guard.apply(TurnExecutionInput::StartConversationRun { run_id })
            }
            TurnPrimitiveKind::ImmediateAppend => {
                guard.apply(TurnExecutionInput::StartImmediateAppend { run_id })
            }
            TurnPrimitiveKind::ImmediateContextAppend => {
                guard.apply(TurnExecutionInput::StartImmediateContext { run_id })
            }
            TurnPrimitiveKind::None => Err(DslTransitionError::guard_rejected(
                "test-turn-state-handle",
                "start_conversation_run with primitive_kind=None".to_string(),
            )),
        }
    }

    fn start_immediate_append(
        &self,
        run_id: RunId,
        _admitted_content_shape: String,
    ) -> Result<(), DslTransitionError> {
        self.lock_state()?
            .apply(TurnExecutionInput::StartImmediateAppend { run_id })
    }

    fn start_immediate_context(
        &self,
        run_id: RunId,
        _admitted_content_shape: String,
    ) -> Result<(), DslTransitionError> {
        self.lock_state()?
            .apply(TurnExecutionInput::StartImmediateContext { run_id })
    }

    fn primitive_applied(&self) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        // `apply_turn_input_via_runtime_handle` in `agent::state` deliberately
        // returns Ok without routing `TurnExecutionInput::StartConversationRun`
        // (and its Immediate* siblings) through the handle — the real runtime
        // DSL absorbs those Start* inputs through a separate wiring layer that
        // does not exist in `meerkat-core`. Tests therefore arrive at
        // `primitive_applied` with the stub still in `Ready`/no-active-run or
        // at a terminal phase from a previous turn. Reset terminal phases and
        // seed a synthetic `StartConversationRun` so the downstream phase
        // transitions flow; multi-turn tests (e.g. compact-between-turns) do
        // not fire `AcknowledgeTerminal` either, so we treat any non-active
        // state as "ready for a fresh run".
        let is_terminal = matches!(
            guard.phase,
            TurnPhase::Completed | TurnPhase::Failed | TurnPhase::Cancelled
        );
        // RMAT-ALLOW(NoGuardedApply): test-stub bootstrap — the
        // conditional resets the stub's synthetic state to a fresh run
        // before unconditionally submitting `StartConversationRun`. See
        // `ensure_active_conversation_run` above for the same pattern
        // and rationale.
        if guard.fields.active_run.is_none() || is_terminal {
            *guard = LocalState::new();
            let run_id = RunId(uuid::Uuid::new_v4());
            guard
                .apply(TurnExecutionInput::StartConversationRun { run_id })
                .map_err(|err| {
                    DslTransitionError::guard_rejected(
                        "test-turn-state-handle",
                        format!("synthetic StartConversationRun rejected: {err:?}"),
                    )
                })?;
        }
        let run_id = active_run_or_err(&guard, "primitive_applied")?;
        guard.apply(TurnExecutionInput::PrimitiveApplied {
            run_id,
            admitted_content_shape: ContentShape("conversation".to_string()),
            vision_enabled: false,
            image_tool_results_enabled: false,
        })
    }

    fn llm_returned_tool_calls(&self, tool_count: u64) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        let run_id = active_run_or_err(&guard, "llm_returned_tool_calls")?;
        guard.apply(TurnExecutionInput::LlmReturnedToolCalls {
            run_id,
            tool_count: u32::try_from(tool_count).unwrap_or(u32::MAX),
        })
    }

    fn llm_returned_terminal(&self) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        let run_id = active_run_or_err(&guard, "llm_returned_terminal")?;
        guard.apply(TurnExecutionInput::LlmReturnedTerminal { run_id })
    }

    fn register_pending_ops(
        &self,
        op_refs: BTreeSet<String>,
        barrier_operation_ids: BTreeSet<String>,
    ) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        let run_id = active_run_or_err(&guard, "register_pending_ops")?;
        let op_refs_vec = op_refs
            .into_iter()
            .map(|s| {
                crate::ops::AsyncOpRef::barrier(OperationId(
                    uuid::Uuid::parse_str(&s).unwrap_or_else(|_| uuid::Uuid::new_v4()),
                ))
            })
            .collect();
        let barrier_vec = barrier_operation_ids
            .into_iter()
            .map(|s| {
                OperationId(uuid::Uuid::parse_str(&s).unwrap_or_else(|_| uuid::Uuid::new_v4()))
            })
            .collect::<Vec<_>>();
        let has_barrier_ops = !barrier_vec.is_empty();
        guard.apply(TurnExecutionInput::RegisterPendingOps {
            run_id,
            op_refs: op_refs_vec,
            barrier_operation_ids: barrier_vec,
            has_barrier_ops,
        })
    }

    fn tool_calls_resolved(&self) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        let run_id = active_run_or_err(&guard, "tool_calls_resolved")?;
        guard.apply(TurnExecutionInput::ToolCallsResolved { run_id })
    }

    fn ops_barrier_satisfied(
        &self,
        operation_ids: BTreeSet<String>,
    ) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        let run_id = active_run_or_err(&guard, "ops_barrier_satisfied")?;
        let ops_vec = operation_ids
            .into_iter()
            .map(|s| {
                OperationId(uuid::Uuid::parse_str(&s).unwrap_or_else(|_| uuid::Uuid::new_v4()))
            })
            .collect::<Vec<_>>();
        guard.apply(TurnExecutionInput::OpsBarrierSatisfied {
            run_id,
            operation_ids: ops_vec,
        })
    }

    fn boundary_continue(&self) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        let run_id = active_run_or_err(&guard, "boundary_continue")?;
        guard.apply(TurnExecutionInput::BoundaryContinue { run_id })
    }

    fn boundary_complete(&self) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        let run_id = active_run_or_err(&guard, "boundary_complete")?;
        guard.apply(TurnExecutionInput::BoundaryComplete { run_id })
    }

    fn enter_extraction(&self, max_retries: u32) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        let run_id = active_run_or_err(&guard, "enter_extraction")?;
        guard.apply(TurnExecutionInput::EnterExtraction {
            run_id,
            max_retries,
        })
    }

    fn extraction_start(&self) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        let run_id = active_run_or_err(&guard, "extraction_start")?;
        guard.apply(TurnExecutionInput::ExtractionStart { run_id })
    }

    fn extraction_validation_passed(&self) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        let run_id = active_run_or_err(&guard, "extraction_validation_passed")?;
        guard.apply(TurnExecutionInput::ExtractionValidationPassed { run_id })
    }

    fn extraction_validation_failed(&self, error: String) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        let run_id = active_run_or_err(&guard, "extraction_validation_failed")?;
        guard.apply(TurnExecutionInput::ExtractionValidationFailed { run_id, error })
    }

    fn recoverable_failure(&self, _error: String) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        let run_id = active_run_or_err(&guard, "recoverable_failure")?;
        guard.apply(TurnExecutionInput::RecoverableFailure { run_id })
    }

    fn fatal_failure(&self, _error: String) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        let run_id = active_run_or_err(&guard, "fatal_failure")?;
        guard.apply(TurnExecutionInput::FatalFailure { run_id })
    }

    fn retry_requested(&self) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        let run_id = active_run_or_err(&guard, "retry_requested")?;
        guard.apply(TurnExecutionInput::RetryRequested { run_id })
    }

    fn cancel_now(&self) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        let run_id = active_run_or_err(&guard, "cancel_now")?;
        guard.apply(TurnExecutionInput::CancelNow { run_id })
    }

    fn request_cancel_after_boundary(&self) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        let run_id = active_run_or_err(&guard, "request_cancel_after_boundary")?;
        guard.apply(TurnExecutionInput::CancelAfterBoundary { run_id })
    }

    fn cancellation_observed(&self) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        let run_id = active_run_or_err(&guard, "cancellation_observed")?;
        guard.apply(TurnExecutionInput::CancellationObserved { run_id })
    }

    fn acknowledge_terminal(
        &self,
        _outcome: TurnTerminalOutcome,
    ) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        let run_id = active_run_or_err(&guard, "acknowledge_terminal")?;
        guard.apply(TurnExecutionInput::AcknowledgeTerminal { run_id })
    }

    fn turn_limit_reached(&self) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        ensure_active_conversation_run(&mut guard)?;
        let run_id = active_run_or_err(&guard, "turn_limit_reached")?;
        guard.apply(TurnExecutionInput::TurnLimitReached { run_id })
    }

    fn budget_exhausted(&self) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        ensure_active_conversation_run(&mut guard)?;
        let run_id = active_run_or_err(&guard, "budget_exhausted")?;
        guard.apply(TurnExecutionInput::BudgetExhausted { run_id })
    }

    fn time_budget_exceeded(&self) -> Result<(), DslTransitionError> {
        let mut guard = self.lock_state()?;
        ensure_active_conversation_run(&mut guard)?;
        let run_id = active_run_or_err(&guard, "time_budget_exceeded")?;
        guard.apply(TurnExecutionInput::TimeBudgetExceeded { run_id })
    }

    fn force_cancel_no_run(&self) -> Result<(), DslTransitionError> {
        self.lock_state()?
            .apply(TurnExecutionInput::ForceCancelNoRun)
    }

    fn run_completed(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
        // The deleted `LocalTurnExecutionState` folded run_completed into
        // the terminal transitions; accept at any terminal phase.
        Ok(())
    }

    fn run_failed(&self, _run_id: RunId, _error: String) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn run_cancelled(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn snapshot(&self) -> TurnStateSnapshot {
        let guard = match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let fields = &guard.fields;
        TurnStateSnapshot {
            active_run_id: fields.active_run.clone(),
            turn_phase: guard.phase,
            primitive_kind: match fields.primitive_kind {
                TurnPrimitiveKind::None => None,
                other => Some(other),
            },
            admitted_content_shape: fields
                .admitted_content_shape
                .as_ref()
                .map(|cs| cs.0.clone()),
            vision_enabled: fields.vision_enabled,
            image_tool_results_enabled: fields.image_tool_results_enabled,
            tool_calls_pending: u64::from(fields.tool_calls_pending),
            pending_op_refs: fields
                .pending_op_ids
                .iter()
                .map(ToString::to_string)
                .collect(),
            barrier_operation_ids: fields
                .barrier_operation_ids
                .iter()
                .map(ToString::to_string)
                .collect(),
            has_barrier_ops: fields.has_barrier_ops,
            barrier_satisfied: fields.barrier_satisfied,
            boundary_count: u64::from(fields.boundary_count),
            cancel_after_boundary: fields.cancel_after_boundary,
            terminal_outcome: match fields.terminal_outcome {
                TurnTerminalOutcome::None => None,
                other => Some(other),
            },
            extraction_attempts: u64::from(fields.extraction_attempts),
            max_extraction_retries: u64::from(fields.max_extraction_retries),
        }
    }
}
