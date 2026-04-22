use std::collections::HashSet;

use crate::error::AgentError;
use crate::lifecycle::RunId;
use crate::ops::{AsyncOpRef, OperationId};
use crate::turn_execution_authority::{
    ContentShape, TurnExecutionEffect, TurnExecutionInput, TurnExecutionTransition, TurnPhase,
    TurnPrimitiveKind, TurnTerminalOutcome,
};

/// Standalone local owner for turn-execution state when no runtime handle is attached.
///
/// This mirrors the runtime turn machine closely enough to preserve existing
/// core-only behavior, but it is intentionally agent-internal and not exported
/// as a crate-level authority surface.
#[derive(Debug, Clone)]
pub(crate) struct LocalTurnExecutionState {
    phase: TurnPhase,
    fields: LocalTurnExecutionFields,
}

#[derive(Debug, Clone)]
struct LocalTurnExecutionFields {
    active_run: Option<RunId>,
    primitive_kind: TurnPrimitiveKind,
    admitted_content_shape: Option<ContentShape>,
    vision_enabled: bool,
    image_tool_results_enabled: bool,
    tool_calls_pending: u32,
    pending_op_refs: Option<Vec<AsyncOpRef>>,
    barrier_operation_ids: Vec<OperationId>,
    has_barrier_ops: bool,
    barrier_satisfied: bool,
    boundary_count: u32,
    cancel_after_boundary: bool,
    terminal_outcome: TurnTerminalOutcome,
    extraction_attempts: u32,
    max_extraction_retries: u32,
}

