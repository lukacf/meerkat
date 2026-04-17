//! Runtime impl of [`meerkat_core::handles::TurnStateHandle`].

use std::sync::Arc;

use meerkat_core::handles::{DslTransitionError, TurnStateHandle};
use meerkat_core::lifecycle::{InputId, RunId};

use super::HandleDslAuthority;
use crate::meerkat_machine::dsl as mm_dsl;

/// Runtime-backed [`TurnStateHandle`] impl.
///
/// Holds a dedicated per-session DSL authority; all transitions route through
/// `mm_dsl::MeerkatMachineInput` / `MeerkatMachineSignal`. Decision logic is
/// entirely in the generated DSL — the impl body only translates sync-typed
/// trait parameters into DSL variants.
#[derive(Debug)]
pub struct RuntimeTurnStateHandle {
    dsl: Arc<HandleDslAuthority>,
}

impl RuntimeTurnStateHandle {
    /// Construct a handle backed by a fresh DSL authority.
    pub fn new() -> Self {
        Self {
            dsl: Arc::new(HandleDslAuthority::new()),
        }
    }
}

impl Default for RuntimeTurnStateHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl TurnStateHandle for RuntimeTurnStateHandle {
    fn start_conversation_run(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::StartConversationRun,
            "TurnStateHandle::start_conversation_run",
        )
    }

    fn start_immediate_append(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::StartImmediateAppend,
            "TurnStateHandle::start_immediate_append",
        )
    }

    fn start_immediate_context(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::StartImmediateContext,
            "TurnStateHandle::start_immediate_context",
        )
    }

    fn call_started(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::CallStarted,
            "TurnStateHandle::call_started",
        )
    }

    fn call_finished(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::CallFinished,
            "TurnStateHandle::call_finished",
        )
    }

    fn boundary_applied(&self, revision: u64) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::BoundaryApplied { revision },
            "TurnStateHandle::boundary_applied",
        )
    }

    fn pending_succeeded(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::PendingSucceeded,
            "TurnStateHandle::pending_succeeded",
        )
    }

    fn pending_failed(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::PendingFailed,
            "TurnStateHandle::pending_failed",
        )
    }

    fn interrupt_current_run(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::InterruptCurrentRun,
            "TurnStateHandle::interrupt_current_run",
        )
    }

    fn cancel_after_boundary(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::CancelAfterBoundary,
            "TurnStateHandle::cancel_after_boundary",
        )
    }

    fn prepare(&self, run_id: &RunId) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::Prepare {
                session_id: mm_dsl::SessionId::default(),
                run_id: mm_dsl::RunId::from_domain(run_id),
            },
            "TurnStateHandle::prepare",
        )
    }

    fn commit(&self, input_id: &InputId, run_id: &RunId) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::Commit {
                input_id: mm_dsl::InputId::from_domain(input_id),
                run_id: mm_dsl::RunId::from_domain(run_id),
            },
            "TurnStateHandle::commit",
        )
    }

    fn fail(&self, run_id: &RunId) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::Fail {
                run_id: mm_dsl::RunId::from_domain(run_id),
            },
            "TurnStateHandle::fail",
        )
    }

    fn drain_queued_run(&self, run_id: &RunId) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::DrainQueuedRun {
                run_id: mm_dsl::RunId::from_domain(run_id),
            },
            "TurnStateHandle::drain_queued_run",
        )
    }
}
