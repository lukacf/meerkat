//! Target attachment state machine — target-side kennel-mode lifecycle.
//!
//! Pure function: `transition(state, event) -> Result<(State, Vec<Effect>), TransitionError>`.
//! No I/O. Caller interprets effects as I/O.

use std::fmt;

// ── States ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    /// Registered with kennel, idle. No active adoption.
    Idle,
    /// Kennel sent Adopted. Target is dialing TUX for direct attach.
    Attaching {
        lease_id: String,
        tux_id: String,
    },
    /// Direct link to TUX confirmed. Kennel control may or may not be alive.
    Attached {
        lease_id: String,
        tux_id: String,
        kennel_alive: bool,
    },
    /// Kennel released us or lease expired. Terminal — caller must reset to Idle.
    Released,
    /// Direct link to TUX failed during attach. Terminal.
    AttachFailed,
    /// Direct link to TUX died. Terminal. `tux_id` present if kennel was down
    /// (caller should re-register with hint).
    DirectLost {
        tux_id: Option<String>,
    },
}

impl State {
    pub fn is_terminal(&self) -> bool {
        matches!(self, State::Released | State::AttachFailed | State::DirectLost { .. })
    }
}

// ── Events ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Event {
    /// Kennel sent Adopted message.
    Adopted {
        lease_id: String,
        tux_id: String,
        tux_pubkey: String,
        tux_direct_addr: String,
    },
    /// Kennel sent LeaseRebound after recovery (skip attach handshake).
    LeaseRebound {
        lease_id: String,
        tux_id: String,
    },
    /// Direct link to TUX established (mcm.attach handshake completed).
    DirectLinkEstablished,
    /// Kennel sent Released for our lease.
    KennelReleased {
        lease_id: String,
    },
    /// Kennel TCP connection dropped (read returned None / write failed).
    KennelDisconnected,
    /// Kennel heartbeat write failed (same semantic as KennelDisconnected but from heartbeat path).
    KennelHeartbeatFailed,
    /// Direct heartbeat to TUX failed (3 consecutive).
    DirectLinkLost,
    /// Direct attach send failed or timed out.
    AttachSendFailed,
}

