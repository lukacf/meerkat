//! TUX claim state machine — per-target claim lifecycle on the TUX side.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Available {
        target_id: String,
        target_name: String,
    },
    ClaimRequested {
        target_id: String,
        target_name: String,
    },
    Attaching {
        target_id: String,
        target_name: String,
        target_pubkey: String,
        target_direct_addr: String,
        lease_id: String,
        started_at_ms: i64,
    },
    Claimed {
        target_id: String,
        target_name: String,
        target_pubkey: String,
        target_direct_addr: String,
        lease_id: String,
    },
    ClaimUncertain {
        target_id: String,
        target_name: String,
        target_pubkey: String,
        target_direct_addr: String,
        lease_id: String,
    },
    ReleasePending {
        target_id: String,
        target_name: String,
        target_pubkey: String,
        target_direct_addr: String,
        lease_id: String,
    },
}

impl State {
    pub fn target_id(&self) -> &str {
        match self {
            State::Available { target_id, .. }
            | State::ClaimRequested { target_id, .. }
            | State::Attaching { target_id, .. }
            | State::Claimed { target_id, .. }
            | State::ClaimUncertain { target_id, .. }
            | State::ReleasePending { target_id, .. } => target_id,
        }
    }

    pub fn target_name(&self) -> &str {
        match self {
            State::Available { target_name, .. }
            | State::ClaimRequested { target_name, .. }
            | State::Attaching { target_name, .. }
            | State::Claimed { target_name, .. }
            | State::ClaimUncertain { target_name, .. }
            | State::ReleasePending { target_name, .. } => target_name,
        }
    }

    pub fn lease_id(&self) -> Option<&str> {
        match self {
            State::Attaching { lease_id, .. }
            | State::Claimed { lease_id, .. }
            | State::ClaimUncertain { lease_id, .. }
            | State::ReleasePending { lease_id, .. } => Some(lease_id),
            State::Available { .. } | State::ClaimRequested { .. } => None,
        }
    }

    pub fn target_pubkey(&self) -> Option<&str> {
        match self {
            State::Attaching { target_pubkey, .. }
            | State::Claimed { target_pubkey, .. }
            | State::ClaimUncertain { target_pubkey, .. }
            | State::ReleasePending { target_pubkey, .. } => Some(target_pubkey),
            State::Available { .. } | State::ClaimRequested { .. } => None,
        }
    }

    pub fn target_direct_addr(&self) -> Option<&str> {
        match self {
            State::Attaching {
                target_direct_addr, ..
            }
            | State::Claimed {
                target_direct_addr, ..
            }
            | State::ClaimUncertain {
                target_direct_addr, ..
            }
            | State::ReleasePending {
                target_direct_addr, ..
            } => Some(target_direct_addr),
            State::Available { .. } | State::ClaimRequested { .. } => None,
        }
    }

    pub fn is_attached_for_rebind(&self) -> bool {
        matches!(self, State::Claimed { .. } | State::ClaimUncertain { .. })
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    SeenAvailable {
        target_id: String,
        target_name: String,
    },
    ClaimRequested,
    ClaimGranted {
        lease_id: String,
        target_pubkey: String,
        target_direct_addr: String,
        now_ms: i64,
    },
    AttachConfirmed {
        lease_id: String,
    },
    LeaseRebound {
        new_lease_id: String,
        target_pubkey: String,
        target_direct_addr: String,
    },
    ClaimReleased,
    ReleaseRequested,
    MineListDropped,
    AttachTimeout {
        now_ms: i64,
        timeout_ms: i64,
    },
    KennelDisconnected,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    EnsureTrustedPeer {
        target_pubkey: String,
        target_direct_addr: String,
    },
    RemoveTrustedPeer {
        target_pubkey: String,
    },
    SendClaimToKennel {
        target_id: String,
    },
    SendClaimAck {
        lease_id: String,
    },
    SendAttachConfirmed {
        lease_id: String,
    },
    SendReleaseToKennel {
        lease_id: String,
    },
    ClearUiProjection,
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
            State::Available {
                target_id,
                target_name,
            },
            Event::ClaimRequested,
        ) => Ok((
            State::ClaimRequested {
                target_id: target_id.clone(),
                target_name: target_name.clone(),
            },
            vec![Effect::SendClaimToKennel { target_id }],
        )),
        (
            State::Available {
                target_id: _,
                target_name: _,
            },
            Event::SeenAvailable {
                target_id,
                target_name,
            },
        ) => Ok((
            State::Available {
                target_id,
                target_name,
            },
            vec![],
        )),
        (
            State::Available {
                target_id,
                target_name,
            },
            Event::KennelDisconnected | Event::MineListDropped,
        ) => Ok((
            State::Available {
                target_id,
                target_name,
            },
            vec![],
        )),

