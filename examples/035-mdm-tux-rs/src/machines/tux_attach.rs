//! TUX-side attach gate for kennel mode.
//!
//! Owns validation of incoming direct attach requests against canonical claim
//! facts and only produces semantic confirmation after the attach ack side
//! effect has been delivered successfully.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimFacts {
    pub target_id: String,
    pub target_name: String,
    pub lease_id: String,
    pub target_pubkey: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Idle,
    AwaitingAckDelivery {
        request_peer: String,
        target_id: String,
        target_name: String,
        lease_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    AttachRequested {
        request_peer: String,
        request_target_id: String,
        request_lease_id: String,
        request_pubkey: String,
        claim: Option<ClaimFacts>,
    },
    AckDelivered,
    AckDeliveryFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    SendAttachAck {
        request_peer: String,
        lease_id: String,
    },
    ConfirmAttached {
        target_id: String,
        target_name: String,
        lease_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
            Event::AttachRequested {
                request_peer,
                request_target_id,
                request_lease_id,
                request_pubkey,
                claim,
            },
        ) => {
            let Some(claim) = claim else {
                return Ok((State::Idle, vec![]));
            };
            if claim.target_id != request_target_id {
                return Ok((State::Idle, vec![]));
            }
            if claim.lease_id != request_lease_id {
                return Ok((State::Idle, vec![]));
            }
            if claim.target_pubkey != request_pubkey {
                return Ok((State::Idle, vec![]));
            }
            Ok((
                State::AwaitingAckDelivery {
                    request_peer: request_peer.clone(),
                    target_id: claim.target_id,
                    target_name: claim.target_name,
                    lease_id: claim.lease_id.clone(),
                },
                vec![Effect::SendAttachAck {
                    request_peer,
                    lease_id: claim.lease_id,
                }],
            ))
        }
        (
            State::AwaitingAckDelivery {
                target_id,
                target_name,
                lease_id,
                ..
            },
            Event::AckDelivered,
        ) => Ok((
            State::Idle,
            vec![Effect::ConfirmAttached {
                target_id,
                target_name,
                lease_id,
            }],
        )),
        (State::AwaitingAckDelivery { .. }, Event::AckDeliveryFailed) => Ok((State::Idle, vec![])),
        (state, event) => {
            let state_name = match state {
                State::Idle => "Idle",
                State::AwaitingAckDelivery { .. } => "AwaitingAckDelivery",
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

    fn claim() -> ClaimFacts {
        ClaimFacts {
            target_id: "target-1".into(),
            target_name: "target-1".into(),
            lease_id: "lease-1".into(),
            target_pubkey: "ed25519:target".into(),
        }
    }

    #[test]
    fn valid_request_requires_ack_delivery_before_confirmation() {
        let (state, effects) = transition(
            State::Idle,
            Event::AttachRequested {
                request_peer: "target-1".into(),
                request_target_id: "target-1".into(),
                request_lease_id: "lease-1".into(),
                request_pubkey: "ed25519:target".into(),
                claim: Some(claim()),
            },
        )
        .unwrap();
        assert!(matches!(
            effects.as_slice(),
            [Effect::SendAttachAck {
                request_peer,
                lease_id
            }] if request_peer == "target-1" && lease_id == "lease-1"
        ));

        let (state, effects) = transition(state, Event::AckDelivered).unwrap();
        assert!(matches!(state, State::Idle));
        assert!(matches!(
            effects.as_slice(),
            [Effect::ConfirmAttached {
                target_id,
                target_name,
                lease_id
            }] if target_id == "target-1" && target_name == "target-1" && lease_id == "lease-1"
        ));
    }

    #[test]
    fn invalid_pubkey_is_ignored() {
        let (state, effects) = transition(
            State::Idle,
            Event::AttachRequested {
                request_peer: "target-1".into(),
                request_target_id: "target-1".into(),
                request_lease_id: "lease-1".into(),
                request_pubkey: "ed25519:wrong".into(),
                claim: Some(claim()),
            },
        )
        .unwrap();
        assert!(matches!(state, State::Idle));
        assert!(effects.is_empty());
    }

    #[test]
    fn missing_claim_is_ignored() {
        let (state, effects) = transition(
            State::Idle,
            Event::AttachRequested {
                request_peer: "target-1".into(),
                request_target_id: "target-1".into(),
                request_lease_id: "lease-1".into(),
                request_pubkey: "ed25519:target".into(),
                claim: None,
            },
        )
        .unwrap();
        assert!(matches!(state, State::Idle));
        assert!(effects.is_empty());
    }

    #[test]
    fn failed_ack_does_not_confirm() {
        let (state, _) = transition(
            State::Idle,
            Event::AttachRequested {
                request_peer: "target-1".into(),
                request_target_id: "target-1".into(),
                request_lease_id: "lease-1".into(),
                request_pubkey: "ed25519:target".into(),
                claim: Some(claim()),
            },
        )
        .unwrap();
        let (state, effects) = transition(state, Event::AckDeliveryFailed).unwrap();
        assert!(matches!(state, State::Idle));
        assert!(effects.is_empty());
    }
}
