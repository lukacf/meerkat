//! Runtime comms bridge helpers.
//!
//! These helpers translate drained comms interactions into the runtime-owned
//! input families used by the host-mode cutover bridge.

use chrono::Utc;
use meerkat_core::interaction::{
    InboxInteraction, InteractionContent, PeerInputClass, ResponseStatus,
};
use meerkat_core::lifecycle::InputId;

use crate::identifiers::{CorrelationId, LogicalRuntimeId};
use crate::input::{
    ExternalEventInput, Input, InputDurability, InputHeader, InputOrigin, InputVisibility,
    PeerConvention, PeerInput, ResponseProgressPhase, ResponseTerminalStatus,
    TerminalPeerResponseInput,
};

/// Convert a classified comms interaction into the appropriate runtime input.
///
/// Routing is driven by the typed `PeerInputClass` discriminator — the
/// canonical classification produced by the comms authority at ingress time.
/// `PlainEvent` → `ExternalEventInput`; all others → `PeerInput`.
///
/// Projection rule: the `rendered_text` field on `InboxInteraction` is the
/// canonical text rendering produced by the comms runtime. This bridge
/// consumes it directly instead of re-deriving from raw JSON.
pub fn interaction_to_runtime_input(
    interaction: &InboxInteraction,
    runtime_id: &LogicalRuntimeId,
    class: PeerInputClass,
) -> Result<Input, PeerBridgeError> {
    if class == PeerInputClass::PlainEvent {
        return Ok(plain_event_to_input(interaction));
    }

    interaction_to_peer_input(interaction, runtime_id)
}

/// Convert a plain event interaction to an `ExternalEventInput`.
///
/// The event source name is extracted from the `from` field (which uses the
/// `"event:{source}"` convention set by the comms runtime). The payload text
/// comes from `rendered_text` (canonical), falling back to the message body
/// only when rendered_text is empty.
fn plain_event_to_input(interaction: &InboxInteraction) -> Input {
    // Extract source name from the "event:{source}" convention.
    // This is a display label, not a classification signal — the classification
    // was already done by the caller via PeerInputClass::PlainEvent.
    let source_name = interaction
        .from
        .strip_prefix("event:")
        .unwrap_or(&interaction.from);

    let (payload_text, blocks) = match &interaction.content {
        InteractionContent::Message { body, blocks } => {
            let text = if interaction.rendered_text.is_empty() {
                body.clone()
            } else {
                interaction.rendered_text.clone()
            };
            (text, blocks.clone())
        }
        _ => {
            let text = if interaction.rendered_text.is_empty() {
                serde_json::to_string(&interaction.content)
                    .unwrap_or_else(|e| format!("<serialization failed: {e}>"))
            } else {
                interaction.rendered_text.clone()
            };
            (text, None)
        }
    };

    Input::ExternalEvent(ExternalEventInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::External {
                source_name: source_name.to_string(),
            },
            durability: InputDurability::Durable,
            visibility: InputVisibility {
                transcript_eligible: true,
                operator_eligible: true,
            },
            idempotency_key: None,
            supersession_key: None,
            correlation_id: Some(CorrelationId::from_uuid(interaction.id.0)),
        },
        event_type: source_name.to_string(),
        payload: serde_json::json!({ "text": payload_text }),
        blocks,
    })
}

/// Convert a non-terminal `InboxInteraction` to `Input::Peer`.
///
/// Returns `Err` if the interaction is a terminal response (Completed/Failed).
/// Terminal responses must use [`interaction_to_terminal_peer_response`]
/// instead — they are a single semantic input carrying both content and
/// continuation semantics.
pub fn interaction_to_peer_input(
    interaction: &InboxInteraction,
    runtime_id: &LogicalRuntimeId,
) -> Result<Input, PeerBridgeError> {
    let (convention, body, blocks) = map_convention(interaction);

    // Hard enforcement: terminal responses are structurally forbidden here.
    if matches!(convention, PeerConvention::ResponseTerminal { .. }) {
        return Err(PeerBridgeError::TerminalResponseForbidden);
    }

    let durability = map_durability(&convention);

    Ok(Input::Peer(PeerInput {
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
        blocks,
    }))
}

