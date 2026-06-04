//! Target-side kennel control-session authority.
//!
//! The generated DSL authority owns register/ack/control-session lifecycle and
//! admits control payloads through typed machine input.

use meerkat_machine_dsl::machine;

machine! {
    machine TargetKennelSession {
        version: 1,
        rust: "mdm-tux" / "machines::target_kennel_session",

        state {
            phase: TargetKennelSessionPhase,
            pending_attached_tux_id: Option<String>,
        }

        init(Disconnected) {
            pending_attached_tux_id = None,
        }

        terminal []

        phase TargetKennelSessionPhase {
            Disconnected,
            RegisteringInitial,
            RegisteringRefresh,
            Registered,
            Rejected,
        }

        input TargetKennelSessionInput {
            RegisterSent,
            RegistrationAcked,
            RegistrationRejected,
            ControlLost,
            AdmitControlPayload,
            RecordReregisterIntent {
                attached_tux_id: Option<String>,
            },
        }

        effect TargetKennelSessionEffect {
            RegisterPayloadAuthorized {
                attached_tux_id: Option<String>,
            },
            ControlPayloadAdmitted,
        }

        disposition RegisterPayloadAuthorized => local,
        disposition ControlPayloadAdmitted => local,

        transition RecordReregisterIntentDisconnected {
            on input RecordReregisterIntent { attached_tux_id }
            guard { self.phase == Phase::Disconnected }
            update {
                self.pending_attached_tux_id = attached_tux_id;
            }
            to Disconnected
        }

        transition RecordReregisterIntentRegistered {
            on input RecordReregisterIntent { attached_tux_id }
            guard { self.phase == Phase::Registered }
            update {
                self.pending_attached_tux_id = attached_tux_id;
            }
            to Registered
        }

        transition RegisterSentInitial {
            on input RegisterSent
            guard { self.phase == Phase::Disconnected }
            update {}
            to RegisteringInitial
            emit RegisterPayloadAuthorized { attached_tux_id: self.pending_attached_tux_id }
        }

        transition RegisterSentRefresh {
            on input RegisterSent
            guard { self.phase == Phase::Registered }
            update {}
            to RegisteringRefresh
            emit RegisterPayloadAuthorized { attached_tux_id: self.pending_attached_tux_id }
        }

        transition RegistrationAckedInitial {
            on input RegistrationAcked
            guard { self.phase == Phase::RegisteringInitial }
            update {
                self.pending_attached_tux_id = None;
            }
            to Registered
        }

        transition RegistrationAckedRefresh {
            on input RegistrationAcked
            guard { self.phase == Phase::RegisteringRefresh }
            update {
                self.pending_attached_tux_id = None;
            }
            to Registered
        }

        transition RegistrationRejectedInitial {
            on input RegistrationRejected
            guard { self.phase == Phase::RegisteringInitial }
            update {}
            to Rejected
        }

        transition RegistrationRejectedRefresh {
            on input RegistrationRejected
            guard { self.phase == Phase::RegisteringRefresh }
            update {}
            to Rejected
        }

        transition ControlLostDisconnected {
            on input ControlLost
            guard { self.phase == Phase::Disconnected }
            update {}
            to Disconnected
        }

        transition ControlLostRegisteringInitial {
            on input ControlLost
            guard { self.phase == Phase::RegisteringInitial }
            update {}
            to Disconnected
        }

        transition ControlLostRegisteringRefresh {
            on input ControlLost
            guard { self.phase == Phase::RegisteringRefresh }
            update {}
            to Disconnected
        }

        transition ControlLostRegistered {
            on input ControlLost
            guard { self.phase == Phase::Registered }
            update {}
            to Disconnected
        }

        transition ControlLostRejected {
            on input ControlLost
            guard { self.phase == Phase::Rejected }
            update {}
            to Disconnected
        }

        transition AdmitControlPayloadRegistered {
            on input AdmitControlPayload
            guard { self.phase == Phase::Registered }
            update {}
            to Registered
            emit ControlPayloadAdmitted
        }

        transition AdmitControlPayloadDuringRefresh {
            on input AdmitControlPayload
            guard { self.phase == Phase::RegisteringRefresh }
            update {}
            to RegisteringRefresh
            emit ControlPayloadAdmitted
        }
    }
}

