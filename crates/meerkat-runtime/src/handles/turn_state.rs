//! Runtime impl of [`meerkat_core::handles::TurnStateHandle`].

use std::collections::BTreeSet;
use std::sync::Arc;

use meerkat_core::handles::{
    DslTransitionError, TurnStateHandle, TurnStateScalars, TurnStateSnapshot,
};
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
    standalone_session_id: Option<meerkat_core::SessionId>,
}

impl RuntimeTurnStateHandle {
    /// Construct a handle backed by the session's shared DSL authority.
    pub fn new(dsl: Arc<HandleDslAuthority>) -> Self {
        Self {
            dsl,
            standalone_session_id: None,
        }
    }

    /// Construct the standalone facade handle. With no runtime loop to stage
    /// `Prepare`, the turn handle performs that generated input immediately
    /// before each run-start input on the same shared authority.
    pub(crate) fn standalone(
        dsl: Arc<HandleDslAuthority>,
        session_id: meerkat_core::SessionId,
    ) -> Self {
        Self {
            dsl,
            standalone_session_id: Some(session_id),
        }
    }

    /// Construct a handle backed by an ephemeral DSL authority.
    pub fn ephemeral() -> Self {
        Self::new(Arc::new(HandleDslAuthority::ephemeral()))
    }

    /// Construct the generated process-local turn authority used by one
    /// noncommitting live bridge execution.
    ///
    /// This owns a fresh DSL authority with no session runtime entry,
    /// durability-health gate, RuntimeStore coordinator, or shared ordinary
    /// member-machine handle. Dropping it discards the complete turn state.
    pub(crate) fn isolated_live_bridge() -> Self {
        Self::ephemeral()
    }

    fn prepare_standalone_run(&self, run_id: &RunId) -> Result<(), DslTransitionError> {
        let Some(session_id) = self.standalone_session_id.as_ref() else {
            return Ok(());
        };
        // Intra-machine generated input: no routed effect crosses a machine
        // boundary, so the CompositionDispatcher is not applicable here.
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::Prepare {
                session_id: mm_dsl::SessionId::from_domain(session_id),
                run_id: mm_dsl::RunId::from_domain(run_id),
            },
            "TurnStateHandle::standalone_prepare",
        )
    }

    fn close_standalone_turn(
        &self,
        run_id: &RunId,
        terminal_phase: mm_dsl::TurnPhase,
    ) -> Result<(), DslTransitionError> {
        let Some(session_id) = self.standalone_session_id.as_ref() else {
            // Runtime-backed lifecycle publication remains owned by Commit/Fail.
            return Ok(());
        };
        // This generated, effect-free close stays within the shared authority;
        // it emits no cross-machine route for a CompositionDispatcher to consume.
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::CloseStandaloneTurn {
                session_id: mm_dsl::SessionId::from_domain(session_id),
                run_id: mm_dsl::RunId::from_domain(run_id),
                terminal_phase,
            },
            "TurnStateHandle::close_standalone_turn",
        )
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

