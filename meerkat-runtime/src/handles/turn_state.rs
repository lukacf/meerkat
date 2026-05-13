//! Runtime impl of [`meerkat_core::handles::TurnStateHandle`].

use std::collections::BTreeSet;
use std::sync::Arc;

use meerkat_core::handles::{DslTransitionError, TurnStateHandle, TurnStateSnapshot};
use meerkat_core::lifecycle::RunId;
use meerkat_core::ops::{AsyncOpRef, OperationId, WaitPolicy};
use meerkat_core::retry::LlmRetrySchedule;
use meerkat_core::turn_execution_authority::{
    StructuredOutputFailureReason, TurnExecutionEffect, TurnExecutionInput,
    TurnExecutionTransition, TurnFailureReason, TurnPhase, TurnPrimitiveKind, TurnTerminalOutcome,
    terminal_outcome_for_budget_exceeded,
};

use super::HandleDslAuthority;
use crate::meerkat_machine::dsl as mm_dsl;

/// Runtime-backed [`TurnStateHandle`] impl.
#[derive(Debug)]
pub struct RuntimeTurnStateHandle {
    dsl: Arc<HandleDslAuthority>,
}

impl RuntimeTurnStateHandle {
    /// Construct a handle backed by the session's shared DSL authority.
    pub fn new(dsl: Arc<HandleDslAuthority>) -> Self {
        Self { dsl }
    }

    /// Construct a handle backed by an ephemeral DSL authority.
    pub fn ephemeral() -> Self {
        Self::new(Arc::new(HandleDslAuthority::ephemeral()))
    }
}

fn domain_run_id_from_dsl(
    run_id: &mm_dsl::RunId,
    context: &'static str,
) -> Result<RunId, DslTransitionError> {
    let uuid = uuid::Uuid::parse_str(run_id.0.as_str()).map_err(|err| {
        DslTransitionError::no_matching(
            context,
            format!(
                "MeerkatMachine emitted invalid run_id `{}`: {err}",
                run_id.0
            ),
        )
    })?;
    Ok(RunId::from_uuid(uuid))
}

fn turn_effects_from_dsl(
    effects: Vec<mm_dsl::MeerkatMachineEffect>,
    context: &'static str,
) -> Result<Vec<TurnExecutionEffect>, DslTransitionError> {
    let mut projected = Vec::new();
    for effect in effects {
        match effect {
            mm_dsl::MeerkatMachineEffect::TurnRunStarted { run_id } => {
                projected.push(TurnExecutionEffect::RunStarted {
                    run_id: domain_run_id_from_dsl(&run_id, context)?,
                });
            }
            mm_dsl::MeerkatMachineEffect::TurnBoundaryApplied {
                run_id,
                boundary_sequence,
            } => {
                projected.push(TurnExecutionEffect::BoundaryApplied {
                    run_id: domain_run_id_from_dsl(&run_id, context)?,
                    boundary_sequence,
                });
            }
            mm_dsl::MeerkatMachineEffect::TurnRunCompleted { run_id, .. } => {
                projected.push(TurnExecutionEffect::RunCompleted {
                    run_id: domain_run_id_from_dsl(&run_id, context)?,
                });
            }
            mm_dsl::MeerkatMachineEffect::TurnRunFailed {
                run_id,
                terminal_cause_kind,
                failure_class,
                error,
            } => {
                let cause_kind = terminal_cause_kind.into();
                projected.push(TurnExecutionEffect::RunFailed {
                    run_id: domain_run_id_from_dsl(&run_id, context)?,
                    reason: TurnFailureReason::with_cause(cause_kind, failure_class.into(), error),
                });
            }
            mm_dsl::MeerkatMachineEffect::TurnRunCancelled { run_id, .. } => {
                projected.push(TurnExecutionEffect::RunCancelled {
                    run_id: domain_run_id_from_dsl(&run_id, context)?,
                });
            }
            mm_dsl::MeerkatMachineEffect::TurnCheckCompaction => {
                projected.push(TurnExecutionEffect::CheckCompaction);
            }
            _ => {}
        }
    }
    Ok(projected)
}