pub type State = TargetKennelSessionAuthority;
pub type Event = TargetKennelSessionInput;
pub type Effect = TargetKennelSessionEffect;
pub type TransitionError = TargetKennelSessionTransitionError;

pub fn transition(mut state: State, event: Event) -> Result<State, TransitionError> {
    TargetKennelSessionMutator::apply(&mut state, event)?;
    Ok(state)
}

pub fn register_sent(mut state: State) -> Result<(State, Option<String>), TransitionError> {
    let transition = TargetKennelSessionMutator::apply(&mut state, Event::RegisterSent)?;
    let attached_tux_id = transition
        .into_effects()
        .into_iter()
        .find_map(|effect| match effect {
            Effect::RegisterPayloadAuthorized { attached_tux_id } => Some(attached_tux_id),
            Effect::ControlPayloadAdmitted => None,
        })
        .expect("RegisterSent transition must emit RegisterPayloadAuthorized");
    Ok((state, attached_tux_id))
}

pub fn record_reregister_intent(
    mut state: State,
    attached_tux_id: Option<String>,
) -> Result<State, TransitionError> {
    TargetKennelSessionMutator::apply(
        &mut state,
        Event::RecordReregisterIntent { attached_tux_id },
    )?;
    Ok(state)
}

pub fn admit_control_payload(
    state: &mut State,
) -> Result<TargetKennelSessionTransition, TransitionError> {
    TargetKennelSessionMutator::apply(state, Event::AdmitControlPayload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_ack_enters_registered() {
        let state = transition(State::new(), Event::RegisterSent).unwrap();
        let state = transition(state, Event::RegistrationAcked).unwrap();
        assert_eq!(state.state().phase(), TargetKennelSessionPhase::Registered);
        let mut state = state;
        let transition = admit_control_payload(&mut state).unwrap();
        assert_eq!(transition.effects, vec![Effect::ControlPayloadAdmitted]);
    }

    #[test]
    fn reregister_refresh_keeps_control_session_alive() {
        let state = transition(State::new(), Event::RegisterSent).unwrap();
        let state = transition(state, Event::RegistrationAcked).unwrap();
        let state = transition(state, Event::RegisterSent).unwrap();
        assert_eq!(
            state.state().phase(),
            TargetKennelSessionPhase::RegisteringRefresh
        );
        let mut state = state;
        let admission = admit_control_payload(&mut state).unwrap();
        assert_eq!(admission.effects, vec![Effect::ControlPayloadAdmitted]);
        let state = transition(state, Event::RegistrationAcked).unwrap();
        assert_eq!(state.state().phase(), TargetKennelSessionPhase::Registered);
    }

    #[test]
    fn reregister_intent_survives_retry_until_ack() {
        let (state, attached_tux_id) = register_sent(State::new()).unwrap();
        assert_eq!(attached_tux_id, None);
        let state = transition(state, Event::RegistrationAcked).unwrap();
        let state = record_reregister_intent(state, Some("tux-1".into())).unwrap();
        let (state, attached_tux_id) = register_sent(state).unwrap();
        assert_eq!(attached_tux_id, Some("tux-1".into()));
        let state = transition(state, Event::ControlLost).unwrap();
        let (state, attached_tux_id) = register_sent(state).unwrap();
        assert_eq!(attached_tux_id, Some("tux-1".into()));
        let state = transition(state, Event::RegistrationAcked).unwrap();
        let (state, attached_tux_id) = register_sent(state).unwrap();
        assert_eq!(state.state().phase(), TargetKennelSessionPhase::RegisteringRefresh);
        assert_eq!(attached_tux_id, None);
    }

    #[test]
    fn rejected_registration_is_terminal_until_disconnect() {
        let state = transition(State::new(), Event::RegisterSent).unwrap();
        let state = transition(state, Event::RegistrationRejected).unwrap();
        assert_eq!(state.state().phase(), TargetKennelSessionPhase::Rejected);
        let state = transition(state, Event::ControlLost).unwrap();
        assert_eq!(state.state().phase(), TargetKennelSessionPhase::Disconnected);
    }

    #[test]
    fn control_payload_before_registration_is_rejected_by_generated_authority() {
        let mut state = State::new();
        assert!(admit_control_payload(&mut state).is_err());
    }
}