        (
            State::ClaimRequested {
                target_id,
                target_name,
            },
            Event::ClaimGranted {
                lease_id,
                target_pubkey,
                target_direct_addr,
                now_ms,
            },
        ) => Ok((
            State::Attaching {
                target_id: target_id.clone(),
                target_name,
                target_pubkey: target_pubkey.clone(),
                target_direct_addr: target_direct_addr.clone(),
                lease_id: lease_id.clone(),
                started_at_ms: now_ms,
            },
            vec![
                Effect::EnsureTrustedPeer {
                    target_pubkey,
                    target_direct_addr,
                },
                Effect::SendClaimAck { lease_id },
            ],
        )),
        (
            State::ClaimRequested {
                target_id,
                target_name,
            },
            Event::KennelDisconnected | Event::ClaimReleased | Event::MineListDropped,
        ) => Ok((
            State::Available {
                target_id,
                target_name,
            },
            vec![Effect::ClearUiProjection],
        )),

        (
            State::Attaching {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr,
                lease_id,
                ..
            },
            Event::AttachConfirmed { lease_id: cf_lid },
        ) => {
            if cf_lid != lease_id {
                return Err(err("Attaching", "AttachConfirmed", "lease_id mismatch"));
            }
            Ok((
                State::Claimed {
                    target_id,
                    target_name,
                    target_pubkey,
                    target_direct_addr,
                    lease_id: cf_lid.clone(),
                },
                vec![Effect::SendAttachConfirmed { lease_id: cf_lid }],
            ))
        }
        (
            State::Attaching {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr,
                lease_id,
                started_at_ms,
            },
            Event::AttachTimeout { now_ms, timeout_ms },
        ) => {
            if now_ms - started_at_ms >= timeout_ms {
                Ok((
                    State::ClaimUncertain {
                        target_id,
                        target_name,
                        target_pubkey,
                        target_direct_addr,
                        lease_id,
                    },
                    vec![],
                ))
            } else {
                Ok((
                    State::Attaching {
                        target_id,
                        target_name,
                        target_pubkey,
                        target_direct_addr,
                        lease_id,
                        started_at_ms,
                    },
                    vec![],
                ))
            }
        }
        (
            State::Attaching {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr: _,
                lease_id,
                ..
            },
            Event::ClaimReleased | Event::MineListDropped,
        ) => Ok((
            State::Available {
                target_id,
                target_name,
            },
            vec![
                Effect::RemoveTrustedPeer { target_pubkey },
                Effect::ClearUiProjection,
                Effect::SendReleaseToKennel { lease_id },
            ],
        )),
        (
            State::Attaching {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr,
                lease_id,
                ..
            },
            Event::KennelDisconnected,
        ) => Ok((
            State::ClaimUncertain {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr,
                lease_id,
            },
            vec![],
        )),

        (
            State::Claimed {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr,
                lease_id,
            },
            Event::ReleaseRequested,
        ) => Ok((
            State::ReleasePending {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr,
                lease_id: lease_id.clone(),
            },
            vec![Effect::SendReleaseToKennel { lease_id }],
        )),
        (
            State::Claimed {
                target_id,
                target_name,
                target_pubkey: _,
                target_direct_addr: _,
                lease_id: _,
            },
            Event::LeaseRebound {
                new_lease_id,
                target_pubkey: rebound_pk,
                target_direct_addr: rebound_addr,
            },
        ) => Ok((
            State::Claimed {
                target_id,
                target_name,
                target_pubkey: rebound_pk.clone(),
                target_direct_addr: rebound_addr.clone(),
                lease_id: new_lease_id,
            },
            vec![Effect::EnsureTrustedPeer {
                target_pubkey: rebound_pk,
                target_direct_addr: rebound_addr,
            }],
        )),
        (
            State::Claimed {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr: _,
                lease_id,
            },
            Event::ClaimReleased | Event::MineListDropped,
        ) => Ok((
            State::Available {
                target_id,
                target_name,
            },
            vec![
                Effect::RemoveTrustedPeer { target_pubkey },
                Effect::ClearUiProjection,
                Effect::SendReleaseToKennel { lease_id },
            ],
        )),
        (
            State::Claimed {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr,
                lease_id,
            },
            Event::KennelDisconnected,
        ) => Ok((
            State::ClaimUncertain {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr,
                lease_id,
            },
            vec![],
        )),