/// Error from [`interaction_to_peer_input`] when a terminal response is
/// incorrectly routed through the Peer path.
#[derive(Debug)]
pub enum PeerBridgeError {
    /// Terminal responses must use [`interaction_to_terminal_peer_response`].
    TerminalResponseForbidden,
}

impl std::fmt::Display for PeerBridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TerminalResponseForbidden => write!(
                f,
                "terminal peer responses must use interaction_to_terminal_peer_response, \
                 not interaction_to_peer_input"
            ),
        }
    }
}

impl std::error::Error for PeerBridgeError {}

/// Convert a terminal response interaction to a `Input::TerminalPeerResponse`.
///
/// This constructs the single-input variant that carries both the response
/// content (rendered to LLM) and the continuation semantics (Steer handling
/// mode) in one admission. No separate Continuation input is needed.
pub fn interaction_to_terminal_peer_response(
    interaction: &InboxInteraction,
    runtime_id: &LogicalRuntimeId,
    request_id: Option<String>,
) -> Input {
    let (convention, body, blocks) = map_convention(interaction);

    Input::TerminalPeerResponse(TerminalPeerResponseInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Peer {
                peer_id: interaction.from.clone(),
                runtime_id: Some(runtime_id.clone()),
            },
            durability: InputDurability::Durable,
            visibility: InputVisibility {
                transcript_eligible: true,
                operator_eligible: true,
            },
            idempotency_key: None,
            supersession_key: None,
            correlation_id: Some(CorrelationId::from_uuid(interaction.id.0)),
        },
        body,
        blocks,
        convention,
        request_id,
    })
}

