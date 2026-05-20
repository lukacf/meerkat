//! Authority adapter for the MeerkatMachine DSL.
//!
//! Bridges between the runtime's distributed state and the DSL's flat
//! state representation. The DSL validates transition legality; the
//! runtime shell executes the actual mutations.

use std::collections::BTreeSet;

use super::dsl as mm_dsl;
use crate::identifiers::LogicalRuntimeId;
use crate::runtime_state::RuntimeState;
use meerkat_core::lifecycle::RunId;
use meerkat_core::types::SessionId;

/// Map DSL transition errors into plain strings with context.
pub(crate) fn map_error(err: mm_dsl::MeerkatMachineTransitionError, context: &str) -> String {
    match err {
        mm_dsl::MeerkatMachineTransitionError::NoMatchingTransition { phase, trigger } => {
            format!("DSL authority ({context}): no matching transition from {phase} for {trigger}")
        }
        mm_dsl::MeerkatMachineTransitionError::GuardRejected { phase, trigger } => {
            format!(
                "DSL authority ({context}): guard rejected transition from {phase} for {trigger}"
            )
        }
    }
}

pub(crate) fn write_back_phase(dsl_phase: mm_dsl::MeerkatPhase) -> RuntimeState {
    match dsl_phase {
        mm_dsl::MeerkatPhase::Initializing => RuntimeState::Initializing,
        mm_dsl::MeerkatPhase::Idle => RuntimeState::Idle,
        mm_dsl::MeerkatPhase::Attached => RuntimeState::Attached,
        mm_dsl::MeerkatPhase::Running => RuntimeState::Running,
        mm_dsl::MeerkatPhase::Retired => RuntimeState::Retired,
        mm_dsl::MeerkatPhase::Stopped => RuntimeState::Stopped,
        mm_dsl::MeerkatPhase::Destroyed => RuntimeState::Destroyed,
    }
}

pub(crate) fn current_run_id_from_dsl(run_id: &mm_dsl::RunId) -> Option<RunId> {
    uuid::Uuid::parse_str(&run_id.0).ok().map(RunId::from_uuid)
}

pub(crate) fn pre_run_phase_to_runtime_state(phase: mm_dsl::PreRunPhase) -> RuntimeState {
    match phase {
        mm_dsl::PreRunPhase::Idle => RuntimeState::Idle,
        mm_dsl::PreRunPhase::Attached => RuntimeState::Attached,
        mm_dsl::PreRunPhase::Retired => RuntimeState::Retired,
    }
}

pub(crate) fn runtime_phase_from_authority(
    authority: &mm_dsl::MeerkatMachineAuthority,
) -> RuntimeState {
    write_back_phase(authority.state().lifecycle_phase)
}

pub(crate) fn visible_runtime_phase(
    phase: RuntimeState,
    pre_run_phase: Option<RuntimeState>,
) -> RuntimeState {
    if phase == RuntimeState::Running && pre_run_phase == Some(RuntimeState::Retired) {
        RuntimeState::Retired
    } else {
        phase
    }
}

pub(crate) fn should_publish_control_over_dsl(
    control_phase: RuntimeState,
    dsl_phase: RuntimeState,
    dsl_pre_run_phase: Option<RuntimeState>,
) -> bool {
    if control_phase == RuntimeState::Retired
        && dsl_phase == RuntimeState::Running
        && dsl_pre_run_phase == Some(RuntimeState::Retired)
    {
        return false;
    }
    control_phase != dsl_phase
        && (matches!(
            dsl_phase,
            RuntimeState::Retired | RuntimeState::Stopped | RuntimeState::Destroyed
        ) || matches!(
            control_phase,
            RuntimeState::Running
                | RuntimeState::Retired
                | RuntimeState::Stopped
                | RuntimeState::Destroyed
        ))
}

pub(crate) fn current_run_id_from_authority(
    authority: &mm_dsl::MeerkatMachineAuthority,
) -> Option<RunId> {
    authority
        .state()
        .current_run_id
        .as_ref()
        .and_then(current_run_id_from_dsl)
}

pub(crate) fn pre_run_phase_from_authority(
    authority: &mm_dsl::MeerkatMachineAuthority,
) -> Option<RuntimeState> {
    authority
        .state()
        .pre_run_phase
        .map(pre_run_phase_to_runtime_state)
}

pub(crate) fn observed_runtime_lifecycle_state(
    state: RuntimeState,
) -> mm_dsl::RuntimeLifecycleObservedState {
    match state {
        RuntimeState::Initializing => mm_dsl::RuntimeLifecycleObservedState::Initializing,
        RuntimeState::Idle => mm_dsl::RuntimeLifecycleObservedState::Idle,
        RuntimeState::Attached => mm_dsl::RuntimeLifecycleObservedState::Attached,
        RuntimeState::Running => mm_dsl::RuntimeLifecycleObservedState::Running,
        RuntimeState::Retired => mm_dsl::RuntimeLifecycleObservedState::Retired,
        RuntimeState::Stopped => mm_dsl::RuntimeLifecycleObservedState::Stopped,
        RuntimeState::Destroyed => mm_dsl::RuntimeLifecycleObservedState::Destroyed,
    }
}

