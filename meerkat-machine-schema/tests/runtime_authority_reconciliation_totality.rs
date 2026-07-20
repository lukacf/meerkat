//! Totality pin for level-triggered runtime-authority recovery.
//!
//! Durable lifecycle rows are observed realization, not a catalog of legal
//! phase-restoration shapes. Every combination of optional decoded fields --
//! including partial bindings and partial run pairs -- must therefore take one
//! generated classifier transition and produce one output-only obligation.

use meerkat_machine_schema::TriggerMatch;
use meerkat_machine_schema::catalog::dsl::dsl_meerkat_machine;
use meerkat_machine_schema::catalog::dsl::meerkat_machine::{
    AgentRuntimeId, FenceToken, Generation, MeerkatMachineAuthority, MeerkatMachineEffect,
    MeerkatMachineInput, MeerkatMachineMutator, MeerkatPhase, PreRunPhase, RunId,
    RuntimeAuthorityObservationKind, RuntimeAuthorityReconcileDecision, RuntimeEpochId,
    RuntimeLifecycleObservedState,
};

const CLASSIFY: &str = "ClassifyRuntimeAuthorityReconciliation";

fn optional<T: Clone>(present: bool, value: T) -> Option<T> {
    present.then_some(value)
}

#[allow(clippy::too_many_arguments)] // one complete raw-observation tuple
fn expected_decision(
    observation_kind: RuntimeAuthorityObservationKind,
    state: Option<RuntimeLifecycleObservedState>,
    agent_runtime_id: &Option<AgentRuntimeId>,
    fence_token: Option<FenceToken>,
    runtime_generation: Option<Generation>,
    runtime_epoch_id: &Option<RuntimeEpochId>,
    current_run_id: &Option<RunId>,
    pre_run_phase: Option<PreRunPhase>,
    malformed_reclaim_safe: bool,
) -> RuntimeAuthorityReconcileDecision {
    match observation_kind {
        RuntimeAuthorityObservationKind::Missing => {
            RuntimeAuthorityReconcileDecision::NormalizeOrReplace
        }
        RuntimeAuthorityObservationKind::Decoded
            if state == Some(RuntimeLifecycleObservedState::Idle)
                && agent_runtime_id.is_none()
                && fence_token.is_none()
                && runtime_generation.is_none()
                && runtime_epoch_id.is_none()
                && current_run_id.is_none()
                && pre_run_phase.is_none() =>
        {
            RuntimeAuthorityReconcileDecision::Converged
        }
        RuntimeAuthorityObservationKind::Decoded => {
            RuntimeAuthorityReconcileDecision::NormalizeOrReplace
        }
        RuntimeAuthorityObservationKind::Unsupported => {
            RuntimeAuthorityReconcileDecision::RepairBlocked
        }
        RuntimeAuthorityObservationKind::Malformed if malformed_reclaim_safe => {
            RuntimeAuthorityReconcileDecision::Quarantine
        }
        RuntimeAuthorityObservationKind::Malformed => {
            RuntimeAuthorityReconcileDecision::RepairBlocked
        }
        RuntimeAuthorityObservationKind::Unavailable => RuntimeAuthorityReconcileDecision::Backoff,
    }
}

#[test]
fn runtime_authority_classifier_is_one_state_free_initializing_self_loop() {
    let schema = dsl_meerkat_machine();
    let transitions = schema
        .transitions
        .iter()
        .filter(|transition| match &transition.on {
            TriggerMatch::Input { variant, .. } => variant.as_ref() == CLASSIFY,
            TriggerMatch::Signal { .. } => false,
        })
        .collect::<Vec<_>>();

    assert_eq!(transitions.len(), 1, "classifier must have exactly one arm");
    let transition = transitions[0];
    assert_eq!(transition.name.as_ref(), CLASSIFY);
    assert_eq!(
        transition
            .from
            .iter()
            .map(|phase| phase.as_ref())
            .collect::<Vec<_>>(),
        vec!["Initializing"],
        "the classifier authority is a fresh Initializing machine"
    );
    assert_eq!(transition.to.as_ref(), "Initializing");
    assert!(
        transition.updates.is_empty(),
        "observed rows and decisions must not be mirrored into machine state"
    );
    assert_eq!(
        transition.emit.len(),
        1,
        "every observation must emit exactly one classified obligation"
    );
    assert_eq!(
        transition.emit[0].variant.as_ref(),
        "RuntimeAuthorityReconciliationClassified"
    );
    assert!(
        transition.guards.is_empty(),
        "phase extraction may constrain `from`, but no observation-shape guard is allowed"
    );
}

