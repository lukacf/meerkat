//! TUX claim state machine — per-target claim lifecycle on the TUX side.
//!
//! Pure function: `transition(state, event) -> Result<(State, Vec<Effect>), TransitionError>`.
//! No I/O. Caller interprets effects as I/O.

use std::fmt;

// ── States ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    /// Target visible in available list, not claimed.
    Available,
    /// /claim sent to kennel, waiting for ClaimGranted.
    ClaimRequested,
    /// ClaimGranted received, ClaimAck sent. Waiting for target direct attach.
    Attaching {
        lease_id: String,
        target_pubkey: String,
        target_direct_addr: String,
        started_at_ms: i64,
    },
    /// Target directly attached and confirmed. Active session.
    Claimed {
        lease_id: String,
    },
    /// Attach timed out locally OR kennel disconnected. Kennel may still hold the claim.
    ClaimUncertain {
        lease_id: String,
    },
}

// ── Events ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Event {
    /// User requested /claim.
    ClaimRequested,
    /// Kennel responded with ClaimGranted.
    ClaimGranted {
        lease_id: String,
        target_pubkey: String,
        target_direct_addr: String,
        now_ms: i64,
    },
    /// Target attached directly and we confirmed to kennel.
    AttachConfirmed {
        lease_id: String,
    },
    /// Kennel sent LeaseRebound after recovery.
    LeaseRebound {
        new_lease_id: String,
    },
    /// Kennel sent ClaimReleased or TargetLost.
    ClaimReleased {
        reason: String,
    },
    /// User requested /release.
    ReleaseRequested,
    /// Kennel mine-list no longer includes this target.
    MineListDropped,
    /// Local attach timeout fired.
    AttachTimeout {
        now_ms: i64,
        timeout_ms: i64,
    },
    /// Kennel control connection lost.
    KennelDisconnected,
}

// ── Effects ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Send ClaimTargets to kennel.
    SendClaimToKennel,
    /// Add target as trusted peer for direct comms.
    AddTrustedPeer {
        target_pubkey: String,
        target_direct_addr: String,
    },
    /// Send ClaimAck to kennel.
    SendClaimAck {
        lease_id: String,
    },
    /// Send AttachConfirmed to kennel.
    SendAttachConfirmed {
        lease_id: String,
    },
    /// Send ReleaseTargets to kennel.
    SendReleaseToKennel {
        lease_id: String,
    },
    /// Clear UI busy/streaming state for this target.
    ClearUiState,
}

// ── Error ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct TransitionError {
    pub state: &'static str,
    pub event: String,
    pub reason: String,
}

impl fmt::Display for TransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid transition: {} in state {} ({})", self.event, self.state, self.reason)
    }
}

fn err(state: &'static str, event: &str, reason: &str) -> TransitionError {
    TransitionError { state, event: event.to_string(), reason: reason.to_string() }
}

// ── Transition function ──────────────────────────────────────────────────────

