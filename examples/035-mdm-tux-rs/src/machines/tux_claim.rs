//! TUX claim state machine — per-target claim lifecycle on the TUX side.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum TargetRoute {
    AwaitingRebind,
    Known {
        target_pubkey: String,
        target_direct_addr: String,
    },
}

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
        route: TargetRoute,
        lease_id: String,
    },
    ClaimUncertain {
        target_id: String,
        target_name: String,
        route: TargetRoute,
        lease_id: String,
    },
    ReleasePending {
        target_id: String,
        target_name: String,
        route: TargetRoute,
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
            State::Attaching { target_pubkey, .. } => Some(target_pubkey),
            State::Claimed { route, .. }
            | State::ClaimUncertain { route, .. }
            | State::ReleasePending { route, .. } => route.target_pubkey(),
            State::Available { .. } | State::ClaimRequested { .. } => None,
        }
    }

    pub fn target_direct_addr(&self) -> Option<&str> {
        match self {
            State::Attaching {
                target_direct_addr, ..
            } => Some(target_direct_addr),
            State::Claimed { route, .. }
            | State::ClaimUncertain { route, .. }
            | State::ReleasePending { route, .. } => route.target_direct_addr(),
            State::Available { .. } | State::ClaimRequested { .. } => None,
        }
    }

    pub fn is_attached_for_rebind(&self) -> bool {
        matches!(self, State::Claimed { .. } | State::ClaimUncertain { .. })
    }

    pub fn route(&self) -> Option<&TargetRoute> {
        match self {
            State::Claimed { route, .. }
            | State::ClaimUncertain { route, .. }
            | State::ReleasePending { route, .. } => Some(route),
            State::Available { .. } | State::ClaimRequested { .. } | State::Attaching { .. } => {
                None
            }
        }
    }
}

impl TargetRoute {
    pub fn target_pubkey(&self) -> Option<&str> {
        match self {
            TargetRoute::AwaitingRebind => None,
            TargetRoute::Known { target_pubkey, .. } => Some(target_pubkey),
        }
    }

