//! Target attachment state machine — target-side kennel-mode lifecycle.
//!
//! Pure function: `transition(state, event) -> Result<(State, Vec<Effect>), TransitionError>`.
//! No I/O. Caller interprets effects as I/O.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Idle,
    Attaching {
        target_id: String,
        lease_id: String,
        tux_id: String,
        tux_pubkey: String,
        tux_direct_addr: String,
        kennel_alive: bool,
    },
    Attached {
        target_id: String,
        lease_id: String,
        tux_id: String,
        tux_pubkey: String,
        tux_direct_addr: String,
        kennel_alive: bool,
    },
    Released,
    AttachFailed,
    DirectLost {
        tux_id: Option<String>,
    },
}

impl State {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            State::Released | State::AttachFailed | State::DirectLost { .. }
        )
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Adopted {
        target_id: String,
        lease_id: String,
        tux_id: String,
        tux_pubkey: String,
        tux_direct_addr: String,
    },
    LeaseRebound {
        target_id: String,
        lease_id: String,
        tux_id: String,
        tux_pubkey: String,
        tux_direct_addr: String,
    },
    DirectAttachAck {
        lease_id: String,
    },
    KennelReleased {
        lease_id: String,
    },
    KennelDisconnected,
    KennelHeartbeatFailed,
    DirectLinkLost,
    AttachSendFailed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    EnsureTuxPeer {
        tux_pubkey: String,
        tux_direct_addr: String,
    },
    SendAttachRequest {
        target_id: String,
        lease_id: String,
    },
    StartDirectHeartbeat,
    StopDirectHeartbeat,
    ReregisterWithHint {
        tux_id: String,
    },
    ReregisterClean,
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
        (
            State::Idle,
            Event::Adopted {
                target_id,
                lease_id,
                tux_id,
                tux_pubkey,
                tux_direct_addr,
            },
        ) => Ok((
            State::Attaching {
                target_id: target_id.clone(),
                lease_id: lease_id.clone(),
                tux_id: tux_id.clone(),
                tux_pubkey: tux_pubkey.clone(),
                tux_direct_addr: tux_direct_addr.clone(),
                kennel_alive: true,
            },
            vec![
                Effect::EnsureTuxPeer {
                    tux_pubkey,
                    tux_direct_addr,
                },
                Effect::SendAttachRequest {
                    target_id,
                    lease_id,
                },
            ],
        )),
        (
            State::Idle,
            Event::LeaseRebound {
                target_id,
                lease_id,
                tux_id,
                tux_pubkey,
                tux_direct_addr,
            },
        ) => Ok((
            State::Attached {
                target_id,
                lease_id,
                tux_id,
                tux_pubkey: tux_pubkey.clone(),
                tux_direct_addr: tux_direct_addr.clone(),
                kennel_alive: true,
            },
            vec![
                Effect::EnsureTuxPeer {
                    tux_pubkey,
                    tux_direct_addr,
                },
                Effect::StartDirectHeartbeat,
            ],
        )),

        (
            State::Attaching {
                target_id,
                lease_id,
                tux_id,
                tux_pubkey,
                tux_direct_addr,
                kennel_alive: _,
            },
            Event::DirectAttachAck { lease_id: ack_lid },
        ) => {
            if ack_lid != lease_id {
                return Err(err("Attaching", "DirectAttachAck", "lease_id mismatch"));
            }
            Ok((
                State::Attached {
                    target_id,
                    lease_id,
                    tux_id,
                    tux_pubkey,
                    tux_direct_addr,
                    kennel_alive: true,
                },
                vec![Effect::StartDirectHeartbeat],
            ))
        }
        (
            State::Attaching {
                target_id,
                lease_id,
                tux_id,
                tux_pubkey,
                tux_direct_addr,
                ..
            },
            Event::KennelReleased { lease_id: rel_lid },
        ) => {
            if rel_lid != lease_id && !rel_lid.is_empty() {
                return Ok((
                    State::Attaching {
                        target_id,
                        lease_id,
                        tux_id,
                        tux_pubkey,
                        tux_direct_addr,
                        kennel_alive: true,
                    },
                    vec![],
                ));
            }
            Ok((State::Released, vec![]))
        }
        (State::Attaching { .. }, Event::KennelDisconnected)
        | (State::Attaching { .. }, Event::KennelHeartbeatFailed) => {
            Ok((State::AttachFailed, vec![Effect::ReregisterClean]))
        }
        (State::Attaching { .. }, Event::AttachSendFailed) => Ok((State::AttachFailed, vec![])),
        (State::Attaching { .. }, Event::DirectLinkLost) => {
            Ok((State::AttachFailed, vec![Effect::ReregisterClean]))
        }

        (
            State::Attached {
                target_id,
                lease_id,
                tux_id,
                tux_pubkey,
                tux_direct_addr,
                kennel_alive,
                ..
            },
            Event::KennelReleased { lease_id: rel_lid },
        ) => {
            if rel_lid.is_empty() || rel_lid == lease_id {
                Ok((State::Released, vec![Effect::StopDirectHeartbeat]))
            } else {
                Ok((
                    State::Attached {
                        target_id,
                        lease_id,
                        tux_id,
                        tux_pubkey,
                        tux_direct_addr,
                        kennel_alive,
                    },
                    vec![],
                ))
            }
        }
        (
            State::Attached {
                target_id,
                lease_id,
                tux_id,
                tux_pubkey,
                tux_direct_addr,
                ..
            },
            Event::LeaseRebound {
                target_id: rebound_target_id,
                lease_id: rebound_lid,
                tux_id: rebound_tid,
                tux_pubkey: rebound_pk,
                tux_direct_addr: rebound_addr,
            },
        ) => {
            if rebound_target_id != target_id {
                return Err(err("Attached", "LeaseRebound", "target_id mismatch"));
            }
            if rebound_tid != tux_id {
                return Err(err("Attached", "LeaseRebound", "tux_id mismatch"));
            }
            if rebound_lid == lease_id
                && rebound_pk == tux_pubkey
                && rebound_addr == tux_direct_addr
            {
                Ok((
                    State::Attached {
                        target_id,
                        lease_id,
                        tux_id,
                        tux_pubkey,
                        tux_direct_addr,
                        kennel_alive: true,
                    },
                    vec![],
                ))
            } else {
                Ok((
                    State::Attached {
                        target_id,
                        lease_id: rebound_lid,
                        tux_id,
                        tux_pubkey: rebound_pk.clone(),
                        tux_direct_addr: rebound_addr.clone(),
                        kennel_alive: true,
                    },
                    vec![Effect::EnsureTuxPeer {
                        tux_pubkey: rebound_pk,
                        tux_direct_addr: rebound_addr,
                    }],
                ))
            }
        }
        (State::Attached { tux_id, .. }, Event::KennelDisconnected)
        | (State::Attached { tux_id, .. }, Event::KennelHeartbeatFailed) => Ok((
            State::DirectLost {
                tux_id: Some(tux_id.clone()),
            },
            vec![
                Effect::StopDirectHeartbeat,
                Effect::ReregisterWithHint { tux_id },
            ],
        )),
        (State::Attached { .. }, Event::DirectLinkLost) => Ok((
            State::DirectLost { tux_id: None },
            vec![Effect::StopDirectHeartbeat],
        )),

        (State::Released, event) => Err(err("Released", &format!("{event:?}"), "terminal state")),
        (State::AttachFailed, event) => {
            Err(err("AttachFailed", &format!("{event:?}"), "terminal state"))
        }
        (State::DirectLost { .. }, event) => {
            Err(err("DirectLost", &format!("{event:?}"), "terminal state"))
        }

        (state, event) => {
            let name = match &state {
                State::Idle => "Idle",
                State::Attaching { .. } => "Attaching",
                State::Attached { .. } => "Attached",
                State::Released => "Released",
                State::AttachFailed => "AttachFailed",
                State::DirectLost { .. } => "DirectLost",
            };
            Err(err(
                name,
                &format!("{event:?}"),
                "unhandled event in this state",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adopted() -> Event {
        Event::Adopted {
            target_id: "target-1".into(),
            lease_id: "lease-1".into(),
            tux_id: "tux-1".into(),
            tux_pubkey: "ed25519:tux".into(),
            tux_direct_addr: "tcp://1.2.3.4:9000".into(),
        }
    }

    #[test]
    fn adopted_emits_peer_refresh_and_attach_request() {
        let (state, effects) = transition(State::Idle, adopted()).unwrap();
        assert!(matches!(state, State::Attaching { .. }));
        assert_eq!(
            effects,
            vec![
                Effect::EnsureTuxPeer {
                    tux_pubkey: "ed25519:tux".into(),
                    tux_direct_addr: "tcp://1.2.3.4:9000".into()
                },
                Effect::SendAttachRequest {
                    target_id: "target-1".into(),
                    lease_id: "lease-1".into()
                }
            ]
        );
    }

    #[test]
    fn stale_attach_ack_is_rejected() {
        let (state, _) = transition(State::Idle, adopted()).unwrap();
        let err = transition(
            state,
            Event::DirectAttachAck {
                lease_id: "wrong".into(),
            },
        )
        .unwrap_err();
        assert!(err.reason.contains("lease_id mismatch"));
    }
}
