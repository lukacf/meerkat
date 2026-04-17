//! Runtime impl of [`meerkat_core::handles::TurnStateHandle`].

use std::collections::BTreeSet;
use std::sync::Arc;

use meerkat_core::handles::{DslTransitionError, TurnStateHandle, TurnStateSnapshot};
use meerkat_core::lifecycle::RunId;

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

impl TurnStateHandle for RuntimeTurnStateHandle {
    fn start_conversation_run(
        &self,
        run_id: RunId,
        primitive_kind: String,
        admitted_content_shape: String,
        vision_enabled: bool,
        image_tool_results_enabled: bool,
        max_extraction_retries: u64,
    ) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::StartConversationRun {
                run_id: mm_dsl::RunId::from_domain(&run_id),
                primitive_kind,
                admitted_content_shape,
                vision_enabled,
                image_tool_results_enabled,
                max_extraction_retries,
            },
            "TurnStateHandle::start_conversation_run",
        )
    }

    fn start_immediate_append(
        &self,
        run_id: RunId,
        admitted_content_shape: String,
    ) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::StartImmediateAppend {
                run_id: mm_dsl::RunId::from_domain(&run_id),
                admitted_content_shape,
            },
            "TurnStateHandle::start_immediate_append",
        )
    }

    fn start_immediate_context(
        &self,
        run_id: RunId,
        admitted_content_shape: String,
    ) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::StartImmediateContext {
                run_id: mm_dsl::RunId::from_domain(&run_id),
                admitted_content_shape,
            },
            "TurnStateHandle::start_immediate_context",
        )
    }

    fn primitive_applied(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::PrimitiveApplied,
            "TurnStateHandle::primitive_applied",
        )
    }

    fn llm_returned_tool_calls(&self, tool_count: u64) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::LlmReturnedToolCalls { tool_count },
            "TurnStateHandle::llm_returned_tool_calls",
        )
    }

    fn llm_returned_terminal(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::LlmReturnedTerminal,
            "TurnStateHandle::llm_returned_terminal",
        )
    }

    fn register_pending_ops(
        &self,
        op_refs: BTreeSet<String>,
        barrier_operation_ids: BTreeSet<String>,
    ) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::RegisterPendingOps {
                op_refs,
                barrier_operation_ids,
            },
            "TurnStateHandle::register_pending_ops",
        )
    }

    fn tool_calls_resolved(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::ToolCallsResolved,
            "TurnStateHandle::tool_calls_resolved",
        )
    }

    fn ops_barrier_satisfied(
        &self,
        operation_ids: BTreeSet<String>,
    ) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::OpsBarrierSatisfied { operation_ids },
            "TurnStateHandle::ops_barrier_satisfied",
        )
    }

    fn boundary_continue(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::BoundaryContinue,
            "TurnStateHandle::boundary_continue",
        )
    }

    fn boundary_complete(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::BoundaryComplete,
            "TurnStateHandle::boundary_complete",
        )
    }

    fn enter_extraction(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::EnterExtraction,
            "TurnStateHandle::enter_extraction",
        )
    }

    fn extraction_start(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::ExtractionStart,
            "TurnStateHandle::extraction_start",
        )
    }

    fn extraction_validation_passed(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::ExtractionValidationPassed,
            "TurnStateHandle::extraction_validation_passed",
        )
    }

    fn extraction_validation_failed(&self, error: String) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::ExtractionValidationFailed { error },
            "TurnStateHandle::extraction_validation_failed",
        )
    }

    fn recoverable_failure(&self, error: String) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::RecoverableFailure { error },
            "TurnStateHandle::recoverable_failure",
        )
    }

    fn fatal_failure(&self, error: String) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::FatalFailure { error },
            "TurnStateHandle::fatal_failure",
        )
    }

    fn retry_requested(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::RetryRequested,
            "TurnStateHandle::retry_requested",
        )
    }

    fn cancel_now(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::CancelNow,
            "TurnStateHandle::cancel_now",
        )
    }

    fn request_cancel_after_boundary(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::RequestCancelAfterBoundary,
            "TurnStateHandle::request_cancel_after_boundary",
        )
    }

    fn cancellation_observed(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::CancellationObserved,
            "TurnStateHandle::cancellation_observed",
        )
    }

    fn acknowledge_terminal(&self, outcome: String) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::AcknowledgeTerminal { outcome },
            "TurnStateHandle::acknowledge_terminal",
        )
    }

    fn turn_limit_reached(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::TurnLimitReached,
            "TurnStateHandle::turn_limit_reached",
        )
    }

    fn budget_exhausted(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::BudgetExhausted,
            "TurnStateHandle::budget_exhausted",
        )
    }

    fn time_budget_exceeded(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::TimeBudgetExceeded,
            "TurnStateHandle::time_budget_exceeded",
        )
    }

    fn force_cancel_no_run(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::ForceCancelNoRun,
            "TurnStateHandle::force_cancel_no_run",
        )
    }

    fn run_completed(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::RunCompleted {
                run_id: mm_dsl::RunId::from_domain(&run_id),
            },
            "TurnStateHandle::run_completed",
        )
    }

    fn run_failed(&self, run_id: RunId, error: String) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::RunFailed {
                run_id: mm_dsl::RunId::from_domain(&run_id),
                error,
            },
            "TurnStateHandle::run_failed",
        )
    }

    fn run_cancelled(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::RunCancelled {
                run_id: mm_dsl::RunId::from_domain(&run_id),
            },
            "TurnStateHandle::run_cancelled",
        )
    }

    fn snapshot(&self) -> TurnStateSnapshot {
        let state = self.dsl.snapshot_state();
        TurnStateSnapshot {
            turn_phase: state.turn_phase.clone(),
            primitive_kind: state.primitive_kind.clone(),
            admitted_content_shape: state.admitted_content_shape.clone(),
            vision_enabled: state.vision_enabled,
            image_tool_results_enabled: state.image_tool_results_enabled,
            tool_calls_pending: state.tool_calls_pending,
            pending_op_refs: state.pending_op_refs.clone(),
            barrier_operation_ids: state.barrier_operation_ids.clone(),
            has_barrier_ops: state.has_barrier_ops,
            barrier_satisfied: state.barrier_satisfied,
            boundary_count: state.boundary_count,
            cancel_after_boundary: state.cancel_after_boundary,
            terminal_outcome: state.terminal_outcome.clone(),
            extraction_attempts: state.extraction_attempts,
            max_extraction_retries: state.max_extraction_retries,
        }
    }
}