    pub fn target_direct_addr(&self) -> Option<&str> {
        match self {
            TargetRoute::AwaitingRebind => None,
            TargetRoute::Known {
                target_direct_addr, ..
            } => Some(target_direct_addr),
        }
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
    TargetLost,
    ClaimReleased,
    ReleaseRequested,
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

fn known_route(target_pubkey: String, target_direct_addr: String) -> TargetRoute {
    TargetRoute::Known {
        target_pubkey,
        target_direct_addr,
    }
}

fn maybe_remove_trusted_peer(route: &TargetRoute) -> Vec<Effect> {
    match route {
        TargetRoute::AwaitingRebind => vec![],
        TargetRoute::Known { target_pubkey, .. } => {
            vec![Effect::RemoveTrustedPeer {
                target_pubkey: target_pubkey.clone(),
            }]
        }
    }
}

fn claim_effects_for_release(route: &TargetRoute, lease_id: String) -> Vec<Effect> {
    let mut effects = maybe_remove_trusted_peer(route);
    effects.push(Effect::SendReleaseToKennel { lease_id });
    effects
}

fn claim_effects_for_drop(route: &TargetRoute) -> Vec<Effect> {
    maybe_remove_trusted_peer(route)
}

fn maybe_ensure_trusted_peer(route: &TargetRoute) -> Vec<Effect> {
    match route {
        TargetRoute::Known {
            target_pubkey,
            target_direct_addr,
        } => vec![Effect::EnsureTrustedPeer {
            target_pubkey: target_pubkey.clone(),
            target_direct_addr: target_direct_addr.clone(),
        }],
        TargetRoute::AwaitingRebind => vec![],
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
                route: TargetRoute::AwaitingRebind,
                lease_id,
            },
            vec![],
        )),
        (
            State::Available {
                target_id,
                target_name,
            },
            Event::LeaseRebound {
                new_lease_id,
                target_pubkey,
                target_direct_addr,
            },
        ) => {
            let route = known_route(target_pubkey.clone(), target_direct_addr.clone());
            Ok((
                State::Claimed {
                    target_id,
                    target_name,
                    route,
                    lease_id: new_lease_id,
                },
                vec![Effect::EnsureTrustedPeer {
                    target_pubkey,
                    target_direct_addr,
                }],
            ))
        }
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
            Event::SeenMine { lease_id },
        ) => Ok((
            State::Claimed {
                target_id,
                target_name,
                route: TargetRoute::AwaitingRebind,
                lease_id,
            },
            vec![],
        )),
        (
            State::ClaimRequested {
                target_id,
                target_name,
            },
            Event::LeaseRebound {
                new_lease_id,
                target_pubkey,
                target_direct_addr,
            },
        ) => {
            let route = known_route(target_pubkey.clone(), target_direct_addr.clone());
            Ok((
                State::Claimed {
                    target_id,
                    target_name,
                    route,
                    lease_id: new_lease_id,
                },
                vec![Effect::EnsureTrustedPeer {
                    target_pubkey,
                    target_direct_addr,
                }],
            ))
        }
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
            State::Attaching {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr,
                lease_id,
                ..
            },
            Event::AttachConfirmed {
                lease_id: confirmed_lease,
            },
        ) => {
            if confirmed_lease != lease_id {
                return Err(err("Attaching", "AttachConfirmed", "lease_id mismatch"));
            }
            Ok((
                State::Claimed {
                    target_id,
                    target_name,
                    route: known_route(target_pubkey, target_direct_addr),
                    lease_id: confirmed_lease.clone(),
                },
                vec![Effect::SendAttachConfirmed {
                    lease_id: confirmed_lease,
                }],
            ))
        }
        (
            State::Attaching {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr,
                lease_id,
                ..
            },
            Event::ClaimReleased,
        ) => {
            let route = known_route(target_pubkey, target_direct_addr);
            Ok((
                State::Available {
                    target_id,
                    target_name,
                },
                claim_effects_for_release(&route, lease_id),
            ))
        }
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
                route: known_route(target_pubkey, target_direct_addr),
                lease_id,
            },
            vec![],
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
            Event::TargetLost,
        ) => Ok((
            State::ClaimUncertain {
                target_id,
                target_name,
                route: known_route(target_pubkey.clone(), target_direct_addr),
                lease_id,
            },
            vec![Effect::RemoveTrustedPeer { target_pubkey }],
        )),
        (
            State::Attaching {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr,
                started_at_ms,
                ..
            },
            Event::SeenMine { lease_id },
        ) => Ok((
            State::Attaching {
                target_id,
                target_name,
                target_pubkey,
                target_direct_addr,
                lease_id,
                started_at_ms,
            },
            vec![],
        )),

        (
            State::Claimed {
                target_id,
                target_name,
                route,
                lease_id,
            },
            Event::ReleaseRequested,
        ) => Ok((
            State::ReleasePending {
                target_id,
                target_name,
                route,
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
            Event::LeaseRebound {
                new_lease_id,
                target_pubkey,
                target_direct_addr,
            },
        ) => {
            let route = known_route(target_pubkey.clone(), target_direct_addr.clone());
            Ok((
                State::Claimed {
                    target_id,
                    target_name,
                    route,
                    lease_id: new_lease_id,
                },
                vec![Effect::EnsureTrustedPeer {
                    target_pubkey,
                    target_direct_addr,
                }],
            ))
        }
        (
            State::Claimed {
                target_id,
                target_name,
                route,
                ..
            },
            Event::SeenMine { lease_id },
        ) => Ok((
            State::Claimed {
                target_id,
                target_name,
                route,
                lease_id,
            },
            vec![],
        )),
        (
            State::Claimed {
                target_id,
                target_name,
                route,
                lease_id,
            },
            Event::ClaimReleased,
        ) => Ok((
            State::Available {
                target_id,
                target_name,
            },
            claim_effects_for_release(&route, lease_id),
        )),
        (
            State::Claimed {
                target_id,
                target_name,
                route,
                lease_id,
            },
            Event::KennelDisconnected,
        ) => Ok((
            State::ClaimUncertain {
                target_id,
                target_name,
                route,
                lease_id,
            },
            vec![],
        )),
        (
            State::Claimed {
                target_id,
                target_name,
                route,
                lease_id,
            },
            Event::TargetLost,
        ) => {
            let effects = maybe_remove_trusted_peer(&route);
            Ok((
                State::ClaimUncertain {
                    target_id,
                    target_name,
                    route,
                    lease_id,
                },
                effects,
            ))
        }

        (
            State::ClaimUncertain {
                target_id,
                target_name,
                route,
                lease_id,
            },
            Event::AttachConfirmed {
                lease_id: confirmed_lease,
            },
        ) => {
            if confirmed_lease != lease_id {
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
                    route,
                    lease_id: confirmed_lease.clone(),
                },
                vec![Effect::SendAttachConfirmed {
                    lease_id: confirmed_lease,
                }],
            ))
        }
        (
            State::ClaimUncertain {
                target_id,
                target_name,
                ..
            },
            Event::LeaseRebound {
                new_lease_id,
                target_pubkey,
                target_direct_addr,
            },
        ) => {
            let route = known_route(target_pubkey.clone(), target_direct_addr.clone());
            Ok((
                State::Claimed {
                    target_id,
                    target_name,
                    route,
                    lease_id: new_lease_id,
                },
                vec![Effect::EnsureTrustedPeer {
                    target_pubkey,
                    target_direct_addr,
                }],
            ))
        }
        (
            State::ClaimUncertain {
                target_id,
                target_name,
                route,
                ..
            },
            Event::SeenMine { lease_id },
        ) => Ok((
            State::ClaimUncertain {
                target_id,
                target_name,
                route,
                lease_id,
            },
            vec![],
        )),
        (
            State::ClaimUncertain {
                target_id,
                target_name,
                route,
                ..
            },
            Event::ClaimReleased,
        ) => Ok((
            State::Available {
                target_id,
                target_name,
            },
            claim_effects_for_drop(&route),
        )),
        (
            State::ClaimUncertain {
                target_id,
                target_name,
                route,
                lease_id,
            },
            Event::KennelDisconnected | Event::TargetLost,
        ) => Ok((
            State::ClaimUncertain {
                target_id,
                target_name,
                route,
                lease_id,
            },
            vec![],
        )),
        (
            State::ClaimUncertain {
                target_id,
                target_name,
                route,
                lease_id,
            },
            Event::ReleaseRequested,
        ) => Ok((
            State::ReleasePending {
                target_id,
                target_name,
                route,
                lease_id: lease_id.clone(),
            },
            vec![Effect::SendReleaseToKennel { lease_id }],
        )),

        (
            State::ReleasePending {
                target_id,
                target_name,
                route,
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
                    route,
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
            Event::LeaseRebound {
                new_lease_id,
                target_pubkey,
                target_direct_addr,
            },
        ) => {
            let route = known_route(target_pubkey.clone(), target_direct_addr.clone());
            let mut effects = maybe_ensure_trusted_peer(&route);
            effects.push(Effect::SendReleaseToKennel {
                lease_id: new_lease_id.clone(),
            });
            Ok((
                State::ReleasePending {
                    target_id,
                    target_name,
                    route,
                    lease_id: new_lease_id,
                },
                effects,
            ))
        }
        (
            State::ReleasePending {
                target_id,
                target_name,
                route,
                ..
            },
            Event::ClaimReleased,
        ) => Ok((
            State::Available {
                target_id,
                target_name,
            },
            claim_effects_for_drop(&route),
        )),
        (
            State::ReleasePending {
                target_id,
                target_name,
                route,
                lease_id,
            },
            Event::KennelDisconnected | Event::TargetLost,
        ) => Ok((
            State::ReleasePending {
                target_id,
                target_name,
                route,
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

    #[test]
    fn release_pending_rebind_reissues_release_with_new_lease() {
        let (state, _) = transition(
            State::Claimed {
                target_id: "t1".into(),
                target_name: "target-1".into(),
                route: known_route("ed25519:t".into(), "tcp://1.2.3.4:9".into()),
                lease_id: "lease-old".into(),
            },
            Event::ReleaseRequested,
        )
        .unwrap();
        assert!(matches!(state, State::ReleasePending { .. }));

        let (state, effects) = transition(
            state,
            Event::LeaseRebound {
                new_lease_id: "lease-new".into(),
                target_pubkey: "ed25519:t".into(),
                target_direct_addr: "tcp://1.2.3.4:10".into(),
            },
        )
        .unwrap();

        assert!(matches!(
            state,
            State::ReleasePending {
                lease_id,
                route: TargetRoute::Known { .. },
                ..
            } if lease_id == "lease-new"
        ));
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::SendReleaseToKennel { lease_id } if lease_id == "lease-new"
        )));
    }

    #[test]
    fn available_can_be_rebound_into_claimed() {
        let (state, effects) = transition(
            State::Available {
                target_id: "t1".into(),
                target_name: "target-1".into(),
            },
            Event::LeaseRebound {
                new_lease_id: "lease-new".into(),
                target_pubkey: "ed25519:t".into(),
                target_direct_addr: "tcp://1.2.3.4:10".into(),
            },
        )
        .unwrap();

        assert!(matches!(
            state,
            State::Claimed {
                lease_id,
                route: TargetRoute::Known { .. },
                ..
            } if lease_id == "lease-new"
        ));
        assert!(matches!(
            effects.as_slice(),
            [Effect::EnsureTrustedPeer { .. }]
        ));
    }

    #[test]
    fn claim_requested_accepts_lease_rebound() {
        let (state, effects) = transition(
            State::ClaimRequested {
                target_id: "t1".into(),
                target_name: "target-1".into(),
            },
            Event::LeaseRebound {
                new_lease_id: "lease-rebound".into(),
                target_pubkey: "ed25519:t".into(),
                target_direct_addr: "tcp://1.2.3.4:10".into(),
            },
        )
        .unwrap();

        assert!(matches!(
            state,
            State::Claimed {
                lease_id,
                route: TargetRoute::Known { .. },
                ..
            } if lease_id == "lease-rebound"
        ));
        assert!(matches!(
            effects.as_slice(),
            [Effect::EnsureTrustedPeer { .. }]
        ));
    }

    #[test]
    fn release_pending_seen_mine_reissues_release_for_updated_lease() {
        let (state, effects) = transition(
            State::ReleasePending {
                target_id: "t1".into(),
                target_name: "target-1".into(),
                route: known_route("ed25519:t".into(), "tcp://1.2.3.4:9".into()),
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
