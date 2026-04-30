//! Runtime impl of [`meerkat_core::handles::TurnStateHandle`].

use std::collections::BTreeSet;
use std::sync::Arc;

use meerkat_core::handles::{DslTransitionError, TurnStateHandle, TurnStateSnapshot};
use meerkat_core::lifecycle::RunId;
use meerkat_core::ops::{AsyncOpRef, OperationId, WaitPolicy};
use meerkat_core::retry::LlmRetrySchedule;
use meerkat_core::turn_execution_authority::{
    TurnFailureReason, TurnPhase, TurnPrimitiveKind, TurnTerminalOutcome,
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

    fn close_direct_run(
        &self,
        run_id: &RunId,
        terminal_context: &'static str,
    ) -> Result<(), DslTransitionError> {
        let state = self.dsl.snapshot_state();
        let is_bound_running = state.lifecycle_phase == mm_dsl::MeerkatPhase::Running
            && state
                .current_run_id
                .as_ref()
                .is_some_and(|bound| bound.0 == run_id.to_string())
            && state.pre_run_phase.is_some();
        if !is_bound_running {
            return Ok(());
        }
        if state
            .input_run_associations
            .values()
            .any(|bound| bound == &run_id.to_string())
        {
            return Ok(());
        }

        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::Commit {
                input_id: mm_dsl::InputId::from(format!("direct-run:{run_id}")),
                run_id: mm_dsl::RunId::from_domain(run_id),
            },
            terminal_context,
        )
    }
}

impl TurnStateHandle for RuntimeTurnStateHandle {
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

    fn extraction_validation_failed(&self, error: String) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::ExtractionValidationFailed { error },
            "TurnStateHandle::extraction_validation_failed",
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
                error: reason.to_dsl_error(),
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

    fn run_completed(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::RunCompleted {
                run_id: mm_dsl::RunId::from_domain(&run_id),
            },
            "TurnStateHandle::run_completed",
        )?;
        self.close_direct_run(&run_id, "TurnStateHandle::run_completed:commit")
    }

    fn run_failed(
        &self,
        run_id: RunId,
        reason: TurnFailureReason,
    ) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::RunFailed {
                run_id: mm_dsl::RunId::from_domain(&run_id),
                runtime_apply_failure_cause: None,
                runtime_apply_failure_message: None,
                error: reason.to_dsl_error(),
            },
            "TurnStateHandle::run_failed",
        )?;
        self.close_direct_run(&run_id, "TurnStateHandle::run_failed:commit")
    }

    fn run_cancelled(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::RunCancelled {
                run_id: mm_dsl::RunId::from_domain(&run_id),
            },
            "TurnStateHandle::run_cancelled",
        )?;
        self.close_direct_run(&run_id, "TurnStateHandle::run_cancelled:commit")
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
        assert_eq!(snapshot.active_run_id, Some(run_id));
        assert_eq!(snapshot.turn_phase, TurnPhase::ApplyingPrimitive);
        assert_eq!(
            snapshot.primitive_kind,
            Some(TurnPrimitiveKind::ConversationTurn)
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
}