#[test]
fn runtime_authority_classifier_is_total_over_every_raw_tuple() {
    let kinds = [
        RuntimeAuthorityObservationKind::Missing,
        RuntimeAuthorityObservationKind::Decoded,
        RuntimeAuthorityObservationKind::Unsupported,
        RuntimeAuthorityObservationKind::Malformed,
        RuntimeAuthorityObservationKind::Unavailable,
    ];
    let states = [
        None,
        Some(RuntimeLifecycleObservedState::Initializing),
        Some(RuntimeLifecycleObservedState::Idle),
        Some(RuntimeLifecycleObservedState::Attached),
        Some(RuntimeLifecycleObservedState::Running),
        Some(RuntimeLifecycleObservedState::Retired),
        Some(RuntimeLifecycleObservedState::Stopped),
        Some(RuntimeLifecycleObservedState::Destroyed),
    ];
    let pre_run_phases = [
        None,
        Some(PreRunPhase::Idle),
        Some(PreRunPhase::Attached),
        Some(PreRunPhase::Retired),
    ];

    let mut cases = 0_u64;
    let mut converged = 0_u64;
    for observation_kind in kinds {
        for state in states {
            for runtime_present in [false, true] {
                for fence_present in [false, true] {
                    for generation_present in [false, true] {
                        for epoch_present in [false, true] {
                            for run_present in [false, true] {
                                for pre_run_phase in pre_run_phases {
                                    for malformed_reclaim_safe in [false, true] {
                                        let agent_runtime_id = optional(
                                            runtime_present,
                                            AgentRuntimeId("runtime-observed".to_owned()),
                                        );
                                        let fence_token = optional(fence_present, FenceToken(17));
                                        let runtime_generation =
                                            optional(generation_present, Generation(23));
                                        let runtime_epoch_id = optional(
                                            epoch_present,
                                            RuntimeEpochId("epoch-observed".to_owned()),
                                        );
                                        let current_run_id =
                                            optional(run_present, RunId("run-observed".to_owned()));
                                        let expected = expected_decision(
                                            observation_kind,
                                            state,
                                            &agent_runtime_id,
                                            fence_token,
                                            runtime_generation,
                                            &runtime_epoch_id,
                                            &current_run_id,
                                            pre_run_phase,
                                            malformed_reclaim_safe,
                                        );

                                        let mut authority = MeerkatMachineAuthority::new();
                                        let result = MeerkatMachineMutator::apply(
                                            &mut authority,
                                            MeerkatMachineInput::ClassifyRuntimeAuthorityReconciliation {
                                                observation_kind,
                                                state,
                                                agent_runtime_id,
                                                fence_token,
                                                runtime_generation,
                                                runtime_epoch_id,
                                                current_run_id,
                                                pre_run_phase,
                                                malformed_reclaim_safe,
                                            },
                                        );
                                        assert!(
                                            result.is_ok(),
                                            "classifier refused kind={observation_kind:?}, state={state:?}, runtime={runtime_present}, fence={fence_present}, generation={generation_present}, epoch={epoch_present}, run={run_present}, pre_run={pre_run_phase:?}, reclaim_safe={malformed_reclaim_safe}: {:?}",
                                            result.as_ref().err()
                                        );
                                        let Ok(transition) = result else {
                                            continue;
                                        };

                                        assert_eq!(
                                            transition.from_phase,
                                            MeerkatPhase::Initializing
                                        );
                                        assert_eq!(transition.to_phase, MeerkatPhase::Initializing);
                                        assert_eq!(
                                            transition.into_effects(),
                                            vec![MeerkatMachineEffect::RuntimeAuthorityReconciliationClassified {
                                                decision: expected,
                                            }]
                                        );
                                        cases += 1;
                                        if expected == RuntimeAuthorityReconcileDecision::Converged
                                        {
                                            converged += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 5 record kinds x 8 optional lifecycle states x 2^5 optional binding/run
    // presences x 4 optional pre-run values x 2 reclaim-safety values.
    assert_eq!(cases, 10_240);
    // The exact clean decoded Idle row is the sole fixed point; the reclaim bit
    // is irrelevant outside Malformed, so that row appears twice.
    assert_eq!(converged, 2);
}