pub fn transition(state: State, event: Event) -> Result<(State, Vec<Effect>), TransitionError> {
    match (state, event) {
        // ── Available ────────────────────────────────────────────────────
        (State::Available, Event::ClaimRequested) => {
            Ok((State::ClaimRequested, vec![Effect::SendClaimToKennel]))
        }
        (State::Available, Event::KennelDisconnected) => {
            Ok((State::Available, vec![]))
        }
        (State::Available, Event::MineListDropped) => {
            Ok((State::Available, vec![]))
        }

        // ── ClaimRequested ───────────────────────────────────────────────
        (State::ClaimRequested, Event::ClaimGranted { lease_id, target_pubkey, target_direct_addr, now_ms }) => {
            Ok((
                State::Attaching { lease_id: lease_id.clone(), target_pubkey: target_pubkey.clone(), target_direct_addr: target_direct_addr.clone(), started_at_ms: now_ms },
                vec![
                    Effect::AddTrustedPeer { target_pubkey, target_direct_addr },
                    Effect::SendClaimAck { lease_id },
                ],
            ))
        }
        (State::ClaimRequested, Event::ClaimReleased { .. }) => {
            Ok((State::Available, vec![Effect::ClearUiState]))
        }
        (State::ClaimRequested, Event::KennelDisconnected) => {
            Ok((State::Available, vec![Effect::ClearUiState]))
        }

        // ── Attaching ────────────────────────────────────────────────────
        (State::Attaching { lease_id, .. }, Event::AttachConfirmed { lease_id: cf_lid }) => {
            if cf_lid != lease_id {
                return Err(err("Attaching", "AttachConfirmed", "lease_id mismatch"));
            }
            Ok((
                State::Claimed { lease_id: cf_lid.clone() },
                vec![Effect::SendAttachConfirmed { lease_id: cf_lid }],
            ))
        }
        (State::Attaching { lease_id, target_pubkey, target_direct_addr, started_at_ms }, Event::AttachTimeout { now_ms, timeout_ms }) => {
            if now_ms - started_at_ms >= timeout_ms {
                Ok((State::ClaimUncertain { lease_id }, vec![]))
            } else {
                Ok((State::Attaching { lease_id, target_pubkey, target_direct_addr, started_at_ms }, vec![]))
            }
        }
        (State::Attaching { .. }, Event::ClaimReleased { .. }) => {
            Ok((State::Available, vec![Effect::ClearUiState]))
        }
        (State::Attaching { lease_id, .. }, Event::KennelDisconnected) => {
            Ok((State::ClaimUncertain { lease_id }, vec![]))
        }
        (State::Attaching { lease_id, target_pubkey, target_direct_addr, started_at_ms }, Event::MineListDropped) => {
            // Mine list may not include pending attaches — don't drop.
            // No-op: preserve all state fields unchanged.
            Ok((State::Attaching { lease_id, target_pubkey, target_direct_addr, started_at_ms }, vec![]))
        }

        // ── Claimed ──────────────────────────────────────────────────────
        (State::Claimed { lease_id }, Event::ReleaseRequested) => {
            Ok((State::Available, vec![
                Effect::SendReleaseToKennel { lease_id },
                Effect::ClearUiState,
            ]))
        }
        (State::Claimed { .. }, Event::ClaimReleased { .. }) => {
            Ok((State::Available, vec![Effect::ClearUiState]))
        }
        (State::Claimed { .. }, Event::LeaseRebound { new_lease_id }) => {
            Ok((State::Claimed { lease_id: new_lease_id }, vec![]))
        }
        (State::Claimed { .. }, Event::MineListDropped) => {
            Ok((State::Available, vec![Effect::ClearUiState]))
        }
        (State::Claimed { lease_id }, Event::KennelDisconnected) => {
            Ok((State::ClaimUncertain { lease_id }, vec![]))
        }

        // ── ClaimUncertain ───────────────────────────────────────────────
        (State::ClaimUncertain { lease_id }, Event::AttachConfirmed { lease_id: cf_lid }) => {
            if cf_lid != lease_id {
                return Err(err("ClaimUncertain", "AttachConfirmed", "lease_id mismatch"));
            }
            Ok((
                State::Claimed { lease_id: cf_lid.clone() },
                vec![Effect::SendAttachConfirmed { lease_id: cf_lid }],
            ))
        }
        (State::ClaimUncertain { .. }, Event::ClaimReleased { .. }) => {
            Ok((State::Available, vec![Effect::ClearUiState]))
        }
        (State::ClaimUncertain { .. }, Event::MineListDropped) => {
            Ok((State::Available, vec![Effect::ClearUiState]))
        }
        (State::ClaimUncertain { lease_id }, Event::KennelDisconnected) => {
            // Already uncertain — stay uncertain.
            Ok((State::ClaimUncertain { lease_id }, vec![]))
        }
        (State::ClaimUncertain { .. }, Event::LeaseRebound { new_lease_id }) => {
            Ok((State::Claimed { lease_id: new_lease_id }, vec![]))
        }

        // ── Rejection ────────────────────────────────────────────────────
        (State::Available, Event::ReleaseRequested) => {
            Err(err("Available", "ReleaseRequested", "nothing to release"))
        }
        (State::Claimed { .. }, Event::ClaimRequested) => {
            Err(err("Claimed", "ClaimRequested", "already claimed"))
        }

        // ── Catch-all ────────────────────────────────────────────────────
        (state, event) => {
            let name = match &state {
                State::Available => "Available",
                State::ClaimRequested => "ClaimRequested",
                State::Attaching { .. } => "Attaching",
                State::Claimed { .. } => "Claimed",
                State::ClaimUncertain { .. } => "ClaimUncertain",
            };
            Err(err(name, &format!("{event:?}"), "unhandled event in this state"))
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn lid() -> String { "lease-1".into() }
    fn tpk() -> String { "ed25519:target-abc".into() }
    fn taddr() -> String { "tcp://10.0.0.5:9000".into() }

    // ── Happy path ───────────────────────────────────────────────────────

    #[test]
    fn available_claim_requested() {
        let (state, effects) = transition(State::Available, Event::ClaimRequested).unwrap();
        assert_eq!(state, State::ClaimRequested);
        assert_eq!(effects, vec![Effect::SendClaimToKennel]);
    }

    #[test]
    fn claim_requested_granted() {
        let (state, effects) = transition(
            State::ClaimRequested,
            Event::ClaimGranted { lease_id: lid(), target_pubkey: tpk(), target_direct_addr: taddr(), now_ms: 100 },
        ).unwrap();
        assert_eq!(state, State::Attaching { lease_id: lid(), target_pubkey: tpk(), target_direct_addr: taddr(), started_at_ms: 100 });
        assert!(effects.contains(&Effect::AddTrustedPeer { target_pubkey: tpk(), target_direct_addr: taddr() }));
        assert!(effects.contains(&Effect::SendClaimAck { lease_id: lid() }));
    }

    #[test]
    fn attaching_attach_confirmed() {
        let (state, effects) = transition(
            State::Attaching { lease_id: lid(), target_pubkey: tpk(), target_direct_addr: taddr(), started_at_ms: 100 },
            Event::AttachConfirmed { lease_id: lid() },
        ).unwrap();
        assert_eq!(state, State::Claimed { lease_id: lid() });
        assert_eq!(effects, vec![Effect::SendAttachConfirmed { lease_id: lid() }]);
    }

    #[test]
    fn claimed_release_requested() {
        let (state, effects) = transition(
            State::Claimed { lease_id: lid() },
            Event::ReleaseRequested,
        ).unwrap();
        assert_eq!(state, State::Available);
        assert!(effects.contains(&Effect::SendReleaseToKennel { lease_id: lid() }));
        assert!(effects.contains(&Effect::ClearUiState));
    }

    #[test]
    fn claimed_lease_rebound() {
        let (state, effects) = transition(
            State::Claimed { lease_id: lid() },
            Event::LeaseRebound { new_lease_id: "lease-2".into() },
        ).unwrap();
        assert_eq!(state, State::Claimed { lease_id: "lease-2".into() });
        assert!(effects.is_empty());
    }

    // ── Timeout / uncertain ──────────────────────────────────────────────

    #[test]
    fn attaching_timeout() {
        let (state, effects) = transition(
            State::Attaching { lease_id: lid(), target_pubkey: tpk(), target_direct_addr: taddr(), started_at_ms: 100 },
            Event::AttachTimeout { now_ms: 20200, timeout_ms: 20000 },
        ).unwrap();
        assert_eq!(state, State::ClaimUncertain { lease_id: lid() });
        assert!(effects.is_empty());
    }

    #[test]
    fn uncertain_attach_confirmed() {
        let (state, effects) = transition(
            State::ClaimUncertain { lease_id: lid() },
            Event::AttachConfirmed { lease_id: lid() },
        ).unwrap();
        assert_eq!(state, State::Claimed { lease_id: lid() });
        assert_eq!(effects, vec![Effect::SendAttachConfirmed { lease_id: lid() }]);
    }

    #[test]
    fn uncertain_claim_released() {
        let (state, effects) = transition(
            State::ClaimUncertain { lease_id: lid() },
            Event::ClaimReleased { reason: "lease_expired".into() },
        ).unwrap();
        assert_eq!(state, State::Available);
        assert_eq!(effects, vec![Effect::ClearUiState]);
    }

    // ── Kennel disconnect ────────────────────────────────────────────────

    #[test]
    fn attaching_kennel_disconnected() {
        let (state, _) = transition(
            State::Attaching { lease_id: lid(), target_pubkey: tpk(), target_direct_addr: taddr(), started_at_ms: 100 },
            Event::KennelDisconnected,
        ).unwrap();
        assert_eq!(state, State::ClaimUncertain { lease_id: lid() });
    }

    #[test]
    fn claimed_kennel_disconnected() {
        let (state, _) = transition(
            State::Claimed { lease_id: lid() },
            Event::KennelDisconnected,
        ).unwrap();
        assert_eq!(state, State::ClaimUncertain { lease_id: lid() });
    }

    #[test]
    fn available_kennel_disconnected_noop() {
        let (state, effects) = transition(State::Available, Event::KennelDisconnected).unwrap();
        assert_eq!(state, State::Available);
        assert!(effects.is_empty());
    }

    // ── Mine-list reconciliation ─────────────────────────────────────────

    #[test]
    fn claimed_mine_list_dropped() {
        let (state, effects) = transition(
            State::Claimed { lease_id: lid() },
            Event::MineListDropped,
        ).unwrap();
        assert_eq!(state, State::Available);
        assert_eq!(effects, vec![Effect::ClearUiState]);
    }

    #[test]
    fn uncertain_mine_list_dropped() {
        let (state, effects) = transition(
            State::ClaimUncertain { lease_id: lid() },
            Event::MineListDropped,
        ).unwrap();
        assert_eq!(state, State::Available);
        assert_eq!(effects, vec![Effect::ClearUiState]);
    }

    #[test]
    fn claim_requested_released() {
        let (state, effects) = transition(
            State::ClaimRequested,
            Event::ClaimReleased { reason: "rejected".into() },
        ).unwrap();
        assert_eq!(state, State::Available);
        assert_eq!(effects, vec![Effect::ClearUiState]);
    }

    // ── Rejection ────────────────────────────────────────────────────────

    #[test]
    fn available_cannot_release() {
        let result = transition(State::Available, Event::ReleaseRequested);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().reason, "nothing to release");
    }

    #[test]
    fn claimed_cannot_claim_again() {
        let result = transition(State::Claimed { lease_id: lid() }, Event::ClaimRequested);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().reason, "already claimed");
    }

    // ── Regression: no-op paths must preserve all fields ─────────────────

    #[test]
    fn attach_timeout_not_expired_preserves_fields() {
        // Regression: non-expired timeout must not zero target_pubkey/addr
        let state = State::Attaching {
            lease_id: lid(), target_pubkey: tpk(), target_direct_addr: taddr(), started_at_ms: 100,
        };
        let (new_state, _) = transition(state.clone(), Event::AttachTimeout { now_ms: 100, timeout_ms: 20000 }).unwrap();
        assert_eq!(new_state, state);
    }

    #[test]
    fn mine_list_dropped_preserves_attaching_fields() {
        // Regression: MineListDropped in Attaching must not zero lease_id
        let state = State::Attaching {
            lease_id: lid(), target_pubkey: tpk(), target_direct_addr: taddr(), started_at_ms: 100,
        };
        let (new_state, _) = transition(state.clone(), Event::MineListDropped).unwrap();
        assert_eq!(new_state, state);
    }
}
