//! Runtime impl of [`meerkat_core::handles::SessionAdmissionHandle`].

use std::sync::Arc;

use meerkat_core::SessionId;
use meerkat_core::comms::InputSource;
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
    session_id: SessionId,
}

impl RuntimeSessionAdmissionHandle {
    /// Construct a handle backed by the session's shared DSL authority.
    pub fn new(dsl: Arc<HandleDslAuthority>, session_id: SessionId) -> Self {
        Self { dsl, session_id }
    }

    /// Construct a handle backed by an ephemeral DSL authority.
    ///
    /// See [`RuntimeTurnStateHandle::ephemeral`].
    pub fn ephemeral() -> Self {
        Self::new(Arc::new(HandleDslAuthority::ephemeral()), SessionId::new())
    }
}

impl SessionAdmissionHandle for RuntimeSessionAdmissionHandle {
    fn ingest(
        &self,
        runtime_id: &str,
        work_id: &str,
        origin: InputSource,
    ) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::Ingest {
                runtime_id: mm_dsl::AgentRuntimeId::from(runtime_id.to_string()),
                work_id: mm_dsl::WorkId::from(work_id.to_string()),
                origin: mm_dsl::WorkOrigin::from(origin),
            },
            "SessionAdmissionHandle::ingest",
        )
    }

    fn accept_with_completion(
        &self,
        input_id: &InputId,
        request_immediate_processing: bool,
        interrupt_yielding: bool,
        wake_if_idle: bool,
    ) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::AcceptWithCompletion {
                input_id: mm_dsl::InputId::from_domain(input_id),
                request_immediate_processing,
                interrupt_yielding,
                wake_if_idle,
            },
            "SessionAdmissionHandle::accept_with_completion",
        )
    }

    fn accept_without_wake(&self, input_id: &InputId) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::AcceptWithoutWake {
                input_id: mm_dsl::InputId::from_domain(input_id),
            },
            "SessionAdmissionHandle::accept_without_wake",
        )
    }

    fn prepare(&self, run_id: &RunId) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::Prepare {
                session_id: mm_dsl::SessionId::from_domain(&self.session_id),
                run_id: mm_dsl::RunId::from_domain(run_id),
            },
            "SessionAdmissionHandle::prepare",
        )
    }

    fn commit(&self, _input_id: &InputId, _run_id: &RunId) -> Result<(), DslTransitionError> {
        // Runtime-backed commit terminalization is owned by
        // MeerkatMachineCommand::Commit and its durable receipt path. The
        // handle remains an observation-only compatibility hook.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use uuid::Uuid;

    #[test]
    fn commit_effect_is_observation_only() {
        let handle = RuntimeSessionAdmissionHandle::ephemeral();
        handle
            .commit(&InputId(Uuid::from_u128(1)), &RunId(Uuid::from_u128(2)))
            .expect("runtime-backed admission commit is observation-only");
    }

    #[test]
    fn prepare_uses_bound_session_identity_instead_of_default() {
        let session_id = SessionId::from_uuid(Uuid::from_u128(0x212));
        let dsl_session_id = mm_dsl::SessionId::from_domain(&session_id);
        let state = mm_dsl::MeerkatMachineState {
            lifecycle_phase: mm_dsl::MeerkatPhase::Idle,
            session_id: Some(dsl_session_id.clone()),
            ..Default::default()
        };
        let shared = Arc::new(HandleDslAuthority::from_shared(Arc::new(Mutex::new(
            mm_dsl::MeerkatMachineAuthority::from_state(state),
        ))));
        let handle = RuntimeSessionAdmissionHandle::new(Arc::clone(&shared), session_id);
        let run_id = RunId(Uuid::from_u128(0x213));

        handle
            .prepare(&run_id)
            .expect("prepare should use canonical bound session id");

        let state = shared.snapshot_state();
        assert_eq!(state.session_id, Some(dsl_session_id));
        assert_eq!(
            state.current_run_id,
            Some(mm_dsl::RunId::from_domain(&run_id))
        );
        assert_ne!(
            state.session_id,
            Some(mm_dsl::SessionId::default()),
            "prepare must never fabricate a nil/default session identity"
        );
    }
}
