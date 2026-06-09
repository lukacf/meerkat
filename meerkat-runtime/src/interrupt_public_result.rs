use crate::meerkat_machine::dsl;
use crate::runtime_state::RuntimeState;
use crate::traits::RuntimeDriverError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserInterruptObservation {
    Accepted,
    StagedNoop,
    NotReady(RuntimeState),
    Destroyed,
    NotInterruptible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserInterruptPublicResult {
    Interrupted,
    /// #348: a staged (not-yet-promoted) session interrupt is a typed no-op
    /// terminal, distinct from a real `Interrupted` cancellation.
    StagedNoop,
    NotFound,
    SessionBusy,
    Conflict,
}

impl UserInterruptObservation {
    fn into_dsl(self) -> dsl::UserInterruptObservationKind {
        match self {
            Self::Accepted => dsl::UserInterruptObservationKind::Accepted,
            Self::StagedNoop => dsl::UserInterruptObservationKind::StagedNoop,
            Self::NotReady(RuntimeState::Idle) => dsl::UserInterruptObservationKind::IdleNoop,
            Self::NotReady(RuntimeState::Attached) => {
                dsl::UserInterruptObservationKind::AttachedNoop
            }
            Self::NotReady(RuntimeState::Destroyed) | Self::Destroyed => {
                dsl::UserInterruptObservationKind::Destroyed
            }
            Self::NotReady(_) | Self::NotInterruptible => {
                dsl::UserInterruptObservationKind::NotInterruptible
            }
        }
    }
}

pub fn resolve_user_interrupt_public_result(
    observation: UserInterruptObservation,
    target_present: bool,
    staged_promotion_busy: bool,
) -> Result<UserInterruptPublicResult, RuntimeDriverError> {
    let mut authority = dsl::MeerkatMachineAuthority::new();
    let transition = dsl::MeerkatMachineMutator::apply(
        &mut authority,
        dsl::MeerkatMachineInput::ResolveUserInterruptPublicResult {
            observation: observation.into_dsl(),
            target_present,
            staged_promotion_busy,
        },
    )
    .map_err(|err| {
        RuntimeDriverError::Internal(crate::meerkat_machine::dsl_authority::map_error(
            err,
            "ResolveUserInterruptPublicResult",
        ))
    })?;

    let mut resolved = None;
    for effect in transition.into_effects() {
        let dsl::MeerkatMachineEffect::UserInterruptPublicResultResolved { result } = effect else {
            return Err(RuntimeDriverError::Internal(format!(
                "unexpected user interrupt public-result effect: {effect:?}"
            )));
        };
        if resolved.replace(result).is_some() {
            return Err(RuntimeDriverError::Internal(
                "generated user interrupt authority emitted multiple public results".to_string(),
            ));
        }
    }

    match resolved.ok_or_else(|| {
        RuntimeDriverError::Internal(
            "generated user interrupt authority emitted no public result".to_string(),
        )
    })? {
        dsl::UserInterruptPublicResultKind::Interrupted => {
            Ok(UserInterruptPublicResult::Interrupted)
        }
        dsl::UserInterruptPublicResultKind::StagedNoop => Ok(UserInterruptPublicResult::StagedNoop),
        dsl::UserInterruptPublicResultKind::NotFound => Ok(UserInterruptPublicResult::NotFound),
        dsl::UserInterruptPublicResultKind::SessionBusy => {
            Ok(UserInterruptPublicResult::SessionBusy)
        }
        dsl::UserInterruptPublicResultKind::Conflict => Ok(UserInterruptPublicResult::Conflict),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_interrupt_result_classifies_noop_success() {
        let result = resolve_user_interrupt_public_result(
            UserInterruptObservation::NotReady(RuntimeState::Idle),
            true,
            false,
        )
        .expect("idle interrupt result should classify");
        assert_eq!(result, UserInterruptPublicResult::Interrupted);
    }

    #[test]
    fn generated_interrupt_result_classifies_staged_noop_success() {
        // #348: a staged-session interrupt now resolves to the typed
        // `StagedNoop` terminal, not `Interrupted` (no live run was cancelled).
        let result =
            resolve_user_interrupt_public_result(UserInterruptObservation::StagedNoop, true, false)
                .expect("staged noop interrupt result should classify");
        assert_eq!(result, UserInterruptPublicResult::StagedNoop);
    }

    #[test]
    fn generated_interrupt_result_classifies_destroyed_missing_as_not_found() {
        let result =
            resolve_user_interrupt_public_result(UserInterruptObservation::Destroyed, false, false)
                .expect("destroyed missing interrupt result should classify");
        assert_eq!(result, UserInterruptPublicResult::NotFound);
    }

    #[test]
    fn generated_interrupt_result_classifies_promoting_rejection_as_session_busy() {
        let result = resolve_user_interrupt_public_result(
            UserInterruptObservation::NotInterruptible,
            true,
            true,
        )
        .expect("promoting interrupt result should classify");
        assert_eq!(result, UserInterruptPublicResult::SessionBusy);
    }
}
