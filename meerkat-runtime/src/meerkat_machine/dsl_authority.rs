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

/// Typed projection of a generated-machine transition rejection.
///
/// Carries a stable per-variant discriminant (the same `&'static str`
/// convention as `meerkat_core::error::error_code()`) alongside the
/// human-readable rejection detail, so consumer seams can move the typed
/// refusal across boundaries without re-parsing a flattened reason string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DslTransitionRefusal {
    pub(crate) error_code: &'static str,
    pub(crate) message: String,
}

impl DslTransitionRefusal {
    /// Build a non-transition refusal (e.g. unregistered session, raw
    /// internal input, committed-effect dispatch failure) under an explicit
    /// stable discriminant.
    pub(crate) fn other(error_code: &'static str, message: impl Into<String>) -> Self {
        Self {
            error_code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for DslTransitionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} [{}]", self.message, self.error_code)
    }
}

/// Map a generated-machine transition rejection to its typed refusal with a
/// stable per-variant `error_code`.
pub(crate) fn refusal(
    err: mm_dsl::MeerkatMachineTransitionError,
    context: &str,
) -> DslTransitionRefusal {
    let error_code = match &err {
        mm_dsl::MeerkatMachineTransitionError::NoMatchingTransition { .. } => {
            "dsl_no_matching_transition"
        }
        mm_dsl::MeerkatMachineTransitionError::GuardRejected { .. } => "dsl_guard_rejected",
        mm_dsl::MeerkatMachineTransitionError::RecoveredStateInvariantRejected { .. } => {
            "dsl_recovered_state_invariant_rejected"
        }
    };
    DslTransitionRefusal {
        error_code,
        message: map_error(err, context),
    }
}

