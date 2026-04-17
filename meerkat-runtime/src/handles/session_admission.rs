//! Runtime impl of [`meerkat_core::handles::SessionAdmissionHandle`].

use std::sync::Arc;

use meerkat_core::handles::{DslTransitionError, SessionAdmissionHandle};
use meerkat_core::lifecycle::{InputId, RunId};

use super::HandleDslAuthority;
use crate::meerkat_machine::dsl as mm_dsl;

/// Runtime-backed [`SessionAdmissionHandle`] impl.
///
/// Routes every trait method to the corresponding DSL input on a dedicated
/// per-session MeerkatMachine DSL authority.
#[derive(Debug)]
pub struct RuntimeSessionAdmissionHandle {
    dsl: Arc<HandleDslAuthority>,
}

impl RuntimeSessionAdmissionHandle {
    /// Construct a handle backed by a fresh DSL authority.
    pub fn new() -> Self {
        Self {
            dsl: Arc::new(HandleDslAuthority::new()),
        }
    }
}

impl Default for RuntimeSessionAdmissionHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionAdmissionHandle for RuntimeSessionAdmissionHandle {
    fn ingest(
        &self,
        runtime_id: &str,
        work_id: &str,
        origin: &str,
    ) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::Ingest {
                runtime_id: mm_dsl::AgentRuntimeId::from(runtime_id.to_string()),
                work_id: mm_dsl::WorkId::from(work_id.to_string()),
                origin: origin.to_string(),
            },
            "SessionAdmissionHandle::ingest",
        )
    }

    fn accept_with_completion(
        &self,
        input_id: &InputId,
        request_immediate_processing: bool,
        interrupt_yielding: bool,
        run_id: &RunId,
    ) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::AcceptWithCompletion {
                input_id: mm_dsl::InputId::from_domain(input_id),
                request_immediate_processing,
                interrupt_yielding,
                run_id: mm_dsl::RunId::from_domain(run_id),
            },
            "SessionAdmissionHandle::accept_with_completion",
        )
    }

    fn accept_without_wake(&self, input_id: &InputId) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::AcceptWithoutWake {
                input_id: mm_dsl::InputId::from_domain(input_id),
            },
            "SessionAdmissionHandle::accept_without_wake",
        )
    }

    fn prepare(&self, run_id: &RunId) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::Prepare {
                session_id: mm_dsl::SessionId::default(),
                run_id: mm_dsl::RunId::from_domain(run_id),
            },
            "SessionAdmissionHandle::prepare",
        )
    }

    fn commit(&self, input_id: &InputId, run_id: &RunId) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::Commit {
                input_id: mm_dsl::InputId::from_domain(input_id),
                run_id: mm_dsl::RunId::from_domain(run_id),
            },
            "SessionAdmissionHandle::commit",
        )
    }

    fn fail(&self, run_id: &RunId) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::Fail {
                run_id: mm_dsl::RunId::from_domain(run_id),
            },
            "SessionAdmissionHandle::fail",
        )
    }

    fn recycle(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::Recycle,
            "SessionAdmissionHandle::recycle",
        )
    }
}
