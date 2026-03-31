//! Target session binding machine — owns which session receives the next
//! inbound remote turn on the target surface.

use std::fmt;

use meerkat_core::types::SessionId;

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Unbound,
    Bound {
        session_id: SessionId,
    },
    Switching {
        from_session_id: SessionId,
        to_session_id: SessionId,
    },
}

impl State {
    pub fn current_session_id(&self) -> Option<&SessionId> {
        match self {
            State::Bound { session_id } => Some(session_id),
            State::Switching { to_session_id, .. } => Some(to_session_id),
            State::Unbound => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    BootResolved { session_id: SessionId },
    CreateNewRequested,
    ResumeRequested { session_id: SessionId },
    SwitchPrepared { session_id: SessionId },
    SwitchCommitted { session_id: SessionId },
    SwitchFailed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    SetupSession { resume_id: Option<SessionId> },
    SubscribeSessionEvents { session_id: SessionId },
    UnregisterRuntimeSession { session_id: SessionId },
    DiscardLiveSession { session_id: SessionId },
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

pub fn transition(state: State, event: Event) -> Result<(State, Vec<Effect>), TransitionError> {
    match (state, event) {
        (State::Unbound, Event::BootResolved { session_id }) => {
            Ok((State::Bound { session_id }, vec![]))
        }

        (State::Bound { session_id }, Event::CreateNewRequested) => Ok((
            State::Switching {
                from_session_id: session_id.clone(),
                to_session_id: session_id,
            },
            vec![Effect::SetupSession { resume_id: None }],
        )),
        (
            State::Bound { session_id },
            Event::ResumeRequested {
                session_id: resume_id,
            },
        ) => {
            if resume_id == session_id {
                Ok((State::Bound { session_id }, vec![]))
            } else {
                Ok((
                    State::Switching {
                        from_session_id: session_id.clone(),
                        to_session_id: session_id,
                    },
                    vec![Effect::SetupSession {
                        resume_id: Some(resume_id),
                    }],
                ))
            }
        }

        (
            State::Switching {
                from_session_id, ..
            },
            Event::SwitchPrepared { session_id },
        ) => {
            if session_id == from_session_id {
                Ok((State::Bound { session_id }, vec![]))
            } else {
                Ok((
                    State::Switching {
                        from_session_id: from_session_id.clone(),
                        to_session_id: session_id.clone(),
                    },
                    vec![
                        Effect::SubscribeSessionEvents {
                            session_id: session_id.clone(),
                        },
                        // Ordering is semantically significant: unregister first,
                        // then discard. Callers must execute effects sequentially
                        // in emitted order.
                        Effect::UnregisterRuntimeSession {
                            session_id: from_session_id.clone(),
                        },
                        Effect::DiscardLiveSession {
                            session_id: from_session_id,
                        },
                    ],
                ))
            }
        }
        (State::Switching { to_session_id, .. }, Event::SwitchCommitted { session_id }) => {
            if session_id != to_session_id {
                return Err(err("Switching", "SwitchCommitted", "session_id mismatch"));
            }
            Ok((State::Bound { session_id }, vec![]))
        }
        (
            State::Switching {
                from_session_id, ..
            },
            Event::SwitchFailed,
        ) => Ok((
            State::Bound {
                session_id: from_session_id,
            },
            vec![],
        )),

        (state, event) => {
            let state_name = match &state {
                State::Unbound => "Unbound",
                State::Bound { .. } => "Bound",
                State::Switching { .. } => "Switching",
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

    fn sid() -> SessionId {
        SessionId::new()
    }

    #[test]
    fn boot_binds_session() {
        let id = sid();
        let (state, effects) = transition(
            State::Unbound,
            Event::BootResolved {
                session_id: id.clone(),
            },
        )
        .unwrap();
        assert_eq!(state, State::Bound { session_id: id });
        assert!(effects.is_empty());
    }

    #[test]
    fn resume_current_session_is_noop() {
        let id = sid();
        let (state, effects) = transition(
            State::Bound {
                session_id: id.clone(),
            },
            Event::ResumeRequested {
                session_id: id.clone(),
            },
        )
        .unwrap();
        assert_eq!(state, State::Bound { session_id: id });
        assert!(effects.is_empty());
    }

    #[test]
    fn switching_emits_ordered_teardown_effects() {
        let old = sid();
        let new = sid();
        let (state, _effects) = transition(
            State::Bound {
                session_id: old.clone(),
            },
            Event::CreateNewRequested,
        )
        .unwrap();
        let (state, effects) = transition(
            state,
            Event::SwitchPrepared {
                session_id: new.clone(),
            },
        )
        .unwrap();
        assert_eq!(
            state,
            State::Switching {
                from_session_id: old.clone(),
                to_session_id: new.clone()
            }
        );
        assert_eq!(
            effects,
            vec![
                Effect::SubscribeSessionEvents {
                    session_id: new.clone()
                },
                Effect::UnregisterRuntimeSession {
                    session_id: old.clone()
                },
                Effect::DiscardLiveSession { session_id: old },
            ]
        );
        assert!(matches!(
            effects.as_slice(),
            [
                Effect::SubscribeSessionEvents { .. },
                Effect::UnregisterRuntimeSession { .. },
                Effect::DiscardLiveSession { .. }
            ]
        ));
    }
}