impl TurnStateHandle for RuntimeTurnStateHandle {
    fn apply_turn_input(
        &self,
        input: &TurnExecutionInput,
    ) -> Result<TurnExecutionTransition, DslTransitionError> {
        let before = self.snapshot();
        let context = "TurnStateHandle::apply_turn_input";
        let effects = match input {
            TurnExecutionInput::StartConversationRun { run_id }
                if before.active_run_id.as_ref() == Some(run_id) =>
            {
                Vec::new()
            }
            TurnExecutionInput::StartConversationRun { run_id } => {
                self.dsl.apply_input_with_effects(
                    mm_dsl::MeerkatMachineInput::StartConversationRun {
                        run_id: mm_dsl::RunId::from_domain(run_id),
                        primitive_kind: mm_dsl::TurnPrimitiveKind::ConversationTurn,
                        admitted_content_shape: mm_dsl::ContentShape::Conversation,
                        vision_enabled: false,
                        image_tool_results_enabled: false,
                        max_extraction_retries: 0,
                    },
                    context,
                )?
            }
            TurnExecutionInput::StartImmediateAppend { run_id }
                if before.active_run_id.as_ref() == Some(run_id) =>
            {
                Vec::new()
            }
            TurnExecutionInput::StartImmediateAppend { run_id } => {
                self.dsl.apply_input_with_effects(
                    mm_dsl::MeerkatMachineInput::StartImmediateAppend {
                        run_id: mm_dsl::RunId::from_domain(run_id),
                    },
                    context,
                )?
            }
            TurnExecutionInput::StartImmediateContext { run_id }
                if before.active_run_id.as_ref() == Some(run_id) =>
            {
                Vec::new()
            }
            TurnExecutionInput::StartImmediateContext { run_id } => {
                self.dsl.apply_input_with_effects(
                    mm_dsl::MeerkatMachineInput::StartImmediateContext {
                        run_id: mm_dsl::RunId::from_domain(run_id),
                    },
                    context,
                )?
            }
            TurnExecutionInput::PrimitiveApplied { .. } => self
                .dsl
                .apply_input_with_effects(mm_dsl::MeerkatMachineInput::PrimitiveApplied, context)?,
            TurnExecutionInput::LlmReturnedToolCalls { tool_count, .. } => {
                self.dsl.apply_input_with_effects(
                    mm_dsl::MeerkatMachineInput::LlmReturnedToolCalls {
                        tool_count: u64::from(*tool_count),
                    },
                    context,
                )?
            }
            TurnExecutionInput::LlmReturnedTerminal { .. } => self.dsl.apply_input_with_effects(
                mm_dsl::MeerkatMachineInput::LlmReturnedTerminal,
                context,
            )?,
            TurnExecutionInput::RegisterPendingOps {
                op_refs,
                barrier_operation_ids,
                ..
            } => self.dsl.apply_input_with_effects(
                mm_dsl::MeerkatMachineInput::RegisterPendingOps {
                    op_refs: op_refs
                        .iter()
                        .map(|op_ref| op_ref.operation_id.to_string())
                        .collect(),
                    barrier_operation_ids: barrier_operation_ids
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                },
                context,
            )?,
            TurnExecutionInput::ToolCallsResolved { .. } => self.dsl.apply_input_with_effects(
                mm_dsl::MeerkatMachineInput::ToolCallsResolved,
                context,
            )?,
            TurnExecutionInput::OpsBarrierSatisfied { operation_ids, .. } => {
                self.dsl.apply_input_with_effects(
                    mm_dsl::MeerkatMachineInput::OpsBarrierSatisfied {
                        operation_ids: operation_ids.iter().map(ToString::to_string).collect(),
                    },
                    context,
                )?
            }
            TurnExecutionInput::BoundaryContinue { .. } => self
                .dsl
                .apply_input_with_effects(mm_dsl::MeerkatMachineInput::BoundaryContinue, context)?,
            TurnExecutionInput::BoundaryComplete { .. } => self
                .dsl
                .apply_input_with_effects(mm_dsl::MeerkatMachineInput::BoundaryComplete, context)?,
            TurnExecutionInput::RecoverableFailure { retry, .. } => {
                self.dsl.apply_input_with_effects(
                    mm_dsl::MeerkatMachineInput::RecoverableFailure {
                        failure_kind: retry.failure.kind.into(),
                        retry_attempt: u64::from(retry.plan.attempt),
                        max_retries: u64::from(retry.plan.max_retries),
                        selected_delay_ms: retry.plan.selected_delay_ms,
                        error: retry.failure.message.clone(),
                    },
                    context,
                )?
            }
            TurnExecutionInput::FatalFailure { reason, .. } => self.dsl.apply_input_with_effects(
                mm_dsl::MeerkatMachineInput::FatalFailure {
                    terminal_cause_kind: mm_dsl::TurnTerminalCauseKind::from(reason.cause_kind),
                    failure_class: mm_dsl::TurnFailureClass::from(reason.class),
                    error: reason.message.clone(),
                },
                context,
            )?,
            TurnExecutionInput::RetryRequested { retry_attempt, .. } => {
                self.dsl.apply_input_with_effects(
                    mm_dsl::MeerkatMachineInput::RetryRequested {
                        retry_attempt: u64::from(*retry_attempt),
                    },
                    context,
                )?
            }
            TurnExecutionInput::CancelNow { .. } => self
                .dsl
                .apply_input_with_effects(mm_dsl::MeerkatMachineInput::CancelNow, context)?,
            TurnExecutionInput::CancelAfterBoundary { .. } => self.dsl.apply_input_with_effects(
                mm_dsl::MeerkatMachineInput::RequestCancelAfterBoundary,
                context,
            )?,
            TurnExecutionInput::CancellationObserved { .. } => self.dsl.apply_input_with_effects(
                mm_dsl::MeerkatMachineInput::CancellationObserved,
                context,
            )?,
            TurnExecutionInput::AcknowledgeTerminal { .. } => self.dsl.apply_input_with_effects(
                mm_dsl::MeerkatMachineInput::AcknowledgeTerminal {
                    outcome: mm_dsl::TurnTerminalOutcome::from(
                        before.terminal_outcome.unwrap_or(TurnTerminalOutcome::None),
                    ),
                },
                context,
            )?,
            TurnExecutionInput::TurnLimitReached { .. } => self
                .dsl
                .apply_input_with_effects(mm_dsl::MeerkatMachineInput::TurnLimitReached, context)?,
            TurnExecutionInput::BudgetExhausted { .. } => self
                .dsl
                .apply_input_with_effects(mm_dsl::MeerkatMachineInput::BudgetExhausted, context)?,
            TurnExecutionInput::TimeBudgetExceeded { .. } => self.dsl.apply_input_with_effects(
                mm_dsl::MeerkatMachineInput::TimeBudgetExceeded,
                context,
            )?,
            TurnExecutionInput::BudgetLimitExceeded { exceeded, .. } => {
                match terminal_outcome_for_budget_exceeded(*exceeded) {
                    TurnTerminalOutcome::TimeBudgetExceeded => self.dsl.apply_input_with_effects(
                        mm_dsl::MeerkatMachineInput::TimeBudgetExceeded,
                        context,
                    )?,
                    TurnTerminalOutcome::BudgetExhausted => self.dsl.apply_input_with_effects(
                        mm_dsl::MeerkatMachineInput::BudgetExhausted,
                        context,
                    )?,
                    _ => unreachable!("budget exceeded maps only to budget terminal outcomes"),
                }
            }
            TurnExecutionInput::EnterExtraction { max_retries, .. } => {
                self.dsl.apply_input_with_effects(
                    mm_dsl::MeerkatMachineInput::EnterExtraction {
                        max_extraction_retries: u64::from(*max_retries),
                    },
                    context,
                )?
            }
            TurnExecutionInput::ExtractionValidationPassed { .. } => {
                self.dsl.apply_input_with_effects(
                    mm_dsl::MeerkatMachineInput::ExtractionValidationPassed,
                    context,
                )?
            }
            TurnExecutionInput::ExtractionValidationFailed { failure, .. } => {
                self.dsl.apply_input_with_effects(
                    mm_dsl::MeerkatMachineInput::ExtractionValidationFailed {
                        error: failure.message().to_string(),
                    },
                    context,
                )?
            }
            TurnExecutionInput::ExtractionFailed { failure, .. } => {
                self.dsl.apply_input_with_effects(
                    mm_dsl::MeerkatMachineInput::ExtractionFailed {
                        error: failure.message().to_string(),
                    },
                    context,
                )?
            }
            TurnExecutionInput::ExtractionStart { .. } => self
                .dsl
                .apply_input_with_effects(mm_dsl::MeerkatMachineInput::ExtractionStart, context)?,
            TurnExecutionInput::ForceCancelNoRun => self
                .dsl
                .apply_input_with_effects(mm_dsl::MeerkatMachineInput::ForceCancelNoRun, context)?,
        };
        let after = self.snapshot();
        Ok(TurnExecutionTransition {
            prev_phase: before.turn_phase,
            next_phase: after.turn_phase,
            effects: turn_effects_from_dsl(effects, context)?,
        })
    }