        (
            State::ClaimUncertain {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr,
                lease_id,
            },
            Event::AttachConfirmed { lease_id: cf_lid },
        ) => {
            if cf_lid != lease_id {
                return Err(err(
                    "ClaimUncertain",
                    "AttachConfirmed",
                    "lease_id mismatch",
                ));
            }
            Ok((
                State::Claimed {
                    target_id,
                    target_name,
                    target_pubkey,
                    target_direct_addr,
                    lease_id: cf_lid.clone(),
                },
                vec![Effect::SendAttachConfirmed { lease_id: cf_lid }],
            ))
        }
        (
            State::ClaimUncertain {
                target_id,
                target_name,
                target_pubkey: _,
                target_direct_addr: _,
                lease_id: _,
            },
            Event::LeaseRebound {
                new_lease_id,
                target_pubkey: rebound_pk,
                target_direct_addr: rebound_addr,
            },
        ) => Ok((
            State::Claimed {
                target_id,
                target_name,
                target_pubkey: rebound_pk.clone(),
                target_direct_addr: rebound_addr.clone(),
                lease_id: new_lease_id,
            },
            vec![Effect::EnsureTrustedPeer {
                target_pubkey: rebound_pk,
                target_direct_addr: rebound_addr,
            }],
        )),
        (
            State::ClaimUncertain {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr: _,
                lease_id: _,
            },
            Event::ClaimReleased | Event::MineListDropped,
        ) => Ok((
            State::Available {
                target_id,
                target_name,
            },
            vec![
                Effect::RemoveTrustedPeer { target_pubkey },
                Effect::ClearUiProjection,
            ],
        )),
        (
            State::ClaimUncertain {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr,
                lease_id,
            },
            Event::KennelDisconnected,
        ) => Ok((
            State::ClaimUncertain {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr,
                lease_id,
            },
            vec![],
        )),
        (
            State::ClaimUncertain {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr,
                lease_id,
            },
            Event::ReleaseRequested,
        ) => Ok((
            State::ReleasePending {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr,
                lease_id: lease_id.clone(),
            },
            vec![Effect::SendReleaseToKennel { lease_id }],
        )),

        (
            State::ReleasePending {
                target_id,
                target_name,
                target_pubkey,
                ..
            },
            Event::ClaimReleased | Event::MineListDropped,
        ) => Ok((
            State::Available {
                target_id,
                target_name,
            },
            vec![
                Effect::RemoveTrustedPeer { target_pubkey },
                Effect::ClearUiProjection,
            ],
        )),
        (
            State::ReleasePending {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr,
                lease_id,
            },
            Event::KennelDisconnected,
        ) => Ok((
            State::ReleasePending {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr,
                lease_id,
            },
            vec![],
        )),
        (
            State::ReleasePending {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr,
                lease_id,
            },
            Event::LeaseRebound { .. },
        ) => Ok((
            State::ReleasePending {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr,
                lease_id,
            },
            vec![],
        )),

        (state, event) => {
            let name = match &state {
                State::Available { .. } => "Available",
                State::ClaimRequested { .. } => "ClaimRequested",
                State::Attaching { .. } => "Attaching",
                State::Claimed { .. } => "Claimed",
                State::ClaimUncertain { .. } => "ClaimUncertain",
                State::ReleasePending { .. } => "ReleasePending",
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

    fn available() -> State {
        State::Available {
            target_id: "t1".into(),
            target_name: "target-1".into(),
        }
    }

    #[test]
    fn release_pending_does_not_rebind() {
        let (state, _) = transition(available(), Event::ClaimRequested).unwrap();
        let (state, _) = transition(
            state,
            Event::ClaimGranted {
                lease_id: "l1".into(),
                target_pubkey: "ed25519:t".into(),
                target_direct_addr: "tcp://1.2.3.4:9".into(),
                now_ms: 0,
            },
        )
        .unwrap();
        let (state, _) = transition(
            state,
            Event::AttachConfirmed {
                lease_id: "l1".into(),
            },
        )
        .unwrap();
        let (state, _) = transition(state, Event::ReleaseRequested).unwrap();
        let (state, effects) = transition(
            state,
            Event::LeaseRebound {
                new_lease_id: "l2".into(),
                target_pubkey: "ed25519:t".into(),
                target_direct_addr: "tcp://1.2.3.4:10".into(),
            },
        )
        .unwrap();
        assert!(matches!(state, State::ReleasePending { .. }));
        assert!(effects.is_empty());
    }
}
