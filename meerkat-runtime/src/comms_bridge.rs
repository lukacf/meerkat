//! CommsInputBridge — translates InboxInteraction → v9 Input::PeerInput.
//!
//! This bridge maps the existing comms system (InboxInteraction) into the
//! v9 input model without changing meerkat-core's comms module.

use chrono::Utc;
use meerkat_core::interaction::{InboxInteraction, InteractionContent, ResponseStatus};
use meerkat_core::lifecycle::InputId;

use crate::identifiers::{CorrelationId, LogicalRuntimeId};
use crate::input::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, PeerConvention, PeerInput,
    ResponseProgressPhase, ResponseTerminalStatus,
};

/// Convert an `InboxInteraction` to a v9 `Input::Peer`.
pub fn interaction_to_peer_input(
    interaction: &InboxInteraction,
    runtime_id: &LogicalRuntimeId,
) -> Input {
    let (convention, body) = map_convention(&interaction.content);
    let durability = map_durability(&convention);

    Input::Peer(PeerInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Peer {
                peer_id: interaction.from.clone(),
                runtime_id: Some(runtime_id.clone()),
            },
            durability,
            visibility: InputVisibility {
                transcript_eligible: true,
                operator_eligible: true,
            },
            idempotency_key: None,
            supersession_key: None,
            correlation_id: Some(CorrelationId::from_uuid(interaction.id.0)),
        },
        convention: Some(convention),
        body,
        blocks: None,
    })
}

fn map_convention(content: &InteractionContent) -> (PeerConvention, String) {
    match content {
        InteractionContent::Message { body, .. } => (PeerConvention::Message, body.clone()),
        InteractionContent::Request { intent, params, .. } => (
            PeerConvention::Request {
                request_id: uuid::Uuid::now_v7().to_string(),
                intent: intent.clone(),
            },
            serde_json::to_string(params).unwrap_or_default(),
        ),
        InteractionContent::Response {
            status,
            result,
            in_reply_to,
            ..
        } => {
            let request_id = in_reply_to.to_string();
            let body = serde_json::to_string(result).unwrap_or_default();
            match status {
                ResponseStatus::Completed => (
                    PeerConvention::ResponseTerminal {
                        request_id,
                        status: ResponseTerminalStatus::Completed,
                    },
                    body,
                ),
                ResponseStatus::Failed => (
                    PeerConvention::ResponseTerminal {
                        request_id,
                        status: ResponseTerminalStatus::Failed,
                    },
                    body,
                ),
                ResponseStatus::Accepted => (
                    PeerConvention::ResponseProgress {
                        request_id,
                        phase: ResponseProgressPhase::Accepted,
                    },
                    body,
                ),
            }
        }
    }
}