    fn start_conversation_run(
        &self,
        run_id: RunId,
        primitive_kind: TurnPrimitiveKind,
        admitted_content_shape: meerkat_core::turn_execution_authority::ContentShape,
        vision_enabled: bool,
        image_tool_results_enabled: bool,
        max_extraction_retries: u64,
    ) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::StartConversationRun {
                run_id: mm_dsl::RunId::from_domain(&run_id),
                primitive_kind: mm_dsl::TurnPrimitiveKind::from(primitive_kind),
                admitted_content_shape: mm_dsl::ContentShape::from(admitted_content_shape),
                vision_enabled,
                image_tool_results_enabled,
                max_extraction_retries,
            },
            "TurnStateHandle::start_conversation_run",
        )
    }

    fn start_immediate_append(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::StartImmediateAppend {
                run_id: mm_dsl::RunId::from_domain(&run_id),
            },
            "TurnStateHandle::start_immediate_append",
        )
    }

    fn start_immediate_context(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::StartImmediateContext {
                run_id: mm_dsl::RunId::from_domain(&run_id),
            },
            "TurnStateHandle::start_immediate_context",
        )
    }

    fn primitive_applied(&self) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::PrimitiveApplied,
            "TurnStateHandle::primitive_applied",
        )
    }

    fn llm_returned_tool_calls(&self, tool_count: u64) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::LlmReturnedToolCalls { tool_count },
            "TurnStateHandle::llm_returned_tool_calls",
        )
    }

    fn llm_returned_terminal(&self) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::LlmReturnedTerminal,
            "TurnStateHandle::llm_returned_terminal",
        )
    }

    fn register_pending_ops(
        &self,
        op_refs: BTreeSet<AsyncOpRef>,
        barrier_operation_ids: BTreeSet<OperationId>,
    ) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::RegisterPendingOps {
                op_refs: op_refs
                    .iter()
                    .map(|op_ref| op_ref.operation_id.to_string())
                    .collect(),
                barrier_operation_ids: barrier_operation_ids
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            },
            "TurnStateHandle::register_pending_ops",
        )
    }

    fn tool_calls_resolved(&self) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::ToolCallsResolved,
            "TurnStateHandle::tool_calls_resolved",
        )
    }

    fn ops_barrier_satisfied(
        &self,
        operation_ids: BTreeSet<OperationId>,
    ) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::OpsBarrierSatisfied {
                operation_ids: operation_ids.iter().map(ToString::to_string).collect(),
            },
            "TurnStateHandle::ops_barrier_satisfied",
        )
    }

    fn boundary_continue(&self) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::BoundaryContinue,
            "TurnStateHandle::boundary_continue",
        )
    }

    fn boundary_complete(&self) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::BoundaryComplete,
            "TurnStateHandle::boundary_complete",
        )
    }

    fn enter_extraction(&self, max_retries: u32) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::EnterExtraction {
                max_extraction_retries: u64::from(max_retries),
            },
            "TurnStateHandle::enter_extraction",
        )
    }

    fn extraction_start(&self) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::ExtractionStart,
            "TurnStateHandle::extraction_start",
        )
    }

    fn extraction_validation_passed(&self) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::ExtractionValidationPassed,
            "TurnStateHandle::extraction_validation_passed",
        )
    }

    fn extraction_validation_failed(
        &self,
        failure: StructuredOutputFailureReason,
    ) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::ExtractionValidationFailed {
                error: failure.into_message(),
            },
            "TurnStateHandle::extraction_validation_failed",
        )
    }

    fn extraction_failed(
        &self,
        failure: StructuredOutputFailureReason,
    ) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::ExtractionFailed {
                error: failure.into_message(),
            },
            "TurnStateHandle::extraction_failed",
        )
    }

    fn recoverable_failure(&self, retry: LlmRetrySchedule) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::RecoverableFailure {
                failure_kind: retry.failure.kind.into(),
                retry_attempt: u64::from(retry.plan.attempt),
                max_retries: u64::from(retry.plan.max_retries),
                selected_delay_ms: retry.plan.selected_delay_ms,
                error: retry.failure.message,
            },
            "TurnStateHandle::recoverable_failure",
        )
    }

    fn fatal_failure(&self, reason: TurnFailureReason) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::FatalFailure {
                terminal_cause_kind: mm_dsl::TurnTerminalCauseKind::from(reason.cause_kind),
                failure_class: mm_dsl::TurnFailureClass::from(reason.class),
                error: reason.message,
            },
            "TurnStateHandle::fatal_failure",
        )
    }

    fn retry_requested(&self, retry_attempt: u32) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::RetryRequested {
                retry_attempt: u64::from(retry_attempt),
            },
            "TurnStateHandle::retry_requested",
        )
    }

    fn cancel_now(&self) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::CancelNow,
            "TurnStateHandle::cancel_now",
        )
    }

    fn request_cancel_after_boundary(&self) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::RequestCancelAfterBoundary,
            "TurnStateHandle::request_cancel_after_boundary",
        )
    }

    fn cancellation_observed(&self) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::CancellationObserved,
            "TurnStateHandle::cancellation_observed",
        )
    }

    fn acknowledge_terminal(&self, outcome: TurnTerminalOutcome) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::AcknowledgeTerminal {
                outcome: mm_dsl::TurnTerminalOutcome::from(outcome),
            },
            "TurnStateHandle::acknowledge_terminal",
        )
    }

    fn turn_limit_reached(&self) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::TurnLimitReached,
            "TurnStateHandle::turn_limit_reached",
        )
    }

    fn budget_exhausted(&self) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::BudgetExhausted,
            "TurnStateHandle::budget_exhausted",
        )
    }

    fn time_budget_exceeded(&self) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::TimeBudgetExceeded,
            "TurnStateHandle::time_budget_exceeded",
        )
    }

    fn force_cancel_no_run(&self) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::ForceCancelNoRun,
            "TurnStateHandle::force_cancel_no_run",
        )
    }

    fn run_completed(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
        // Runtime-backed run terminalization is owned by
        // MeerkatMachine::Commit after the durable boundary receipt is ready.
        // Core still emits this effect for standalone/test handles, but this
        // runtime handle must not provide a second terminal writer.
        Ok(())
    }

    fn run_failed(
        &self,
        _run_id: RunId,
        _reason: TurnFailureReason,
    ) -> Result<(), DslTransitionError> {
        // Runtime-backed failure terminalization is owned by
        // MeerkatMachine::Fail/Commit and its durable terminal receipt path.
        Ok(())
    }

    fn run_cancelled(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
        // Runtime-backed cancellation terminalization is owned by machine
        // commands that can keep lifecycle and durable state aligned.
        Ok(())
    }

    fn snapshot(&self) -> TurnStateSnapshot {
        let state = self.dsl.snapshot_state();
        let turn_phase = map_turn_phase(state.turn_phase);
        let barrier_operation_ids: BTreeSet<_> = state
            .barrier_operation_ids
            .iter()
            .filter_map(|id| parse_operation_id(id))
            .collect();
        let pending_op_refs = state
            .pending_op_refs
            .iter()
            .filter_map(|id| {
                parse_operation_id(id).map(|operation_id| AsyncOpRef {
                    wait_policy: if barrier_operation_ids.contains(&operation_id) {
                        WaitPolicy::Barrier
                    } else {
                        WaitPolicy::Detached
                    },
                    operation_id,
                })
            })
            .collect();
        let active_run_id = if matches!(
            turn_phase,
            TurnPhase::Completed | TurnPhase::Failed | TurnPhase::Cancelled
        ) {
            None
        } else {
            state
                .current_run_id
                .as_ref()
                .and_then(|run_id| uuid::Uuid::parse_str(&run_id.0).ok().map(RunId::from_uuid))
        };
        TurnStateSnapshot {
            active_run_id,
            loop_state: map_loop_state(state.turn_phase),
            turn_phase,
            primitive_kind: state.primitive_kind.map(TurnPrimitiveKind::from),
            admitted_content_shape: state.admitted_content_shape.map(Into::into),
            vision_enabled: state.vision_enabled,
            image_tool_results_enabled: state.image_tool_results_enabled,
            tool_calls_pending: state.tool_calls_pending,
            pending_op_refs,
            barrier_operation_ids,
            has_barrier_ops: state.has_barrier_ops,
            barrier_satisfied: state.barrier_satisfied,
            boundary_count: state.boundary_count,
            cancel_after_boundary: state.cancel_after_boundary,
            terminal_outcome: state.terminal_outcome.map(TurnTerminalOutcome::from),
            terminal_cause_kind: state.terminal_cause_kind.map(Into::into),
            terminal_failure_class: state.terminal_failure_class.map(Into::into),
            extraction_attempts: state.extraction_attempts,
            max_extraction_retries: state.max_extraction_retries,
            llm_retry_attempt: u32::try_from(state.llm_retry_attempt).unwrap_or(u32::MAX),
            llm_retry_max_retries: u32::try_from(state.llm_retry_max_retries).unwrap_or(u32::MAX),
            llm_retry_selected_delay_ms: state.llm_retry_selected_delay_ms,
        }
    }
}

