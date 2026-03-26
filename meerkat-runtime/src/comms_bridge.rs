//! Runtime comms bridge helpers.
//!
//! These helpers translate drained comms interactions into the runtime-owned
//! input families used by the comms classification bridge.

use chrono::Utc;
use meerkat_core::interaction::{
    ClassifiedInboxInteraction, InboxInteraction, InteractionContent, PeerInputClass,
    ResponseStatus,
};
use meerkat_core::lifecycle::InputId;

use crate::identifiers::{CorrelationId, LogicalRuntimeId};
use crate::input::{
    ExternalEventInput, Input, InputDurability, InputHeader, InputOrigin, InputVisibility,
    PeerConvention, PeerInput, ResponseProgressPhase, ResponseTerminalStatus,
};

/// Convert a classified comms interaction into the appropriate runtime-owned
/// input family.
pub fn classified_interaction_to_runtime_input(
    classified: &ClassifiedInboxInteraction,
    runtime_id: &LogicalRuntimeId,
) -> Input {
    let interaction = &classified.interaction;

    if classified.class == PeerInputClass::PlainEvent {
        let source_name = interaction
            .from
            .strip_prefix("event:")
            .unwrap_or(interaction.from.as_str());
        let blocks = external_event_blocks(interaction);
        return Input::ExternalEvent(ExternalEventInput {
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
            payload: external_event_payload(interaction),
            blocks,
            handling_mode: interaction.handling_mode,
            render_metadata: interaction.render_metadata.clone(),
        });
    }

    interaction_to_peer_input(interaction, runtime_id)
}

/// Convert an `InboxInteraction` to a v9 `Input::Peer`.
pub fn interaction_to_peer_input(
    interaction: &InboxInteraction,
    runtime_id: &LogicalRuntimeId,
) -> Input {
    let convention = map_convention(interaction);
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
        body: peer_rendered_body(interaction),
        blocks: peer_blocks(interaction),
    })
}

fn map_convention(interaction: &InboxInteraction) -> PeerConvention {
    match &interaction.content {
        InteractionContent::Message { .. } => PeerConvention::Message,
        InteractionContent::Request { intent, .. } => PeerConvention::Request {
            request_id: interaction.id.0.to_string(),
            intent: intent.clone(),
        },
        InteractionContent::Response {
            status,
            in_reply_to,
            ..
        } => {
            let request_id = in_reply_to.to_string();
            match status {
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
            }
        }
    }
}

fn peer_rendered_body(interaction: &InboxInteraction) -> String {
    match &interaction.content {
        InteractionContent::Message { body, .. } => {
            if interaction.from.starts_with("event:") && !interaction.rendered_text.is_empty() {
                interaction.rendered_text.clone()
            } else {
                body.clone()
            }
        }
        InteractionContent::Request { params, .. } => {
            if !interaction.rendered_text.is_empty() {
                return interaction.rendered_text.clone();
            }
            serde_json::to_string(params).unwrap_or_default()
        }
        InteractionContent::Response { result, .. } => {
            if !interaction.rendered_text.is_empty() {
                return interaction.rendered_text.clone();
            }
            serde_json::to_string(result).unwrap_or_default()
        }
    }
}

fn peer_blocks(interaction: &InboxInteraction) -> Option<Vec<meerkat_core::types::ContentBlock>> {
    match &interaction.content {
        InteractionContent::Message { blocks, .. } => blocks.clone(),
        _ => None,
    }
}

fn external_event_payload(interaction: &InboxInteraction) -> serde_json::Value {
    match &interaction.content {
        InteractionContent::Message { body, blocks } => {
            let mut payload = serde_json::json!({ "body": body });
            if let Some(blocks) = blocks {
                payload["blocks"] = serde_json::to_value(blocks).unwrap_or(serde_json::Value::Null);
            }
            payload
        }
        InteractionContent::Request { intent, params } => {
            serde_json::json!({ "intent": intent, "params": params })
        }
        InteractionContent::Response {
            in_reply_to,
            status,
            result,
        } => serde_json::json!({
            "in_reply_to": in_reply_to,
            "status": status,
            "result": result,
        }),
    }
}