// ── Effects ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Add TUX as trusted peer and send mcm.attach request.
    InitiateDirectLink {
        tux_id: String,
        tux_pubkey: String,
        tux_direct_addr: String,
        lease_id: String,
    },
    /// Start the direct heartbeat task to TUX.
    StartDirectHeartbeat,
    /// Stop the direct heartbeat task.
    StopDirectHeartbeat,
    /// Re-register with kennel, passing attached_tux_id hint for recovery.
    ReregisterWithHint {
        tux_id: String,
    },
    /// Re-register with kennel, clean (no attachment hint).
    ReregisterClean,
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
        // ── Idle ─────────────────────────────────────────────────────────
        (State::Idle, Event::Adopted { lease_id, tux_id, tux_pubkey, tux_direct_addr }) => {
            Ok((
                State::Attaching { lease_id: lease_id.clone(), tux_id: tux_id.clone() },
                vec![Effect::InitiateDirectLink { tux_id, tux_pubkey, tux_direct_addr, lease_id }],
            ))
        }
        (State::Idle, Event::LeaseRebound { lease_id, tux_id }) => {
            Ok((
                State::Attached { lease_id, tux_id, kennel_alive: true },
                vec![Effect::StartDirectHeartbeat],
            ))
        }

        // ── Attaching ────────────────────────────────────────────────────
        (State::Attaching { lease_id, tux_id }, Event::DirectLinkEstablished) => {
            Ok((
                State::Attached { lease_id, tux_id, kennel_alive: true },
                vec![Effect::StartDirectHeartbeat],
            ))
        }
        (State::Attaching { .. }, Event::KennelDisconnected) => {
            Ok((State::AttachFailed, vec![Effect::ReregisterClean]))
        }
        (State::Attaching { .. }, Event::AttachSendFailed) => {
            Ok((State::AttachFailed, vec![]))
        }
        (State::Attaching { .. }, Event::KennelReleased { .. }) => {
            Ok((State::Released, vec![]))
        }

        // ── Attached ─────────────────────────────────────────────────────
        (State::Attached { lease_id, tux_id, .. }, Event::KennelReleased { lease_id: rel_lid }) => {
            if rel_lid.is_empty() || rel_lid == lease_id {
                Ok((State::Released, vec![Effect::StopDirectHeartbeat]))
            } else {
                // Release for a different lease — ignore.
                Ok((State::Attached { lease_id, tux_id, kennel_alive: true }, vec![]))
            }
        }
        (State::Attached { lease_id, tux_id, .. }, Event::KennelDisconnected) => {
            Ok((State::Attached { lease_id, tux_id, kennel_alive: false }, vec![]))
        }
        (State::Attached { lease_id, tux_id, .. }, Event::KennelHeartbeatFailed) => {
            Ok((State::Attached { lease_id, tux_id, kennel_alive: false }, vec![]))
        }
        (State::Attached { kennel_alive: true, .. }, Event::DirectLinkLost) => {
            Ok((State::DirectLost { tux_id: None }, vec![Effect::StopDirectHeartbeat]))
        }
        (State::Attached { tux_id, kennel_alive: false, .. }, Event::DirectLinkLost) => {
            Ok((
                State::DirectLost { tux_id: Some(tux_id.clone()) },
                vec![Effect::StopDirectHeartbeat, Effect::ReregisterWithHint { tux_id }],
            ))
        }
        // LeaseRebound while already attached (kennel recovered)
        (State::Attached { tux_id, .. }, Event::LeaseRebound { lease_id, tux_id: reb_tid }) => {
            if reb_tid == tux_id {
                Ok((State::Attached { lease_id, tux_id, kennel_alive: true }, vec![]))
            } else {
                Err(err("Attached", "LeaseRebound", "tux_id mismatch"))
            }
        }

        // ── Terminal states ──────────────────────────────────────────────
        (State::Released, event) => {
            Err(err("Released", &format!("{event:?}"), "terminal state"))
        }
        (State::AttachFailed, event) => {
            Err(err("AttachFailed", &format!("{event:?}"), "terminal state"))
        }
        (State::DirectLost { .. }, event) => {
            Err(err("DirectLost", &format!("{event:?}"), "terminal state"))
        }

        // ── Catch-all ────────────────────────────────────────────────────
        (state, event) => {
            let name = match &state {
                State::Idle => "Idle",
                State::Attaching { .. } => "Attaching",
                State::Attached { .. } => "Attached",
                State::Released => "Released",
                State::AttachFailed => "AttachFailed",
                State::DirectLost { .. } => "DirectLost",
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
    fn tid() -> String { "tux-1".into() }
    fn tpk() -> String { "ed25519:abc".into() }
    fn taddr() -> String { "tcp://1.2.3.4:9000".into() }

    // ── Happy path ───────────────────────────────────────────────────────

    #[test]
    fn idle_adopted() {
        let (state, effects) = transition(
            State::Idle,
            Event::Adopted { lease_id: lid(), tux_id: tid(), tux_pubkey: tpk(), tux_direct_addr: taddr() },
        ).unwrap();
        assert_eq!(state, State::Attaching { lease_id: lid(), tux_id: tid() });
        assert_eq!(effects, vec![Effect::InitiateDirectLink { tux_id: tid(), tux_pubkey: tpk(), tux_direct_addr: taddr(), lease_id: lid() }]);
    }

    #[test]
    fn attaching_link_established() {
        let (state, effects) = transition(
            State::Attaching { lease_id: lid(), tux_id: tid() },
            Event::DirectLinkEstablished,
        ).unwrap();
        assert_eq!(state, State::Attached { lease_id: lid(), tux_id: tid(), kennel_alive: true });
        assert_eq!(effects, vec![Effect::StartDirectHeartbeat]);
    }

    #[test]
    fn idle_lease_rebound() {
        let (state, effects) = transition(
            State::Idle,
            Event::LeaseRebound { lease_id: lid(), tux_id: tid() },
        ).unwrap();
        assert_eq!(state, State::Attached { lease_id: lid(), tux_id: tid(), kennel_alive: true });
        assert_eq!(effects, vec![Effect::StartDirectHeartbeat]);
    }

    #[test]
    fn attached_kennel_released() {
        let (state, effects) = transition(
            State::Attached { lease_id: lid(), tux_id: tid(), kennel_alive: true },
            Event::KennelReleased { lease_id: lid() },
        ).unwrap();
        assert_eq!(state, State::Released);
        assert_eq!(effects, vec![Effect::StopDirectHeartbeat]);
    }

    // ── Degraded modes ───────────────────────────────────────────────────

    #[test]
    fn attached_kennel_disconnected() {
        let (state, effects) = transition(
            State::Attached { lease_id: lid(), tux_id: tid(), kennel_alive: true },
            Event::KennelDisconnected,
        ).unwrap();
        assert_eq!(state, State::Attached { lease_id: lid(), tux_id: tid(), kennel_alive: false });
        assert!(effects.is_empty());
    }

    #[test]
    fn attached_kennel_heartbeat_failed() {
        let (state, effects) = transition(
            State::Attached { lease_id: lid(), tux_id: tid(), kennel_alive: true },
            Event::KennelHeartbeatFailed,
        ).unwrap();
        assert_eq!(state, State::Attached { lease_id: lid(), tux_id: tid(), kennel_alive: false });
        assert!(effects.is_empty());
    }

    #[test]
    fn attached_kennel_alive_direct_lost() {
        let (state, effects) = transition(
            State::Attached { lease_id: lid(), tux_id: tid(), kennel_alive: true },
            Event::DirectLinkLost,
        ).unwrap();
        assert_eq!(state, State::DirectLost { tux_id: None });
        assert_eq!(effects, vec![Effect::StopDirectHeartbeat]);
    }

    #[test]
    fn attached_kennel_dead_direct_lost() {
        let (state, effects) = transition(
            State::Attached { lease_id: lid(), tux_id: tid(), kennel_alive: false },
            Event::DirectLinkLost,
        ).unwrap();
        assert_eq!(state, State::DirectLost { tux_id: Some(tid()) });
        assert!(effects.contains(&Effect::StopDirectHeartbeat));
        assert!(effects.contains(&Effect::ReregisterWithHint { tux_id: tid() }));
    }

    // ── Failure ──────────────────────────────────────────────────────────

    #[test]
    fn attaching_kennel_disconnected() {
        let (state, effects) = transition(
            State::Attaching { lease_id: lid(), tux_id: tid() },
            Event::KennelDisconnected,
        ).unwrap();
        assert_eq!(state, State::AttachFailed);
        assert_eq!(effects, vec![Effect::ReregisterClean]);
    }

    #[test]
    fn attaching_send_failed() {
        let (state, effects) = transition(
            State::Attaching { lease_id: lid(), tux_id: tid() },
            Event::AttachSendFailed,
        ).unwrap();
        assert_eq!(state, State::AttachFailed);
        assert!(effects.is_empty());
    }

    #[test]
    fn attaching_kennel_released() {
        let (state, _) = transition(
            State::Attaching { lease_id: lid(), tux_id: tid() },
            Event::KennelReleased { lease_id: lid() },
        ).unwrap();
        assert_eq!(state, State::Released);
    }

    // ── Terminal states ──────────────────────────────────────────────────

    #[test]
    fn released_is_terminal() {
        let result = transition(State::Released, Event::DirectLinkEstablished);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().reason, "terminal state");
    }

    #[test]
    fn attach_failed_is_terminal() {
        let result = transition(
            State::AttachFailed,
            Event::Adopted { lease_id: lid(), tux_id: tid(), tux_pubkey: tpk(), tux_direct_addr: taddr() },
        );
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().reason, "terminal state");
    }
}