fn parse_operation_id(value: &str) -> Option<OperationId> {
    uuid::Uuid::parse_str(value).ok().map(OperationId)
}

/// Exhaustive 1-to-1 projection of the DSL's typed turn phase into the
/// cross-crate [`TurnPhase`] contract. The compiler enforces that every
/// DSL variant has a core-facing twin; any new variant in either enum
/// must be reflected here.
fn map_turn_phase(phase: mm_dsl::TurnPhase) -> TurnPhase {
    match phase {
        mm_dsl::TurnPhase::Ready => TurnPhase::Ready,
        mm_dsl::TurnPhase::ApplyingPrimitive => TurnPhase::ApplyingPrimitive,
        mm_dsl::TurnPhase::CallingLlm => TurnPhase::CallingLlm,
        mm_dsl::TurnPhase::WaitingForOps => TurnPhase::WaitingForOps,
        mm_dsl::TurnPhase::DrainingBoundary => TurnPhase::DrainingBoundary,
        mm_dsl::TurnPhase::Extracting => TurnPhase::Extracting,
        mm_dsl::TurnPhase::ErrorRecovery => TurnPhase::ErrorRecovery,
        mm_dsl::TurnPhase::Cancelling => TurnPhase::Cancelling,
        mm_dsl::TurnPhase::Completed => TurnPhase::Completed,
        mm_dsl::TurnPhase::Failed => TurnPhase::Failed,
        mm_dsl::TurnPhase::Cancelled => TurnPhase::Cancelled,
    }
}

