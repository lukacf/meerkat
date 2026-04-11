//! TUX-side kennel control composition.
//!
//! Owns reconnect/release/list-reconcile semantics above the
//! per-target claim leaf machine. Simplified for RPC-only TUX:
//! no comms attach flow, no rebind targets, no target-lost handling.

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::{ClaimGrant, TargetListEntry};

use super::tux_claim;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub kennel_connected: bool,
    pub claims: HashMap<String, tux_claim::State>,
}

impl State {
    pub fn current_attached_target_ids(&self) -> Vec<String> {
        self.claims
            .values()
            .filter(|state| state.is_attached_for_rebind())
            .map(|state| state.target_id().to_string())
            .collect()
    }

    pub fn claim_by_lease_id(&self, lease_id: &str) -> Option<tux_claim::State> {
        self.claims
            .values()
            .find(|state| state.lease_id() == Some(lease_id))
            .cloned()
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    KennelConnected,
    KennelDisconnected,
    SeenAvailableList {
        targets: Vec<TargetListEntry>,
    },
    SeenMineList {
        targets: Vec<TargetListEntry>,
    },
    UserClaimTarget {
        target_id: String,
    },
    UserReleaseLease {
        lease_id: String,
    },
    ClaimGranted {
        claims: Vec<ClaimGrant>,
        now_ms: i64,
    },
    ClaimReleased {
        target_id: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Claim {
        target_id: String,
        effect: tux_claim::Effect,
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

fn apply_claim_event(
    claims: &mut HashMap<String, tux_claim::State>,
    target_id: &str,
    event: tux_claim::Event,
    out: &mut Vec<Effect>,
) -> Result<(), TransitionError> {
    let Some(state) = claims.get(target_id).cloned() else {
        return Ok(());
    };
    let (new_state, effects) = tux_claim::transition(state, event)
        .map_err(|e| err("TuxClaim", "ClaimEvent", &e.reason))?;
    claims.insert(target_id.to_string(), new_state);
    out.extend(effects.into_iter().map(|effect| Effect::Claim {
        target_id: target_id.to_string(),
        effect,
    }));
    Ok(())
}

pub fn transition(mut state: State, event: Event) -> Result<(State, Vec<Effect>), TransitionError> {
    let mut out = Vec::new();
    match event {
        Event::KennelConnected => {
            state.kennel_connected = true;
        }
        Event::KennelDisconnected => {
            state.kennel_connected = false;
            let ids: Vec<String> = state.claims.keys().cloned().collect();
            for target_id in ids {
                apply_claim_event(
                    &mut state.claims,
                    &target_id,
                    tux_claim::Event::KennelDisconnected,
                    &mut out,
                )?;
            }
        }
        Event::SeenAvailableList { targets } => {
            let available_ids: HashSet<String> =
                targets.iter().map(|t| t.target_id.clone()).collect();
            for target in targets {
                match state.claims.get(&target.target_id).cloned() {
                    None => {
                        state.claims.insert(
                            target.target_id.clone(),
                            tux_claim::State::Available {
                                target_id: target.target_id.clone(),
                                target_name: target.name.clone(),
                            },
                        );
                    }
                    Some(existing @ tux_claim::State::Available { .. }) => {
                        let (new_state, effects) = tux_claim::transition(
                            existing,
                            tux_claim::Event::SeenAvailable {
                                target_id: target.target_id.clone(),
                                target_name: target.name.clone(),
                            },
                        )
                        .map_err(|e| err("State", "SeenAvailableList", &e.reason))?;
                        state.claims.insert(target.target_id.clone(), new_state);
                        out.extend(effects.into_iter().map(|effect| Effect::Claim {
                            target_id: target.target_id.clone(),
                            effect,
                        }));
                    }
                    Some(_) => {}
                }
            }
            state.claims.retain(|target_id, claim_state| {
                !matches!(claim_state, tux_claim::State::Available { .. })
                    || available_ids.contains(target_id)
            });
        }
        Event::SeenMineList { targets } => {
            for target in &targets {
                if !state.claims.contains_key(&target.target_id) {
                    state.claims.insert(
                        target.target_id.clone(),
                        tux_claim::State::Available {
                            target_id: target.target_id.clone(),
                            target_name: target.name.clone(),
                        },
                    );
                }
                if let Some(lease_id) = target.lease_id.clone() {
                    apply_claim_event(
                        &mut state.claims,
                        &target.target_id,
                        tux_claim::Event::SeenMine { lease_id },
                        &mut out,
                    )?;
                }
            }
        }
        Event::UserClaimTarget { target_id } => {
            apply_claim_event(
                &mut state.claims,
                &target_id,
                tux_claim::Event::ClaimRequested,
                &mut out,
            )?;
        }
        Event::UserReleaseLease { lease_id } => {
            let Some(target_id) = state
                .claims
                .iter()
                .find(|(_, claim_state)| claim_state.lease_id() == Some(lease_id.as_str()))
                .map(|(target_id, _)| target_id.clone())
            else {
                return Ok((state, out));
            };
            apply_claim_event(
                &mut state.claims,
                &target_id,
                tux_claim::Event::ReleaseRequested,
                &mut out,
            )?;
        }
        Event::ClaimGranted { claims, now_ms } => {
            for claim in claims {
                let current = state.claims.get(&claim.target_id).cloned().unwrap_or(
                    tux_claim::State::Available {
                        target_id: claim.target_id.clone(),
                        target_name: claim.target_name.clone(),
                    },
                );
                let (new_state, effects) = tux_claim::transition(
                    current,
                    tux_claim::Event::ClaimGranted {
                        lease_id: claim.lease_id.clone(),
                        rpc_addr: claim.rpc_addr.clone(),
                        now_ms,
                    },
                )
                .map_err(|e| err("State", "ClaimGranted", &e.reason))?;
                state.claims.insert(claim.target_id.clone(), new_state);
                out.extend(effects.into_iter().map(|effect| Effect::Claim {
                    target_id: claim.target_id.clone(),
                    effect,
                }));
            }
        }
        Event::ClaimReleased { target_id, .. } => {
            if state.claims.contains_key(&target_id) {
                apply_claim_event(
                    &mut state.claims,
                    &target_id,
                    tux_claim::Event::ClaimReleased,
                    &mut out,
                )?;
            }
        }
    }
    Ok((state, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KennelTargetState, TargetListEntry};

    #[test]
    fn mine_list_without_lease_seeds_target() {
        let (state, effects) = transition(
            State::default(),
            Event::SeenMineList {
                targets: vec![TargetListEntry {
                    target_id: "t1".into(),
                    name: "target-1".into(),
                    state: KennelTargetState::RecoveringClaim,
                    lease_id: None,
                    rpc_addr: None,
                }],
            },
        )
        .unwrap();

        assert!(effects.is_empty());
        assert!(matches!(
            state.claims.get("t1"),
            Some(tux_claim::State::Available { .. })
        ));
    }

    #[test]
    fn mine_list_with_lease_transitions_to_claimed() {
        let (state, _effects) = transition(
            State {
                kennel_connected: true,
                claims: HashMap::new(),
            },
            Event::SeenMineList {
                targets: vec![TargetListEntry {
                    target_id: "t1".into(),
                    name: "target-1".into(),
                    state: KennelTargetState::Claimed,
                    lease_id: Some("lease-1".into()),
                    rpc_addr: None,
                }],
            },
        )
        .unwrap();

        assert!(matches!(
            state.claims.get("t1"),
            Some(tux_claim::State::Claimed {
                lease_id,
                ..
            }) if lease_id == "lease-1"
        ));
    }

    #[test]
    fn mine_list_omission_does_not_downgrade_live_claims() {
        let claimed = tux_claim::State::Claimed {
            target_id: "t1".into(),
            target_name: "target-1".into(),
            lease_id: "lease-1".into(),
            rpc_addr: None,
        };
        let requested = tux_claim::State::ClaimRequested {
            target_id: "t3".into(),
            target_name: "target-3".into(),
        };

        let mut claims = HashMap::new();
        claims.insert("t1".into(), claimed.clone());
        claims.insert("t3".into(), requested.clone());

        let (state, effects) = transition(
            State {
                kennel_connected: true,
                claims,
            },
            Event::SeenMineList { targets: vec![] },
        )
        .unwrap();

        assert!(effects.is_empty());
        assert_eq!(state.claims.get("t1"), Some(&claimed));
        assert_eq!(state.claims.get("t3"), Some(&requested));
    }
}
