//! Runtime-owned surface request lifecycle authority.
//!
//! The request executor in surface crates owns transport mechanics (task
//! handles and cleanup/cancel closures). This handle routes semantic state
//! changes through the MeerkatMachine DSL so request phase, cancellability,
//! terminal classification, and cleanup decisions are machine-owned.

use std::sync::Arc;

use meerkat_core::handles::{
    CancelActionInstallDecision, CancelOutcome, CancelTransition, CompleteOutcome,
    CompleteTransition, RequestAlreadyExists, RequestTransitionError,
    SurfaceRequestLifecycleHandle, SurfaceRequestPhase, SurfaceRequestTerminalDisposition,
    SurfaceRequestTerminalOutcome,
};

use super::HandleDslAuthority;
use crate::meerkat_machine::dsl as mm_dsl;

/// Runtime-backed [`SurfaceRequestLifecycleHandle`] implementation.
#[derive(Clone)]
pub(crate) struct RuntimeSurfaceRequestLifecycleHandle {
    dsl: Arc<HandleDslAuthority>,
}

impl std::fmt::Debug for RuntimeSurfaceRequestLifecycleHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeSurfaceRequestLifecycleHandle")
            .field("dsl", &self.dsl)
            .finish()
    }
}

impl RuntimeSurfaceRequestLifecycleHandle {
    pub(crate) fn new() -> Self {
        Self::new_with_dsl(Arc::new(HandleDslAuthority::ephemeral()))
    }

    fn new_with_dsl(dsl: Arc<HandleDslAuthority>) -> Self {
        Self { dsl }
    }

    fn dsl_phase(&self, key: &str) -> Option<mm_dsl::SurfaceRequestLifecyclePhase> {
        let key = key.to_owned();
        self.dsl
            .with_state_lock(|state| state.surface_request_phases.get(&key).copied())
    }

    fn transition_error_for_phase(
        phase: Option<mm_dsl::SurfaceRequestLifecyclePhase>,
    ) -> RequestTransitionError {
        match phase {
            Some(phase) => RequestTransitionError::AlreadyTerminal {
                current: phase.into(),
            },
            None => RequestTransitionError::NotFound,
        }
    }

    fn map_transition_error(&self, key: &str) -> RequestTransitionError {
        Self::transition_error_for_phase(self.dsl_phase(key))
    }

    fn decision_from_cancel_effects(
        key: &str,
        effects: &[mm_dsl::MeerkatMachineEffect],
    ) -> CancelTransition {
        effects
            .iter()
            .find_map(|effect| match effect {
                mm_dsl::MeerkatMachineEffect::SurfaceRequestCancelDecision {
                    request_id,
                    outcome,
                    fire_cancel_action,
                } if request_id == key => Some(CancelTransition {
                    outcome: (*outcome).into(),
                    fire_cancel_action: *fire_cancel_action,
                }),
                _ => None,
            })
            .unwrap_or(CancelTransition {
                outcome: CancelOutcome::NotFound,
                fire_cancel_action: false,
            })
    }

    fn decision_from_cancel_action_install_effects(
        key: &str,
        effects: &[mm_dsl::MeerkatMachineEffect],
    ) -> CancelActionInstallDecision {
        effects
            .iter()
            .find_map(|effect| match effect {
                mm_dsl::MeerkatMachineEffect::SurfaceRequestCancelActionInstallDecision {
                    request_id,
                    phase,
                    fire_cancel_action,
                } if request_id == key => Some(CancelActionInstallDecision {
                    phase: phase.map(Into::into),
                    fire_cancel_action: *fire_cancel_action,
                }),
                _ => None,
            })
            .unwrap_or(CancelActionInstallDecision {
                phase: None,
                fire_cancel_action: false,
            })
    }

    fn transition_from_finish_effects(
        key: &str,
        effects: &[mm_dsl::MeerkatMachineEffect],
    ) -> CompleteTransition {
        effects
            .iter()
            .find_map(|effect| match effect {
                mm_dsl::MeerkatMachineEffect::SurfaceRequestUnpublishedFinished {
                    request_id,
                    outcome,
                    run_unpublished_cleanup,
                } if request_id == key => Some(CompleteTransition {
                    outcome: (*outcome).into(),
                    run_unpublished_cleanup: *run_unpublished_cleanup,
                }),
                _ => None,
            })
            .unwrap_or(CompleteTransition {
                outcome: CompleteOutcome::Completed,
                run_unpublished_cleanup: false,
            })
    }
}

impl SurfaceRequestLifecycleHandle for RuntimeSurfaceRequestLifecycleHandle {
    fn try_begin_request(&self, key: String) -> Result<(), RequestAlreadyExists> {
        self.dsl
            .apply_input(
                mm_dsl::MeerkatMachineInput::BeginSurfaceRequest { request_id: key },
                "SurfaceRequestLifecycleHandle::try_begin_request",
            )
            .map_err(|_| RequestAlreadyExists)
    }

    fn authorize_publish_on_success(&self, key: &str) -> Result<(), RequestTransitionError> {
        self.dsl
            .apply_input(
                mm_dsl::MeerkatMachineInput::AuthorizeSurfaceRequestPublishOnSuccess {
                    request_id: key.to_owned(),
                },
                "SurfaceRequestLifecycleHandle::authorize_publish_on_success",
            )
            .map_err(|_| self.map_transition_error(key))
    }

    fn authorize_cancellable_observation(&self, key: &str) -> Result<(), RequestTransitionError> {
        self.dsl
            .apply_input(
                mm_dsl::MeerkatMachineInput::AuthorizeSurfaceRequestCancellableObservation {
                    request_id: key.to_owned(),
                },
                "SurfaceRequestLifecycleHandle::authorize_cancellable_observation",
            )
            .map_err(|_| self.map_transition_error(key))
    }

