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

pub(crate) fn project_phase(state: RuntimeState) -> mm_dsl::MeerkatPhase {
    match state {
        RuntimeState::Initializing => mm_dsl::MeerkatPhase::Initializing,
        RuntimeState::Idle => mm_dsl::MeerkatPhase::Idle,
        RuntimeState::Attached => mm_dsl::MeerkatPhase::Attached,
        RuntimeState::Running => mm_dsl::MeerkatPhase::Running,
        RuntimeState::Retired => mm_dsl::MeerkatPhase::Retired,
        RuntimeState::Stopped => mm_dsl::MeerkatPhase::Stopped,
        RuntimeState::Destroyed => mm_dsl::MeerkatPhase::Destroyed,
    }
}

pub(crate) fn project_state(
    session_id: &SessionId,
    runtime_phase: RuntimeState,
    runtime_id: Option<&LogicalRuntimeId>,
    current_run_id: Option<&RunId>,
    pre_run_phase: Option<RuntimeState>,
    silent_intent_overrides: BTreeSet<String>,
    active_fence_token: Option<u64>,
) -> mm_dsl::MeerkatMachineState {
    let (effective_phase, effective_current_run_id, effective_pre_run_phase) =
        match (runtime_phase, current_run_id, pre_run_phase) {
            (RuntimeState::Running, None, pre_run_phase) => (
                crate::runtime_state::run_return_phase_from_pre_run_phase(pre_run_phase),
                None,
                None,
            ),
            (RuntimeState::Running | RuntimeState::Retired, current_run_id, pre_run_phase) => {
                (runtime_phase, current_run_id, pre_run_phase)
            }
            (phase, _, _) => (phase, None, None),
        };

    mm_dsl::MeerkatMachineState {
        lifecycle_phase: project_phase(effective_phase),
        session_id: Some(mm_dsl::SessionId::from_domain(session_id)),
        active_runtime_id: runtime_id.map(mm_dsl::AgentRuntimeId::from_domain),
        active_fence_token: active_fence_token.map(mm_dsl::FenceToken::from),
        current_run_id: effective_current_run_id.map(mm_dsl::RunId::from_domain),
        pre_run_phase: effective_pre_run_phase.map(|phase| phase.to_string()),
        silent_intent_overrides,
        // Absorbed substate fields — initialised to DSL defaults.
        // These are projected from their respective authority owners
        // during the Phase 3 cutover; until then they carry defaults.
        registration_phase: "Queuing".to_string(),
        drain_phase: "Inactive".to_string(),
        drain_mode: None,
        active_filter: String::new(),
        staged_filter: String::new(),
        active_visibility_revision: 0,
        staged_visibility_revision: 0,
        active_deferred_names: std::collections::BTreeSet::new(),
        staged_deferred_names: std::collections::BTreeSet::new(),
        input_phases: std::collections::BTreeMap::new(),
        input_terminal_outcomes: std::collections::BTreeMap::new(),
        input_attempt_counts: std::collections::BTreeMap::new(),
        input_run_associations: std::collections::BTreeMap::new(),
        next_admission_seq: 0,
        input_admission_seq: std::collections::BTreeMap::new(),
        queue_lane: std::collections::BTreeSet::new(),
        steer_lane: std::collections::BTreeSet::new(),
        op_statuses: std::collections::BTreeMap::new(),
        op_completion_seq: std::collections::BTreeMap::new(),
        active_op_count: 0,
        wait_active: false,
        wait_operation_ids: std::collections::BTreeSet::new(),
        next_completion_seq: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_and_write_back_round_trips() {
        for state in [
            RuntimeState::Initializing,
            RuntimeState::Idle,
            RuntimeState::Attached,
            RuntimeState::Running,
            RuntimeState::Retired,
            RuntimeState::Stopped,
            RuntimeState::Destroyed,
        ] {
            let dsl = project_phase(state);
            let back = write_back_phase(dsl);
            assert_eq!(back, state);
        }
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