impl LocalTurnExecutionFields {
    fn init() -> Self {
        Self {
            active_run: None,
            primitive_kind: TurnPrimitiveKind::None,
            admitted_content_shape: None,
            vision_enabled: false,
            image_tool_results_enabled: false,
            tool_calls_pending: 0,
            pending_op_refs: None,
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

impl Default for LocalTurnExecutionState {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalTurnExecutionState {
    pub(crate) fn new() -> Self {
        Self {
            phase: TurnPhase::Ready,
            fields: LocalTurnExecutionFields::init(),
        }
    }

    pub(crate) fn phase(&self) -> TurnPhase {
        self.phase
    }

    pub(crate) fn active_run(&self) -> Option<&RunId> {
        self.fields.active_run.as_ref()
    }

    pub(crate) fn primitive_kind(&self) -> TurnPrimitiveKind {
        self.fields.primitive_kind
    }

    pub(crate) fn boundary_count(&self) -> u32 {
        self.fields.boundary_count
    }

    pub(crate) fn cancel_after_boundary(&self) -> bool {
        self.fields.cancel_after_boundary
    }

    pub(crate) fn terminal_outcome(&self) -> TurnTerminalOutcome {
        self.fields.terminal_outcome
    }

    pub(crate) fn tool_calls_pending(&self) -> u32 {
        self.fields.tool_calls_pending
    }

    pub(crate) fn pending_op_refs(&self) -> Option<&[AsyncOpRef]> {
        self.fields.pending_op_refs.as_deref()
    }

    pub(crate) fn has_barrier_ops(&self) -> bool {
        self.fields.has_barrier_ops
    }

    pub(crate) fn barrier_satisfied(&self) -> bool {
        self.fields.barrier_satisfied
    }

    pub(crate) fn barrier_op_ids(&self) -> Vec<&OperationId> {
        self.fields.barrier_operation_ids.iter().collect()
    }

    pub(crate) fn pending_op_ids(&self) -> Option<Vec<&OperationId>> {
        self.fields
            .pending_op_refs
            .as_ref()
            .map(|refs| refs.iter().map(|r| &r.operation_id).collect())
    }

    pub(crate) fn vision_enabled(&self) -> bool {
        self.fields.vision_enabled
    }

    pub(crate) fn image_tool_results_enabled(&self) -> bool {
        self.fields.image_tool_results_enabled
    }

    pub(crate) fn admitted_content_shape(&self) -> Option<&ContentShape> {
        self.fields.admitted_content_shape.as_ref()
    }

    pub(crate) fn extraction_attempts(&self) -> u32 {
        self.fields.extraction_attempts
    }

    pub(crate) fn max_extraction_retries(&self) -> u32 {
        self.fields.max_extraction_retries
    }

    pub(crate) fn in_extraction_flow(&self) -> bool {
        self.fields.max_extraction_retries > 0
    }

    pub(crate) fn can_accept(&self, input: &TurnExecutionInput) -> bool {
        self.evaluate(input).is_ok()
    }

    pub(crate) fn apply(
        &mut self,
        input: TurnExecutionInput,
    ) -> Result<TurnExecutionTransition, AgentError> {
        let prev_phase = self.phase;
        let (next_phase, next_fields, effects) = self.evaluate(&input)?;
        self.phase = next_phase;
        self.fields = next_fields;
        Ok(TurnExecutionTransition {
            prev_phase,
            next_phase,
            effects,
        })
    }

    fn invalid(from: TurnPhase, input: &TurnExecutionInput) -> AgentError {
        AgentError::InvalidStateTransition {
            from: from.to_string(),
            to: format!("{input:?}"),
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

    fn evaluate(
        &self,
        input: &TurnExecutionInput,
    ) -> Result<
        (
            TurnPhase,
            LocalTurnExecutionFields,
            Vec<TurnExecutionEffect>,
        ),
        AgentError,
    > {
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
        let mut effects = Vec::new();

        let next_phase = match (phase, input) {
            (Ready, StartConversationRun { run_id }) => {
                fields.active_run = Some(run_id.clone());
                fields.primitive_kind = TurnPrimitiveKind::ConversationTurn;
                fields.tool_calls_pending = 0;
                fields.admitted_content_shape = None;
                fields.vision_enabled = false;
                fields.image_tool_results_enabled = false;
                fields.boundary_count = 0;
                fields.cancel_after_boundary = false;
                fields.terminal_outcome = TurnTerminalOutcome::None;
                fields.pending_op_refs = None;
                fields.barrier_operation_ids = Vec::new();
                fields.has_barrier_ops = false;
                effects.push(TurnExecutionEffect::RunStarted {
                    run_id: run_id.clone(),
                });
                ApplyingPrimitive
            }
            (Ready, StartImmediateAppend { run_id }) => {
                fields.active_run = Some(run_id.clone());
                fields.primitive_kind = TurnPrimitiveKind::ImmediateAppend;
                fields.tool_calls_pending = 0;
                fields.admitted_content_shape = None;
                fields.vision_enabled = false;
                fields.image_tool_results_enabled = false;
                fields.boundary_count = 0;
                fields.cancel_after_boundary = false;
                fields.terminal_outcome = TurnTerminalOutcome::None;
                fields.pending_op_refs = None;
                fields.barrier_operation_ids = Vec::new();
                fields.has_barrier_ops = false;
                effects.push(TurnExecutionEffect::RunStarted {
                    run_id: run_id.clone(),
                });
                ApplyingPrimitive
            }
            (Ready, StartImmediateContext { run_id }) => {
                fields.active_run = Some(run_id.clone());
                fields.primitive_kind = TurnPrimitiveKind::ImmediateContextAppend;
                fields.tool_calls_pending = 0;
                fields.admitted_content_shape = None;
                fields.vision_enabled = false;
                fields.image_tool_results_enabled = false;
                fields.boundary_count = 0;
                fields.cancel_after_boundary = false;
                fields.terminal_outcome = TurnTerminalOutcome::None;
                fields.pending_op_refs = None;
                fields.barrier_operation_ids = Vec::new();
                fields.has_barrier_ops = false;
                effects.push(TurnExecutionEffect::RunStarted {
                    run_id: run_id.clone(),
                });
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
                    return Err(Self::invalid(phase, input));
                }
                fields.admitted_content_shape = Some(admitted_content_shape.clone());
                fields.vision_enabled = *vision_enabled;
                fields.image_tool_results_enabled = *image_tool_results_enabled;

                match fields.primitive_kind {
                    TurnPrimitiveKind::ConversationTurn => {
                        effects.push(TurnExecutionEffect::CheckCompaction);
                        CallingLlm
                    }
                    TurnPrimitiveKind::ImmediateAppend
                    | TurnPrimitiveKind::ImmediateContextAppend => {
                        fields.boundary_count += 1;
                        effects.push(TurnExecutionEffect::BoundaryApplied {
                            run_id: run_id.clone(),
                            boundary_sequence: u64::from(fields.boundary_count),
                        });
                        if fields.cancel_after_boundary {
                            fields.cancel_after_boundary = false;
                            fields.terminal_outcome = TurnTerminalOutcome::Cancelled;
                            effects.push(TurnExecutionEffect::RunCancelled {
                                run_id: run_id.clone(),
                            });
                            Cancelled
                        } else {
                            fields.terminal_outcome = TurnTerminalOutcome::Completed;
                            effects.push(TurnExecutionEffect::RunCompleted {
                                run_id: run_id.clone(),
                            });
                            Completed
                        }
                    }
                    TurnPrimitiveKind::None => return Err(Self::invalid(phase, input)),
                }
            }
            (CallingLlm, LlmReturnedToolCalls { run_id, tool_count }) => {
                if !self.guard_run_matches(run_id) || *tool_count == 0 {
                    return Err(Self::invalid(phase, input));
                }
                fields.tool_calls_pending = *tool_count;
                fields.pending_op_refs = None;
                fields.barrier_operation_ids = Vec::new();
                fields.has_barrier_ops = false;
                fields.barrier_satisfied = true;
                WaitingForOps
            }
            (CallingLlm, LlmReturnedTerminal { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                fields.boundary_count += 1;
                effects.push(TurnExecutionEffect::BoundaryApplied {
                    run_id: run_id.clone(),
                    boundary_sequence: u64::from(fields.boundary_count),
                });
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
                    return Err(Self::invalid(phase, input));
                }
                fields.pending_op_refs = Some(op_refs.clone());
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
                    return Err(Self::invalid(phase, input));
                }
                fields.barrier_satisfied = true;
                WaitingForOps
            }
            (WaitingForOps, ToolCallsResolved { run_id }) => {
                if !self.guard_run_matches(run_id)
                    || fields.tool_calls_pending == 0
                    || fields.pending_op_refs.is_none()
                    || !fields.barrier_satisfied
                {
                    return Err(Self::invalid(phase, input));
                }
                fields.tool_calls_pending = 0;
                fields.pending_op_refs = None;
                fields.barrier_operation_ids = Vec::new();
                fields.has_barrier_ops = false;
                fields.barrier_satisfied = true;
                fields.boundary_count += 1;
                effects.push(TurnExecutionEffect::BoundaryApplied {
                    run_id: run_id.clone(),
                    boundary_sequence: u64::from(fields.boundary_count),
                });
                DrainingBoundary
            }
            (DrainingBoundary, BoundaryContinue { run_id }) => {
                if !self.guard_run_matches(run_id)
                    || fields.primitive_kind != TurnPrimitiveKind::ConversationTurn
                {
                    return Err(Self::invalid(phase, input));
                }
                if fields.cancel_after_boundary {
                    fields.cancel_after_boundary = false;
                    fields.terminal_outcome = TurnTerminalOutcome::Cancelled;
                    effects.push(TurnExecutionEffect::RunCancelled {
                        run_id: run_id.clone(),
                    });
                    Cancelled
                } else {
                    effects.push(TurnExecutionEffect::CheckCompaction);
                    CallingLlm
                }
            }
            (DrainingBoundary, BoundaryComplete { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                if fields.cancel_after_boundary {
                    fields.cancel_after_boundary = false;
                    fields.terminal_outcome = TurnTerminalOutcome::Cancelled;
                    effects.push(TurnExecutionEffect::RunCancelled {
                        run_id: run_id.clone(),
                    });
                    Cancelled
                } else {
                    fields.terminal_outcome = TurnTerminalOutcome::Completed;
                    effects.push(TurnExecutionEffect::RunCompleted {
                        run_id: run_id.clone(),
                    });
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
                    return Err(Self::invalid(phase, input));
                }
                fields.max_extraction_retries = *max_retries;
                Extracting
            }
            (Extracting, ExtractionValidationPassed { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                fields.terminal_outcome = TurnTerminalOutcome::Completed;
                effects.push(TurnExecutionEffect::RunCompleted {
                    run_id: run_id.clone(),
                });
                Completed
            }
            (Extracting, ExtractionStart { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                effects.push(TurnExecutionEffect::CheckCompaction);
                CallingLlm
            }
            (Extracting, ExtractionValidationFailed { run_id, .. }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                fields.extraction_attempts += 1;
                if fields.extraction_attempts < fields.max_extraction_retries {
                    effects.push(TurnExecutionEffect::CheckCompaction);
                    CallingLlm
                } else {
                    fields.terminal_outcome = TurnTerminalOutcome::StructuredOutputValidationFailed;
                    effects.push(TurnExecutionEffect::RunFailed {
                        run_id: run_id.clone(),
                    });
                    Failed
                }
            }
            (CallingLlm | WaitingForOps | DrainingBoundary, RecoverableFailure { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                if matches!(phase, WaitingForOps) {
                    fields.pending_op_refs = None;
                    fields.barrier_operation_ids = Vec::new();
                    fields.has_barrier_ops = false;
                    fields.barrier_satisfied = true;
                }
                ErrorRecovery
            }
            (ErrorRecovery, RetryRequested { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                effects.push(TurnExecutionEffect::CheckCompaction);
                CallingLlm
            }
            (
                ApplyingPrimitive | CallingLlm | WaitingForOps | DrainingBoundary | Extracting
                | ErrorRecovery,
                FatalFailure { run_id },
            ) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                if matches!(phase, WaitingForOps) {
                    fields.pending_op_refs = None;
                    fields.barrier_operation_ids = Vec::new();
                    fields.has_barrier_ops = false;
                    fields.barrier_satisfied = true;
                }
                fields.terminal_outcome = TurnTerminalOutcome::Failed;
                effects.push(TurnExecutionEffect::RunFailed {
                    run_id: run_id.clone(),
                });
                Failed
            }
            (
                ApplyingPrimitive | CallingLlm | WaitingForOps | DrainingBoundary | Extracting
                | ErrorRecovery,
                CancelNow { run_id },
            ) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                if matches!(phase, WaitingForOps) {
                    fields.pending_op_refs = None;
                    fields.barrier_operation_ids = Vec::new();
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
                    return Err(Self::invalid(phase, input));
                }
                fields.cancel_after_boundary = true;
                phase
            }
            (Cancelling, CancellationObserved { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                fields.terminal_outcome = TurnTerminalOutcome::Cancelled;
                fields.cancel_after_boundary = false;
                effects.push(TurnExecutionEffect::RunCancelled {
                    run_id: run_id.clone(),
                });
                Cancelled
            }
            (
                ApplyingPrimitive | CallingLlm | WaitingForOps | DrainingBoundary | Extracting
                | ErrorRecovery,
                TurnLimitReached { run_id },
            ) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                if matches!(phase, WaitingForOps) {
                    fields.pending_op_refs = None;
                    fields.barrier_operation_ids = Vec::new();
                    fields.has_barrier_ops = false;
                    fields.barrier_satisfied = true;
                }
                fields.boundary_count += 1;
                fields.terminal_outcome = TurnTerminalOutcome::Completed;
                effects.push(TurnExecutionEffect::BoundaryApplied {
                    run_id: run_id.clone(),
                    boundary_sequence: u64::from(fields.boundary_count),
                });
                effects.push(TurnExecutionEffect::RunCompleted {
                    run_id: run_id.clone(),
                });
                Completed
            }
            (
                ApplyingPrimitive | CallingLlm | WaitingForOps | DrainingBoundary | Extracting
                | ErrorRecovery,
                BudgetExhausted { run_id },
            ) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                if matches!(phase, WaitingForOps) {
                    fields.pending_op_refs = None;
                    fields.barrier_operation_ids = Vec::new();
                    fields.has_barrier_ops = false;
                    fields.barrier_satisfied = true;
                }
                fields.boundary_count += 1;
                fields.terminal_outcome = TurnTerminalOutcome::BudgetExhausted;
                effects.push(TurnExecutionEffect::BoundaryApplied {
                    run_id: run_id.clone(),
                    boundary_sequence: u64::from(fields.boundary_count),
                });
                effects.push(TurnExecutionEffect::RunCompleted {
                    run_id: run_id.clone(),
                });
                Completed
            }
            (
                ApplyingPrimitive | CallingLlm | WaitingForOps | DrainingBoundary | Extracting
                | ErrorRecovery,
                TimeBudgetExceeded { run_id },
            ) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                if matches!(phase, WaitingForOps) {
                    fields.pending_op_refs = None;
                    fields.barrier_operation_ids = Vec::new();
                    fields.has_barrier_ops = false;
                    fields.barrier_satisfied = true;
                }
                fields.boundary_count += 1;
                fields.terminal_outcome = TurnTerminalOutcome::TimeBudgetExceeded;
                effects.push(TurnExecutionEffect::BoundaryApplied {
                    run_id: run_id.clone(),
                    boundary_sequence: u64::from(fields.boundary_count),
                });
                effects.push(TurnExecutionEffect::RunCompleted {
                    run_id: run_id.clone(),
                });
                Completed
            }
            (
                Ready | ApplyingPrimitive | CallingLlm | WaitingForOps | DrainingBoundary
                | Extracting | ErrorRecovery | Cancelling,
                ForceCancelNoRun,
            ) => {
                if matches!(phase, WaitingForOps) {
                    fields.pending_op_refs = None;
                    fields.barrier_operation_ids = Vec::new();
                    fields.has_barrier_ops = false;
                    fields.barrier_satisfied = true;
                }
                fields.terminal_outcome = TurnTerminalOutcome::Cancelled;
                if let Some(ref run_id) = fields.active_run {
                    effects.push(TurnExecutionEffect::RunCancelled {
                        run_id: run_id.clone(),
                    });
                }
                Cancelled
            }
            (Completed | Failed | Cancelled, AcknowledgeTerminal { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                fields.reset();
                Ready
            }
            _ => return Err(Self::invalid(phase, input)),
        };

        Ok((next_phase, fields, effects))
    }
}