/// Map interaction content to (convention, body_text, blocks).
///
/// Projection rule: for Request and Response variants, use
/// `interaction.rendered_text` as the body text. This is the canonical
/// rendering computed by the comms runtime — consuming it directly avoids
/// re-serializing raw params/result JSON and losing formatting context.
fn map_convention(
    interaction: &InboxInteraction,
) -> (
    PeerConvention,
    String,
    Option<Vec<meerkat_core::types::ContentBlock>>,
) {
    match &interaction.content {
        InteractionContent::Message { body, blocks } => {
            (PeerConvention::Message, body.clone(), blocks.clone())
        }
        InteractionContent::Request { intent, .. } => {
            // Use canonical rendered_text; fall back to raw serialization only
            // when rendered_text is empty (defensive).
            let body = if interaction.rendered_text.is_empty() {
                serde_json::to_string(&interaction.content)
                    .unwrap_or_else(|e| format!("<serialization failed: {e}>"))
            } else {
                interaction.rendered_text.clone()
            };
            (
                PeerConvention::Request {
                    request_id: interaction.id.0.to_string(),
                    intent: intent.clone(),
                },
                body,
                None,
            )
        }
        InteractionContent::Response {
            status,
            in_reply_to,
            ..
        } => {
            let request_id = in_reply_to.to_string();
            // Use canonical rendered_text; fall back to raw serialization only
            // when rendered_text is empty (defensive).
            let body = if interaction.rendered_text.is_empty() {
                serde_json::to_string(&interaction.content)
                    .unwrap_or_else(|e| format!("<serialization failed: {e}>"))
            } else {
                interaction.rendered_text.clone()
            };
            let convention = match status {
                ResponseStatus::Completed => PeerConvention::ResponseTerminal {
                    request_id,
                    status: ResponseTerminalStatus::Completed,
                },
                ResponseStatus::Failed => PeerConvention::ResponseTerminal {
                    request_id,
                    status: ResponseTerminalStatus::Failed,
                },
                ResponseStatus::Accepted => PeerConvention::ResponseProgress {
                    request_id,
                    phase: ResponseProgressPhase::Accepted,
                },
            };
            (convention, body, None)
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
        let input =
            interaction_to_peer_input(&interaction, &LogicalRuntimeId::new("test")).unwrap();
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
        let input =
            interaction_to_peer_input(&interaction, &LogicalRuntimeId::new("test")).unwrap();
        if let Input::Peer(p) = &input {
            assert!(matches!(p.convention, Some(PeerConvention::Request { .. })));
            match p.convention.as_ref() {
                Some(PeerConvention::Request { request_id, .. }) => {
                    assert_eq!(request_id, &interaction.id.0.to_string());
                }
                other => panic!("Expected request convention, got {other:?}"),
            }
            assert_eq!(p.header.durability, InputDurability::Durable);
        } else {
            panic!("Expected PeerInput");
        }
    }

    #[test]
    fn plain_event_uses_rendered_text_when_available() {
        let interaction = InboxInteraction {
            from: "event:webhook".into(),
            content: InteractionContent::Message {
                body: "{\"ok\":true}".into(),
                blocks: None,
            },
            id: make_interaction_id(),
            rendered_text: "[webhook event] {\"ok\":true}".into(),
        };
        let input = interaction_to_runtime_input(
            &interaction,
            &LogicalRuntimeId::new("test"),
            PeerInputClass::PlainEvent,
        )
        .unwrap();
        match input {
            Input::ExternalEvent(event) => {
                assert_eq!(event.event_type, "webhook");
                // Uses rendered_text, not raw body
                assert_eq!(event.payload["text"], "[webhook event] {\"ok\":true}");
            }
            other => panic!("Expected ExternalEvent input, got {other:?}"),
        }
    }

    #[test]
    fn plain_event_falls_back_to_body_when_rendered_text_empty() {
        let interaction = InboxInteraction {
            from: "event:webhook".into(),
            content: InteractionContent::Message {
                body: "{\"ok\":true}".into(),
                blocks: None,
            },
            id: make_interaction_id(),
            rendered_text: String::new(),
        };
        let input = interaction_to_runtime_input(
            &interaction,
            &LogicalRuntimeId::new("test"),
            PeerInputClass::PlainEvent,
        )
        .unwrap();
        match input {
            Input::ExternalEvent(event) => {
                assert_eq!(event.event_type, "webhook");
                // Falls back to body when rendered_text is empty
                assert_eq!(event.payload["text"], "{\"ok\":true}");
            }
            other => panic!("Expected ExternalEvent input, got {other:?}"),
        }
    }

    #[test]
    fn plain_event_with_blocks_preserves_blocks() {
        let blocks = vec![
            meerkat_core::types::ContentBlock::Text {
                text: "event data".into(),
            },
            meerkat_core::types::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc123".into(),
                source_path: None,
            },
        ];
        let interaction = InboxInteraction {
            from: "event:webhook".into(),
            content: InteractionContent::Message {
                body: "event data".into(),
                blocks: Some(blocks.clone()),
            },
            id: make_interaction_id(),
            rendered_text: "[EVENT via webhook] event data".into(),
        };
        let input = interaction_to_runtime_input(
            &interaction,
            &LogicalRuntimeId::new("test"),
            PeerInputClass::PlainEvent,
        )
        .unwrap();
        match input {
            Input::ExternalEvent(event) => {
                assert_eq!(event.event_type, "webhook");
                assert!(event.blocks.is_some(), "blocks should be preserved");
                assert_eq!(event.blocks.as_ref().unwrap().len(), 2);
            }
            other => panic!("Expected ExternalEvent input, got {other:?}"),
        }
    }

    #[test]
    fn response_completed_to_terminal_peer_response() {
        let in_reply_to = make_interaction_id();
        let interaction = InboxInteraction {
            from: "peer-1".into(),
            content: InteractionContent::Response {
                status: ResponseStatus::Completed,
                result: serde_json::json!({"ok": true}),
                in_reply_to,
            },
            id: make_interaction_id(),
            rendered_text: "[COMMS RESPONSE] completed".into(),
        };
        let input = interaction_to_terminal_peer_response(
            &interaction,
            &LogicalRuntimeId::new("test"),
            Some(in_reply_to.0.to_string()),
        );
        if let Input::TerminalPeerResponse(tpr) = &input {
            assert!(matches!(
                tpr.convention,
                PeerConvention::ResponseTerminal {
                    status: ResponseTerminalStatus::Completed,
                    ..
                }
            ));
            assert_eq!(tpr.header.durability, InputDurability::Durable);
        } else {
            panic!("Expected TerminalPeerResponse");
        }
    }

    #[test]
    fn response_failed_to_terminal_peer_response() {
        let in_reply_to = make_interaction_id();
        let interaction = InboxInteraction {
            from: "peer-1".into(),
            content: InteractionContent::Response {
                status: ResponseStatus::Failed,
                result: serde_json::json!({"error": "timeout"}),
                in_reply_to,
            },
            id: make_interaction_id(),
            rendered_text: "[COMMS RESPONSE] failed: timeout".into(),
        };
        let input = interaction_to_terminal_peer_response(
            &interaction,
            &LogicalRuntimeId::new("test"),
            Some(in_reply_to.0.to_string()),
        );
        if let Input::TerminalPeerResponse(tpr) = &input {
            assert!(matches!(
                tpr.convention,
                PeerConvention::ResponseTerminal {
                    status: ResponseTerminalStatus::Failed,
                    ..
                }
            ));
        } else {
            panic!("Expected TerminalPeerResponse");
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
        let input =
            interaction_to_peer_input(&interaction, &LogicalRuntimeId::new("test")).unwrap();
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
            interaction_to_peer_input(&interaction, &LogicalRuntimeId::new("agent-runtime-1"))
                .unwrap();
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
        let rid = LogicalRuntimeId::new("test");

        // Non-terminal interactions → Input::Peer via interaction_to_peer_input
        let peer_interactions = vec![
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
        ];
        for interaction in &peer_interactions {
            let input = interaction_to_peer_input(interaction, &rid).unwrap();
            assert!(matches!(input, Input::Peer(_)));
        }

        // Terminal response → Input::TerminalPeerResponse via dedicated function
        let terminal_interaction = InboxInteraction {
            from: "p".into(),
            content: InteractionContent::Response {
                status: ResponseStatus::Completed,
                result: serde_json::json!(null),
                in_reply_to,
            },
            id: make_interaction_id(),
            rendered_text: "[COMMS RESPONSE] completed".into(),
        };
        let input = interaction_to_terminal_peer_response(
            &terminal_interaction,
            &rid,
            Some(in_reply_to.0.to_string()),
        );
        assert!(matches!(input, Input::TerminalPeerResponse(_)));
    }

    #[test]
    fn message_with_blocks_preserves_blocks() {
        let blocks = vec![meerkat_core::types::ContentBlock::Text {
            text: "hello".into(),
        }];
        let interaction = InboxInteraction {
            from: "peer-1".into(),
            content: InteractionContent::Message {
                body: "hello".into(),
                blocks: Some(blocks.clone()),
            },
            id: make_interaction_id(),
            rendered_text: String::new(),
        };
        let input =
            interaction_to_peer_input(&interaction, &LogicalRuntimeId::new("test")).unwrap();
        if let Input::Peer(p) = &input {
            assert!(p.blocks.is_some(), "blocks should be preserved");
            assert_eq!(p.blocks.as_ref().unwrap().len(), 1);
        } else {
            panic!("Expected PeerInput");
        }
    }

    #[test]
    fn message_without_blocks_has_none() {
        let interaction = InboxInteraction {
            from: "peer-1".into(),
            content: InteractionContent::Message {
                body: "hello".into(),
                blocks: None,
            },
            id: make_interaction_id(),
            rendered_text: String::new(),
        };
        let input =
            interaction_to_peer_input(&interaction, &LogicalRuntimeId::new("test")).unwrap();
        if let Input::Peer(p) = &input {
            assert!(
                p.blocks.is_none(),
                "blocks should be None when not provided"
            );
        } else {
            panic!("Expected PeerInput");
        }
    }

    #[test]
    fn message_with_image_blocks_preserves_all_blocks() {
        let blocks = vec![
            meerkat_core::types::ContentBlock::Text {
                text: "see this".into(),
            },
            meerkat_core::types::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc123".into(),
                source_path: None,
            },
        ];
        let interaction = InboxInteraction {
            from: "peer-1".into(),
            content: InteractionContent::Message {
                body: "see this".into(),
                blocks: Some(blocks),
            },
            id: make_interaction_id(),
            rendered_text: String::new(),
        };
        let input =
            interaction_to_peer_input(&interaction, &LogicalRuntimeId::new("test")).unwrap();
        if let Input::Peer(p) = &input {
            assert!(p.blocks.is_some(), "blocks should be preserved");
            assert_eq!(p.blocks.as_ref().unwrap().len(), 2);
        } else {
            panic!("Expected PeerInput");
        }
    }

    #[test]
    fn request_uses_rendered_text_for_body() {
        let interaction = InboxInteraction {
            from: "peer-1".into(),
            content: InteractionContent::Request {
                intent: "mob.peer_added".into(),
                params: serde_json::json!({"name": "agent-1"}),
            },
            id: make_interaction_id(),
            rendered_text: "[COMMS REQUEST from peer-1]\nIntent: mob.peer_added\nParams: {\"name\":\"agent-1\"}".into(),
        };
        let input =
            interaction_to_peer_input(&interaction, &LogicalRuntimeId::new("test")).unwrap();
        if let Input::Peer(p) = &input {
            // Body should use rendered_text, not raw JSON serialization
            assert!(
                p.body.contains("COMMS REQUEST"),
                "body should use rendered_text: got {:?}",
                p.body
            );
        } else {
            panic!("Expected PeerInput");
        }
    }

    #[test]
    fn terminal_response_uses_rendered_text_for_body() {
        let in_reply_to = make_interaction_id();
        let interaction = InboxInteraction {
            from: "peer-1".into(),
            content: InteractionContent::Response {
                status: ResponseStatus::Completed,
                result: serde_json::json!({"ok": true}),
                in_reply_to,
            },
            id: make_interaction_id(),
            rendered_text: "[COMMS RESPONSE from peer-1]\nStatus: completed\nResult: {\"ok\":true}"
                .into(),
        };
        let input = interaction_to_terminal_peer_response(
            &interaction,
            &LogicalRuntimeId::new("test"),
            Some(in_reply_to.0.to_string()),
        );
        if let Input::TerminalPeerResponse(tpr) = &input {
            // Body should use rendered_text, not raw JSON serialization
            assert!(
                tpr.body.contains("COMMS RESPONSE"),
                "body should use rendered_text: got {:?}",
                tpr.body
            );
        } else {
            panic!("Expected TerminalPeerResponse");
        }
    }

    #[test]
    fn request_interaction_has_no_blocks() {
        let interaction = InboxInteraction {
            from: "peer-1".into(),
            content: InteractionContent::Request {
                intent: "mob.peer_added".into(),
                params: serde_json::json!({"name": "agent-1"}),
            },
            id: make_interaction_id(),
            rendered_text: String::new(),
        };
        let input =
            interaction_to_peer_input(&interaction, &LogicalRuntimeId::new("test")).unwrap();
        if let Input::Peer(p) = &input {
            assert!(
                p.blocks.is_none(),
                "request interactions should not have blocks"
            );
        } else {
            panic!("Expected PeerInput");
        }
    }
}