impl RuntimeTurnStateHandle {
    /// Apply one typed turn input and run `sample` over the state that input
    /// produced, in the same authority critical section as the transition.
    fn apply_turn_input_sampling<S>(
        &self,
        input: TurnExecutionInput,
        sample: impl FnOnce(&mm_dsl::MeerkatMachineState) -> S,
    ) -> Result<(Vec<TurnExecutionEffect>, S), DslTransitionError> {
        let context = "TurnStateHandle::apply_turn_input";
        let standalone_run_id = match &input {
            TurnExecutionInput::StartConversationRun { run_id, .. }
            | TurnExecutionInput::StartImmediateAppend { run_id } => Some(run_id),
            _ => None,
        };
        if let Some(run_id) = standalone_run_id {
            self.prepare_standalone_run(run_id)?;
        }
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
            TurnExecutionInput::CallbackPending { run_id } => {
                mm_dsl::MeerkatMachineInput::CallbackPending {
                    run_id: mm_dsl::RunId::from_domain(&run_id),
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
            TurnExecutionInput::TurnLimitReached {
                run_id,
                turn_count,
                max_turns,
            } => mm_dsl::MeerkatMachineInput::TurnLimitReached {
                run_id: mm_dsl::RunId::from_domain(&run_id),
                turn_count,
                max_turns,
            },
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
        let (transition, sampled) = self
            .dsl
            .apply_input_with_transition_and_sample(dsl_input, context, sample)?;
        let effects = transition
            .into_effects()
            .into_iter()
            .map(|effect| map_generated_turn_effect(effect, context))
            .filter_map(Result::transpose)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((effects, sampled))
    }
}

impl TurnStateHandle for RuntimeTurnStateHandle {
    fn isolated_live_bridge_child(&self) -> Result<Arc<dyn TurnStateHandle>, DslTransitionError> {
        Ok(Arc::new(Self::isolated_live_bridge()))
    }

    fn apply_turn_input(
        &self,
        input: TurnExecutionInput,
    ) -> Result<Vec<TurnExecutionEffect>, DslTransitionError> {
        self.apply_turn_input_sampling(input, |_| ())
            .map(|(effects, ())| effects)
    }

    fn apply_turn_input_observing_phase(
        &self,
        input: TurnExecutionInput,
    ) -> Result<(Vec<TurnExecutionEffect>, TurnPhase), DslTransitionError> {
        self.apply_turn_input_sampling(input, |state| map_turn_phase(state.turn_phase))
    }

    fn turn_phase(&self) -> TurnPhase {
        self.dsl
            .with_state_lock(|state| map_turn_phase(state.turn_phase))
    }

    fn scalars(&self) -> TurnStateScalars {
        self.dsl.with_state_lock(project_turn_scalars)
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
        self.prepare_standalone_run(&run_id)?;
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
        self.prepare_standalone_run(&run_id)?;
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::StartImmediateAppend {
                run_id: mm_dsl::RunId::from_domain(&run_id),
            },
            "TurnStateHandle::start_immediate_append",
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

    fn turn_limit_reached(
        &self,
        run_id: RunId,
        turn_count: u64,
        max_turns: u64,
    ) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::TurnLimitReached {
            run_id,
            turn_count,
            max_turns,
        })
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

    fn run_completed(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.close_standalone_turn(&run_id, mm_dsl::TurnPhase::Completed)
    }

    fn run_failed(
        &self,
        run_id: RunId,
        _reason: TurnFailureReason,
    ) -> Result<(), DslTransitionError> {
        self.close_standalone_turn(&run_id, mm_dsl::TurnPhase::Failed)
    }

    fn run_cancelled(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.close_standalone_turn(&run_id, mm_dsl::TurnPhase::Cancelled)
    }

    #[allow(clippy::panic)]
    fn snapshot(&self) -> TurnStateSnapshot {
        // One coherent observation under the authority lock: the projection
        // copies only the fields it returns, and the generated terminality
        // classification recovers (and so validates) the one owned copy taken
        // in the same hold. The projection never panics under the lock, where
        // a panic would poison it: a malformed generated value is reported
        // here, after the lock is released.
        let (projection, observed) = self
            .dsl
            .with_state_lock(|state| (project_turn_state(state), state.clone()));
        let (mut snapshot, current_run_id) = match projection {
            Ok(projected) => projected,
            Err(MalformedTurnState(invariant)) => panic!("{invariant}"),
        };
        snapshot.turn_terminal = classify_turn_terminal(observed);
        if !snapshot.turn_terminal {
            snapshot.active_run_id = current_run_id;
        }
        snapshot
    }
}

/// The scalar facts of [`project_turn_state`], from a borrowed DSL state.
fn project_turn_scalars(state: &mm_dsl::MeerkatMachineState) -> TurnStateScalars {
    TurnStateScalars {
        turn_phase: map_turn_phase(state.turn_phase),
        cancel_after_boundary: state.cancel_after_boundary,
        has_barrier_ops: state.has_barrier_ops,
        extraction_active: state.extraction_active,
        terminal_outcome: state.terminal_outcome.map(TurnTerminalOutcome::from),
        terminal_cause_kind: state.terminal_cause_kind.map(Into::into),
        extraction_attempts: state.extraction_attempts,
    }
}

/// Project the returned turn-state fields from a borrowed DSL state, copying
/// nothing else. The terminality verdict is not a state field: the projection
/// is fail-closed (terminal, no active run) until the caller applies
/// [`classify_turn_terminal`]'s verdict, and the current run id it then
/// exposes for a live turn is returned beside it.
///
/// It runs under the authority lock, so it never panics there: a generated
/// value it cannot represent is returned as [`MalformedTurnState`], for the
/// caller to report after the lock is released.
fn project_turn_state(
    state: &mm_dsl::MeerkatMachineState,
) -> Result<(TurnStateSnapshot, Option<RunId>), MalformedTurnState> {
    let barrier_operation_ids = state
        .barrier_operation_ids
        .iter()
        .map(|id| parse_operation_id(id.0.as_str()))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let pending_op_refs = state
        .pending_op_refs
        .iter()
        .map(|id| {
            parse_operation_id(id).map(|operation_id| AsyncOpRef {
                wait_policy: if barrier_operation_ids.contains(&operation_id) {
                    WaitPolicy::Barrier
                } else {
                    WaitPolicy::Detached
                },
                operation_id,
            })
        })
        .collect::<Result<_, _>>()?;
    let snapshot = TurnStateSnapshot {
        active_run_id: None,
        terminal_run_id: state
            .turn_terminal_run_id
            .as_ref()
            .map(parse_snapshot_run_id)
            .transpose()?,
        loop_state: map_loop_state(state.turn_phase),
        turn_phase: map_turn_phase(state.turn_phase),
        turn_terminal: true,
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
        extraction_active: state.extraction_active,
        llm_retry_attempt: u32::try_from(state.llm_retry_attempt).map_err(|_| {
            MalformedTurnState("generated MeerkatMachine llm_retry_attempt must fit u32")
        })?,
        llm_retry_max_retries: u32::try_from(state.llm_retry_max_retries).map_err(|_| {
            MalformedTurnState("generated MeerkatMachine llm_retry_max_retries must fit u32")
        })?,
        llm_retry_selected_delay_ms: state.llm_retry_selected_delay_ms,
    };
    let current_run_id = state
        .current_run_id
        .as_ref()
        .map(parse_snapshot_run_id)
        .transpose()?;
    Ok((snapshot, current_run_id))
}

/// A generated turn-state value [`project_turn_state`] cannot represent.
#[derive(Debug)]
struct MalformedTurnState(&'static str);

fn parse_operation_id(value: &str) -> Result<OperationId, MalformedTurnState> {
    uuid::Uuid::parse_str(value).map(OperationId).map_err(|_| {
        MalformedTurnState("generated MeerkatMachine operation id projection must be well formed")
    })
}

fn parse_snapshot_run_id(run_id: &mm_dsl::RunId) -> Result<RunId, MalformedTurnState> {
    uuid::Uuid::parse_str(&run_id.0)
        .map(RunId::from_uuid)
        .map_err(|_| {
            MalformedTurnState(
                "generated MeerkatMachine current_run_id projection must be well formed",
            )
        })
}

/// Mirror the canonical MeerkatMachine turn-terminality verdict over the
/// recovered turn state.
///
/// The terminality verdict (which turn phases are terminal) is a machine fact:
/// this owner extracts no fact — it recovers a read-only authority from the DSL
/// state, drives the `ClassifyTurnTerminality` input, and mirrors the emitted
/// `TurnTerminalityClassified.terminal`. It decides nothing. Fails closed:
/// an unclassifiable state is treated as terminal so a live run is never assumed
/// to still be active off an unreadable snapshot. It takes the observed copy
/// by value: recovery validates and consumes it, so no second copy is made.
fn classify_turn_terminal(state: mm_dsl::MeerkatMachineState) -> bool {
    let Ok(mut authority) = mm_dsl::MeerkatMachineAuthority::recover_from_state(state) else {
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

    fn start_running_conversation_turn(handle: &dyn TurnStateHandle, run_id: &RunId) {
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

    fn registered_turn_handle(
        runtime_owned: bool,
    ) -> (
        meerkat_core::SessionId,
        Arc<HandleDslAuthority>,
        RuntimeTurnStateHandle,
    ) {
        let session_id = meerkat_core::SessionId::new();
        let authority = if runtime_owned {
            crate::meerkat_machine::dsl_authority::new_registered_authority(
                &session_id,
                &meerkat_core::RuntimeEpochId::new(),
            )
        } else {
            crate::meerkat_machine::dsl_authority::new_registered_authority_without_runtime_entry(
                &session_id,
            )
        }
        .unwrap();
        let dsl = Arc::new(HandleDslAuthority::from_shared(Arc::new(
            std::sync::Mutex::new(authority),
        )));
        let handle = if runtime_owned {
            RuntimeTurnStateHandle::new(dsl.clone())
        } else {
            RuntimeTurnStateHandle::standalone(dsl.clone(), session_id.clone())
        };
        (session_id, dsl, handle)
    }

    fn complete_turn(handle: &RuntimeTurnStateHandle, run_id: &RunId) {
        handle.llm_returned_terminal(run_id.clone()).unwrap();
        handle.boundary_complete(run_id.clone()).unwrap();
    }

    fn assert_standalone_close_rejected(
        handle: &RuntimeTurnStateHandle,
        run_id: &RunId,
        terminal_phase: mm_dsl::TurnPhase,
    ) {
        let before = handle.dsl.snapshot_state();
        let error = handle
            .close_standalone_turn(run_id, terminal_phase)
            .expect_err("the generated standalone close guard must refuse this input");
        assert_eq!(error.context, "TurnStateHandle::close_standalone_turn");
        assert_eq!(
            error.kind,
            meerkat_core::handles::DslRejectionKind::GuardRejected
        );
        assert_eq!(handle.dsl.snapshot_state(), before);
    }

    fn assert_recovered_standalone_close_rejected(
        session_id: &meerkat_core::SessionId,
        run_id: &RunId,
        state: mm_dsl::MeerkatMachineState,
        terminal_phase: mm_dsl::TurnPhase,
    ) {
        let authority = mm_dsl::MeerkatMachineAuthority::recover_from_state(state).unwrap();
        let handle = RuntimeTurnStateHandle::standalone(
            Arc::new(HandleDslAuthority::from_shared(Arc::new(
                std::sync::Mutex::new(authority),
            ))),
            session_id.clone(),
        );
        assert_standalone_close_rejected(&handle, run_id, terminal_phase);
    }

    #[test]
    fn standalone_terminal_close_has_no_composition_effects() {
        for phase in [
            mm_dsl::TurnPhase::Completed,
            mm_dsl::TurnPhase::Failed,
            mm_dsl::TurnPhase::Cancelled,
        ] {
            let (session_id, dsl, handle) = registered_turn_handle(false);
            let run_id = RunId::new();
            start_running_conversation_turn(&handle, &run_id);
            if phase == mm_dsl::TurnPhase::Completed {
                complete_turn(&handle, &run_id);
            } else if phase == mm_dsl::TurnPhase::Failed {
                handle
                    .apply_turn_input(TurnExecutionInput::FatalFailure {
                        run_id: run_id.clone(),
                        failure: failure_source(TurnFailureSourceKind::Llm, "synthetic failure"),
                    })
                    .unwrap();
            } else {
                handle.cancel_now(run_id.clone()).unwrap();
                handle.cancellation_observed(run_id.clone()).unwrap();
            }
            let mut expected = dsl.snapshot_state();
            let effects = dsl
                .apply_input_with_effects(
                    mm_dsl::MeerkatMachineInput::CloseStandaloneTurn {
                        session_id: mm_dsl::SessionId::from_domain(&session_id),
                        run_id: mm_dsl::RunId::from_domain(&run_id),
                        terminal_phase: phase,
                    },
                    "standalone close composition contract",
                )
                .unwrap();
            assert!(
                effects.is_empty(),
                "standalone close gained an undispatched effect"
            );
            expected.lifecycle_phase = mm_dsl::MeerkatPhase::Idle;
            expected.current_run_id = None;
            expected.pre_run_phase = None;
            assert_eq!(dsl.snapshot_state(), expected);
        }
    }

    #[test]
    fn standalone_terminal_close_preserves_evidence_and_admits_successive_runs() {
        let (_, dsl, handle) = registered_turn_handle(false);
        for _ in 0..3 {
            let run_id = RunId::new();
            start_running_conversation_turn(&handle, &run_id);
            complete_turn(&handle, &run_id);
            let before = dsl.snapshot_state();
            handle.run_completed(run_id.clone()).unwrap();
            let after = dsl.snapshot_state();
            assert_eq!(after.lifecycle_phase, mm_dsl::MeerkatPhase::Idle);
            assert_eq!(after.current_run_id, None);
            assert_eq!(after.pre_run_phase, None);
            assert_eq!(after.turn_terminal_run_id, before.turn_terminal_run_id);
            assert_eq!(after.terminal_outcome, before.terminal_outcome);
            assert_eq!(after.terminal_cause_kind, before.terminal_cause_kind);
            assert_eq!(after.runtime_completion_result_run_id, None);
            assert!(after.runtime_completion_result_resolved);
            assert_eq!(handle.snapshot().terminal_run_id, Some(run_id.clone()));
            assert_standalone_close_rejected(&handle, &run_id, mm_dsl::TurnPhase::Completed);
        }
    }

    #[test]
    fn standalone_terminal_close_preserves_failure_then_allows_retry() {
        let (_, dsl, handle) = registered_turn_handle(false);
        let run_id = RunId::new();
        start_running_conversation_turn(&handle, &run_id);
        let effects = handle
            .apply_turn_input(TurnExecutionInput::FatalFailure {
                run_id: run_id.clone(),
                failure: failure_source(TurnFailureSourceKind::Llm, "synthetic failure"),
            })
            .unwrap();
        let reason = effects
            .into_iter()
            .find_map(|effect| match effect {
                TurnExecutionEffect::RunFailed { reason, .. } => Some(reason),
                _ => None,
            })
            .unwrap();
        let before = handle.snapshot();
        handle.run_failed(run_id, reason).unwrap();
        assert_eq!(
            dsl.snapshot_state().lifecycle_phase,
            mm_dsl::MeerkatPhase::Idle
        );
        assert_eq!(handle.snapshot().terminal_outcome, before.terminal_outcome);
        assert_eq!(
            handle.snapshot().terminal_cause_kind,
            before.terminal_cause_kind
        );
        start_running_conversation_turn(&handle, &RunId::new());
    }

    #[test]
    fn standalone_terminal_close_requires_observed_cancellation_before_retry() {
        let (_, dsl, handle) = registered_turn_handle(false);
        let run_id = RunId::new();
        start_running_conversation_turn(&handle, &run_id);
        assert_standalone_close_rejected(&handle, &run_id, mm_dsl::TurnPhase::Cancelled);
        handle.cancel_now(run_id.clone()).unwrap();
        assert_eq!(handle.snapshot().turn_phase, TurnPhase::Cancelling);
        assert_standalone_close_rejected(&handle, &run_id, mm_dsl::TurnPhase::Cancelled);
        handle.cancellation_observed(run_id.clone()).unwrap();
        handle.run_cancelled(run_id.clone()).unwrap();
        assert_eq!(
            dsl.snapshot_state().lifecycle_phase,
            mm_dsl::MeerkatPhase::Idle
        );
        assert_eq!(handle.snapshot().terminal_run_id, Some(run_id));
        assert_eq!(
            handle.snapshot().terminal_outcome,
            Some(TurnTerminalOutcome::Cancelled)
        );
        start_running_conversation_turn(&handle, &RunId::new());
    }

    #[test]
    fn standalone_terminal_close_rejects_foreign_and_stale_effects() {
        let (_, dsl, handle) = registered_turn_handle(false);
        let run_id = RunId::new();
        start_running_conversation_turn(&handle, &run_id);
        complete_turn(&handle, &run_id);
        let before = dsl.snapshot_state();
        assert_standalone_close_rejected(&handle, &RunId::new(), mm_dsl::TurnPhase::Completed);
        assert_standalone_close_rejected(&handle, &run_id, mm_dsl::TurnPhase::Cancelled);
        let foreign =
            RuntimeTurnStateHandle::standalone(dsl.clone(), meerkat_core::SessionId::new());
        assert_standalone_close_rejected(&foreign, &run_id, mm_dsl::TurnPhase::Completed);
        assert_eq!(dsl.snapshot_state(), before);
        handle.run_completed(run_id.clone()).unwrap();
        start_running_conversation_turn(&handle, &RunId::new());
        let successor = dsl.snapshot_state();
        assert_standalone_close_rejected(&handle, &run_id, mm_dsl::TurnPhase::Completed);
        assert_eq!(dsl.snapshot_state(), successor);
    }

    #[test]
    fn standalone_terminal_close_rejects_incoherent_terminal_and_completion_debt() {
        let (session_id, dsl, handle) = registered_turn_handle(false);
        let run_id = RunId::new();
        start_running_conversation_turn(&handle, &run_id);
        complete_turn(&handle, &run_id);
        let mutations: [fn(&mut mm_dsl::MeerkatMachineState); 6] = [
            |state| state.turn_terminal_run_id = None,
            |state| {
                state.current_run_id =
                    Some(mm_dsl::RunId::from_domain(&RunId::from_uuid(Uuid::nil())));
            },
            |state| state.terminal_outcome = Some(mm_dsl::TurnTerminalOutcome::Cancelled),
            |state| state.terminal_cause_kind = Some(mm_dsl::TurnTerminalCauseKind::Unknown),
            |state| state.pre_run_phase = Some(mm_dsl::PreRunPhase::Attached),
            |state| {
                state.runtime_completion_result_run_id = state.current_run_id.clone();
                state.runtime_completion_result_resolved = false;
            },
        ];
        for mutate in mutations {
            let mut state = dsl.snapshot_state();
            mutate(&mut state);
            assert_recovered_standalone_close_rejected(
                &session_id,
                &run_id,
                state,
                mm_dsl::TurnPhase::Completed,
            );
        }
    }

    #[test]
    fn standalone_terminal_close_rejects_failed_cause_outcome_mismatches() {
        let (session_id, dsl, handle) = registered_turn_handle(false);
        let run_id = RunId::new();
        start_running_conversation_turn(&handle, &run_id);
        handle
            .fatal_failure(
                run_id.clone(),
                failure_source(TurnFailureSourceKind::Llm, "synthetic failure"),
            )
            .unwrap();
        let terminal = dsl.snapshot_state();
        assert_eq!(terminal.turn_phase, mm_dsl::TurnPhase::Failed);
        let mutations: [fn(&mut mm_dsl::MeerkatMachineState); 6] = [
            |state| state.terminal_cause_kind = None,
            |state| state.terminal_cause_kind = Some(mm_dsl::TurnTerminalCauseKind::Unknown),
            |state| state.terminal_outcome = Some(mm_dsl::TurnTerminalOutcome::Completed),
            |state| {
                state.terminal_cause_kind = Some(mm_dsl::TurnTerminalCauseKind::BudgetExhausted);
            },
            |state| {
                state.terminal_cause_kind = Some(mm_dsl::TurnTerminalCauseKind::TimeBudgetExceeded);
            },
            |state| {
                state.terminal_cause_kind =
                    Some(mm_dsl::TurnTerminalCauseKind::StructuredOutputValidationFailed);
            },
        ];
        for mutate in mutations {
            let mut state = terminal.clone();
            mutate(&mut state);
            assert_recovered_standalone_close_rejected(
                &session_id,
                &run_id,
                state,
                mm_dsl::TurnPhase::Failed,
            );
        }
    }

    #[test]
    fn standalone_terminal_close_rejects_cancelled_incoherence() {
        let (session_id, dsl, handle) = registered_turn_handle(false);
        let run_id = RunId::new();
        start_running_conversation_turn(&handle, &run_id);
        handle.cancel_now(run_id.clone()).unwrap();
        handle.cancellation_observed(run_id.clone()).unwrap();
        let terminal = dsl.snapshot_state();
        assert_eq!(terminal.turn_phase, mm_dsl::TurnPhase::Cancelled);
        let mutations: [fn(&mut mm_dsl::MeerkatMachineState); 3] = [
            |state| state.terminal_cause_kind = Some(mm_dsl::TurnTerminalCauseKind::LlmFailure),
            |state| state.terminal_outcome = Some(mm_dsl::TurnTerminalOutcome::Failed),
            |state| state.terminal_outcome = None,
        ];
        for mutate in mutations {
            let mut state = terminal.clone();
            mutate(&mut state);
            assert_recovered_standalone_close_rejected(
                &session_id,
                &run_id,
                state,
                mm_dsl::TurnPhase::Cancelled,
            );
        }
    }

    #[test]
    fn standalone_terminal_close_rejects_epochless_bound_placement() {
        let (session_id, dsl, handle) = registered_turn_handle(false);
        // The model permits generation-typed epochless oracle placements.
        // This isolates the placement guard from the separate epoch guard.
        dsl.apply_input(
            mm_dsl::MeerkatMachineInput::PrepareBindings {
                agent_runtime_id: mm_dsl::AgentRuntimeId::from("root02-placed-oracle"),
                fence_token: mm_dsl::FenceToken::from(1),
                generation: Some(mm_dsl::Generation::from(1)),
                runtime_epoch_id: None,
                session_id: mm_dsl::SessionId::from_domain(&session_id),
                residency_owner: mm_dsl::SessionResidencyOwner::Session,
            },
            "standalone close placement countercase",
        )
        .unwrap();
        let run_id = RunId::new();
        start_running_conversation_turn(&handle, &run_id);
        complete_turn(&handle, &run_id);
        let state = dsl.snapshot_state();
        assert!(state.active_runtime_id.is_some());
        assert!(state.active_runtime_epoch_id.is_none());
        assert_standalone_close_rejected(&handle, &run_id, mm_dsl::TurnPhase::Completed);
    }

    #[test]
    fn standalone_terminal_close_cannot_take_runtime_commit_ownership() {
        let (session_id, dsl, handle) = registered_turn_handle(true);
        let run_id = RunId::new();
        dsl.apply_input(
            mm_dsl::MeerkatMachineInput::Prepare {
                session_id: mm_dsl::SessionId::from_domain(&session_id),
                run_id: mm_dsl::RunId::from_domain(&run_id),
            },
            "runtime-owned test prepare",
        )
        .unwrap();
        start_running_conversation_turn(&handle, &run_id);
        complete_turn(&handle, &run_id);
        let before = dsl.snapshot_state();
        handle.run_completed(run_id.clone()).unwrap();
        handle
            .run_failed(
                run_id.clone(),
                TurnFailureReason::with_cause(
                    TurnTerminalCauseKind::LlmFailure,
                    TurnTerminalCauseKind::LlmFailure.agent_error_class(),
                    "synthetic failure",
                ),
            )
            .unwrap();
        handle.run_cancelled(run_id.clone()).unwrap();
        assert_eq!(dsl.snapshot_state(), before);
        assert_eq!(before.lifecycle_phase, mm_dsl::MeerkatPhase::Running);
        assert!(
            before.active_runtime_id.is_none(),
            "epoch fences even unplaced runtime entries"
        );
        let forged_standalone = RuntimeTurnStateHandle::standalone(dsl.clone(), session_id);
        assert_standalone_close_rejected(&forged_standalone, &run_id, mm_dsl::TurnPhase::Completed);
        assert_eq!(dsl.snapshot_state(), before);
        dsl.apply_input(
            mm_dsl::MeerkatMachineInput::ServiceTurnCommitted {
                run_id: mm_dsl::RunId::from_domain(&run_id),
            },
            "runtime-owned service receipt",
        )
        .unwrap();
        assert_eq!(
            dsl.snapshot_state().lifecycle_phase,
            mm_dsl::MeerkatPhase::Idle
        );
    }

    #[test]
    fn isolated_live_bridge_turn_authority_cannot_mutate_ordinary_member_machine() {
        let ordinary = RuntimeTurnStateHandle::ephemeral();
        let before = ordinary.snapshot();
        let isolated = ordinary
            .isolated_live_bridge_child()
            .expect("runtime authority mints isolated bridge child");
        let run_id = RunId(Uuid::from_u128(0x001b_1d6e));

        start_running_conversation_turn(isolated.as_ref(), &run_id);
        isolated
            .llm_returned_terminal(run_id.clone())
            .expect("isolated LLM terminal");
        isolated
            .boundary_complete(run_id)
            .expect("isolated turn terminal");

        assert_eq!(
            ordinary.snapshot(),
            before,
            "isolated bridge authority must not publish into the ordinary member machine"
        );
        assert_eq!(isolated.snapshot().turn_phase, TurnPhase::Completed);
    }

    #[tokio::test]
    async fn isolated_live_bridge_turn_authority_has_no_runtime_store_publication_path() {
        use crate::store::{InMemoryRuntimeStore, RuntimeStore};

        let store = InMemoryRuntimeStore::new();
        let runtime_id = crate::LogicalRuntimeId::new("isolated-live-bridge-store-probe");
        let isolated = RuntimeTurnStateHandle::ephemeral()
            .isolated_live_bridge_child()
            .expect("runtime authority mints isolated bridge child");
        let run_id = RunId(Uuid::from_u128(0x570e_1e55));

        start_running_conversation_turn(isolated.as_ref(), &run_id);
        isolated
            .llm_returned_terminal(run_id.clone())
            .expect("isolated LLM terminal");
        isolated
            .boundary_complete(run_id)
            .expect("isolated turn terminal");

        assert!(
            store
                .load_session_snapshot(&runtime_id)
                .await
                .expect("read untouched runtime store")
                .is_none(),
            "process-local bridge turn authority must have no RuntimeStore publication path"
        );
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
    fn turn_limit_handle_rejects_unreached_limit() {
        let handle = RuntimeTurnStateHandle::ephemeral();
        let run_id = RunId(Uuid::from_u128(31));
        start_running_conversation_turn(&handle, &run_id);

        handle
            .turn_limit_reached(run_id.clone(), 1, 2)
            .expect_err("machine guard must reject turn_count below max_turns");
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.turn_phase, TurnPhase::CallingLlm);
        assert_eq!(snapshot.terminal_cause_kind, None);

        handle
            .turn_limit_reached(run_id, 2, 2)
            .expect("turn limit reached at the boundary");
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.turn_phase, TurnPhase::Failed);
        assert_eq!(
            snapshot.terminal_cause_kind,
            Some(TurnTerminalCauseKind::TurnLimitReached)
        );
    }

    #[test]
    fn fatal_failure_rejects_max_turns_source() {
        let handle = RuntimeTurnStateHandle::ephemeral();
        let run_id = RunId(Uuid::from_u128(32));
        start_running_conversation_turn(&handle, &run_id);

        handle
            .fatal_failure(
                run_id.clone(),
                failure_source(TurnFailureSourceKind::MaxTurnsReached, "max turns"),
            )
            .expect_err("turn-limit terminality must use counted TurnLimitReached input");
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.turn_phase, TurnPhase::CallingLlm);
        assert_eq!(snapshot.terminal_cause_kind, None);
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

    /// The projection as it was before #24: a full copy of the state, the
    /// projection from that copy, and the classification over a second copy.
    /// The borrowed projection must equal it on every state.
    #[allow(clippy::expect_used)]
    fn legacy_snapshot(handle: &RuntimeTurnStateHandle) -> TurnStateSnapshot {
        let state = handle.dsl.snapshot_state();
        let turn_phase = map_turn_phase(state.turn_phase);
        let barrier_operation_ids: BTreeSet<_> = state
            .barrier_operation_ids
            .iter()
            .map(|id| legacy_parse_operation_id(id.0.as_str()))
            .collect();
        let pending_op_refs = state
            .pending_op_refs
            .iter()
            .map(|id| {
                let operation_id = legacy_parse_operation_id(id);
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
        let turn_terminal = legacy_classify_turn_terminal(&state);
        let active_run_id = if turn_terminal {
            None
        } else {
            state
                .current_run_id
                .as_ref()
                .map(legacy_parse_snapshot_run_id)
        };
        TurnStateSnapshot {
            active_run_id,
            terminal_run_id: state
                .turn_terminal_run_id
                .as_ref()
                .map(legacy_parse_snapshot_run_id),
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
            extraction_active: state.extraction_active,
            llm_retry_attempt: u32::try_from(state.llm_retry_attempt)
                .expect("generated MeerkatMachine llm_retry_attempt must fit u32"),
            llm_retry_max_retries: u32::try_from(state.llm_retry_max_retries)
                .expect("generated MeerkatMachine llm_retry_max_retries must fit u32"),
            llm_retry_selected_delay_ms: state.llm_retry_selected_delay_ms,
        }
    }

    #[allow(clippy::expect_used)]
    fn legacy_parse_operation_id(value: &str) -> OperationId {
        Uuid::parse_str(value)
            .map(OperationId)
            .expect("generated MeerkatMachine operation id projection must be well formed")
    }

    #[allow(clippy::expect_used)]
    fn legacy_parse_snapshot_run_id(run_id: &mm_dsl::RunId) -> RunId {
        Uuid::parse_str(&run_id.0)
            .map(RunId::from_uuid)
            .expect("generated MeerkatMachine current_run_id projection must be well formed")
    }

    fn legacy_classify_turn_terminal(state: &mm_dsl::MeerkatMachineState) -> bool {
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

    /// One generated turn input of the differential walk, on run `n`.
    #[derive(Clone, Copy, Debug)]
    enum OracleStep {
        Start(u128),
        ImmediateAppend(u128),
        PrimitiveApplied(u128),
        ToolCalls(u128, u64),
        RegisterOps(u128),
        ToolsResolved(u128),
        BarrierSatisfied(u128),
        BoundaryContinue(u128),
        LlmTerminal(u128),
        BoundaryComplete(u128),
        EnterExtraction(u128),
        ExtractionStart(u128),
        ExtractionRejected(u128),
        ExtractionPassed(u128),
        Recoverable(u128, u32),
        Retry(u128, u32),
        Fatal(u128),
        CancelNow(u128),
        CancellationObserved(u128),
        CancelAfterBoundary(u128),
        BudgetExhausted(u128),
        TimeBudgetExceeded(u128),
        TurnLimit(u128),
        ForceCancelNoRun,
    }

    fn oracle_run(n: u128) -> RunId {
        RunId(Uuid::from_u128(0x0de0_0000 + n))
    }

    fn oracle_ops() -> (BTreeSet<AsyncOpRef>, BTreeSet<OperationId>) {
        let op = |n: u128| OperationId(Uuid::from_u128(0x0de1_0000 + n));
        let refs = (0..8)
            .map(|i| AsyncOpRef {
                operation_id: op(i),
                wait_policy: if i < 2 {
                    WaitPolicy::Barrier
                } else {
                    WaitPolicy::Detached
                },
            })
            .collect();
        (refs, (0..2).map(op).collect())
    }

    fn apply_oracle_step(
        handle: &RuntimeTurnStateHandle,
        step: OracleStep,
    ) -> Result<(), DslTransitionError> {
        match step {
            OracleStep::Start(n) => handle.start_conversation_run(
                oracle_run(n),
                TurnPrimitiveKind::ConversationTurn,
                ContentShape::Conversation,
                n % 2 == 0,
                n % 3 == 0,
                2,
            ),
            OracleStep::ImmediateAppend(n) => handle.start_immediate_append(oracle_run(n)),
            OracleStep::PrimitiveApplied(n) => handle.primitive_applied(oracle_run(n)),
            OracleStep::ToolCalls(n, count) => handle.llm_returned_tool_calls(oracle_run(n), count),
            OracleStep::RegisterOps(n) => {
                let (refs, barrier) = oracle_ops();
                handle.register_pending_ops(oracle_run(n), refs, barrier)
            }
            OracleStep::ToolsResolved(n) => handle.tool_calls_resolved(oracle_run(n)),
            OracleStep::BarrierSatisfied(n) => {
                handle.ops_barrier_satisfied(oracle_run(n), oracle_ops().1)
            }
            OracleStep::BoundaryContinue(n) => handle.boundary_continue(oracle_run(n)),
            OracleStep::LlmTerminal(n) => handle.llm_returned_terminal(oracle_run(n)),
            OracleStep::BoundaryComplete(n) => handle.boundary_complete(oracle_run(n)),
            OracleStep::EnterExtraction(n) => handle.enter_extraction(oracle_run(n), 2),
            OracleStep::ExtractionStart(n) => handle.extraction_start(oracle_run(n)),
            OracleStep::ExtractionRejected(n) => {
                handle.extraction_validation_failed(oracle_run(n), "schema mismatch".into())
            }
            OracleStep::ExtractionPassed(n) => handle.extraction_validation_passed(oracle_run(n)),
            OracleStep::Recoverable(n, attempt) => {
                handle.recoverable_failure(oracle_run(n), retry_schedule(attempt))
            }
            OracleStep::Retry(n, attempt) => handle.retry_requested(oracle_run(n), attempt),
            OracleStep::Fatal(n) => handle.fatal_failure(
                oracle_run(n),
                failure_source(TurnFailureSourceKind::Llm, "fatal provider failure"),
            ),
            OracleStep::CancelNow(n) => handle.cancel_now(oracle_run(n)),
            OracleStep::CancellationObserved(n) => handle.cancellation_observed(oracle_run(n)),
            OracleStep::CancelAfterBoundary(n) => {
                handle.request_cancel_after_boundary(oracle_run(n))
            }
            OracleStep::BudgetExhausted(n) => handle.budget_exhausted(oracle_run(n)),
            OracleStep::TimeBudgetExceeded(n) => handle.time_budget_exceeded(oracle_run(n)),
            OracleStep::TurnLimit(n) => handle.turn_limit_reached(oracle_run(n), 4, 4),
            OracleStep::ForceCancelNoRun => handle.force_cancel_no_run(),
        }
    }

    /// Generated input scripts that between them reach every turn phase:
    /// tool rounds with barrier ops, immediate appends, extraction with a
    /// rejected attempt, retries, fatal and budget terminals, cancellation
    /// now and after a boundary.
    fn oracle_scripts() -> [Vec<OracleStep>; 5] {
        use OracleStep::*;
        [
            vec![
                Start(1),
                PrimitiveApplied(1),
                ToolCalls(1, 3),
                RegisterOps(1),
                BarrierSatisfied(1),
                ToolsResolved(1),
                BoundaryContinue(1),
                LlmTerminal(1),
                BoundaryComplete(1),
                ImmediateAppend(2),
                PrimitiveApplied(2),
                BoundaryComplete(2),
                Start(3),
                PrimitiveApplied(3),
                LlmTerminal(3),
                CancelAfterBoundary(3),
                BoundaryComplete(3),
            ],
            // The agent's order: every extraction reply drains a boundary
            // and re-enters Extracting before it is validated.
            vec![
                Start(4),
                PrimitiveApplied(4),
                LlmTerminal(4),
                EnterExtraction(4),
                ExtractionStart(4),
                LlmTerminal(4),
                EnterExtraction(4),
                ExtractionRejected(4),
                LlmTerminal(4),
                EnterExtraction(4),
                ExtractionPassed(4),
                BoundaryComplete(4),
            ],
            vec![
                Start(5),
                PrimitiveApplied(5),
                Recoverable(5, 1),
                Retry(5, 1),
                Recoverable(5, 2),
                Retry(5, 2),
                Fatal(5),
                Start(6),
                PrimitiveApplied(6),
                BudgetExhausted(6),
                Start(7),
                PrimitiveApplied(7),
                TimeBudgetExceeded(7),
                Start(8),
                PrimitiveApplied(8),
                TurnLimit(8),
            ],
            vec![
                Start(9),
                PrimitiveApplied(9),
                ToolCalls(9, 1),
                CancelNow(9),
                CancellationObserved(9),
                Start(10),
                CancelNow(10),
                CancellationObserved(10),
            ],
            vec![ForceCancelNoRun, Start(11), PrimitiveApplied(11)],
        ]
    }

    /// Each snapshot field's non-default witness, by name: `turn_terminal`
    /// takes both values, and `pending_op_refs` both wait policies, so a
    /// projection that inverts or collapses the policy is observable. The
    /// pattern names every field, so a field added to the snapshot must be
    /// added here.
    fn snapshot_field_witnesses(snapshot: &TurnStateSnapshot) -> [(&'static str, bool); 26] {
        let TurnStateSnapshot {
            active_run_id,
            terminal_run_id,
            loop_state,
            turn_phase,
            turn_terminal,
            primitive_kind,
            admitted_content_shape,
            vision_enabled,
            image_tool_results_enabled,
            tool_calls_pending,
            pending_op_refs,
            barrier_operation_ids,
            has_barrier_ops,
            barrier_satisfied,
            boundary_count,
            cancel_after_boundary,
            terminal_outcome,
            terminal_cause_kind,
            extraction_attempts,
            max_extraction_retries,
            extraction_active,
            llm_retry_attempt,
            llm_retry_max_retries,
            llm_retry_selected_delay_ms,
        } = snapshot;
        let has_policy = |policy| pending_op_refs.iter().any(|op| op.wait_policy == policy);
        [
            ("active_run_id", active_run_id.is_some()),
            ("terminal_run_id", terminal_run_id.is_some()),
            (
                "loop_state",
                *loop_state != meerkat_core::LoopState::default(),
            ),
            ("turn_phase", *turn_phase != TurnPhase::Ready),
            ("turn_terminal true", *turn_terminal),
            ("turn_terminal false", !*turn_terminal),
            ("primitive_kind", primitive_kind.is_some()),
            ("admitted_content_shape", admitted_content_shape.is_some()),
            ("vision_enabled", *vision_enabled),
            ("image_tool_results_enabled", *image_tool_results_enabled),
            ("tool_calls_pending", *tool_calls_pending > 0),
            ("pending_op_refs Barrier", has_policy(WaitPolicy::Barrier)),
            ("pending_op_refs Detached", has_policy(WaitPolicy::Detached)),
            ("barrier_operation_ids", !barrier_operation_ids.is_empty()),
            ("has_barrier_ops", *has_barrier_ops),
            ("barrier_satisfied", *barrier_satisfied),
            ("boundary_count", *boundary_count > 0),
            ("cancel_after_boundary", *cancel_after_boundary),
            ("terminal_outcome", terminal_outcome.is_some()),
            ("terminal_cause_kind", terminal_cause_kind.is_some()),
            ("extraction_attempts", *extraction_attempts > 0),
            ("max_extraction_retries", *max_extraction_retries > 0),
            ("extraction_active", *extraction_active),
            ("llm_retry_attempt", *llm_retry_attempt > 0),
            ("llm_retry_max_retries", *llm_retry_max_retries > 0),
            (
                "llm_retry_selected_delay_ms",
                *llm_retry_selected_delay_ms > 0,
            ),
        ]
    }

    /// #24 differential oracle: the borrowed projection equals the copying
    /// projection after every generated input of the walk, accepted or
    /// refused, the walk observes every turn phase, and it gives every
    /// snapshot field a non-default value at least once, so a field read
    /// from the wrong source is observable.
    #[test]
    fn borrowed_projection_equals_copying_projection_on_every_turn_phase() {
        let mut observed = Vec::new();
        let mut witnessed = std::collections::BTreeMap::new();
        let mut accepted = 0_usize;
        for script in oracle_scripts() {
            let handle = RuntimeTurnStateHandle::ephemeral();
            let mut check = |step: Option<OracleStep>| {
                let snapshot = handle.snapshot();
                assert_eq!(snapshot, legacy_snapshot(&handle), "after {step:?}");
                if !observed.contains(&snapshot.turn_phase) {
                    observed.push(snapshot.turn_phase);
                }
                for (field, non_default) in snapshot_field_witnesses(&snapshot) {
                    *witnessed.entry(field).or_insert(false) |= non_default;
                }
            };
            check(None);
            for step in script {
                if apply_oracle_step(&handle, step).is_ok() {
                    accepted += 1;
                }
                check(Some(step));
            }
        }
        for phase in [
            TurnPhase::Ready,
            TurnPhase::ApplyingPrimitive,
            TurnPhase::CallingLlm,
            TurnPhase::WaitingForOps,
            TurnPhase::DrainingBoundary,
            TurnPhase::Extracting,
            TurnPhase::ErrorRecovery,
            TurnPhase::Cancelling,
            TurnPhase::Completed,
            TurnPhase::Failed,
            TurnPhase::Cancelled,
        ] {
            assert!(
                observed.contains(&phase),
                "the walk never observed {phase:?}: {observed:?}"
            );
        }
        let unmoved: Vec<_> = witnessed
            .iter()
            .filter(|(_, non_default)| !**non_default)
            .map(|(field, _)| *field)
            .collect();
        assert!(
            unmoved.is_empty(),
            "the walk never gave these snapshot fields a non-default value: {unmoved:?}"
        );
        assert!(accepted >= 40, "only {accepted} inputs were accepted");
    }

    /// The owned classification keeps the copying one's verdicts, including
    /// its fail-closed verdict for a state recovery refuses.
    #[test]
    fn owned_classification_matches_copying_classification_and_fails_closed() {
        let handle = RuntimeTurnStateHandle::ephemeral();
        let run_id = oracle_run(12);
        start_running_conversation_turn(&handle, &run_id);
        let live = handle.dsl.snapshot_state();
        assert!(!legacy_classify_turn_terminal(&live));
        assert!(!classify_turn_terminal(live.clone()));
        let mut unrecoverable = live;
        unrecoverable
            .input_authority_bindings
            .insert("orphan-input".into(), "orphan-binding".into());
        assert!(
            mm_dsl::MeerkatMachineAuthority::recover_from_state(unrecoverable.clone()).is_err(),
            "the mutation must make the state unrecoverable"
        );
        assert!(legacy_classify_turn_terminal(&unrecoverable));
        assert!(classify_turn_terminal(unrecoverable));
    }

    /// The typed turn input of one oracle step.
    fn oracle_input(step: OracleStep) -> TurnExecutionInput {
        match step {
            OracleStep::Start(n) => TurnExecutionInput::StartConversationRun {
                run_id: oracle_run(n),
                primitive_kind: TurnPrimitiveKind::ConversationTurn,
                admitted_content_shape: ContentShape::Conversation,
                vision_enabled: n % 2 == 0,
                image_tool_results_enabled: n % 3 == 0,
                max_extraction_retries: 2,
            },
            OracleStep::ImmediateAppend(n) => TurnExecutionInput::StartImmediateAppend {
                run_id: oracle_run(n),
            },
            OracleStep::PrimitiveApplied(n) => TurnExecutionInput::PrimitiveApplied {
                run_id: oracle_run(n),
            },
            OracleStep::ToolCalls(n, count) => TurnExecutionInput::LlmReturnedToolCalls {
                run_id: oracle_run(n),
                tool_count: u32::try_from(count).unwrap(),
            },
            OracleStep::RegisterOps(n) => {
                let (refs, barrier) = oracle_ops();
                TurnExecutionInput::RegisterPendingOps {
                    run_id: oracle_run(n),
                    op_refs: refs.into_iter().collect(),
                    barrier_operation_ids: barrier.into_iter().collect(),
                    has_barrier_ops: true,
                }
            }
            OracleStep::ToolsResolved(n) => TurnExecutionInput::ToolCallsResolved {
                run_id: oracle_run(n),
            },
            OracleStep::BarrierSatisfied(n) => TurnExecutionInput::OpsBarrierSatisfied {
                run_id: oracle_run(n),
                operation_ids: oracle_ops().1.into_iter().collect(),
            },
            OracleStep::BoundaryContinue(n) => TurnExecutionInput::BoundaryContinue {
                run_id: oracle_run(n),
            },
            OracleStep::LlmTerminal(n) => TurnExecutionInput::LlmReturnedTerminal {
                run_id: oracle_run(n),
            },
            OracleStep::BoundaryComplete(n) => TurnExecutionInput::BoundaryComplete {
                run_id: oracle_run(n),
            },
            OracleStep::EnterExtraction(n) => TurnExecutionInput::EnterExtraction {
                run_id: oracle_run(n),
                max_retries: 2,
            },
            OracleStep::ExtractionStart(n) => TurnExecutionInput::ExtractionStart {
                run_id: oracle_run(n),
            },
            OracleStep::ExtractionRejected(n) => TurnExecutionInput::ExtractionValidationFailed {
                run_id: oracle_run(n),
                error: "schema mismatch".into(),
            },
            OracleStep::ExtractionPassed(n) => TurnExecutionInput::ExtractionValidationPassed {
                run_id: oracle_run(n),
            },
            OracleStep::Recoverable(n, attempt) => TurnExecutionInput::RecoverableFailure {
                run_id: oracle_run(n),
                retry: retry_schedule(attempt),
            },
            OracleStep::Retry(n, attempt) => TurnExecutionInput::RetryRequested {
                run_id: oracle_run(n),
                retry_attempt: attempt,
            },
            OracleStep::Fatal(n) => TurnExecutionInput::FatalFailure {
                run_id: oracle_run(n),
                failure: failure_source(TurnFailureSourceKind::Llm, "fatal provider failure"),
            },
            OracleStep::CancelNow(n) => TurnExecutionInput::CancelNow {
                run_id: oracle_run(n),
            },
            OracleStep::CancellationObserved(n) => TurnExecutionInput::CancellationObserved {
                run_id: oracle_run(n),
            },
            OracleStep::CancelAfterBoundary(n) => TurnExecutionInput::CancelAfterBoundary {
                run_id: oracle_run(n),
            },
            OracleStep::BudgetExhausted(n) => TurnExecutionInput::BudgetExhausted {
                run_id: oracle_run(n),
            },
            OracleStep::TimeBudgetExceeded(n) => TurnExecutionInput::TimeBudgetExceeded {
                run_id: oracle_run(n),
            },
            OracleStep::TurnLimit(n) => TurnExecutionInput::TurnLimitReached {
                run_id: oracle_run(n),
                turn_count: 4,
                max_turns: 4,
            },
            OracleStep::ForceCancelNoRun => TurnExecutionInput::ForceCancelNoRun,
        }
    }

    /// #24 P2: applying a typed input while observing the phase yields the
    /// same effects and the same next phase as applying it and then reading
    /// the projection, or the same refusal (its kind, context and reason, so
    /// the Display text the agent embeds too), on every generated input of
    /// the walk, and the phase read alone equals the projection's phase
    /// before every input.
    #[test]
    fn observing_apply_matches_apply_then_read_on_every_input() {
        let mut compared = 0_usize;
        let mut refused = 0_usize;
        for script in oracle_scripts() {
            let observing = RuntimeTurnStateHandle::ephemeral();
            let reading = RuntimeTurnStateHandle::ephemeral();
            for step in script {
                assert_eq!(
                    observing.turn_phase(),
                    reading.snapshot().turn_phase,
                    "before {step:?}"
                );
                let observed = observing.apply_turn_input_observing_phase(oracle_input(step));
                let read = reading
                    .apply_turn_input(oracle_input(step))
                    .map(|effects| (effects, reading.snapshot().turn_phase));
                match (observed, read) {
                    (Ok(observed), Ok(read)) => {
                        assert_eq!(observed, read, "after {step:?}");
                        compared += 1;
                    }
                    (Err(observed), Err(read)) => {
                        assert_eq!(observed, read, "refusing {step:?}");
                        assert_eq!(observed.to_string(), read.to_string(), "refusing {step:?}");
                        refused += 1;
                    }
                    (observed, read) => {
                        panic!("{step:?}: observing {observed:?}, reading {read:?}")
                    }
                }
                assert_eq!(observing.snapshot(), reading.snapshot(), "after {step:?}");
            }
        }
        assert!(compared >= 40, "only {compared} inputs were accepted");
        assert!(refused >= 3, "only {refused} inputs were refused");
    }

    /// #24 N1: the projection runs under the authority lock and never
    /// panics there. A malformed generated value still fails the snapshot,
    /// after the lock is released, so the lock is not poisoned.
    #[test]
    fn malformed_turn_state_fails_the_snapshot_after_the_lock_is_released() {
        let live = RuntimeTurnStateHandle::ephemeral();
        for step in [
            OracleStep::Start(13),
            OracleStep::PrimitiveApplied(13),
            OracleStep::ToolCalls(13, 3),
            OracleStep::RegisterOps(13),
        ] {
            apply_oracle_step(&live, step).expect("the tool round is accepted");
        }
        let mut state = live.dsl.snapshot_state();
        let detached = state
            .pending_op_refs
            .iter()
            .find(|id| !state.barrier_operation_ids.iter().any(|b| b.0 == **id))
            .cloned()
            .expect("the round registers detached operations");
        state.pending_op_refs.remove(&detached);
        state.pending_op_refs.insert("not-an-operation-id".into());
        let authority = Arc::new(std::sync::Mutex::new(
            mm_dsl::MeerkatMachineAuthority::recover_from_state(state)
                .expect("recovery does not parse operation ids"),
        ));
        let handle = RuntimeTurnStateHandle::new(Arc::new(HandleDslAuthority::from_shared(
            Arc::clone(&authority),
        )));
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handle.snapshot()))
            .expect_err("a malformed operation id must still fail the snapshot");
        let message = panic.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(
            message.contains("operation id projection must be well formed"),
            "{message}"
        );
        assert!(
            !authority.is_poisoned(),
            "the projection panicked under the authority lock"
        );
    }

    /// #24 P3: the borrowed scalar read equals the scalars of the copying
    /// projection after every generated input of the walk, accepted or
    /// refused, and the walk gives every scalar field a non-default value at
    /// least once, so a field read from the wrong source is observable.
    #[test]
    fn borrowed_scalars_equal_copying_projection_scalars_on_every_input() {
        let mut seen = [false; 6];
        let mut compared = 0_usize;
        for script in oracle_scripts() {
            let handle = RuntimeTurnStateHandle::ephemeral();
            let mut check = |step: Option<OracleStep>| {
                let scalars = handle.scalars();
                assert_eq!(
                    scalars,
                    TurnStateScalars::from_snapshot(&legacy_snapshot(&handle)),
                    "after {step:?}"
                );
                seen[0] |= scalars.cancel_after_boundary;
                seen[1] |= scalars.has_barrier_ops;
                seen[2] |= scalars.extraction_active;
                seen[3] |= scalars.terminal_outcome.is_some();
                seen[4] |= scalars.terminal_cause_kind.is_some();
                seen[5] |= scalars.extraction_attempts > 0;
                compared += 1;
            };
            check(None);
            for step in script {
                let _ = apply_oracle_step(&handle, step);
                check(Some(step));
            }
        }
        assert_eq!(
            seen, [true; 6],
            "cancel_after_boundary, has_barrier_ops, extraction_active, terminal_outcome, \
             terminal_cause_kind, extraction_attempts must each be non-default once"
        );
        assert!(compared >= 50, "only {compared} reads were compared");
    }
}
