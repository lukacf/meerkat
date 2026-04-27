//! TUX claim state machine — per-target claim lifecycle on the TUX side.
//!
//! Simplified for RPC-only TUX: no comms attach flow. After ClaimGranted + Ack,
//! the claim goes directly to Claimed and emits RpcConnect.

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
    Claimed {
        target_id: String,
        target_name: String,
        lease_id: String,
        rpc_addr: Option<String>,
    },
    ReleasePending {
        target_id: String,
        target_name: String,
        lease_id: String,
    },
}

impl State {
    pub fn target_id(&self) -> &str {
        match self {
            State::Available { target_id, .. }
            | State::ClaimRequested { target_id, .. }
            | State::Claimed { target_id, .. }
            | State::ReleasePending { target_id, .. } => target_id,
        }
    }

    pub fn target_name(&self) -> &str {
        match self {
            State::Available { target_name, .. }
            | State::ClaimRequested { target_name, .. }
            | State::Claimed { target_name, .. }
            | State::ReleasePending { target_name, .. } => target_name,
        }
    }

    pub fn lease_id(&self) -> Option<&str> {
        match self {
            State::Claimed { lease_id, .. } | State::ReleasePending { lease_id, .. } => {
                Some(lease_id)
            }
            State::Available { .. } | State::ClaimRequested { .. } => None,
        }
    }

    pub fn is_attached_for_rebind(&self) -> bool {
        matches!(self, State::Claimed { .. })
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    SeenAvailable {
        target_id: String,
        target_name: String,
    },
    SeenMine {
        lease_id: String,
    },
    ClaimRequested,
    ClaimGranted {
        lease_id: String,
        rpc_addr: Option<String>,
        now_ms: i64,
    },
    ClaimReleased,
    ReleaseRequested,
    KennelDisconnected,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    SendClaimToKennel { target_id: String },
    SendClaimAck { lease_id: String },
    SendReleaseToKennel { lease_id: String },
    RpcConnect { rpc_addr: String },
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
            Event::SeenMine { lease_id },
        ) => Ok((
            State::Claimed {
                target_id,
                target_name,
                lease_id,
                rpc_addr: None,
            },
            vec![],
        )),
        (
            State::Available {
                target_id,
                target_name,
            },
            Event::KennelDisconnected,
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
                rpc_addr,
                now_ms: _,
            },
        ) => {
            let mut effects = vec![Effect::SendClaimAck {
                lease_id: lease_id.clone(),
            }];
            if let Some(addr) = &rpc_addr {
                effects.push(Effect::RpcConnect {
                    rpc_addr: addr.clone(),
                });
            }
            Ok((
                State::Claimed {
                    target_id,
                    target_name,
                    lease_id,
                    rpc_addr,
                },
                effects,
            ))
        }
        (
            State::ClaimRequested {
                target_id,
                target_name,
            },
            Event::SeenMine { lease_id },
        ) => Ok((
            State::Claimed {
                target_id,
                target_name,
                lease_id,
                rpc_addr: None,
            },
            vec![],
        )),
        (
            State::ClaimRequested {
                target_id,
                target_name,
            },
            Event::KennelDisconnected | Event::ClaimReleased,
        ) => Ok((
            State::Available {
                target_id,
                target_name,
            },
            vec![],
        )),

        (
            State::Claimed {
                target_id,
                target_name,
                lease_id,
                ..
            },
            Event::ReleaseRequested,
        ) => Ok((
            State::ReleasePending {
                target_id,
                target_name,
                lease_id: lease_id.clone(),
            },
            vec![Effect::SendReleaseToKennel { lease_id }],
        )),
        (
            State::Claimed {
                target_id,
                target_name,
                ..
            },
            Event::SeenMine { lease_id },
        ) => Ok((
            State::Claimed {
                target_id,
                target_name,
                lease_id,
                rpc_addr: None,
            },
            vec![],
        )),
        (
            State::Claimed {
                target_id,
                target_name,
                ..
            },
            Event::ClaimReleased,
        ) => Ok((
            State::Available {
                target_id,
                target_name,
            },
            vec![],
        )),
        (
            State::Claimed {
                target_id,
                target_name,
                lease_id,
                rpc_addr,
            },
            Event::KennelDisconnected,
        ) => Ok((
            // Stay claimed — kennel disconnect doesn't invalidate the lease
            State::Claimed {
                target_id,
                target_name,
                lease_id,
                rpc_addr,
            },
            vec![],
        )),

        (
            State::ReleasePending {
                target_id,
                target_name,
                lease_id: current_lease_id,
            },
            Event::SeenMine { lease_id },
        ) => {
            let effects = if lease_id != current_lease_id {
                vec![Effect::SendReleaseToKennel {
                    lease_id: lease_id.clone(),
                }]
            } else {
                vec![]
            };
            Ok((
                State::ReleasePending {
                    target_id,
                    target_name,
                    lease_id,
                },
                effects,
            ))
        }
        (
            State::ReleasePending {
                target_id,
                target_name,
                ..
            },
            Event::ClaimReleased,
        ) => Ok((
            State::Available {
                target_id,
                target_name,
            },
            vec![],
        )),
        (
            State::ReleasePending {
                target_id,
                target_name,
                lease_id,
            },
            Event::KennelDisconnected,
        ) => Ok((
            State::ReleasePending {
                target_id,
                target_name,
                lease_id,
            },
            vec![],
        )),

        (state, event) => {
            let name = match &state {
                State::Available { .. } => "Available",
                State::ClaimRequested { .. } => "ClaimRequested",
                State::Claimed { .. } => "Claimed",
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

    #[test]
    fn claim_granted_goes_directly_to_claimed_with_rpc_connect() {
        let (state, effects) = transition(
            State::ClaimRequested {
                target_id: "t1".into(),
                target_name: "target-1".into(),
            },
            Event::ClaimGranted {
                lease_id: "lease-1".into(),
                rpc_addr: Some("tcp://1.2.3.4:4800".into()),
                now_ms: 100,
            },
        )
        .unwrap();

        assert!(matches!(
            state,
            State::Claimed {
                lease_id, ..
            } if lease_id == "lease-1"
        ));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SendClaimAck { .. }))
        );
        assert!(effects.iter().any(
            |e| matches!(e, Effect::RpcConnect { rpc_addr } if rpc_addr == "tcp://1.2.3.4:4800")
        ));
    }

    #[test]
    fn claim_granted_without_rpc_addr_omits_rpc_connect() {
        let (_, effects) = transition(
            State::ClaimRequested {
                target_id: "t1".into(),
                target_name: "target-1".into(),
            },
            Event::ClaimGranted {
                lease_id: "lease-1".into(),
                rpc_addr: None,
                now_ms: 100,
            },
        )
        .unwrap();

        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::RpcConnect { .. }))
        );
    }

    #[test]
    fn release_pending_seen_mine_reissues_release_for_updated_lease() {
        let (state, effects) = transition(
            State::ReleasePending {
                target_id: "t1".into(),
                target_name: "target-1".into(),
                lease_id: "lease-old".into(),
            },
            Event::SeenMine {
                lease_id: "lease-new".into(),
            },
        )
        .unwrap();

        assert!(matches!(
            state,
            State::ReleasePending { lease_id, .. } if lease_id == "lease-new"
        ));
        assert!(matches!(
            effects.as_slice(),
            [Effect::SendReleaseToKennel { lease_id }] if lease_id == "lease-new"
        ));
    }
}
