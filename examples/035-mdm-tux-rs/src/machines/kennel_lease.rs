//! Kennel lease state machine — per-target lifecycle on the kennel side.
//!
//! Pure function: `transition(state, event) -> Result<(State, Vec<Effect>), TransitionError>`.
//! No I/O. Caller injects clock via event fields and interprets effects as I/O.

use std::fmt;

// ── States ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Available,
    AwaitingAck {
        lease_id: String,
        tux_id: String,
        expires_at_ms: i64,
        ack_deadline_ms: i64,
    },
    AwaitingAttach {
        lease_id: String,
        tux_id: String,
        expires_at_ms: i64,
        attach_deadline_ms: i64,
    },
    Claimed {
        lease_id: String,
        tux_id: String,
        expires_at_ms: i64,
    },
    RecoveringClaim {
        tux_id: String,
        recover_deadline_ms: i64,
    },
}

// ── Events ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Event {
    ClaimRequested {
        lease_id: String,
        tux_id: String,
        expires_at_ms: i64,
        ack_deadline_ms: i64,
    },
    ClaimAcked {
        lease_id: String,
        tux_id: String,
        attach_deadline_ms: i64,
    },
    AttachConfirmed {
        lease_id: String,
        tux_id: String,
    },
    LeaseRenewed {
        lease_id: String,
        tux_id: String,
        new_expires_at_ms: i64,
    },
    Released {
        reason: String,
    },
    Rebound {
        new_lease_id: String,
        tux_id: String,
        new_expires_at_ms: i64,
    },
    TargetReregisteredWithAttachment {
        tux_id: String,
        recover_deadline_ms: i64,
    },
    TargetAddressChanged,
    Tick {
        now_ms: i64,
    },
    TargetDisconnected,
    TuxDisconnected {
        tux_id: String,
    },
}

// ── Effects ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    SendAdoptedToTarget {
        lease_id: String,
        tux_id: String,
        expires_at_ms: i64,
    },
    SendReleasedToTarget {
        lease_id: String,
        reason: String,
    },
    SendClaimReleasedToTux {
        lease_id: String,
        tux_id: String,
        reason: String,
    },
    SendTargetLostToTux {
        tux_id: String,
        lease_id: Option<String>,
    },
    SendLeaseRebound {
        lease_id: String,
        tux_id: String,
        expires_at_ms: i64,
    },
    RemoveLease {
        lease_id: String,
    },
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

// ── Transition function ──────────────────────────────────────────────────────

