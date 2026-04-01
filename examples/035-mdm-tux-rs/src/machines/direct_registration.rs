//! Direct TUX registration decision.
//!
//! Owns the semantic registration outcome for the lightweight direct-mode
//! registration protocol so the TCP handler stays transport-only.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingPeer {
    pub name: String,
    pub pubkey: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    NameConflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Register {
        requested_name: String,
        requested_pubkey: String,
        existing_peer: Option<ExistingPeer>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Accept,
    Reject {
        reason: RejectReason,
        message: String,
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

impl std::error::Error for TransitionError {}

pub fn transition(event: Event) -> Result<Effect, TransitionError> {
    match event {
        Event::Register {
            requested_name,
            requested_pubkey,
            existing_peer,
        } => {
            if let Some(existing_peer) = existing_peer
                && existing_peer.pubkey != requested_pubkey
            {
                return Ok(Effect::Reject {
                    reason: RejectReason::NameConflict,
                    message: format!(
                        "name '{}' already registered by a different machine — use --name to pick a unique name",
                        requested_name
                    ),
                });
            }
            Ok(Effect::Accept)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_name_with_different_pubkey_is_rejected() {
        let effect = transition(Event::Register {
            requested_name: "mac".into(),
            requested_pubkey: "ed25519:new".into(),
            existing_peer: Some(ExistingPeer {
                name: "mac".into(),
                pubkey: "ed25519:old".into(),
            }),
        })
        .unwrap();

        assert_eq!(
            effect,
            Effect::Reject {
                reason: RejectReason::NameConflict,
                message:
                    "name 'mac' already registered by a different machine — use --name to pick a unique name"
                        .into(),
            }
        );
    }

    #[test]
    fn same_name_same_pubkey_is_idempotent_accept() {
        let effect = transition(Event::Register {
            requested_name: "mac".into(),
            requested_pubkey: "ed25519:same".into(),
            existing_peer: Some(ExistingPeer {
                name: "mac".into(),
                pubkey: "ed25519:same".into(),
            }),
        })
        .unwrap();

        assert_eq!(effect, Effect::Accept);
    }
}
