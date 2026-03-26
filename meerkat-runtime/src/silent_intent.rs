//! Silent intent override layer for the runtime policy table.
//!
//! When a peer sends a request whose `intent` matches one of the session's
//! `silent_comms_intents`, the runtime must accept the input without triggering
//! an LLM turn. This module provides the override function that mutates a
//! `PolicyDecision` in-place when the silent-intent condition is met.

use crate::input::{Input, PeerConvention};
use crate::policy::{ApplyMode, PolicyDecision, WakeMode};

/// Check if the given input matches a silent comms intent and override the
/// policy decision accordingly.
///
/// Returns `true` if the override was applied (the intent was silenced),
/// `false` otherwise.
pub fn apply_silent_intent_override(
    input: &Input,
    silent_intents: &[String],
    decision: &mut PolicyDecision,
) -> bool {
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

    if silent_intents.iter().any(|s| s == intent) {
        // Keep the input queued so its content is injected into the session
        // as context for the next LLM turn, but suppress waking so it does
        // not trigger a turn by itself.
        decision.apply_mode = ApplyMode::StageRunStart;
        decision.wake_mode = WakeMode::None;
        true
    } else {
        false
    }
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
    use crate::policy_table::DefaultPolicyTable;
    use chrono::Utc;
    use meerkat_core::lifecycle::InputId;

    fn make_peer_request(intent_str: &str) -> Input {
        Input::Peer(PeerInput {
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
            body: "test body".into(),
            blocks: None,
        })
    }

    fn make_peer_message() -> Input {
        Input::Peer(PeerInput {
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
            body: "test body".into(),
            blocks: None,
        })
    }

    #[test]
    fn silent_intent_overrides_policy() {
        let input = make_peer_request("mob.peer_added");
        let silent = vec!["mob.peer_added".to_string(), "mob.peer_retired".to_string()];
        let mut decision = DefaultPolicyTable::resolve(&input, true);

        let applied = apply_silent_intent_override(&input, &silent, &mut decision);
        assert!(applied);
        assert_eq!(decision.apply_mode, ApplyMode::StageRunStart);
        assert_eq!(decision.wake_mode, WakeMode::None);
    }

    #[test]
    fn non_matching_intent_not_overridden() {
        let input = make_peer_request("custom.action");
        let silent = vec!["mob.peer_added".to_string()];
        let mut decision = DefaultPolicyTable::resolve(&input, true);

        let original_apply = decision.apply_mode.clone();
        let applied = apply_silent_intent_override(&input, &silent, &mut decision);
        assert!(!applied);
        assert_eq!(decision.apply_mode, original_apply);
    }

    #[test]
    fn peer_message_not_overridden() {
        let input = make_peer_message();
        let silent = vec!["mob.peer_added".to_string()];
        let mut decision = DefaultPolicyTable::resolve(&input, true);

        let applied = apply_silent_intent_override(&input, &silent, &mut decision);
        assert!(!applied);
    }

    #[test]
    fn empty_silent_list_not_overridden() {
        let input = make_peer_request("mob.peer_added");
        let silent: Vec<String> = vec![];
        let mut decision = DefaultPolicyTable::resolve(&input, true);

        let applied = apply_silent_intent_override(&input, &silent, &mut decision);
        assert!(!applied);
    }

    #[test]
    fn prompt_input_not_overridden() {
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
            text: "hello".into(),
            blocks: None,
            turn_metadata: None,
        });
        let silent = vec!["mob.peer_added".to_string()];
        let mut decision = DefaultPolicyTable::resolve(&input, true);

        let applied = apply_silent_intent_override(&input, &silent, &mut decision);
        assert!(!applied);
    }
}