pub fn transition(
    state: State,
    event: Event,
) -> Result<(State, Vec<Effect>), TransitionError> {
    match (state, event) {
        // ── Available ────────────────────────────────────────────────────
        (State::Available, Event::ClaimRequested { lease_id, tux_id, expires_at_ms, ack_deadline_ms }) => {
            Ok((State::AwaitingAck { lease_id, tux_id, expires_at_ms, ack_deadline_ms }, vec![]))
        }
        (State::Available, Event::TargetReregisteredWithAttachment { tux_id, recover_deadline_ms }) => {
            Ok((State::RecoveringClaim { tux_id, recover_deadline_ms }, vec![]))
        }
        (State::Available, Event::TargetDisconnected) => {
            Ok((State::Available, vec![]))
        }
        (State::Available, Event::Tick { .. }) => {
            Ok((State::Available, vec![]))
        }

        // ── AwaitingAck ──────────────────────────────────────────────────
        (State::AwaitingAck { lease_id, tux_id, expires_at_ms, .. }, Event::ClaimAcked { lease_id: ack_lid, tux_id: ack_tid, attach_deadline_ms }) => {
            if ack_lid != lease_id {
                return Err(err("AwaitingAck", "ClaimAcked", "lease_id mismatch"));
            }
            if ack_tid != tux_id {
                return Err(err("AwaitingAck", "ClaimAcked", "tux_id mismatch"));
            }
            Ok((
                State::AwaitingAttach { lease_id: lease_id.clone(), tux_id: tux_id.clone(), expires_at_ms, attach_deadline_ms },
                vec![Effect::SendAdoptedToTarget { lease_id, tux_id, expires_at_ms }],
            ))
        }
        (State::AwaitingAck { lease_id, tux_id, .. }, Event::Released { reason }) => {
            Ok((State::Available, vec![
                Effect::SendReleasedToTarget { lease_id: lease_id.clone(), reason: reason.clone() },
                Effect::SendClaimReleasedToTux { lease_id: lease_id.clone(), tux_id, reason },
                Effect::RemoveLease { lease_id },
            ]))
        }
        (State::AwaitingAck { lease_id, tux_id, expires_at_ms, ack_deadline_ms }, Event::Tick { now_ms }) => {
            if now_ms >= ack_deadline_ms {
                Ok((State::Available, vec![
                    Effect::SendClaimReleasedToTux { lease_id: lease_id.clone(), tux_id, reason: "claim_ack_timeout".into() },
                    Effect::RemoveLease { lease_id },
                ]))
            } else {
                Ok((State::AwaitingAck { lease_id, tux_id, expires_at_ms, ack_deadline_ms }, vec![]))
            }
        }
        (State::AwaitingAck { lease_id, tux_id, expires_at_ms, ack_deadline_ms }, Event::TuxDisconnected { tux_id: disc_tid }) => {
            if disc_tid != tux_id {
                return Ok((State::AwaitingAck { lease_id, tux_id, expires_at_ms, ack_deadline_ms }, vec![]));
            }
            Ok((State::Available, vec![
                Effect::SendReleasedToTarget { lease_id: lease_id.clone(), reason: "tux_disconnected".into() },
                Effect::RemoveLease { lease_id },
            ]))
        }
        (State::AwaitingAck { lease_id, tux_id, .. }, Event::TargetDisconnected) => {
            Ok((State::Available, vec![
                Effect::SendTargetLostToTux { tux_id, lease_id: Some(lease_id.clone()) },
                Effect::RemoveLease { lease_id },
            ]))
        }

        // ── AwaitingAttach ───────────────────────────────────────────────
        (State::AwaitingAttach { lease_id, tux_id, expires_at_ms, .. }, Event::AttachConfirmed { lease_id: cf_lid, tux_id: cf_tid }) => {
            if cf_lid != lease_id {
                return Err(err("AwaitingAttach", "AttachConfirmed", "lease_id mismatch"));
            }
            if cf_tid != tux_id {
                return Err(err("AwaitingAttach", "AttachConfirmed", "tux_id mismatch"));
            }
            Ok((State::Claimed { lease_id, tux_id, expires_at_ms }, vec![]))
        }
        (State::AwaitingAttach { lease_id, tux_id, .. }, Event::Released { reason }) => {
            Ok((State::Available, vec![
                Effect::SendReleasedToTarget { lease_id: lease_id.clone(), reason: reason.clone() },
                Effect::SendClaimReleasedToTux { lease_id: lease_id.clone(), tux_id, reason },
                Effect::RemoveLease { lease_id },
            ]))
        }
        (State::AwaitingAttach { lease_id, tux_id, attach_deadline_ms, expires_at_ms }, Event::Tick { now_ms }) => {
            if now_ms >= attach_deadline_ms {
                Ok((State::Available, vec![
                    Effect::SendReleasedToTarget { lease_id: lease_id.clone(), reason: "attach_timeout".into() },
                    Effect::SendClaimReleasedToTux { lease_id: lease_id.clone(), tux_id, reason: "attach_timeout".into() },
                    Effect::RemoveLease { lease_id },
                ]))
            } else {
                Ok((State::AwaitingAttach { lease_id, tux_id, expires_at_ms, attach_deadline_ms }, vec![]))
            }
        }
        (State::AwaitingAttach { lease_id, tux_id, expires_at_ms, attach_deadline_ms }, Event::TuxDisconnected { tux_id: disc_tid }) => {
            if disc_tid != tux_id {
                return Ok((State::AwaitingAttach { lease_id, tux_id, expires_at_ms, attach_deadline_ms }, vec![]));
            }
            Ok((State::Available, vec![
                Effect::SendReleasedToTarget { lease_id: lease_id.clone(), reason: "tux_disconnected".into() },
                Effect::RemoveLease { lease_id },
            ]))
        }
        (State::AwaitingAttach { lease_id, tux_id, .. }, Event::TargetDisconnected) => {
            Ok((State::Available, vec![
                Effect::SendTargetLostToTux { tux_id, lease_id: Some(lease_id.clone()) },
                Effect::RemoveLease { lease_id },
            ]))
        }

        // ── Claimed ──────────────────────────────────────────────────────
        (State::Claimed { lease_id, tux_id, .. }, Event::LeaseRenewed { lease_id: ren_lid, tux_id: ren_tid, new_expires_at_ms }) => {
            if ren_lid != lease_id || ren_tid != tux_id {
                return Err(err("Claimed", "LeaseRenewed", "lease_id or tux_id mismatch"));
            }
            Ok((State::Claimed { lease_id, tux_id, expires_at_ms: new_expires_at_ms }, vec![]))
        }
        (State::Claimed { lease_id, tux_id, .. }, Event::Released { reason }) => {
            Ok((State::Available, vec![
                Effect::SendReleasedToTarget { lease_id: lease_id.clone(), reason: reason.clone() },
                Effect::SendClaimReleasedToTux { lease_id: lease_id.clone(), tux_id, reason },
                Effect::RemoveLease { lease_id },
            ]))
        }
        (State::Claimed { lease_id, tux_id, expires_at_ms }, Event::Tick { now_ms }) => {
            if now_ms >= expires_at_ms {
                Ok((State::Available, vec![
                    Effect::SendReleasedToTarget { lease_id: lease_id.clone(), reason: "lease_expired".into() },
                    Effect::SendClaimReleasedToTux { lease_id: lease_id.clone(), tux_id, reason: "lease_expired".into() },
                    Effect::RemoveLease { lease_id },
                ]))
            } else {
                Ok((State::Claimed { lease_id, tux_id, expires_at_ms }, vec![]))
            }
        }
        (State::Claimed { lease_id, tux_id, expires_at_ms }, Event::TuxDisconnected { tux_id: disc_tid }) => {
            if disc_tid != tux_id {
                return Ok((State::Claimed { lease_id, tux_id, expires_at_ms }, vec![]));
            }
            Ok((State::Available, vec![
                Effect::SendReleasedToTarget { lease_id: lease_id.clone(), reason: "tux_disconnected".into() },
                Effect::RemoveLease { lease_id },
            ]))
        }
        (State::Claimed { lease_id, tux_id, .. }, Event::TargetDisconnected) => {
            Ok((State::Available, vec![
                Effect::SendTargetLostToTux { tux_id, lease_id: Some(lease_id.clone()) },
                Effect::RemoveLease { lease_id },
            ]))
        }
        (State::Claimed { lease_id, tux_id, .. }, Event::TargetAddressChanged) => {
            Ok((State::Available, vec![
                Effect::SendClaimReleasedToTux { lease_id: lease_id.clone(), tux_id, reason: "target_address_changed".into() },
                Effect::RemoveLease { lease_id },
            ]))
        }
        (State::AwaitingAck { lease_id, tux_id, .. }, Event::TargetAddressChanged) => {
            Ok((State::Available, vec![
                Effect::SendClaimReleasedToTux { lease_id: lease_id.clone(), tux_id, reason: "target_address_changed".into() },
                Effect::RemoveLease { lease_id },
            ]))
        }
        (State::AwaitingAttach { lease_id, tux_id, .. }, Event::TargetAddressChanged) => {
            Ok((State::Available, vec![
                Effect::SendReleasedToTarget { lease_id: lease_id.clone(), reason: "target_address_changed".into() },
                Effect::SendClaimReleasedToTux { lease_id: lease_id.clone(), tux_id, reason: "target_address_changed".into() },
                Effect::RemoveLease { lease_id },
            ]))
        }
        (State::RecoveringClaim { tux_id, .. }, Event::TargetAddressChanged) => {
            Ok((State::Available, vec![
                Effect::SendClaimReleasedToTux { lease_id: String::new(), tux_id, reason: "target_address_changed".into() },
            ]))
        }

        // ── RecoveringClaim ──────────────────────────────────────────────
        (State::RecoveringClaim { tux_id, .. }, Event::Rebound { new_lease_id, tux_id: reb_tid, new_expires_at_ms }) => {
            if reb_tid != tux_id {
                return Err(err("RecoveringClaim", "Rebound", "tux_id mismatch"));
            }
            Ok((
                State::Claimed { lease_id: new_lease_id.clone(), tux_id: tux_id.clone(), expires_at_ms: new_expires_at_ms },
                vec![Effect::SendLeaseRebound { lease_id: new_lease_id, tux_id, expires_at_ms: new_expires_at_ms }],
            ))
        }
        (State::RecoveringClaim { tux_id, recover_deadline_ms }, Event::Tick { now_ms }) => {
            if now_ms >= recover_deadline_ms {
                Ok((State::Available, vec![
                    Effect::SendReleasedToTarget { lease_id: String::new(), reason: "recovery_expired".into() },
                    Effect::SendClaimReleasedToTux { lease_id: String::new(), tux_id, reason: "recovery_expired".into() },
                ]))
            } else {
                Ok((State::RecoveringClaim { tux_id, recover_deadline_ms }, vec![]))
            }
        }
        (State::RecoveringClaim { tux_id, recover_deadline_ms }, Event::TuxDisconnected { tux_id: disc_tid }) => {
            if disc_tid != tux_id {
                return Ok((State::RecoveringClaim { tux_id, recover_deadline_ms }, vec![]));
            }
            // TUX is gone — no point waiting for rebind. Release immediately
            // so the target returns to Available instead of being hidden for
            // the full 60s recovery window.
            Ok((State::Available, vec![
                Effect::SendReleasedToTarget { lease_id: String::new(), reason: "tux_disconnected".into() },
            ]))
        }
        (State::RecoveringClaim { .. }, Event::TargetDisconnected) => {
            Ok((State::Available, vec![]))
        }

        // ── Invalid transitions ──────────────────────────────────────────
        (State::Claimed { .. }, Event::ClaimRequested { .. }) => {
            Err(err("Claimed", "ClaimRequested", "target already claimed"))
        }
        (State::AwaitingAck { .. }, Event::ClaimRequested { .. }) => {
            Err(err("AwaitingAck", "ClaimRequested", "claim already pending"))
        }
        (State::AwaitingAttach { .. }, Event::ClaimRequested { .. }) => {
            Err(err("AwaitingAttach", "ClaimRequested", "attach already pending"))
        }
        (state, event) => {
            let state_name = match &state {
                State::Available => "Available",
                State::AwaitingAck { .. } => "AwaitingAck",
                State::AwaitingAttach { .. } => "AwaitingAttach",
                State::Claimed { .. } => "Claimed",
                State::RecoveringClaim { .. } => "RecoveringClaim",
            };
            Err(err(state_name, &format!("{event:?}"), "unhandled event in this state"))
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // helpers
    fn lid() -> String { "lease-1".into() }
    fn tid() -> String { "tux-1".into() }

    // ── Happy path ───────────────────────────────────────────────────────

    #[test]
    fn available_claim_requested() {
        let (state, effects) = transition(
            State::Available,
            Event::ClaimRequested { lease_id: lid(), tux_id: tid(), expires_at_ms: 1000, ack_deadline_ms: 500 },
        ).unwrap();
        assert_eq!(state, State::AwaitingAck { lease_id: lid(), tux_id: tid(), expires_at_ms: 1000, ack_deadline_ms: 500 });
        assert!(effects.is_empty());
    }

    #[test]
    fn awaiting_ack_claim_acked() {
        let (state, effects) = transition(
            State::AwaitingAck { lease_id: lid(), tux_id: tid(), expires_at_ms: 1000, ack_deadline_ms: 500 },
            Event::ClaimAcked { lease_id: lid(), tux_id: tid(), attach_deadline_ms: 800 },
        ).unwrap();
        assert_eq!(state, State::AwaitingAttach { lease_id: lid(), tux_id: tid(), expires_at_ms: 1000, attach_deadline_ms: 800 });
        assert_eq!(effects, vec![Effect::SendAdoptedToTarget { lease_id: lid(), tux_id: tid(), expires_at_ms: 1000 }]);
    }

    #[test]
    fn awaiting_attach_confirmed() {
        let (state, effects) = transition(
            State::AwaitingAttach { lease_id: lid(), tux_id: tid(), expires_at_ms: 1000, attach_deadline_ms: 800 },
            Event::AttachConfirmed { lease_id: lid(), tux_id: tid() },
        ).unwrap();
        assert_eq!(state, State::Claimed { lease_id: lid(), tux_id: tid(), expires_at_ms: 1000 });
        assert!(effects.is_empty());
    }

    #[test]
    fn claimed_lease_renewed() {
        let (state, effects) = transition(
            State::Claimed { lease_id: lid(), tux_id: tid(), expires_at_ms: 1000 },
            Event::LeaseRenewed { lease_id: lid(), tux_id: tid(), new_expires_at_ms: 2000 },
        ).unwrap();
        assert_eq!(state, State::Claimed { lease_id: lid(), tux_id: tid(), expires_at_ms: 2000 });
        assert!(effects.is_empty());
    }

    #[test]
    fn claimed_released_by_tux() {
        let (state, effects) = transition(
            State::Claimed { lease_id: lid(), tux_id: tid(), expires_at_ms: 1000 },
            Event::Released { reason: "released_by_tux".into() },
        ).unwrap();
        assert_eq!(state, State::Available);
        assert_eq!(effects.len(), 3);
        assert!(effects.contains(&Effect::SendReleasedToTarget { lease_id: lid(), reason: "released_by_tux".into() }));
        assert!(effects.contains(&Effect::SendClaimReleasedToTux { lease_id: lid(), tux_id: tid(), reason: "released_by_tux".into() }));
        assert!(effects.contains(&Effect::RemoveLease { lease_id: lid() }));
    }

    #[test]
    fn recovering_rebound() {
        let (state, effects) = transition(
            State::RecoveringClaim { tux_id: tid(), recover_deadline_ms: 9999 },
            Event::Rebound { new_lease_id: "lease-2".into(), tux_id: tid(), new_expires_at_ms: 3000 },
        ).unwrap();
        assert_eq!(state, State::Claimed { lease_id: "lease-2".into(), tux_id: tid(), expires_at_ms: 3000 });
        assert_eq!(effects, vec![Effect::SendLeaseRebound { lease_id: "lease-2".into(), tux_id: tid(), expires_at_ms: 3000 }]);
    }

    #[test]
    fn available_reregister_with_attachment() {
        let (state, effects) = transition(
            State::Available,
            Event::TargetReregisteredWithAttachment { tux_id: tid(), recover_deadline_ms: 5000 },
        ).unwrap();
        assert_eq!(state, State::RecoveringClaim { tux_id: tid(), recover_deadline_ms: 5000 });
        assert!(effects.is_empty());
    }

    // ── Timeouts ─────────────────────────────────────────────────────────

    #[test]
    fn awaiting_ack_timeout() {
        let (state, effects) = transition(
            State::AwaitingAck { lease_id: lid(), tux_id: tid(), expires_at_ms: 1000, ack_deadline_ms: 500 },
            Event::Tick { now_ms: 500 },
        ).unwrap();
        assert_eq!(state, State::Available);
        assert!(effects.contains(&Effect::SendClaimReleasedToTux { lease_id: lid(), tux_id: tid(), reason: "claim_ack_timeout".into() }));
        assert!(effects.contains(&Effect::RemoveLease { lease_id: lid() }));
    }

    #[test]
    fn awaiting_attach_timeout() {
        let (state, effects) = transition(
            State::AwaitingAttach { lease_id: lid(), tux_id: tid(), expires_at_ms: 1000, attach_deadline_ms: 800 },
            Event::Tick { now_ms: 800 },
        ).unwrap();
        assert_eq!(state, State::Available);
        assert!(effects.contains(&Effect::SendReleasedToTarget { lease_id: lid(), reason: "attach_timeout".into() }));
        assert!(effects.contains(&Effect::SendClaimReleasedToTux { lease_id: lid(), tux_id: tid(), reason: "attach_timeout".into() }));
        assert!(effects.contains(&Effect::RemoveLease { lease_id: lid() }));
    }

    #[test]
    fn claimed_lease_expired() {
        let (state, effects) = transition(
            State::Claimed { lease_id: lid(), tux_id: tid(), expires_at_ms: 1000 },
            Event::Tick { now_ms: 1000 },
        ).unwrap();
        assert_eq!(state, State::Available);
        assert!(effects.contains(&Effect::SendReleasedToTarget { lease_id: lid(), reason: "lease_expired".into() }));
        assert!(effects.contains(&Effect::SendClaimReleasedToTux { lease_id: lid(), tux_id: tid(), reason: "lease_expired".into() }));
    }

    #[test]
    fn recovering_expired() {
        let (state, effects) = transition(
            State::RecoveringClaim { tux_id: tid(), recover_deadline_ms: 5000 },
            Event::Tick { now_ms: 5000 },
        ).unwrap();
        assert_eq!(state, State::Available);
        assert!(effects.contains(&Effect::SendReleasedToTarget { lease_id: String::new(), reason: "recovery_expired".into() }));
    }

    // ── Disconnect ───────────────────────────────────────────────────────

    #[test]
    fn awaiting_ack_tux_disconnected() {
        let (state, effects) = transition(
            State::AwaitingAck { lease_id: lid(), tux_id: tid(), expires_at_ms: 1000, ack_deadline_ms: 500 },
            Event::TuxDisconnected { tux_id: tid() },
        ).unwrap();
        assert_eq!(state, State::Available);
        assert!(effects.contains(&Effect::SendReleasedToTarget { lease_id: lid(), reason: "tux_disconnected".into() }));
        assert!(effects.contains(&Effect::RemoveLease { lease_id: lid() }));
    }

    #[test]
    fn claimed_target_disconnected() {
        let (state, effects) = transition(
            State::Claimed { lease_id: lid(), tux_id: tid(), expires_at_ms: 1000 },
            Event::TargetDisconnected,
        ).unwrap();
        assert_eq!(state, State::Available);
        assert!(effects.contains(&Effect::SendTargetLostToTux { tux_id: tid(), lease_id: Some(lid()) }));
        assert!(effects.contains(&Effect::RemoveLease { lease_id: lid() }));
    }

    #[test]
    fn claimed_tux_disconnected() {
        let (state, effects) = transition(
            State::Claimed { lease_id: lid(), tux_id: tid(), expires_at_ms: 1000 },
            Event::TuxDisconnected { tux_id: tid() },
        ).unwrap();
        assert_eq!(state, State::Available);
        assert!(effects.contains(&Effect::SendReleasedToTarget { lease_id: lid(), reason: "tux_disconnected".into() }));
        assert!(effects.contains(&Effect::RemoveLease { lease_id: lid() }));
    }

    #[test]
    fn claimed_address_changed() {
        let (state, effects) = transition(
            State::Claimed { lease_id: lid(), tux_id: tid(), expires_at_ms: 1000 },
            Event::TargetAddressChanged,
        ).unwrap();
        assert_eq!(state, State::Available);
        assert!(effects.contains(&Effect::SendClaimReleasedToTux { lease_id: lid(), tux_id: tid(), reason: "target_address_changed".into() }));
        assert!(effects.contains(&Effect::RemoveLease { lease_id: lid() }));
    }

    // ── Rejection ────────────────────────────────────────────────────────

    #[test]
    fn claimed_cannot_be_claimed_again() {
        let result = transition(
            State::Claimed { lease_id: lid(), tux_id: tid(), expires_at_ms: 1000 },
            Event::ClaimRequested { lease_id: "lease-2".into(), tux_id: "tux-2".into(), expires_at_ms: 2000, ack_deadline_ms: 500 },
        );
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().reason, "target already claimed");
    }

    #[test]
    fn ack_wrong_lease_id() {
        let result = transition(
            State::AwaitingAck { lease_id: lid(), tux_id: tid(), expires_at_ms: 1000, ack_deadline_ms: 500 },
            Event::ClaimAcked { lease_id: "wrong".into(), tux_id: tid(), attach_deadline_ms: 800 },
        );
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().reason, "lease_id mismatch");
    }

    #[test]
    fn ack_wrong_tux_id() {
        let result = transition(
            State::AwaitingAck { lease_id: lid(), tux_id: tid(), expires_at_ms: 1000, ack_deadline_ms: 500 },
            Event::ClaimAcked { lease_id: lid(), tux_id: "wrong".into(), attach_deadline_ms: 800 },
        );
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().reason, "tux_id mismatch");
    }

    #[test]
    fn tick_no_timeout_not_expired() {
        let state = State::Claimed { lease_id: lid(), tux_id: tid(), expires_at_ms: 1000 };
        let (new_state, effects) = transition(state.clone(), Event::Tick { now_ms: 500 }).unwrap();
        assert_eq!(new_state, state);
        assert!(effects.is_empty());
    }

    // ── Regression: no-op paths must preserve all fields ─────────────────

    #[test]
    fn wrong_tux_disconnect_preserves_claimed_expiry() {
        // Regression: wrong-TUX disconnect must not zero expires_at_ms
        let state = State::Claimed { lease_id: lid(), tux_id: tid(), expires_at_ms: 9999 };
        let (new_state, effects) = transition(
            state, Event::TuxDisconnected { tux_id: "other-tux".into() },
        ).unwrap();
        assert_eq!(new_state, State::Claimed { lease_id: lid(), tux_id: tid(), expires_at_ms: 9999 });
        assert!(effects.is_empty());
    }

    #[test]
    fn wrong_tux_disconnect_preserves_awaiting_ack_deadlines() {
        let state = State::AwaitingAck { lease_id: lid(), tux_id: tid(), expires_at_ms: 5000, ack_deadline_ms: 3000 };
        let (new_state, _) = transition(
            state, Event::TuxDisconnected { tux_id: "other-tux".into() },
        ).unwrap();
        assert_eq!(new_state, State::AwaitingAck { lease_id: lid(), tux_id: tid(), expires_at_ms: 5000, ack_deadline_ms: 3000 });
    }

    #[test]
    fn wrong_tux_disconnect_preserves_awaiting_attach_deadlines() {
        let state = State::AwaitingAttach { lease_id: lid(), tux_id: tid(), expires_at_ms: 5000, attach_deadline_ms: 4000 };
        let (new_state, _) = transition(
            state, Event::TuxDisconnected { tux_id: "other-tux".into() },
        ).unwrap();
        assert_eq!(new_state, State::AwaitingAttach { lease_id: lid(), tux_id: tid(), expires_at_ms: 5000, attach_deadline_ms: 4000 });
    }

    #[test]
    fn tick_preserves_awaiting_ack_expiry() {
        // Regression: non-expired tick must not zero expires_at_ms
        let state = State::AwaitingAck { lease_id: lid(), tux_id: tid(), expires_at_ms: 9999, ack_deadline_ms: 5000 };
        let (new_state, _) = transition(state, Event::Tick { now_ms: 100 }).unwrap();
        assert_eq!(new_state, State::AwaitingAck { lease_id: lid(), tux_id: tid(), expires_at_ms: 9999, ack_deadline_ms: 5000 });
    }

    #[test]
    fn recovering_expired_notifies_tux() {
        // Regression: recovery expiry must send ClaimReleasedToTux
        let (state, effects) = transition(
            State::RecoveringClaim { tux_id: tid(), recover_deadline_ms: 5000 },
            Event::Tick { now_ms: 5000 },
        ).unwrap();
        assert_eq!(state, State::Available);
        assert!(effects.iter().any(|e| matches!(e, Effect::SendClaimReleasedToTux { .. })));
    }

    #[test]
    fn address_changed_in_awaiting_ack() {
        let (state, effects) = transition(
            State::AwaitingAck { lease_id: lid(), tux_id: tid(), expires_at_ms: 1000, ack_deadline_ms: 500 },
            Event::TargetAddressChanged,
        ).unwrap();
        assert_eq!(state, State::Available);
        assert!(effects.contains(&Effect::RemoveLease { lease_id: lid() }));
    }

    #[test]
    fn address_changed_in_awaiting_attach() {
        let (state, effects) = transition(
            State::AwaitingAttach { lease_id: lid(), tux_id: tid(), expires_at_ms: 1000, attach_deadline_ms: 800 },
            Event::TargetAddressChanged,
        ).unwrap();
        assert_eq!(state, State::Available);
        assert!(effects.contains(&Effect::SendReleasedToTarget { lease_id: lid(), reason: "target_address_changed".into() }));
    }

    #[test]
    fn recovering_claim_tux_disconnected() {
        let (state, effects) = transition(
            State::RecoveringClaim { tux_id: tid(), recover_deadline_ms: 9999 },
            Event::TuxDisconnected { tux_id: tid() },
        ).unwrap();
        assert_eq!(state, State::Available);
        assert!(effects.contains(&Effect::SendReleasedToTarget { lease_id: String::new(), reason: "tux_disconnected".into() }));
    }

    #[test]
    fn recovering_claim_wrong_tux_disconnected_noop() {
        let state = State::RecoveringClaim { tux_id: tid(), recover_deadline_ms: 9999 };
        let (new_state, effects) = transition(
            state.clone(), Event::TuxDisconnected { tux_id: "other-tux".into() },
        ).unwrap();
        assert_eq!(new_state, state);
        assert!(effects.is_empty());
    }
}