fn external_event_blocks(
    interaction: &InboxInteraction,
) -> Option<Vec<meerkat_core::types::ContentBlock>> {
    match &interaction.content {
        InteractionContent::Message { blocks, .. } => blocks.clone(),
        _ => None,
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
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
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
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
        };
        let input = interaction_to_peer_input(&interaction, &LogicalRuntimeId::new("test"));
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
    fn plain_event_to_external_event_input() {
        let classified = ClassifiedInboxInteraction {
            class: PeerInputClass::PlainEvent,
            lifecycle_peer: None,
            interaction: InboxInteraction {
                from: "event:webhook".into(),
                content: InteractionContent::Message {
                    body: "{\"ok\":true}".into(),
                    blocks: None,
                },
                id: make_interaction_id(),
                rendered_text: String::new(),
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                render_metadata: None,
            },
        };
        let input =
            classified_interaction_to_runtime_input(&classified, &LogicalRuntimeId::new("test"));
        match input {
            Input::ExternalEvent(event) => {
                assert_eq!(event.event_type, "webhook");
                assert_eq!(event.payload["body"], "{\"ok\":true}");
                assert_eq!(event.blocks, None);
                assert_eq!(
                    event.handling_mode,
                    meerkat_core::types::HandlingMode::Queue
                );
                assert_eq!(event.render_metadata, None);
            }
            other => panic!("Expected ExternalEvent input, got {other:?}"),
        }
    }

    #[test]
    fn peer_named_event_prefix_stays_peer_without_plain_event_class() {
        let classified = ClassifiedInboxInteraction {
            class: PeerInputClass::ActionableMessage,
            lifecycle_peer: None,
            interaction: InboxInteraction {
                from: "event:webhook".into(),
                content: InteractionContent::Message {
                    body: "hello".into(),
                    blocks: None,
                },
                id: make_interaction_id(),
                rendered_text: "[COMMS MESSAGE from event:webhook]\nhello".into(),
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                render_metadata: None,
            },
        };
        let input =
            classified_interaction_to_runtime_input(&classified, &LogicalRuntimeId::new("test"));
        match input {
            Input::Peer(peer) => {
                assert_eq!(peer.body, "[COMMS MESSAGE from event:webhook]\nhello");
                match peer.header.source {
                    InputOrigin::Peer { peer_id, .. } => assert_eq!(peer_id, "event:webhook"),
                    other => panic!("Expected peer source, got {other:?}"),
                }
            }
            other => panic!("Expected Peer input, got {other:?}"),
        }
    }

    #[test]
    fn request_body_uses_rendered_text_projection() {
        let interaction = InboxInteraction {
            from: "event:webhook".into(),
            content: InteractionContent::Request {
                intent: "mob.peer_added".into(),
                params: serde_json::json!({"peer":"agent-1"}),
            },
            id: make_interaction_id(),
            rendered_text: "[COMMS REQUEST from event:webhook (id: req)]\nIntent: mob.peer_added"
                .into(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
        };
        let input = interaction_to_peer_input(&interaction, &LogicalRuntimeId::new("test"));
        if let Input::Peer(peer) = input {
            assert_eq!(
                peer.body,
                "[COMMS REQUEST from event:webhook (id: req)]\nIntent: mob.peer_added"
            );
        } else {
            panic!("Expected PeerInput");
        }
    }

    #[test]
    fn message_blocks_are_preserved_on_peer_input() {
        let blocks = vec![
            meerkat_core::types::ContentBlock::Text {
                text: "see image".into(),
            },
            meerkat_core::types::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc".into(),
            },
        ];
        let interaction = InboxInteraction {
            from: "peer-1".into(),
            content: InteractionContent::Message {
                body: "see image".into(),
                blocks: Some(blocks.clone()),
            },
            id: make_interaction_id(),
            rendered_text: "[COMMS MESSAGE from peer-1]\nsee image".into(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
        };
        let input = interaction_to_peer_input(&interaction, &LogicalRuntimeId::new("test"));
        if let Input::Peer(peer) = input {
            assert_eq!(peer.body, "see image");
            assert_eq!(peer.blocks, Some(blocks));
        } else {
            panic!("Expected PeerInput");
        }
    }

    #[test]
    fn multimodal_message_prefers_raw_body_over_rendered_projection() {
        let blocks = vec![
            meerkat_core::types::ContentBlock::Text {
                text: "caption text".into(),
            },
            meerkat_core::types::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc".into(),
            },
        ];
        let interaction = InboxInteraction {
            from: "peer-1".into(),
            content: InteractionContent::Message {
                body: "please inspect this image".into(),
                blocks: Some(blocks),
            },
            id: make_interaction_id(),
            rendered_text: "[COMMS MESSAGE from peer-1]\ncaption text\n[image: image/png]".into(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
        };
        let input = interaction_to_peer_input(&interaction, &LogicalRuntimeId::new("test"));
        if let Input::Peer(peer) = input {
            assert_eq!(peer.body, "please inspect this image");
        } else {
            panic!("Expected PeerInput");
        }
    }

    #[test]
    fn plain_event_blocks_are_preserved_on_external_event_input() {
        let blocks = vec![
            meerkat_core::types::ContentBlock::Text {
                text: "see image".into(),
            },
            meerkat_core::types::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc".into(),
            },
        ];
        let classified = ClassifiedInboxInteraction {
            class: PeerInputClass::PlainEvent,
            lifecycle_peer: None,
            interaction: InboxInteraction {
                from: "event:webhook".into(),
                content: InteractionContent::Message {
                    body: "see image".into(),
                    blocks: Some(blocks.clone()),
                },
                id: make_interaction_id(),
                rendered_text: "[EVENT via webhook] see image".into(),
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                render_metadata: None,
            },
        };
        let input =
            classified_interaction_to_runtime_input(&classified, &LogicalRuntimeId::new("test"));
        match input {
            Input::ExternalEvent(event) => {
                assert_eq!(event.payload["body"], "see image");
                assert!(event.payload["blocks"].is_array());
                assert_eq!(event.blocks, Some(blocks));
                assert_eq!(
                    event.handling_mode,
                    meerkat_core::types::HandlingMode::Queue
                );
                assert_eq!(event.render_metadata, None);
            }
            other => panic!("Expected ExternalEvent input, got {other:?}"),
        }
    }

    #[test]
    fn plain_event_preserves_handling_mode_and_render_metadata() {
        let render_metadata = meerkat_core::types::RenderMetadata {
            class: meerkat_core::types::RenderClass::ExternalEvent,
            salience: meerkat_core::types::RenderSalience::Urgent,
        };
        let classified = ClassifiedInboxInteraction {
            class: PeerInputClass::PlainEvent,
            lifecycle_peer: None,
            interaction: InboxInteraction {
                from: "event:webhook".into(),
                content: InteractionContent::Message {
                    body: "urgent".into(),
                    blocks: None,
                },
                id: make_interaction_id(),
                rendered_text: "[EVENT via webhook] urgent".into(),
                handling_mode: meerkat_core::types::HandlingMode::Steer,
                render_metadata: Some(render_metadata.clone()),
            },
        };

        match classified_interaction_to_runtime_input(&classified, &LogicalRuntimeId::new("test")) {
            Input::ExternalEvent(event) => {
                assert_eq!(
                    event.handling_mode,
                    meerkat_core::types::HandlingMode::Steer
                );
                assert_eq!(event.render_metadata, Some(render_metadata));
            }
            other => panic!("Expected ExternalEvent input, got {other:?}"),
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
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
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
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
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
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
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
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
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
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                render_metadata: None,
            },
            InboxInteraction {
                from: "p".into(),
                content: InteractionContent::Request {
                    intent: "i".into(),
                    params: serde_json::json!({}),
                },
                id: make_interaction_id(),
                rendered_text: String::new(),
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                render_metadata: None,
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
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                render_metadata: None,
            },
        ];

        let rid = LogicalRuntimeId::new("test");
        for interaction in &interactions {
            let input = interaction_to_peer_input(interaction, &rid);
            assert!(matches!(input, Input::Peer(_)));
        }
    }
}