pub(crate) fn runtime_state_from_observed_lifecycle_state(
    state: mm_dsl::RuntimeLifecycleObservedState,
) -> RuntimeState {
    match state {
        mm_dsl::RuntimeLifecycleObservedState::Initializing => RuntimeState::Initializing,
        mm_dsl::RuntimeLifecycleObservedState::Idle => RuntimeState::Idle,
        mm_dsl::RuntimeLifecycleObservedState::Attached => RuntimeState::Attached,
        mm_dsl::RuntimeLifecycleObservedState::Running => RuntimeState::Running,
        mm_dsl::RuntimeLifecycleObservedState::Retired => RuntimeState::Retired,
        mm_dsl::RuntimeLifecycleObservedState::Stopped => RuntimeState::Stopped,
        mm_dsl::RuntimeLifecycleObservedState::Destroyed => RuntimeState::Destroyed,
    }
}

#[allow(clippy::expect_used)]
pub(crate) fn new_initialized_authority(context: &'static str) -> mm_dsl::MeerkatMachineAuthority {
    let mut authority = mm_dsl::MeerkatMachineAuthority::new();
    authority
        .apply_signal(mm_dsl::MeerkatMachineSignal::Initialize)
        .expect(context);
    authority
}

pub(crate) fn recover_authority_from_runtime_observation(
    session_id: &SessionId,
    runtime_phase: RuntimeState,
    runtime_id: Option<&LogicalRuntimeId>,
    current_run_id: Option<&RunId>,
    pre_run_phase: Option<RuntimeState>,
    silent_intent_overrides: BTreeSet<String>,
    active_fence_token: Option<u64>,
) -> Result<mm_dsl::MeerkatMachineAuthority, mm_dsl::MeerkatMachineTransitionError> {
    recover_authority_from_runtime_observation_id(
        mm_dsl::SessionId::from_domain(session_id),
        runtime_phase,
        runtime_id,
        current_run_id,
        pre_run_phase,
        silent_intent_overrides,
        active_fence_token,
    )
}

pub(crate) fn recover_authority_from_runtime_observation_id(
    session_id: mm_dsl::SessionId,
    runtime_phase: RuntimeState,
    runtime_id: Option<&LogicalRuntimeId>,
    current_run_id: Option<&RunId>,
    pre_run_phase: Option<RuntimeState>,
    silent_intent_overrides: BTreeSet<String>,
    active_fence_token: Option<u64>,
) -> Result<mm_dsl::MeerkatMachineAuthority, mm_dsl::MeerkatMachineTransitionError> {
    let mut authority = mm_dsl::MeerkatMachineAuthority::new();
    mm_dsl::MeerkatMachineMutator::apply(
        &mut authority,
        mm_dsl::MeerkatMachineInput::RecoverRuntimeAuthority {
            session_id,
            state: observed_runtime_lifecycle_state(runtime_phase),
            agent_runtime_id: runtime_id.map(mm_dsl::AgentRuntimeId::from_domain),
            fence_token: active_fence_token.map(mm_dsl::FenceToken::from),
            current_run_id: current_run_id.map(mm_dsl::RunId::from_domain),
            pre_run_phase: pre_run_phase.and_then(pre_run_phase_from_runtime_state),
            silent_intent_overrides,
        },
    )?;
    Ok(authority)
}

/// Map a persisted pre-run phase (as a [`RuntimeState`]) into the typed
/// [`mm_dsl::PreRunPhase`] carried in the DSL state. Only `Idle`, `Attached`,
/// and `Retired` are valid pre-run markers — any other [`RuntimeState`]
/// indicates the session is not in a run-in-progress shape and the DSL
/// treats the pre-run slot as absent.
pub(crate) fn pre_run_phase_from_runtime_state(state: RuntimeState) -> Option<mm_dsl::PreRunPhase> {
    match state {
        RuntimeState::Idle => Some(mm_dsl::PreRunPhase::Idle),
        RuntimeState::Attached => Some(mm_dsl::PreRunPhase::Attached),
        RuntimeState::Retired => Some(mm_dsl::PreRunPhase::Retired),
        RuntimeState::Initializing
        | RuntimeState::Running
        | RuntimeState::Stopped
        | RuntimeState::Destroyed => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recover_authority_from_runtime_observation_uses_generated_input() {
        let session_id = SessionId::from_uuid(uuid::Uuid::nil());
        let authority = recover_authority_from_runtime_observation(
            &session_id,
            RuntimeState::Attached,
            None,
            None,
            None,
            BTreeSet::from(["silent".to_string()]),
            None,
        )
        .expect("generated recovery input should accept attached authority");

        assert_eq!(
            authority.state().lifecycle_phase,
            mm_dsl::MeerkatPhase::Attached
        );
        assert_eq!(
            authority.state().session_id,
            Some(mm_dsl::SessionId::from_domain(&session_id))
        );
        assert!(authority.state().silent_intent_overrides.contains("silent"));
    }

    #[test]
    fn recover_authority_from_runtime_observation_rejects_incoherent_run_binding() {
        let session_id = SessionId::from_uuid(uuid::Uuid::nil());
        let run_id = RunId::from_uuid(uuid::Uuid::nil());

        let result = recover_authority_from_runtime_observation(
            &session_id,
            RuntimeState::Running,
            None,
            Some(&run_id),
            None,
            BTreeSet::new(),
            None,
        );

        assert!(matches!(
            result,
            Err(mm_dsl::MeerkatMachineTransitionError::GuardRejected { .. })
        ));
    }

    #[test]
    fn map_error_includes_context() {
        let err = mm_dsl::MeerkatMachineTransitionError::NoMatchingTransition {
            phase: "Idle".into(),
            trigger: "Destroy".into(),
        };
        let msg = map_error(err, "test_context");
        assert!(msg.contains("test_context"));
        assert!(msg.contains("Idle"));
    }
}
