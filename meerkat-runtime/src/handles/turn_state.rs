//! Runtime impl of [`meerkat_core::handles::TurnStateHandle`].

use std::collections::BTreeSet;
use std::sync::Arc;

use meerkat_core::handles::{DslTransitionError, TurnStateHandle, TurnStateSnapshot};
use meerkat_core::lifecycle::RunId;
use meerkat_core::ops::{AsyncOpRef, OperationId, WaitPolicy};
use meerkat_core::retry::LlmRetrySchedule;
#[cfg(test)]
use meerkat_core::turn_execution_authority::TurnFailureSourceKind;
use meerkat_core::turn_execution_authority::{
    CallTimeoutSource as TurnCallTimeoutSource, CallTimeoutVerdict as TurnCallTimeoutVerdict,
    ContentShape, LlmFailureRecoveryKind, TurnExecutionEffect, TurnExecutionInput,
    TurnFailureReason, TurnFailureSource, TurnPhase, TurnPrimitiveKind, TurnTerminalCauseKind,
    TurnTerminalOutcome, terminal_outcome_for_budget_exceeded,
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

fn parse_effect_run_id(
    run_id: &mm_dsl::RunId,
    context: &'static str,
) -> Result<RunId, DslTransitionError> {
    uuid::Uuid::parse_str(&run_id.0)
        .map(RunId::from_uuid)
        .map_err(|err| {
            DslTransitionError::guard_rejected(
                context,
                format!(
                    "generated MeerkatMachine turn effect carried malformed run_id `{}`: {err}",
                    run_id.0
                ),
            )
        })
}

fn map_generated_turn_effect(
    effect: mm_dsl::MeerkatMachineEffect,
    context: &'static str,
) -> Result<Option<TurnExecutionEffect>, DslTransitionError> {
    Ok(Some(match effect {
        mm_dsl::MeerkatMachineEffect::TurnRunStarted { run_id } => {
            TurnExecutionEffect::RunStarted {
                run_id: parse_effect_run_id(&run_id, context)?,
            }
        }
        mm_dsl::MeerkatMachineEffect::TurnBoundaryApplied {
            run_id,
            boundary_sequence,
        } => TurnExecutionEffect::BoundaryApplied {
            run_id: parse_effect_run_id(&run_id, context)?,
            boundary_sequence,
        },
        mm_dsl::MeerkatMachineEffect::TurnRunCompleted { run_id, .. } => {
            TurnExecutionEffect::RunCompleted {
                run_id: parse_effect_run_id(&run_id, context)?,
            }
        }
        mm_dsl::MeerkatMachineEffect::TurnRunFailed {
            run_id,
            terminal_cause_kind,
            error,
        } => {
            let cause_kind: TurnTerminalCauseKind = terminal_cause_kind.into();
            if !cause_kind.is_specific_failure_cause() {
                return Err(DslTransitionError::guard_rejected(
                    context,
                    "generated MeerkatMachine TurnRunFailed effect carried unknown terminal_cause_kind",
                ));
            }
            TurnExecutionEffect::RunFailed {
                run_id: parse_effect_run_id(&run_id, context)?,
                reason: TurnFailureReason::with_cause(
                    cause_kind,
                    cause_kind.agent_error_class(),
                    error,
                ),
            }
        }
        mm_dsl::MeerkatMachineEffect::TurnRunCancelled { run_id, .. } => {
            TurnExecutionEffect::RunCancelled {
                run_id: parse_effect_run_id(&run_id, context)?,
            }
        }
        mm_dsl::MeerkatMachineEffect::TurnCheckCompaction => TurnExecutionEffect::CheckCompaction,
        mm_dsl::MeerkatMachineEffect::LlmFailureRecoveryClassified { recovery } => {
            TurnExecutionEffect::LlmFailureRecoveryClassified {
                recovery: match recovery {
                    mm_dsl::LlmFailureRecoveryKind::Recover => LlmFailureRecoveryKind::Recover,
                    mm_dsl::LlmFailureRecoveryKind::Exhausted => LlmFailureRecoveryKind::Exhausted,
                    mm_dsl::LlmFailureRecoveryKind::Fatal => LlmFailureRecoveryKind::Fatal,
                },
            }
        }
        mm_dsl::MeerkatMachineEffect::AssistantOutputClassified {
            empty_response_terminal,
        } => TurnExecutionEffect::AssistantOutputClassified {
            empty_response_terminal,
        },
        mm_dsl::MeerkatMachineEffect::CallTimeoutClassified {
            verdict,
            timeout_ms,
        } => TurnExecutionEffect::CallTimeoutClassified {
            verdict: match verdict {
                mm_dsl::CallTimeoutVerdict::RetryableCallTimeout => {
                    TurnCallTimeoutVerdict::RetryableCallTimeout
                }
                mm_dsl::CallTimeoutVerdict::TerminalTurnBudget => {
                    TurnCallTimeoutVerdict::TerminalTurnBudget
                }
            },
            timeout_ms,
        },
        _ => return Ok(None),
    }))
}

impl TurnStateHandle for RuntimeTurnStateHandle {
    fn apply_turn_input(
        &self,
        input: TurnExecutionInput,
    ) -> Result<Vec<TurnExecutionEffect>, DslTransitionError> {
        let context = "TurnStateHandle::apply_turn_input";
        let dsl_input = match input {
            TurnExecutionInput::StartConversationRun {
                run_id,
                primitive_kind,
                admitted_content_shape,
                vision_enabled,
                image_tool_results_enabled,
                max_extraction_retries,
            } => mm_dsl::MeerkatMachineInput::StartConversationRun {
                run_id: mm_dsl::RunId::from_domain(&run_id),
                primitive_kind: mm_dsl::TurnPrimitiveKind::from(primitive_kind),
                admitted_content_shape: mm_dsl::ContentShape::from(admitted_content_shape),
                vision_enabled,
                image_tool_results_enabled,
                max_extraction_retries,
            },
            TurnExecutionInput::StartImmediateAppend { run_id } => {
                mm_dsl::MeerkatMachineInput::StartImmediateAppend {
                    run_id: mm_dsl::RunId::from_domain(&run_id),
                }
            }
            TurnExecutionInput::StartImmediateContext { run_id } => {
                mm_dsl::MeerkatMachineInput::StartImmediateContext {
                    run_id: mm_dsl::RunId::from_domain(&run_id),
                }
            }
            TurnExecutionInput::PrimitiveApplied { run_id } => {
                mm_dsl::MeerkatMachineInput::PrimitiveApplied {
                    run_id: mm_dsl::RunId::from_domain(&run_id),
                }
            }
            TurnExecutionInput::LlmReturnedToolCalls { run_id, tool_count } => {
                mm_dsl::MeerkatMachineInput::LlmReturnedToolCalls {
                    run_id: mm_dsl::RunId::from_domain(&run_id),
                    tool_count: u64::from(tool_count),
                }
            }
            TurnExecutionInput::LlmReturnedTerminal { run_id } => {
                mm_dsl::MeerkatMachineInput::LlmReturnedTerminal {
                    run_id: mm_dsl::RunId::from_domain(&run_id),
                }
            }
            TurnExecutionInput::RegisterPendingOps {
                run_id,
                op_refs,
                barrier_operation_ids,
                ..
            } => mm_dsl::MeerkatMachineInput::RegisterPendingOps {
                run_id: mm_dsl::RunId::from_domain(&run_id),
                op_refs: op_refs
                    .iter()
                    .map(|op_ref| op_ref.operation_id.to_string())
                    .collect(),
                // #354: barrier ids are now a typed `Set<OperationId>` in the
                // DSL. The token repr stays the plain-UUID Display string that
                // `parse_operation_id` round-trips (NOT the JSON `from_domain`
                // form), so the projection back to domain `OperationId` is
                // lossless.
                barrier_operation_ids: barrier_operation_ids
                    .iter()
                    .map(|id| mm_dsl::OperationId::from(id.to_string()))
                    .collect(),
            },
            TurnExecutionInput::ToolCallsResolved { run_id } => {
                mm_dsl::MeerkatMachineInput::ToolCallsResolved {
                    run_id: mm_dsl::RunId::from_domain(&run_id),
                }
            }
            TurnExecutionInput::OpsBarrierSatisfied {
                run_id,
                operation_ids,
            } => mm_dsl::MeerkatMachineInput::OpsBarrierSatisfied {
                run_id: mm_dsl::RunId::from_domain(&run_id),
                // #354: typed `Set<OperationId>`; same plain-UUID token repr.
                operation_ids: operation_ids
                    .iter()
                    .map(|id| mm_dsl::OperationId::from(id.to_string()))
                    .collect(),
            },
            TurnExecutionInput::BoundaryContinue { run_id } => {
                mm_dsl::MeerkatMachineInput::BoundaryContinue {
                    run_id: mm_dsl::RunId::from_domain(&run_id),
                }
            }
            TurnExecutionInput::BoundaryComplete { run_id } => {
                mm_dsl::MeerkatMachineInput::BoundaryComplete {
                    run_id: mm_dsl::RunId::from_domain(&run_id),
                }
            }
            TurnExecutionInput::RecoverableFailure { run_id, retry } => {
                mm_dsl::MeerkatMachineInput::RecoverableFailure {
                    run_id: mm_dsl::RunId::from_domain(&run_id),
                    failure_kind: retry.failure.kind.into(),
                    retry_attempt: u64::from(retry.plan.attempt),
                    max_retries: u64::from(retry.plan.max_retries),
                    selected_delay_ms: retry.plan.selected_delay_ms,
                    error: retry.failure.message,
                }
            }
            TurnExecutionInput::FatalFailure { run_id, failure } => {
                mm_dsl::MeerkatMachineInput::FatalFailure {
                    run_id: mm_dsl::RunId::from_domain(&run_id),
                    terminal_failure_source: mm_dsl::RunFailureSourceKind::from(
                        failure.source_kind,
                    ),
                    error: failure.message,
                }
            }
            TurnExecutionInput::RetryRequested {
                run_id,
                retry_attempt,
            } => mm_dsl::MeerkatMachineInput::RetryRequested {
                run_id: mm_dsl::RunId::from_domain(&run_id),
                retry_attempt: u64::from(retry_attempt),
            },
            TurnExecutionInput::ClassifyLlmFailureRecovery {
                failure_kind,
                retry_attempt,
                max_retries,
            } => mm_dsl::MeerkatMachineInput::ClassifyLlmFailureRecovery {
                failure_kind: failure_kind.map(Into::into),
                retry_attempt: u64::from(retry_attempt),
                max_retries: u64::from(max_retries),
            },
            TurnExecutionInput::ClassifyAssistantOutput {
                has_visible_or_actionable,
            } => mm_dsl::MeerkatMachineInput::ClassifyAssistantOutput {
                has_visible_or_actionable,
            },
            TurnExecutionInput::ClassifyCallTimeout { source, timeout_ms } => {
                mm_dsl::MeerkatMachineInput::ClassifyCallTimeout {
                    source: match source {
                        TurnCallTimeoutSource::CallBudget => mm_dsl::CallTimeoutSource::CallBudget,
                        TurnCallTimeoutSource::TurnBudget => mm_dsl::CallTimeoutSource::TurnBudget,
                    },
                    timeout_ms,
                }
            }
            TurnExecutionInput::CancelNow { run_id } => mm_dsl::MeerkatMachineInput::CancelNow {
                run_id: mm_dsl::RunId::from_domain(&run_id),
            },
            TurnExecutionInput::CancelAfterBoundary { run_id } => {
                mm_dsl::MeerkatMachineInput::RequestCancelAfterBoundary {
                    run_id: mm_dsl::RunId::from_domain(&run_id),
                }
            }
            TurnExecutionInput::CancellationObserved { run_id } => {
                mm_dsl::MeerkatMachineInput::CancellationObserved {
                    run_id: mm_dsl::RunId::from_domain(&run_id),
                }
            }
            TurnExecutionInput::AcknowledgeTerminal { run_id } => {
                let outcome = self.snapshot().terminal_outcome.ok_or_else(|| {
                    DslTransitionError::guard_rejected(
                        context,
                        "generated MeerkatMachine terminal outcome missing for AcknowledgeTerminal",
                    )
                })?;
                mm_dsl::MeerkatMachineInput::AcknowledgeTerminal {
                    run_id: mm_dsl::RunId::from_domain(&run_id),
                    outcome: mm_dsl::TurnTerminalOutcome::from(outcome),
                }
            }
            TurnExecutionInput::TurnLimitReached { run_id } => {
                mm_dsl::MeerkatMachineInput::TurnLimitReached {
                    run_id: mm_dsl::RunId::from_domain(&run_id),
                }
            }
            TurnExecutionInput::BudgetExhausted { run_id } => {
                mm_dsl::MeerkatMachineInput::BudgetExhausted {
                    run_id: mm_dsl::RunId::from_domain(&run_id),
                }
            }
            TurnExecutionInput::TimeBudgetExceeded { run_id } => {
                mm_dsl::MeerkatMachineInput::TimeBudgetExceeded {
                    run_id: mm_dsl::RunId::from_domain(&run_id),
                }
            }
            TurnExecutionInput::BudgetLimitExceeded { run_id, exceeded } => {
                match terminal_outcome_for_budget_exceeded(exceeded) {
                    TurnTerminalOutcome::TimeBudgetExceeded => {
                        mm_dsl::MeerkatMachineInput::TimeBudgetExceeded {
                            run_id: mm_dsl::RunId::from_domain(&run_id),
                        }
                    }
                    TurnTerminalOutcome::BudgetExhausted => {
                        mm_dsl::MeerkatMachineInput::BudgetExhausted {
                            run_id: mm_dsl::RunId::from_domain(&run_id),
                        }
                    }
                    _ => unreachable!("budget exceeded maps only to budget terminal outcomes"),
                }
            }
            TurnExecutionInput::EnterExtraction {
                run_id,
                max_retries,
            } => mm_dsl::MeerkatMachineInput::EnterExtraction {
                run_id: mm_dsl::RunId::from_domain(&run_id),
                max_extraction_retries: u64::from(max_retries),
            },
            TurnExecutionInput::ExtractionValidationPassed { run_id } => {
                mm_dsl::MeerkatMachineInput::ExtractionValidationPassed {
                    run_id: mm_dsl::RunId::from_domain(&run_id),
                }
            }
            TurnExecutionInput::ExtractionValidationFailed { run_id, error } => {
                mm_dsl::MeerkatMachineInput::ExtractionValidationFailed {
                    run_id: mm_dsl::RunId::from_domain(&run_id),
                    error,
                }
            }
            TurnExecutionInput::ExtractionFailed { run_id, error } => {
                mm_dsl::MeerkatMachineInput::ExtractionFailed {
                    run_id: mm_dsl::RunId::from_domain(&run_id),
                    error,
                }
            }
            TurnExecutionInput::ExtractionStart { run_id } => {
                mm_dsl::MeerkatMachineInput::ExtractionStart {
                    run_id: mm_dsl::RunId::from_domain(&run_id),
                }
            }
            TurnExecutionInput::ForceCancelNoRun => mm_dsl::MeerkatMachineInput::ForceCancelNoRun,
        };
        self.dsl
            .apply_input_with_effects(dsl_input, context)?
            .into_iter()
            .map(|effect| map_generated_turn_effect(effect, context))
            .filter_map(Result::transpose)
            .collect()
    }

    fn start_conversation_run(
        &self,
        run_id: RunId,
        primitive_kind: TurnPrimitiveKind,
        admitted_content_shape: ContentShape,
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

    fn primitive_applied(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::PrimitiveApplied {
                run_id: mm_dsl::RunId::from_domain(&run_id),
            },
            "TurnStateHandle::primitive_applied",
        )
    }

    fn llm_returned_tool_calls(
        &self,
        run_id: RunId,
        tool_count: u64,
    ) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::LlmReturnedToolCalls {
            run_id,
            tool_count: u32::try_from(tool_count).map_err(|_| {
                DslTransitionError::guard_rejected(
                    "TurnStateHandle::llm_returned_tool_calls",
                    "tool_count exceeds u32 turn input range",
                )
            })?,
        })
        .map(|_| ())
    }

    fn llm_returned_terminal(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::LlmReturnedTerminal { run_id })
            .map(|_| ())
    }

    fn register_pending_ops(
        &self,
        run_id: RunId,
        op_refs: BTreeSet<AsyncOpRef>,
        barrier_operation_ids: BTreeSet<OperationId>,
    ) -> Result<(), DslTransitionError> {
        let has_barrier_ops = !barrier_operation_ids.is_empty();
        self.apply_turn_input(TurnExecutionInput::RegisterPendingOps {
            run_id,
            op_refs: op_refs.into_iter().collect(),
            barrier_operation_ids: barrier_operation_ids.into_iter().collect(),
            has_barrier_ops,
        })
        .map(|_| ())
    }

    fn tool_calls_resolved(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::ToolCallsResolved { run_id })
            .map(|_| ())
    }

    fn ops_barrier_satisfied(
        &self,
        run_id: RunId,
        operation_ids: BTreeSet<OperationId>,
    ) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::OpsBarrierSatisfied {
            run_id,
            operation_ids: operation_ids.into_iter().collect(),
        })
        .map(|_| ())
    }

    fn boundary_continue(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::BoundaryContinue { run_id })
            .map(|_| ())
    }

    fn boundary_complete(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::BoundaryComplete { run_id })
            .map(|_| ())
    }

    fn enter_extraction(&self, run_id: RunId, max_retries: u32) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::EnterExtraction {
            run_id,
            max_retries,
        })
        .map(|_| ())
    }

    fn extraction_start(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::ExtractionStart { run_id })
            .map(|_| ())
    }

    fn extraction_validation_passed(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::ExtractionValidationPassed { run_id })
            .map(|_| ())
    }

    fn extraction_validation_failed(
        &self,
        run_id: RunId,
        error: String,
    ) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::ExtractionValidationFailed { run_id, error })
            .map(|_| ())
    }

    fn extraction_failed(&self, run_id: RunId, error: String) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::ExtractionFailed { run_id, error })
            .map(|_| ())
    }

    fn recoverable_failure(
        &self,
        run_id: RunId,
        retry: LlmRetrySchedule,
    ) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::RecoverableFailure { run_id, retry })
            .map(|_| ())
    }

    fn fatal_failure(
        &self,
        run_id: RunId,
        failure: TurnFailureSource,
    ) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::FatalFailure { run_id, failure })
            .map(|_| ())
    }

    fn retry_requested(&self, run_id: RunId, retry_attempt: u32) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::RetryRequested {
            run_id,
            retry_attempt,
        })
        .map(|_| ())
    }

    fn cancel_now(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::CancelNow { run_id })
            .map(|_| ())
    }

    fn request_cancel_after_boundary(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::CancelAfterBoundary { run_id })
            .map(|_| ())
    }

    fn cancellation_observed(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::CancellationObserved { run_id })
            .map(|_| ())
    }

    fn acknowledge_terminal(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::AcknowledgeTerminal { run_id })
            .map(|_| ())
    }

    fn turn_limit_reached(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::TurnLimitReached { run_id })
            .map(|_| ())
    }

    fn budget_exhausted(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::BudgetExhausted { run_id })
            .map(|_| ())
    }

    fn time_budget_exceeded(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::TimeBudgetExceeded { run_id })
            .map(|_| ())
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

    #[allow(clippy::expect_used)]
    fn snapshot(&self) -> TurnStateSnapshot {
        let state = self.dsl.snapshot_state();
        let turn_phase = map_turn_phase(state.turn_phase);
        let barrier_operation_ids: BTreeSet<_> = state
            .barrier_operation_ids
            .iter()
            .map(|id| parse_operation_id(id.0.as_str()))
            .collect();
        let pending_op_refs = state
            .pending_op_refs
            .iter()
            .map(|id| {
                let operation_id = parse_operation_id(id);
                AsyncOpRef {
                    wait_policy: if barrier_operation_ids.contains(&operation_id) {
                        WaitPolicy::Barrier
                    } else {
                        WaitPolicy::Detached
                    },
                    operation_id,
                }
            })
            .collect();
        let turn_terminal = classify_turn_terminal(&state);
        let active_run_id = if turn_terminal {
            None
        } else {
            state.current_run_id.as_ref().map(parse_snapshot_run_id)
        };
        TurnStateSnapshot {
            active_run_id,
            loop_state: map_loop_state(state.turn_phase),
            turn_phase,
            turn_terminal,
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
            extraction_attempts: state.extraction_attempts,
            max_extraction_retries: state.max_extraction_retries,
            llm_retry_attempt: u32::try_from(state.llm_retry_attempt)
                .expect("generated MeerkatMachine llm_retry_attempt must fit u32"),
            llm_retry_max_retries: u32::try_from(state.llm_retry_max_retries)
                .expect("generated MeerkatMachine llm_retry_max_retries must fit u32"),
            llm_retry_selected_delay_ms: state.llm_retry_selected_delay_ms,
        }
    }
}