/// Map DSL transition errors into plain strings with context.
pub(crate) fn map_error(err: mm_dsl::MeerkatMachineTransitionError, context: &str) -> String {
    match err {
        mm_dsl::MeerkatMachineTransitionError::NoMatchingTransition { phase, trigger } => {
            format!(
                "DSL authority ({context}): no matching transition from {phase:?} for {trigger}"
            )
        }
        mm_dsl::MeerkatMachineTransitionError::GuardRejected { phase, trigger } => {
            format!(
                "DSL authority ({context}): guard rejected transition from {phase:?} for {trigger}"
            )
        }
        mm_dsl::MeerkatMachineTransitionError::RecoveredStateInvariantRejected {
            phase,
            invariant,
        } => {
            format!(
                "DSL authority ({context}): recovered state violated invariant {invariant} in phase {phase:?}"
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn recover_authority_from_runtime_observation(
    session_id: &SessionId,
    runtime_phase: RuntimeState,
    runtime_id: Option<&LogicalRuntimeId>,
    current_run_id: Option<&RunId>,
    pre_run_phase: Option<RuntimeState>,
    silent_intent_overrides: BTreeSet<String>,
    active_fence_token: Option<u64>,
    active_runtime_generation: Option<mm_dsl::Generation>,
    active_runtime_epoch_id: Option<mm_dsl::RuntimeEpochId>,
) -> Result<mm_dsl::MeerkatMachineAuthority, mm_dsl::MeerkatMachineTransitionError> {
    recover_authority_from_runtime_observation_id(
        mm_dsl::SessionId::from_domain(session_id),
        runtime_phase,
        runtime_id,
        current_run_id,
        pre_run_phase,
        silent_intent_overrides,
        active_fence_token,
        active_runtime_generation,
        active_runtime_epoch_id,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn recover_authority_from_runtime_observation_id(
    session_id: mm_dsl::SessionId,
    runtime_phase: RuntimeState,
    runtime_id: Option<&LogicalRuntimeId>,
    current_run_id: Option<&RunId>,
    pre_run_phase: Option<RuntimeState>,
    silent_intent_overrides: BTreeSet<String>,
    active_fence_token: Option<u64>,
    active_runtime_generation: Option<mm_dsl::Generation>,
    active_runtime_epoch_id: Option<mm_dsl::RuntimeEpochId>,
) -> Result<mm_dsl::MeerkatMachineAuthority, mm_dsl::MeerkatMachineTransitionError> {
    let mut authority = mm_dsl::MeerkatMachineAuthority::new();
    let agent_runtime_id =
        if active_runtime_generation.is_some() || active_runtime_epoch_id.is_some() {
            runtime_id.map(mm_dsl::AgentRuntimeId::from_domain)
        } else {
            None
        };
    mm_dsl::MeerkatMachineMutator::apply(
        &mut authority,
        mm_dsl::MeerkatMachineInput::RecoverRuntimeAuthority {
            session_id,
            state: observed_runtime_lifecycle_state(runtime_phase),
            agent_runtime_id,
            fence_token: active_fence_token.map(mm_dsl::FenceToken::from),
            runtime_generation: active_runtime_generation,
            runtime_epoch_id: active_runtime_epoch_id,
            current_run_id: current_run_id.map(mm_dsl::RunId::from_domain),
            pre_run_phase: pre_run_phase.and_then(pre_run_phase_from_runtime_state),
            silent_intent_overrides,
        },
    )?;
    Ok(authority)
}

#[derive(Debug, Clone, Copy)]
enum SupervisorRotationRecoveryKind {
    CurrentOperation,
    TerminalReceipt,
}

fn recover_supervisor_rotation_receipt(
    authority: &mut mm_dsl::MeerkatMachineAuthority,
    rotation: &crate::store::SupervisorRotationReceipt,
    kind: SupervisorRotationRecoveryKind,
) -> Result<(), mm_dsl::MeerkatMachineTransitionError> {
    let phase = match rotation.phase() {
        crate::store::SupervisorRotationPersistencePhase::PreviousRevokePending => {
            mm_dsl::SupervisorRotationPhase::PreviousRevokePending
        }
        crate::store::SupervisorRotationPersistencePhase::NextPublishPending => {
            mm_dsl::SupervisorRotationPhase::NextPublishPending
        }
        crate::store::SupervisorRotationPersistencePhase::Completed => {
            mm_dsl::SupervisorRotationPhase::Completed
        }
        crate::store::SupervisorRotationPersistencePhase::Rejected => {
            mm_dsl::SupervisorRotationPhase::Rejected
        }
    };
    let rejection = rotation.rejection().map(|rejection| match rejection {
        crate::store::SupervisorRotationRejection::OperationConflict => {
            mm_dsl::SupervisorRotationRejectionKind::OperationConflict
        }
        crate::store::SupervisorRotationRejection::NotBound => {
            mm_dsl::SupervisorRotationRejectionKind::NotBound
        }
        crate::store::SupervisorRotationRejection::SenderMismatch => {
            mm_dsl::SupervisorRotationRejectionKind::SenderMismatch
        }
        crate::store::SupervisorRotationRejection::TargetEpochNotAdvanced => {
            mm_dsl::SupervisorRotationRejectionKind::TargetEpochNotAdvanced
        }
        crate::store::SupervisorRotationRejection::InvalidTarget => {
            mm_dsl::SupervisorRotationRejectionKind::InvalidTarget
        }
        crate::store::SupervisorRotationRejection::UnsupportedProtocolVersion => {
            mm_dsl::SupervisorRotationRejectionKind::UnsupportedProtocolVersion
        }
    });
    let previous = rotation.previous();
    let next = rotation.next();
    let operation_id = rotation.operation_id().to_string();
    let input = match kind {
        SupervisorRotationRecoveryKind::CurrentOperation => {
            mm_dsl::MeerkatMachineInput::RecoverSupervisorRotationOperation {
                operation_id,
                phase,
                rejection,
                previous_name: previous.name().to_owned(),
                previous_peer_id: previous.peer_id().to_owned(),
                previous_address: previous.address().to_owned(),
                previous_signing_public_key: previous.signing_public_key().to_owned(),
                previous_epoch: previous.epoch(),
                next_name: next.name().to_owned(),
                next_peer_id: next.peer_id().to_owned(),
                next_address: next.address().to_owned(),
                next_signing_public_key: next.signing_public_key().to_owned(),
                next_epoch: next.epoch(),
            }
        }
        SupervisorRotationRecoveryKind::TerminalReceipt => {
            mm_dsl::MeerkatMachineInput::RecoverSupervisorRotationTerminalReceipt {
                operation_id,
                phase,
                rejection,
                previous_name: previous.name().to_owned(),
                previous_peer_id: previous.peer_id().to_owned(),
                previous_address: previous.address().to_owned(),
                previous_signing_public_key: previous.signing_public_key().to_owned(),
                previous_epoch: previous.epoch(),
                next_name: next.name().to_owned(),
                next_peer_id: next.peer_id().to_owned(),
                next_address: next.address().to_owned(),
                next_signing_public_key: next.signing_public_key().to_owned(),
                next_epoch: next.epoch(),
            }
        }
    };
    mm_dsl::MeerkatMachineMutator::apply(authority, input).map(|_| ())
}

pub(crate) fn recover_supervisor_rotation_operation(
    authority: &mut mm_dsl::MeerkatMachineAuthority,
    rotation: &crate::store::SupervisorRotationReceipt,
) -> Result<(), mm_dsl::MeerkatMachineTransitionError> {
    recover_supervisor_rotation_receipt(
        authority,
        rotation,
        SupervisorRotationRecoveryKind::CurrentOperation,
    )
}

pub(crate) fn recover_supervisor_rotation_terminal_receipt(
    authority: &mut mm_dsl::MeerkatMachineAuthority,
    rotation: &crate::store::SupervisorRotationReceipt,
) -> Result<(), mm_dsl::MeerkatMachineTransitionError> {
    recover_supervisor_rotation_receipt(
        authority,
        rotation,
        SupervisorRotationRecoveryKind::TerminalReceipt,
    )
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
            None,
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
            None,
            None,
        );

        assert!(matches!(
            result,
            Err(mm_dsl::MeerkatMachineTransitionError::GuardRejected { .. })
        ));
    }

    #[test]
    fn recover_authority_from_runtime_observation_normalizes_cold_running_to_idle() {
        let session_id = SessionId::from_uuid(uuid::Uuid::nil());

        let authority = recover_authority_from_runtime_observation(
            &session_id,
            RuntimeState::Running,
            None,
            None,
            None,
            BTreeSet::from(["silent".to_string()]),
            None,
            None,
            None,
        )
        .expect("cold Running authority without a durable run witness must recover");

        assert_eq!(
            authority.state().lifecycle_phase,
            mm_dsl::MeerkatPhase::Idle,
            "a process-local executor cannot survive cold recovery"
        );
        assert_eq!(
            authority.state().session_id,
            Some(mm_dsl::SessionId::from_domain(&session_id))
        );
        assert!(authority.state().current_run_id.is_none());
        assert!(authority.state().pre_run_phase.is_none());
        assert!(authority.state().silent_intent_overrides.contains("silent"));
    }

    #[test]
    fn recover_authority_from_runtime_observation_retains_complete_running_witness() {
        let session_id = SessionId::from_uuid(uuid::Uuid::nil());
        let run_id = RunId::new();

        let authority = recover_authority_from_runtime_observation(
            &session_id,
            RuntimeState::Running,
            None,
            Some(&run_id),
            Some(RuntimeState::Retired),
            BTreeSet::new(),
            None,
            None,
            None,
        )
        .expect("complete live run witness should retain Running authority");

        assert_eq!(
            authority.state().lifecycle_phase,
            mm_dsl::MeerkatPhase::Running
        );
        assert_eq!(current_run_id_from_authority(&authority), Some(run_id));
        assert_eq!(
            pre_run_phase_from_authority(&authority),
            Some(RuntimeState::Retired)
        );
    }

    #[test]
    fn map_error_includes_context() {
        let err = mm_dsl::MeerkatMachineTransitionError::NoMatchingTransition {
            phase: mm_dsl::MeerkatPhase::Idle,
            trigger: mm_dsl::MeerkatMachineTransitionTrigger::Input(
                mm_dsl::MeerkatMachineInputVariant::Destroy,
            ),
        };
        let msg = map_error(err, "test_context");
        assert!(msg.contains("test_context"));
        assert!(msg.contains("Idle"));
    }

    // P0 Dogma Invariant 1, FOLD 1: the visible-runtime-phase arbitration is now
    // owned by the MeerkatMachine `ResolveVisibleRuntimePhase` classifier. This
    // test pins EXACT behavioral parity with the prior handwritten shell policy
    // (`should_publish_control_over_dsl` + `visible_runtime_phase`) for every
    // combination of the five pure observations, including the
    // Running+pre_run(Retired) special cases.
    #[test]
    #[allow(clippy::expect_used)]
    fn resolve_visible_runtime_phase_matches_legacy_policy_for_all_combos() {
        // Reference implementation of the deleted shell policy (tests only).
        fn reference_should_publish_control(
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
        fn reference_visible(phase: RuntimeState, pre_run: Option<RuntimeState>) -> RuntimeState {
            if phase == RuntimeState::Running && pre_run == Some(RuntimeState::Retired) {
                RuntimeState::Retired
            } else {
                phase
            }
        }

        const STATES: [RuntimeState; 7] = [
            RuntimeState::Initializing,
            RuntimeState::Idle,
            RuntimeState::Attached,
            RuntimeState::Running,
            RuntimeState::Retired,
            RuntimeState::Stopped,
            RuntimeState::Destroyed,
        ];
        const PRE_RUNS: [Option<RuntimeState>; 4] = [
            None,
            Some(RuntimeState::Idle),
            Some(RuntimeState::Attached),
            Some(RuntimeState::Retired),
        ];

        for &dsl_phase in &STATES {
            for &control_phase in &STATES {
                for &dsl_pre_run in &PRE_RUNS {
                    for &control_pre_run in &PRE_RUNS {
                        for &has_persistence in &[false, true] {
                            let plan = crate::meerkat_machine::resolve_visible_runtime_phase(
                                dsl_phase,
                                dsl_pre_run,
                                control_phase,
                                control_pre_run,
                                has_persistence,
                            )
                            .expect("total classifier always emits a verdict");

                            let expect_publish = has_persistence
                                && reference_should_publish_control(
                                    control_phase,
                                    dsl_phase,
                                    dsl_pre_run,
                                );
                            assert_eq!(
                                plan.publish_control, expect_publish,
                                "publish_control mismatch dsl={dsl_phase:?} control={control_phase:?} \
                                 dsl_pre={dsl_pre_run:?} ctrl_pre={control_pre_run:?} persist={has_persistence}"
                            );

                            let expect_raw = if expect_publish {
                                control_phase
                            } else {
                                dsl_phase
                            };
                            assert_eq!(
                                plan.selected_raw_phase, expect_raw,
                                "selected_raw_phase mismatch dsl={dsl_phase:?} control={control_phase:?} \
                                 dsl_pre={dsl_pre_run:?} ctrl_pre={control_pre_run:?} persist={has_persistence}"
                            );

                            let expect_visible = if expect_publish {
                                reference_visible(control_phase, control_pre_run)
                            } else {
                                reference_visible(dsl_phase, dsl_pre_run)
                            };
                            assert_eq!(
                                plan.visible_phase, expect_visible,
                                "visible_phase mismatch dsl={dsl_phase:?} control={control_phase:?} \
                                 dsl_pre={dsl_pre_run:?} ctrl_pre={control_pre_run:?} persist={has_persistence}"
                            );
                        }
                    }
                }
            }
        }
    }
}
