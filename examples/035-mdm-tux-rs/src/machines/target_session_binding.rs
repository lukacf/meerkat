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
    SettingUp {
        from_session_id: SessionId,
        resume_id: Option<SessionId>,
    },
    Subscribing {
        from_session_id: SessionId,
        to_session_id: SessionId,
    },
    TearingDown {
        from_session_id: SessionId,
        to_session_id: SessionId,
    },
}

impl State {
    pub fn current_session_id(&self) -> Option<&SessionId> {
        match self {
            State::Bound { session_id } => Some(session_id),
            State::SettingUp {
                from_session_id, ..
            }
            | State::Subscribing {
                from_session_id, ..
            }
            | State::TearingDown {
                from_session_id, ..
            } => Some(from_session_id),
            State::Unbound => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    BootResolved { session_id: SessionId },
    CreateNewRequested,
    ResumeRequested { session_id: SessionId },
    SetupSucceeded { session_id: SessionId },
    SetupFailed,
    SubscriptionEstablished,
    SubscriptionFailed,
    TeardownCompleted,
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
            State::SettingUp {
                from_session_id: session_id,
                resume_id: None,
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
                    State::SettingUp {
                        from_session_id: session_id,
                        resume_id: Some(resume_id.clone()),
                    },
                    vec![Effect::SetupSession {
                        resume_id: Some(resume_id),
                    }],
                ))
            }
        }

        (
            State::SettingUp {
                from_session_id,
                resume_id,
            },
            Event::SetupSucceeded { session_id },
        ) => {
            if Some(session_id.clone()) == resume_id || resume_id.is_none() {
                Ok((
                    State::Subscribing {
                        from_session_id,
                        to_session_id: session_id.clone(),
                    },
                    vec![Effect::SubscribeSessionEvents { session_id }],
                ))
            } else {
                Err(err("SettingUp", "SetupSucceeded", "session_id mismatch"))
            }
        }
        (
            State::SettingUp {
                from_session_id, ..
            },
            Event::SetupFailed,
        ) => Ok((
            State::Bound {
                session_id: from_session_id,
            },
            vec![],
        )),

        (
            State::Subscribing {
                from_session_id,
                to_session_id,
            },
            Event::SubscriptionEstablished,
        ) => Ok((
            State::TearingDown {
                from_session_id: from_session_id.clone(),
                to_session_id,
            },
            vec![
                // Ordering is semantically significant: unregister first, then
                // discard. Callers must execute effects sequentially in emitted
                // order.
                Effect::UnregisterRuntimeSession {
                    session_id: from_session_id.clone(),
                },
                Effect::DiscardLiveSession {
                    session_id: from_session_id,
                },
            ],
        )),
        (
            State::Subscribing {
                from_session_id,
                to_session_id,
            },
            Event::SubscriptionFailed,
        ) => Ok((
            State::Bound {
                session_id: from_session_id,
            },
            vec![
                Effect::UnregisterRuntimeSession {
                    session_id: to_session_id.clone(),
                },
                Effect::DiscardLiveSession {
                    session_id: to_session_id,
                },
            ],
        )),

        (State::TearingDown { to_session_id, .. }, Event::TeardownCompleted) => Ok((
            State::Bound {
                session_id: to_session_id,
            },
            vec![],
        )),

        (state, event) => {
            let state_name = match &state {
                State::Unbound => "Unbound",
                State::Bound { .. } => "Bound",
                State::SettingUp { .. } => "SettingUp",
                State::Subscribing { .. } => "Subscribing",
                State::TearingDown { .. } => "TearingDown",
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
    fn switching_follows_explicit_phases() {
        let old = sid();
        let new = sid();
        let (state, effects) = transition(
            State::Bound {
                session_id: old.clone(),
            },
            Event::CreateNewRequested,
        )
        .unwrap();
        assert_eq!(
            state,
            State::SettingUp {
                from_session_id: old.clone(),
                resume_id: None
            }
        );
        assert_eq!(effects, vec![Effect::SetupSession { resume_id: None }]);

        let (state, effects) = transition(
            state,
            Event::SetupSucceeded {
                session_id: new.clone(),
            },
        )
        .unwrap();
        assert_eq!(
            state,
            State::Subscribing {
                from_session_id: old.clone(),
                to_session_id: new.clone()
            }
        );
        assert_eq!(
            effects,
            vec![Effect::SubscribeSessionEvents {
                session_id: new.clone()
            }]
        );

        let (state, effects) = transition(state, Event::SubscriptionEstablished).unwrap();
        assert_eq!(
            state,
            State::TearingDown {
                from_session_id: old.clone(),
                to_session_id: new.clone()
            }
        );
        assert_eq!(
            effects,
            vec![
                Effect::UnregisterRuntimeSession {
                    session_id: old.clone()
                },
                Effect::DiscardLiveSession {
                    session_id: old.clone()
                }
            ]
        );

        let (state, effects) = transition(state, Event::TeardownCompleted).unwrap();
        assert_eq!(state, State::Bound { session_id: new });
        assert!(effects.is_empty());
    }

    #[test]
    fn subscription_failure_rolls_back_and_cleans_new_session() {
        let old = sid();
        let new = sid();
        let (state, _) = transition(
            State::Bound {
                session_id: old.clone(),
            },
            Event::CreateNewRequested,
        )
        .unwrap();
        let (state, _) = transition(
            state,
            Event::SetupSucceeded {
                session_id: new.clone(),
            },
        )
        .unwrap();
        let (state, effects) = transition(state, Event::SubscriptionFailed).unwrap();
        assert_eq!(state, State::Bound { session_id: old });
        assert_eq!(
            effects,
            vec![
                Effect::UnregisterRuntimeSession {
                    session_id: new.clone()
                },
                Effect::DiscardLiveSession { session_id: new }
            ]
        );
    }
}