#[allow(clippy::expect_used)]
fn parse_operation_id(value: &str) -> OperationId {
    uuid::Uuid::parse_str(value)
        .map(OperationId)
        .expect("generated MeerkatMachine operation id projection must be well formed")
}

#[allow(clippy::expect_used)]
fn parse_snapshot_run_id(run_id: &mm_dsl::RunId) -> RunId {
    uuid::Uuid::parse_str(&run_id.0)
        .map(RunId::from_uuid)
        .expect("generated MeerkatMachine current_run_id projection must be well formed")
}

/// Mirror the canonical MeerkatMachine turn-terminality verdict over the
/// recovered turn state.
///
/// The terminality verdict (which turn phases are terminal) is a machine fact:
/// this owner extracts no fact — it recovers a read-only authority from the DSL
/// state, drives the `ClassifyTurnTerminality` input, and mirrors the emitted
/// `TurnTerminalityClassified.terminal`. It decides nothing. Fails closed:
/// an unclassifiable state is treated as terminal so a live run is never assumed
/// to still be active off an unreadable snapshot.
fn classify_turn_terminal(state: &mm_dsl::MeerkatMachineState) -> bool {
    let Ok(mut authority) = mm_dsl::MeerkatMachineAuthority::recover_from_state(state.clone())
    else {
        return true;
    };
    let Ok(transition) = mm_dsl::MeerkatMachineMutator::apply(
        &mut authority,
        mm_dsl::MeerkatMachineInput::ClassifyTurnTerminality {},
    ) else {
        return true;
    };
    let mut classified = None;
    for effect in transition.effects() {
        if let mm_dsl::MeerkatMachineEffect::TurnTerminalityClassified { terminal } = effect
            && classified.replace(*terminal).is_some()
        {
            return true;
        }
    }
    classified.unwrap_or(true)
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
        retry_schedule_with_kind(attempt, 3, LlmRetryFailureKind::RateLimited)
    }

    fn retry_schedule_with_kind(
        attempt: u32,
        max_retries: u32,
        kind: LlmRetryFailureKind,
    ) -> LlmRetrySchedule {
        LlmRetrySchedule {
            failure: LlmRetryFailure {
                provider: "test".to_string(),
                kind,
                retry_after_ms: Some(1_000),
                duration_ms: None,
                message: "rate limited".to_string(),
            },
            plan: LlmRetryPlan {
                attempt,
                max_retries,
                computed_delay_ms: 500,
                selected_delay_ms: 1_000,
                retry_after_hint_ms: Some(1_000),
                rate_limit_floor_applied: false,
                budget_capped: false,
            },
        }
    }

    fn start_running_conversation_turn(handle: &RuntimeTurnStateHandle, run_id: &RunId) {
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
        handle.primitive_applied(run_id.clone()).unwrap();
    }

    fn unknown_failure_source(message: &'static str) -> TurnFailureSource {
        TurnFailureSource::new(TurnFailureSourceKind::Unknown, message)
    }

    fn failure_source(
        source_kind: TurnFailureSourceKind,
        message: &'static str,
    ) -> TurnFailureSource {
        TurnFailureSource::new(source_kind, message)
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
    fn primitive_applied_rejects_mismatched_run_id() {
        let handle = RuntimeTurnStateHandle::ephemeral();
        let run_id = RunId(Uuid::from_u128(21));
        let stale_run_id = RunId(Uuid::from_u128(22));

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

        assert!(handle.primitive_applied(stale_run_id).is_err());
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.active_run_id, Some(run_id));
        assert_eq!(snapshot.turn_phase, TurnPhase::ApplyingPrimitive);
    }

    #[test]
    fn post_primitive_observation_rejects_mismatched_run_id() {
        let handle = RuntimeTurnStateHandle::ephemeral();
        let run_id = RunId(Uuid::from_u128(23));
        let stale_run_id = RunId(Uuid::from_u128(24));

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
        handle.primitive_applied(run_id.clone()).unwrap();

        assert!(handle.llm_returned_terminal(stale_run_id).is_err());
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.active_run_id, Some(run_id));
        assert_eq!(snapshot.turn_phase, TurnPhase::CallingLlm);
    }

    #[test]
    fn snapshot_clears_active_run_id_after_terminal_turn() {
        let handle = RuntimeTurnStateHandle::ephemeral();
        let run_id = RunId(Uuid::from_u128(8));

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
        handle.primitive_applied(run_id.clone()).unwrap();
        handle.llm_returned_terminal(run_id.clone()).unwrap();
        handle.boundary_complete(run_id).unwrap();

        let snapshot = handle.snapshot();
        assert_eq!(snapshot.turn_phase, TurnPhase::Completed);
        assert_eq!(snapshot.active_run_id, None);
    }

    #[test]
    fn cancel_after_boundary_cancels_continuation_boundary() {
        let handle = RuntimeTurnStateHandle::ephemeral();
        let run_id = RunId(Uuid::from_u128(18));

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
        handle.primitive_applied(run_id.clone()).unwrap();
        handle.llm_returned_tool_calls(run_id.clone(), 1).unwrap();
        handle
            .register_pending_ops(run_id.clone(), BTreeSet::new(), BTreeSet::new())
            .unwrap();
        handle.tool_calls_resolved(run_id.clone()).unwrap();
        handle
            .request_cancel_after_boundary(run_id.clone())
            .unwrap();
        handle.boundary_continue(run_id).unwrap();

        let snapshot = handle.snapshot();
        assert_eq!(snapshot.turn_phase, TurnPhase::Cancelled);
        assert_eq!(
            snapshot.terminal_outcome,
            Some(TurnTerminalOutcome::Cancelled)
        );
        assert!(!snapshot.cancel_after_boundary);
        assert_eq!(snapshot.active_run_id, None);
    }

    #[test]
    fn cancel_after_boundary_cancels_terminal_boundary() {
        let handle = RuntimeTurnStateHandle::ephemeral();
        let run_id = RunId(Uuid::from_u128(19));

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
        handle.primitive_applied(run_id.clone()).unwrap();
        handle.llm_returned_terminal(run_id.clone()).unwrap();
        handle
            .request_cancel_after_boundary(run_id.clone())
            .unwrap();
        handle.boundary_complete(run_id).unwrap();

        let snapshot = handle.snapshot();
        assert_eq!(snapshot.turn_phase, TurnPhase::Cancelled);
        assert_eq!(
            snapshot.terminal_outcome,
            Some(TurnTerminalOutcome::Cancelled)
        );
        assert!(!snapshot.cancel_after_boundary);
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
    fn cancel_after_boundary_cancels_immediate_boundary() {
        let handle = RuntimeTurnStateHandle::ephemeral();
        let run_id = RunId(Uuid::from_u128(20));

        handle.start_immediate_append(run_id.clone()).unwrap();
        handle
            .request_cancel_after_boundary(run_id.clone())
            .unwrap();
        handle.primitive_applied(run_id).unwrap();

        let snapshot = handle.snapshot();
        assert_eq!(snapshot.turn_phase, TurnPhase::Cancelled);
        assert_eq!(
            snapshot.terminal_outcome,
            Some(TurnTerminalOutcome::Cancelled)
        );
        assert!(!snapshot.cancel_after_boundary);
        assert_eq!(snapshot.active_run_id, None);
    }

    #[test]
    fn retry_schedule_is_recorded_and_attempt_guarded() {
        let handle = RuntimeTurnStateHandle::ephemeral();
        let run_id = RunId(Uuid::from_u128(9));

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
        handle.primitive_applied(run_id.clone()).unwrap();

        handle
            .recoverable_failure(run_id.clone(), retry_schedule(2))
            .unwrap();

        let snapshot = handle.snapshot();
        assert_eq!(snapshot.turn_phase, TurnPhase::ErrorRecovery);
        assert_eq!(snapshot.llm_retry_attempt, 2);
        assert_eq!(snapshot.llm_retry_max_retries, 3);
        assert_eq!(snapshot.llm_retry_selected_delay_ms, 1_000);

        assert!(handle.retry_requested(run_id.clone(), 1).is_err());
        handle.retry_requested(run_id, 2).unwrap();
        assert_eq!(handle.snapshot().turn_phase, TurnPhase::CallingLlm);
    }

    /// P0 Dogma Invariant 1: the machine — not the shell `RetryPolicy` — is the
    /// authority on retry exhaustion. A `RecoverableFailure` whose one-based
    /// `retry_attempt` exceeds `max_retries` must be machine-rejected so the
    /// turn cannot enter `ErrorRecovery` past exhaustion.
    #[test]
    fn recoverable_failure_past_exhaustion_is_machine_rejected() {
        let handle = RuntimeTurnStateHandle::ephemeral();
        let run_id = RunId(Uuid::from_u128(31));
        start_running_conversation_turn(&handle, &run_id);

        // attempt 4 with max_retries 3 is past exhaustion.
        let exhausted = retry_schedule_with_kind(4, 3, LlmRetryFailureKind::RateLimited);
        let err = handle
            .recoverable_failure(run_id.clone(), exhausted)
            .expect_err("exhausted retry must be rejected by the machine");
        assert!(err.is_guard_rejected(), "expected guard rejection: {err:?}");

        // The turn never entered recovery; it remains in CallingLlm.
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.turn_phase, TurnPhase::CallingLlm);
        assert_eq!(snapshot.llm_retry_attempt, 0);

        // A retry at the exhaustion boundary (attempt == max_retries) is still
        // legitimate and the machine accepts it.
        let last = retry_schedule_with_kind(3, 3, LlmRetryFailureKind::NetworkTimeout);
        handle.recoverable_failure(run_id, last).unwrap();
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.turn_phase, TurnPhase::ErrorRecovery);
        assert_eq!(snapshot.llm_retry_attempt, 3);
        assert_eq!(snapshot.llm_retry_max_retries, 3);
    }

    #[test]
    fn fatal_failure_unknown_source_rejects_before_machine_apply() {
        let handle = RuntimeTurnStateHandle::ephemeral();
        let run_id = RunId(Uuid::from_u128(11));

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

        let err = handle
            .fatal_failure(
                run_id.clone(),
                unknown_failure_source("display text must not classify fatal failure"),
            )
            .expect_err("unknown fatal source should reject before state mutation");

        assert!(err.is_guard_rejected(), "expected guard rejection: {err:?}");
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.turn_phase, TurnPhase::ApplyingPrimitive);
        assert_eq!(snapshot.terminal_cause_kind, None);

        handle
            .fatal_failure(
                run_id,
                failure_source(TurnFailureSourceKind::InternalError, "fatal failure"),
            )
            .expect("specific fatal source should remain accepted");
        assert_eq!(
            handle.snapshot().terminal_cause_kind,
            Some(meerkat_core::TurnTerminalCauseKind::FatalFailure)
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
                TurnFailureReason::with_cause(
                    meerkat_core::TurnTerminalCauseKind::Unknown,
                    meerkat_core::event::AgentErrorClass::Internal,
                    "display text must not classify run failure",
                ),
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