    fn classify_terminal(
        &self,
        key: &str,
        outcome: SurfaceRequestTerminalOutcome,
    ) -> SurfaceRequestTerminalDisposition {
        let Ok(effects) = self.dsl.apply_input_with_effects(
            mm_dsl::MeerkatMachineInput::ClassifySurfaceRequestTerminal {
                request_id: key.to_owned(),
                outcome: outcome.into(),
            },
            "SurfaceRequestLifecycleHandle::classify_terminal",
        ) else {
            return SurfaceRequestTerminalDisposition::Inline;
        };

        effects
            .iter()
            .find_map(|effect| match effect {
                mm_dsl::MeerkatMachineEffect::SurfaceRequestTerminalClassified {
                    request_id,
                    disposition,
                } if request_id == key => Some((*disposition).into()),
                _ => None,
            })
            .unwrap_or(SurfaceRequestTerminalDisposition::Inline)
    }

    fn phase(&self, key: &str) -> Option<SurfaceRequestPhase> {
        self.dsl_phase(key).map(Into::into)
    }

    fn cancel_action_install_decision(&self, key: &str) -> CancelActionInstallDecision {
        match self.dsl.apply_input_with_effects(
            mm_dsl::MeerkatMachineInput::DecideSurfaceRequestCancelActionInstall {
                request_id: key.to_owned(),
            },
            "SurfaceRequestLifecycleHandle::cancel_action_install_decision",
        ) {
            Ok(effects) => Self::decision_from_cancel_action_install_effects(key, &effects),
            Err(_) => CancelActionInstallDecision {
                phase: None,
                fire_cancel_action: false,
            },
        }
    }

    fn cancel_request(&self, key: &str) -> CancelTransition {
        match self.dsl.apply_input_with_effects(
            mm_dsl::MeerkatMachineInput::CancelSurfaceRequest {
                request_id: key.to_owned(),
            },
            "SurfaceRequestLifecycleHandle::cancel_request",
        ) {
            Ok(effects) => Self::decision_from_cancel_effects(key, &effects),
            Err(_) => CancelTransition {
                outcome: CancelOutcome::NotFound,
                fire_cancel_action: false,
            },
        }
    }

    fn publish_and_complete(&self, key: &str) -> Result<(), RequestTransitionError> {
        self.dsl
            .apply_input(
                mm_dsl::MeerkatMachineInput::PublishSurfaceRequest {
                    request_id: key.to_owned(),
                },
                "SurfaceRequestLifecycleHandle::publish_and_complete",
            )
            .map_err(|_| self.map_transition_error(key))
    }

    fn finish_unpublished(&self, key: &str) -> CompleteTransition {
        match self.dsl.apply_input_with_effects(
            mm_dsl::MeerkatMachineInput::FinishSurfaceRequestUnpublished {
                request_id: key.to_owned(),
            },
            "SurfaceRequestLifecycleHandle::finish_unpublished",
        ) {
            Ok(effects) => Self::transition_from_finish_effects(key, &effects),
            Err(_) => CompleteTransition {
                outcome: CompleteOutcome::Completed,
                run_unpublished_cleanup: false,
            },
        }
    }

    fn remove(&self, key: &str) {
        let _ = self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::RemoveSurfaceRequest {
                request_id: key.to_owned(),
            },
            "SurfaceRequestLifecycleHandle::remove",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_action_install_decision_is_dsl_effect_owned() {
        let source = include_str!("surface_request_lifecycle.rs");
        let forbidden_policy_helper = ["terminal_policy", "_cancels"].concat();
        let forbidden_policy_state = ["surface_request_terminal", "_policy"].concat();

        assert!(
            !source.contains(&forbidden_policy_helper),
            "cancel-action install decisions must not mirror terminal policy in a runtime helper"
        );
        assert!(
            !source.contains(&forbidden_policy_state),
            "runtime handle must request the DSL decision instead of reading lifecycle policy state"
        );
        assert!(source.contains("DecideSurfaceRequestCancelActionInstall"));
        assert!(source.contains("SurfaceRequestCancelActionInstallDecision"));
    }

    #[test]
    fn cancel_action_install_decision_fires_after_cancelled_cancellable_request() {
        let handle = RuntimeSurfaceRequestLifecycleHandle::new();

        handle
            .try_begin_request("request".to_owned())
            .expect("request begins");
        handle
            .authorize_cancellable_observation("request")
            .expect("request is cancellable");

        let transition = handle.cancel_request("request");
        assert_eq!(transition.outcome, CancelOutcome::Cancelled);
        assert!(transition.fire_cancel_action);

        let decision = handle.cancel_action_install_decision("request");
        assert_eq!(decision.phase, Some(SurfaceRequestPhase::Cancelled));
        assert!(decision.fire_cancel_action);
    }

    #[test]
    fn cancel_action_install_decision_does_not_promote_inline_request() {
        let handle = RuntimeSurfaceRequestLifecycleHandle::new();

        handle
            .try_begin_request("request".to_owned())
            .expect("request begins");

        let transition = handle.cancel_request("request");
        assert_eq!(transition.outcome, CancelOutcome::Cancelled);
        assert!(!transition.fire_cancel_action);

        let decision = handle.cancel_action_install_decision("request");
        assert_eq!(decision.phase, Some(SurfaceRequestPhase::Cancelled));
        assert!(!decision.fire_cancel_action);
    }
}
