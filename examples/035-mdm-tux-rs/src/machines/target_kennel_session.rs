//! Target-side kennel control-session phase machine.
//!
//! Owns the register/ack/control-session boundary so the target binary does
//! not infer control-phase legality from ad hoc loop structure.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Disconnected,
    RegisteringInitial,
    RegisteringRefresh,
    Registered,
    Rejected,
}

impl State {
    pub fn allows_control_payloads(&self) -> bool {
        matches!(self, State::Registered | State::RegisteringRefresh)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    RegisterSent,
    RegistrationAcked,
    RegistrationRejected,
    ControlLost,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TransitionError {
    pub state: &'static str,
    pub event: String,
    pub reason: String,
}

impl fmt::Display for TransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid transition: {} in state {} ({})",
            self.event, self.state, self.reason
        )
    }
}

fn err(state: &'static str, event: &str, reason: &str) -> TransitionError {
    TransitionError {
        state,
        event: event.to_string(),
        reason: reason.to_string(),
    }
}

pub fn transition(state: State, event: Event) -> Result<State, TransitionError> {
    match (state, event) {
        (State::Disconnected, Event::RegisterSent) => Ok(State::RegisteringInitial),
        (State::Registered, Event::RegisterSent) => Ok(State::RegisteringRefresh),
        (State::RegisteringInitial, Event::RegistrationAcked)
        | (State::RegisteringRefresh, Event::RegistrationAcked) => Ok(State::Registered),
        (State::RegisteringInitial, Event::RegistrationRejected)
        | (State::RegisteringRefresh, Event::RegistrationRejected) => Ok(State::Rejected),
        (State::RegisteringInitial, Event::ControlLost)
        | (State::RegisteringRefresh, Event::ControlLost)
        | (State::Registered, Event::ControlLost) => Ok(State::Disconnected),
        (State::Rejected, Event::ControlLost) => Ok(State::Disconnected),
        (state, event) => {
            let state_name = match &state {
                State::Disconnected => "Disconnected",
                State::RegisteringInitial => "RegisteringInitial",
                State::RegisteringRefresh => "RegisteringRefresh",
                State::Registered => "Registered",
                State::Rejected => "Rejected",
            };
            Err(err(
                state_name,
                &format!("{event:?}"),
                "unhandled event in this state",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_ack_enters_registered() {
        let state = transition(State::Disconnected, Event::RegisterSent).unwrap();
        let state = transition(state, Event::RegistrationAcked).unwrap();
        assert_eq!(state, State::Registered);
        assert!(state.allows_control_payloads());
    }

    #[test]
    fn reregister_refresh_keeps_control_session_alive() {
        let state = transition(State::Registered, Event::RegisterSent).unwrap();
        assert_eq!(state, State::RegisteringRefresh);
        assert!(state.allows_control_payloads());
        let state = transition(state, Event::RegistrationAcked).unwrap();
        assert_eq!(state, State::Registered);
    }

    #[test]
    fn rejected_registration_is_terminal_until_disconnect() {
        let state = transition(State::Disconnected, Event::RegisterSent).unwrap();
        let state = transition(state, Event::RegistrationRejected).unwrap();
        assert_eq!(state, State::Rejected);
        let state = transition(state, Event::ControlLost).unwrap();
        assert_eq!(state, State::Disconnected);
    }
}
