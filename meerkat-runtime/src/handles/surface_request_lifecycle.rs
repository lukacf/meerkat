//! Runtime-owned surface request lifecycle authority.
//!
//! The request executor in surface crates owns transport mechanics (task
//! handles and cleanup/cancel closures). This handle routes semantic state
//! changes through the MeerkatMachine DSL so request phase, cancellability,
//! terminal classification, and cleanup decisions are machine-owned.

use std::sync::Arc;

use meerkat_core::handles::{
    CancelActionInstallDecision, CancelOutcome, CancelTransition, CompleteTransition,
    RequestAlreadyExists, RequestTransitionError, SurfaceRequestKind,
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
    pub(crate) fn new(dsl: Arc<HandleDslAuthority>) -> Self {
        Self { dsl }
    }

    pub(crate) fn standalone() -> Self {
        Self::new(Arc::new(HandleDslAuthority::ephemeral()))
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
    ) -> Option<CompleteTransition> {
        effects.iter().find_map(|effect| match effect {
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
    }
}

impl SurfaceRequestLifecycleHandle for RuntimeSurfaceRequestLifecycleHandle {
    fn try_begin_request(
        &self,
        key: String,
        kind: SurfaceRequestKind,
    ) -> Result<(), RequestAlreadyExists> {
        self.dsl
            .apply_input(
                mm_dsl::MeerkatMachineInput::BeginSurfaceRequest {
                    request_id: key,
                    kind: kind.into(),
                },
                "SurfaceRequestLifecycleHandle::try_begin_request",
            )
            .map_err(|_| RequestAlreadyExists)
    }

    fn classify_terminal(
        &self,
        key: &str,
        outcome: SurfaceRequestTerminalOutcome,
    ) -> Result<SurfaceRequestTerminalDisposition, RequestTransitionError> {
        let effects = self
            .dsl
            .apply_input_with_effects(
                mm_dsl::MeerkatMachineInput::ClassifySurfaceRequestTerminal {
                    request_id: key.to_owned(),
                    outcome: outcome.into(),
                },
                "SurfaceRequestLifecycleHandle::classify_terminal",
            )
            .map_err(|_| self.map_transition_error(key))?;

        effects
            .iter()
            .find_map(|effect| match effect {
                mm_dsl::MeerkatMachineEffect::SurfaceRequestTerminalClassified {
                    request_id,
                    disposition,
                } if request_id == key => Some((*disposition).into()),
                _ => None,
            })
            .ok_or_else(|| self.map_transition_error(key))
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

    fn complete_committed(&self, key: &str) -> Result<(), RequestTransitionError> {
        self.dsl
            .apply_input(
                mm_dsl::MeerkatMachineInput::CompleteSurfaceRequestCommitted {
                    request_id: key.to_owned(),
                },
                "SurfaceRequestLifecycleHandle::complete_committed",
            )
            .map_err(|_| self.map_transition_error(key))
    }

    fn finish_unpublished(&self, key: &str) -> Result<CompleteTransition, RequestTransitionError> {
        let effects = self
            .dsl
            .apply_input_with_effects(
                mm_dsl::MeerkatMachineInput::FinishSurfaceRequestUnpublished {
                    request_id: key.to_owned(),
                },
                "SurfaceRequestLifecycleHandle::finish_unpublished",
            )
            .map_err(|_| self.map_transition_error(key))?;
        Self::transition_from_finish_effects(key, &effects)
            .ok_or_else(|| self.map_transition_error(key))
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
        let handle = RuntimeSurfaceRequestLifecycleHandle::standalone();

        handle
            .try_begin_request(
                "request".to_owned(),
                SurfaceRequestKind::CancellableObservation,
            )
            .expect("request begins");

        let transition = handle.cancel_request("request");
        assert_eq!(transition.outcome, CancelOutcome::Cancelled);
        assert!(transition.fire_cancel_action);

        let decision = handle.cancel_action_install_decision("request");
        assert_eq!(decision.phase, Some(SurfaceRequestPhase::Cancelled));
        assert!(decision.fire_cancel_action);
    }

    #[test]
    fn cancel_action_install_decision_does_not_promote_inline_request() {
        let handle = RuntimeSurfaceRequestLifecycleHandle::standalone();

        handle
            .try_begin_request("request".to_owned(), SurfaceRequestKind::InlineObservation)
            .expect("request begins");

        let transition = handle.cancel_request("request");
        assert_eq!(transition.outcome, CancelOutcome::Cancelled);
        assert!(!transition.fire_cancel_action);

        let decision = handle.cancel_action_install_decision("request");
        assert_eq!(decision.phase, Some(SurfaceRequestPhase::Cancelled));
        assert!(!decision.fire_cancel_action);
    }

    #[test]
    fn terminal_classification_fails_closed_for_unknown_request() {
        let handle = RuntimeSurfaceRequestLifecycleHandle::standalone();

        assert_eq!(
            handle.classify_terminal("missing", SurfaceRequestTerminalOutcome::Succeeded),
            Err(RequestTransitionError::NotFound)
        );
    }

    #[test]
    fn committed_failure_classification_completes_after_cancel() {
        let handle = RuntimeSurfaceRequestLifecycleHandle::standalone();

        handle
            .try_begin_request("request".to_owned(), SurfaceRequestKind::SessionTurn)
            .expect("request begins");
        assert_eq!(
            handle.classify_terminal("request", SurfaceRequestTerminalOutcome::CommittedFailure),
            Ok(SurfaceRequestTerminalDisposition::Commit)
        );
        assert_eq!(
            handle.cancel_request("request").outcome,
            CancelOutcome::Cancelled
        );
        assert_eq!(handle.complete_committed("request"), Ok(()));
        assert_eq!(handle.phase("request"), None);
    }

    #[test]
    fn committed_mutation_success_classification_completes_after_cancel() {
        let handle = RuntimeSurfaceRequestLifecycleHandle::standalone();

        handle
            .try_begin_request("request".to_owned(), SurfaceRequestKind::CommittedMutation)
            .expect("request begins");
        assert_eq!(
            handle.cancel_request("request").outcome,
            CancelOutcome::Cancelled
        );
        assert_eq!(
            handle.classify_terminal("request", SurfaceRequestTerminalOutcome::Succeeded),
            Ok(SurfaceRequestTerminalDisposition::Publish)
        );
        assert_eq!(handle.complete_committed("request"), Ok(()));
        assert_eq!(handle.phase("request"), None);
    }

    #[test]
    fn dsl_terminal_classification_rejects_unknown_request() {
        let authority = HandleDslAuthority::ephemeral();

        let err = authority
            .apply_input_with_effects(
                mm_dsl::MeerkatMachineInput::ClassifySurfaceRequestTerminal {
                    request_id: "missing".to_owned(),
                    outcome: mm_dsl::SurfaceRequestTerminalOutcome::Succeeded,
                },
                "dsl_terminal_classification_rejects_unknown_request",
            )
            .expect_err("missing request classification must be rejected by the DSL authority");

        assert!(err.is_guard_rejected());
    }

    #[test]
    fn publish_and_cancel_fail_closed_for_unknown_request() {
        let handle = RuntimeSurfaceRequestLifecycleHandle::standalone();

        assert_eq!(
            handle.publish_and_complete("missing"),
            Err(RequestTransitionError::NotFound)
        );
        assert_eq!(
            handle.cancel_request("missing"),
            CancelTransition {
                outcome: CancelOutcome::NotFound,
                fire_cancel_action: false
            }
        );
    }

    #[test]
    fn unpublished_finish_fails_closed_for_unknown_request() {
        let handle = RuntimeSurfaceRequestLifecycleHandle::standalone();

        assert_eq!(
            handle.finish_unpublished("missing"),
            Err(RequestTransitionError::NotFound)
        );
    }

    #[test]
    fn dsl_unpublished_finish_rejects_unknown_request() {
        let authority = HandleDslAuthority::ephemeral();

        let err = authority
            .apply_input_with_effects(
                mm_dsl::MeerkatMachineInput::FinishSurfaceRequestUnpublished {
                    request_id: "missing".to_owned(),
                },
                "dsl_unpublished_finish_rejects_unknown_request",
            )
            .expect_err("missing unpublished finish must be rejected by the DSL authority");

        assert!(err.is_guard_rejected());
    }
}