/// Owner-side projection from DSL turn phase to the legacy observable loop
/// state. Keep this beside `map_turn_phase` so the agent runner receives one
/// coherent snapshot from the DSL authority instead of reclassifying phases.
fn map_loop_state(phase: mm_dsl::TurnPhase) -> meerkat_core::LoopState {
    match phase {
        mm_dsl::TurnPhase::Ready
        | mm_dsl::TurnPhase::ApplyingPrimitive
        | mm_dsl::TurnPhase::CallingLlm => meerkat_core::LoopState::CallingLlm,
        mm_dsl::TurnPhase::WaitingForOps => meerkat_core::LoopState::WaitingForOps,
        mm_dsl::TurnPhase::DrainingBoundary | mm_dsl::TurnPhase::Extracting => {
            meerkat_core::LoopState::DrainingEvents
        }
        mm_dsl::TurnPhase::ErrorRecovery => meerkat_core::LoopState::ErrorRecovery,
        mm_dsl::TurnPhase::Cancelling => meerkat_core::LoopState::Cancelling,
        mm_dsl::TurnPhase::Completed | mm_dsl::TurnPhase::Failed | mm_dsl::TurnPhase::Cancelled => {
            meerkat_core::LoopState::Completed
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::retry::{
        LlmRetryFailure, LlmRetryFailureKind, LlmRetryPlan, LlmRetrySchedule,
    };
    use uuid::Uuid;

    fn retry_schedule(attempt: u32) -> LlmRetrySchedule {
        LlmRetrySchedule {
            failure: LlmRetryFailure {
                provider: "test".to_string(),
                kind: LlmRetryFailureKind::RateLimited,
                retry_after_ms: Some(1_000),
                duration_ms: None,
                message: "rate limited".to_string(),
            },
            plan: LlmRetryPlan {
                attempt,
                max_retries: 3,
                computed_delay_ms: 500,
                selected_delay_ms: 1_000,
                retry_after_hint_ms: Some(1_000),
                rate_limit_floor_applied: false,
                budget_capped: false,
            },
        }
    }

    fn unknown_terminal_cause_reason(message: &'static str) -> TurnFailureReason {
        TurnFailureReason::with_cause(
            meerkat_core::TurnTerminalCauseKind::Unknown,
            meerkat_core::event::AgentErrorClass::Internal,
            message,
        )
    }

    fn specific_terminal_cause_reason(
        cause_kind: meerkat_core::TurnTerminalCauseKind,
        message: &'static str,
    ) -> TurnFailureReason {
        TurnFailureReason::with_cause(cause_kind, cause_kind.agent_error_class(), message)
    }

    #[test]
    fn snapshot_carries_active_run_id_for_runtime_backed_turns() {
        let handle = RuntimeTurnStateHandle::ephemeral();
        let run_id = RunId(Uuid::from_u128(7));

        handle
            .start_conversation_run(
                run_id.clone(),
                TurnPrimitiveKind::ConversationTurn,
                meerkat_core::turn_execution_authority::ContentShape::Conversation,
                true,
                false,
                2,
            )
            .unwrap();

        let snapshot = handle.snapshot();
        assert_eq!(snapshot.active_run_id, Some(run_id.clone()));
        assert_eq!(snapshot.turn_phase, TurnPhase::ApplyingPrimitive);
        assert_eq!(
            snapshot.primitive_kind,
            Some(TurnPrimitiveKind::ConversationTurn)
        );
    }

    #[test]
    fn apply_turn_input_returns_dsl_emitted_effects() {
        let handle = RuntimeTurnStateHandle::ephemeral();
        let run_id = RunId(Uuid::from_u128(700));

        let started = handle
            .apply_turn_input(&TurnExecutionInput::StartConversationRun {
                run_id: run_id.clone(),
            })
            .unwrap();
        assert_eq!(
            started.effects,
            vec![TurnExecutionEffect::RunStarted {
                run_id: run_id.clone()
            }]
        );

        let primitive = handle
            .apply_turn_input(&TurnExecutionInput::PrimitiveApplied {
                run_id: run_id.clone(),
                admitted_content_shape: meerkat_core::ContentShape::Conversation,
                vision_enabled: false,
                image_tool_results_enabled: false,
            })
            .unwrap();
        assert_eq!(
            primitive.effects,
            vec![TurnExecutionEffect::CheckCompaction]
        );

        handle
            .apply_turn_input(&TurnExecutionInput::LlmReturnedToolCalls {
                run_id: run_id.clone(),
                tool_count: 1,
            })
            .unwrap();
        let resolved = handle
            .apply_turn_input(&TurnExecutionInput::ToolCallsResolved {
                run_id: run_id.clone(),
            })
            .unwrap();
        assert_eq!(resolved.prev_phase, TurnPhase::WaitingForOps);
        assert_eq!(resolved.next_phase, TurnPhase::CallingLlm);
        assert!(
            resolved.effects.is_empty(),
            "runtime must use DSL-emitted effects, not synthesize CheckCompaction from phase delta"
        );
    }

    #[test]
    fn snapshot_clears_active_run_id_after_terminal_turn() {
        let handle = RuntimeTurnStateHandle::ephemeral();
        let run_id = RunId(Uuid::from_u128(8));

        handle
            .start_conversation_run(
                run_id,
                TurnPrimitiveKind::ConversationTurn,
                meerkat_core::turn_execution_authority::ContentShape::Conversation,
                false,
                false,
                0,
            )
            .unwrap();
        handle.primitive_applied().unwrap();
        handle.llm_returned_terminal().unwrap();
        handle.boundary_complete().unwrap();

        let snapshot = handle.snapshot();
        assert_eq!(snapshot.turn_phase, TurnPhase::Completed);
        assert_eq!(snapshot.active_run_id, None);
    }

    #[test]
    fn immediate_append_derives_content_shape() {
        let handle = RuntimeTurnStateHandle::ephemeral();
        let run_id = RunId(Uuid::from_u128(10));

        handle.start_immediate_append(run_id).unwrap();

        assert_eq!(
            handle.snapshot().admitted_content_shape,
            Some(meerkat_core::turn_execution_authority::ContentShape::ImmediateAppend)
        );
    }

    #[test]
    fn retry_schedule_is_recorded_and_attempt_guarded() {
        let handle = RuntimeTurnStateHandle::ephemeral();
        let run_id = RunId(Uuid::from_u128(9));

        handle
            .start_conversation_run(
                run_id,
                TurnPrimitiveKind::ConversationTurn,
                meerkat_core::turn_execution_authority::ContentShape::Conversation,
                false,
                false,
                0,
            )
            .unwrap();
        handle.primitive_applied().unwrap();

        handle.recoverable_failure(retry_schedule(2)).unwrap();

        let snapshot = handle.snapshot();
        assert_eq!(snapshot.turn_phase, TurnPhase::ErrorRecovery);
        assert_eq!(snapshot.llm_retry_attempt, 2);
        assert_eq!(snapshot.llm_retry_max_retries, 3);
        assert_eq!(snapshot.llm_retry_selected_delay_ms, 1_000);

        assert!(handle.retry_requested(1).is_err());
        handle.retry_requested(2).unwrap();
        assert_eq!(handle.snapshot().turn_phase, TurnPhase::CallingLlm);
    }

    #[test]
    fn fatal_failure_unknown_terminal_cause_rejects_before_machine_apply() {
        let handle = RuntimeTurnStateHandle::ephemeral();
        let run_id = RunId(Uuid::from_u128(11));

        handle
            .start_conversation_run(
                run_id,
                TurnPrimitiveKind::ConversationTurn,
                meerkat_core::turn_execution_authority::ContentShape::Conversation,
                false,
                false,
                0,
            )
            .unwrap();

        let err = handle
            .fatal_failure(unknown_terminal_cause_reason(
                "display text must not classify fatal failure",
            ))
            .expect_err("Unknown fatal cause should reject before state mutation");

        assert!(err.is_guard_rejected(), "expected guard rejection: {err:?}");
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.turn_phase, TurnPhase::ApplyingPrimitive);
        assert_eq!(snapshot.terminal_cause_kind, None);

        handle
            .fatal_failure(specific_terminal_cause_reason(
                meerkat_core::TurnTerminalCauseKind::FatalFailure,
                "explicit fatal failure",
            ))
            .expect("specific fatal cause should remain accepted");
        assert_eq!(
            handle.snapshot().terminal_cause_kind,
            Some(meerkat_core::TurnTerminalCauseKind::FatalFailure)
        );
        assert_eq!(
            handle.snapshot().terminal_failure_class,
            Some(meerkat_core::event::AgentErrorClass::Terminal)
        );
    }

    #[test]
    fn run_failed_effect_does_not_terminalize_runtime_state() {
        let handle = RuntimeTurnStateHandle::ephemeral();
        let run_id = RunId(Uuid::from_u128(12));

        handle
            .start_conversation_run(
                run_id.clone(),
                TurnPrimitiveKind::ConversationTurn,
                meerkat_core::turn_execution_authority::ContentShape::Conversation,
                false,
                false,
                0,
            )
            .unwrap();

        handle
            .run_failed(
                run_id.clone(),
                unknown_terminal_cause_reason("display text must not classify run failure"),
            )
            .expect("runtime-backed run_failed effect is observation-only");

        let snapshot = handle.snapshot();
        assert_eq!(snapshot.active_run_id, Some(run_id.clone()));
        assert_eq!(snapshot.turn_phase, TurnPhase::ApplyingPrimitive);
        assert_eq!(snapshot.terminal_cause_kind, None);

        handle
            .run_completed(run_id.clone())
            .expect("runtime-backed run_completed effect is observation-only");
        handle
            .run_cancelled(run_id)
            .expect("runtime-backed run_cancelled effect is observation-only");
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.turn_phase, TurnPhase::ApplyingPrimitive);
        assert_eq!(snapshot.terminal_cause_kind, None);
    }
}
