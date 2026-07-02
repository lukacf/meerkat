//! Silent intent matching for generated runtime admission.
//!
//! When a peer sends a request whose `intent` matches one of the session's
//! generated `silent_intent_overrides`, the runtime must accept the input
//! without triggering an LLM turn. This module only reports whether the typed
//! machine-owned condition is present; generated admission owns the resulting
//! policy facts.

use crate::input::{Input, PeerConvention};

/// Return whether this input matches a configured silent comms intent.
pub(crate) fn matches_silent_intent(input: &Input, silent_intents: &[String]) -> bool {
    if silent_intents.is_empty() {
        return false;
    }

    let intent = match input {
        Input::Peer(peer) => match &peer.convention {
            Some(PeerConvention::Request { intent, .. }) => intent.as_str(),
            _ => return false,
        },
        _ => return false,
    };

    silent_intents.iter().any(|s| s == intent)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::redundant_clone,
    clippy::clone_on_copy
)]
mod tests {
    use super::*;
    use crate::input::*;
    use chrono::Utc;
    use meerkat_core::lifecycle::InputId;

    fn make_peer_request(intent_str: &str) -> Input {
        Input::Peer(PeerInput {
            sender_taint: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Operator,
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(PeerConvention::Request {
                request_id: "req-1".into(),
                intent: intent_str.into(),
            }),
            content: "test body".into(),
            payload: Some(serde_json::json!({"intent": intent_str})),
            handling_mode: None,
        })
    }

    fn make_peer_message() -> Input {
        Input::Peer(PeerInput {
            sender_taint: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Operator,
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(PeerConvention::Message),
            content: "test body".into(),
            payload: None,
            handling_mode: None,
        })
    }

    #[test]
    fn silent_intent_matches_request_intent() {
        let input = make_peer_request("mob.peer_added");
        let silent = vec!["mob.peer_added".to_string(), "mob.peer_retired".to_string()];

        assert!(matches_silent_intent(&input, &silent));
    }

    #[test]
    fn non_matching_intent_not_silent() {
        let input = make_peer_request("custom.action");
        let silent = vec!["mob.peer_added".to_string()];

        assert!(!matches_silent_intent(&input, &silent));
    }

    #[test]
    fn peer_message_not_silent() {
        let input = make_peer_message();
        let silent = vec!["mob.peer_added".to_string()];

        assert!(!matches_silent_intent(&input, &silent));
    }

    #[test]
    fn empty_silent_list_not_silent() {
        let input = make_peer_request("mob.peer_added");
        let silent: Vec<String> = vec![];

        assert!(!matches_silent_intent(&input, &silent));
    }

    #[test]
    fn prompt_input_not_silent() {
        let input = Input::Prompt(PromptInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Operator,
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            content: "hello".into(),
            typed_turn_appends: Vec::new(),
            turn_metadata: None,
        });
        let silent = vec!["mob.peer_added".to_string()];

        assert!(!matches_silent_intent(&input, &silent));
    }
}