fn map_durability(convention: &PeerConvention) -> InputDurability {
    match convention {
        PeerConvention::ResponseProgress { .. } => InputDurability::Ephemeral,
        _ => InputDurability::Durable,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn make_interaction_id() -> meerkat_core::interaction::InteractionId {
        meerkat_core::interaction::InteractionId(uuid::Uuid::now_v7())
    }

    #[test]
    fn message_to_peer_input() {
        let interaction = InboxInteraction {
            from: "peer-1".into(),
            content: InteractionContent::Message {
                body: "hello".into(),
                blocks: None,
            },
            id: make_interaction_id(),
            rendered_text: String::new(),
        };
        let input = interaction_to_peer_input(&interaction, &LogicalRuntimeId::new("test"));
        if let Input::Peer(p) = &input {
            assert!(matches!(p.convention, Some(PeerConvention::Message)));
            assert_eq!(p.body, "hello");
            assert_eq!(p.header.durability, InputDurability::Durable);
        } else {
            panic!("Expected PeerInput");
        }
    }

    #[test]
    fn request_to_peer_input() {
        let interaction = InboxInteraction {
            from: "peer-1".into(),
            content: InteractionContent::Request {
                intent: "mob.peer_added".into(),
                params: serde_json::json!({"name": "agent-1"}),
            },
            id: make_interaction_id(),
            rendered_text: String::new(),
        };
        let input = interaction_to_peer_input(&interaction, &LogicalRuntimeId::new("test"));
        if let Input::Peer(p) = &input {
            assert!(matches!(p.convention, Some(PeerConvention::Request { .. })));
            assert_eq!(p.header.durability, InputDurability::Durable);
        } else {
            panic!("Expected PeerInput");
        }
    }

    #[test]
    fn response_completed_to_terminal() {
        let in_reply_to = make_interaction_id();
        let interaction = InboxInteraction {
            from: "peer-1".into(),
            content: InteractionContent::Response {
                status: ResponseStatus::Completed,
                result: serde_json::json!({"ok": true}),
                in_reply_to,
            },
            id: make_interaction_id(),
            rendered_text: String::new(),
        };
        let input = interaction_to_peer_input(&interaction, &LogicalRuntimeId::new("test"));
        if let Input::Peer(p) = &input {
            assert!(matches!(
                p.convention,
                Some(PeerConvention::ResponseTerminal {
                    status: ResponseTerminalStatus::Completed,
                    ..
                })
            ));
            assert_eq!(p.header.durability, InputDurability::Durable);
        } else {
            panic!("Expected PeerInput");
        }
    }

    #[test]
    fn response_failed_to_terminal() {
        let in_reply_to = make_interaction_id();
        let interaction = InboxInteraction {
            from: "peer-1".into(),
            content: InteractionContent::Response {
                status: ResponseStatus::Failed,
                result: serde_json::json!({"error": "timeout"}),
                in_reply_to,
            },
            id: make_interaction_id(),
            rendered_text: String::new(),
        };
        let input = interaction_to_peer_input(&interaction, &LogicalRuntimeId::new("test"));
        if let Input::Peer(p) = &input {
            assert!(matches!(
                p.convention,
                Some(PeerConvention::ResponseTerminal {
                    status: ResponseTerminalStatus::Failed,
                    ..
                })
            ));
        } else {
            panic!("Expected PeerInput");
        }
    }

    #[test]
    fn response_accepted_to_progress() {
        let in_reply_to = make_interaction_id();
        let interaction = InboxInteraction {
            from: "peer-1".into(),
            content: InteractionContent::Response {
                status: ResponseStatus::Accepted,
                result: serde_json::json!(null),
                in_reply_to,
            },
            id: make_interaction_id(),
            rendered_text: String::new(),
        };
        let input = interaction_to_peer_input(&interaction, &LogicalRuntimeId::new("test"));
        if let Input::Peer(p) = &input {
            assert!(matches!(
                p.convention,
                Some(PeerConvention::ResponseProgress {
                    phase: ResponseProgressPhase::Accepted,
                    ..
                })
            ));
            assert_eq!(p.header.durability, InputDurability::Ephemeral);
        } else {
            panic!("Expected PeerInput");
        }
    }

    #[test]
    fn peer_source_includes_runtime_id() {
        let interaction = InboxInteraction {
            from: "peer-1".into(),
            content: InteractionContent::Message {
                body: "hi".into(),
                blocks: None,
            },
            id: make_interaction_id(),
            rendered_text: String::new(),
        };
        let input =
            interaction_to_peer_input(&interaction, &LogicalRuntimeId::new("agent-runtime-1"));
        if let Input::Peer(p) = &input {
            if let InputOrigin::Peer {
                peer_id,
                runtime_id,
            } = &p.header.source
            {
                assert_eq!(peer_id, "peer-1");
                assert_eq!(runtime_id.as_ref().unwrap().0, "agent-runtime-1");
            } else {
                panic!("Expected Peer source");
            }
        } else {
            panic!("Expected PeerInput");
        }
    }

    #[test]
    fn all_interaction_types_produce_valid_inputs() {
        let in_reply_to = make_interaction_id();
        let interactions = vec![
            InboxInteraction {
                from: "p".into(),
                content: InteractionContent::Message {
                    body: "m".into(),
                    blocks: None,
                },
                id: make_interaction_id(),
                rendered_text: String::new(),
            },
            InboxInteraction {
                from: "p".into(),
                content: InteractionContent::Request {
                    intent: "i".into(),
                    params: serde_json::json!({}),
                },
                id: make_interaction_id(),
                rendered_text: String::new(),
            },
            InboxInteraction {
                from: "p".into(),
                content: InteractionContent::Response {
                    status: ResponseStatus::Completed,
                    result: serde_json::json!(null),
                    in_reply_to,
                },
                id: make_interaction_id(),
                rendered_text: String::new(),
            },
        ];

        let rid = LogicalRuntimeId::new("test");
        for interaction in &interactions {
            let input = interaction_to_peer_input(interaction, &rid);
            assert!(matches!(input, Input::Peer(_)));
        }
    }
}
