//! Runtime comms bridge helpers.
//!
//! These helpers translate drained comms interactions into the runtime-owned
//! input families used by the comms classification bridge.

use chrono::Utc;
#[cfg(test)]
use meerkat_core::comms::PeerId;
use meerkat_core::interaction::{
    InboxInteraction, InteractionContent, PeerIngressConvention, PeerIngressFact, PeerIngressKind,
    PeerInputCandidate, PeerInputClass,
};
#[cfg(test)]
use meerkat_core::interaction::{PeerIngressIdentity, ResponseStatus};
use meerkat_core::lifecycle::InputId;

use crate::identifiers::{CorrelationId, LogicalRuntimeId};
use crate::input::{
    ExternalEventInput, Input, InputDurability, InputHeader, InputOrigin, InputVisibility,
    PeerConvention, PeerInput, ResponseProgressPhase, ResponseTerminalStatus,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PeerIngressProjectionError {
    #[error(
        "classified peer ingress {interaction_id} ({kind:?}) cannot project to a runtime PeerInput"
    )]
    UnsupportedPeerConvention {
        interaction_id: meerkat_core::InteractionId,
        kind: PeerIngressKind,
    },
    #[error("classified peer ingress {interaction_id} missing canonical peer id")]
    MissingCanonicalPeerId {
        interaction_id: meerkat_core::InteractionId,
    },
}

/// Convert a classified comms interaction into the appropriate runtime-owned
/// input family.
pub fn classified_interaction_to_runtime_input(
    classified: &PeerInputCandidate,
    runtime_id: &LogicalRuntimeId,
) -> Result<Input, PeerIngressProjectionError> {
    let interaction = &classified.interaction;

    if classified.class() == PeerInputClass::PlainEvent {
        let source_name = classified
            .ingress
            .plain_event_source_name()
            .unwrap_or("unknown");
        let blocks = external_event_blocks(interaction);
        return Ok(Input::ExternalEvent(ExternalEventInput {
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
        }));
    }

    peer_candidate_to_peer_input(classified, runtime_id)
}

/// Compatibility alias while callers migrate to the classified bridge naming.
pub fn peer_input_candidate_to_runtime_input(
    classified: &PeerInputCandidate,
    runtime_id: &LogicalRuntimeId,
) -> Result<Input, PeerIngressProjectionError> {
    classified_interaction_to_runtime_input(classified, runtime_id)
}

fn peer_candidate_to_peer_input(
    classified: &PeerInputCandidate,
    runtime_id: &LogicalRuntimeId,
) -> Result<Input, PeerIngressProjectionError> {
    peer_input_from_ingress_fact(
        &classified.interaction,
        runtime_id,
        &classified.ingress,
        classified.response_terminality,
    )
}

fn peer_input_from_ingress_fact(
    interaction: &InboxInteraction,
    runtime_id: &LogicalRuntimeId,
    ingress: &PeerIngressFact,
    response_terminality: Option<meerkat_core::interaction::TerminalityClass>,
) -> Result<Input, PeerIngressProjectionError> {
    let convention = map_ingress_convention(ingress, response_terminality).ok_or(
        PeerIngressProjectionError::UnsupportedPeerConvention {
            interaction_id: interaction.id,
            kind: ingress.kind,
        },
    )?;
    let durability = map_durability(&convention);
    let peer_id = ingress.canonical_peer_id_string().ok_or(
        PeerIngressProjectionError::MissingCanonicalPeerId {
            interaction_id: interaction.id,
        },
    )?;
    let display_identity = ingress
        .route
        .as_ref()
        .map(meerkat_core::PeerRoute::label)
        .or_else(|| ingress.display_label());

    Ok(Input::Peer(PeerInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Peer {
                peer_id,
                display_identity,
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
        payload: peer_payload(interaction),
        blocks: peer_blocks(interaction),
        handling_mode: match interaction.handling_mode {
            meerkat_core::types::HandlingMode::Queue => None,
            mode => Some(mode),
        },
    }))
}

fn map_ingress_convention(
    ingress: &PeerIngressFact,
    response_terminality: Option<meerkat_core::interaction::TerminalityClass>,
) -> Option<PeerConvention> {
    match &ingress.convention {
        PeerIngressConvention::Message => Some(PeerConvention::Message),
        PeerIngressConvention::Request { request_id, intent } => Some(PeerConvention::Request {
            request_id: request_id.clone(),
            intent: intent.clone(),
        }),
        PeerIngressConvention::Response {
            in_reply_to,
            status,
        } => Some(map_response_convention(
            *in_reply_to,
            *status,
            Some(ingress.class),
            response_terminality,
        )),
        PeerIngressConvention::Lifecycle { kind, .. } => Some(PeerConvention::Request {
            request_id: ingress.interaction_id.to_string(),
            intent: kind.to_string(),
        }),
        PeerIngressConvention::Ack { .. } | PeerIngressConvention::PlainEvent { .. } => None,
    }
}

fn map_response_convention(
    in_reply_to: meerkat_core::InteractionId,
    status: meerkat_core::ResponseStatus,
    ingress_class: Option<PeerInputClass>,
    response_terminality: Option<meerkat_core::interaction::TerminalityClass>,
) -> PeerConvention {
    let request_id = in_reply_to.to_string();
    let terminality = match response_terminality {
        Some(terminality) => terminality,
        None => match ingress_class {
            Some(PeerInputClass::ResponseProgress) => {
                meerkat_core::interaction::TerminalityClass::Progress
            }
            Some(PeerInputClass::ResponseTerminal) => {
                match meerkat_core::interaction::classify_response_terminality(status) {
                    terminal @ meerkat_core::interaction::TerminalityClass::Terminal { .. } => {
                        terminal
                    }
                    other => {
                        tracing::warn!(
                            class = ?other,
                            "terminal ingress class conflicted with response status; failing closed as terminal failure"
                        );
                        meerkat_core::interaction::TerminalityClass::Terminal {
                            disposition: meerkat_core::interaction::TerminalDisposition::Failed,
                        }
                    }
                }
            }
            _ => meerkat_core::interaction::classify_response_terminality(status),
        },
    };
    match terminality {
        meerkat_core::interaction::TerminalityClass::Progress => PeerConvention::ResponseProgress {
            request_id,
            phase: ResponseProgressPhase::Accepted,
        },
        meerkat_core::interaction::TerminalityClass::Terminal { disposition } => {
            let term = match disposition {
                meerkat_core::interaction::TerminalDisposition::Completed => {
                    ResponseTerminalStatus::Completed
                }
                meerkat_core::interaction::TerminalDisposition::Failed => {
                    ResponseTerminalStatus::Failed
                }
                other => {
                    tracing::warn!(
                        disposition = ?other,
                        "unknown terminal disposition; treating as Failed"
                    );
                    ResponseTerminalStatus::Failed
                }
            };
            PeerConvention::ResponseTerminal {
                request_id,
                status: term,
            }
        }
        other => {
            tracing::warn!(
                class = ?other,
                "unknown terminality class; routing response as progress (non-terminal)"
            );
            PeerConvention::ResponseProgress {
                request_id,
                phase: ResponseProgressPhase::Accepted,
            }
        }
    }
}

fn peer_rendered_body(interaction: &InboxInteraction) -> String {
    // For every content kind, prefer the comms-owned `rendered_text`
    // projection when present: it already carries the authoritative
    // `[from]: <kind>` framing that later prompt-rendering relies on
    // (see `peer_prompt_text`). Fall back to the content-specific
    // serialization only when rendered_text is empty.
    if !interaction.rendered_text.is_empty() {
        return interaction.rendered_text.clone();
    }
    match &interaction.content {
        InteractionContent::Message { body, .. } => body.clone(),
        InteractionContent::Request { params, .. } => {
            serde_json::to_string(params).unwrap_or_default()
        }
        InteractionContent::Response { result, .. } => {
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

fn peer_payload(interaction: &InboxInteraction) -> Option<serde_json::Value> {
    match &interaction.content {
        InteractionContent::Message { .. } => None,
        InteractionContent::Request { params, .. } => Some(params.clone()),
        InteractionContent::Response { result, .. } => Some(result.clone()),
    }
}

fn external_event_payload(interaction: &InboxInteraction) -> serde_json::Value {
    match &interaction.content {
        InteractionContent::Message { body, .. } => serde_json::json!({ "body": body }),
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
    use meerkat_core::interaction::{PeerIngressIdentity, ResponseStatus};

    fn make_interaction_id() -> meerkat_core::interaction::InteractionId {
        meerkat_core::interaction::InteractionId(uuid::Uuid::now_v7())
    }

    fn plain_event_ingress(
        id: meerkat_core::interaction::InteractionId,
        source_name: &str,
    ) -> PeerIngressFact {
        PeerIngressFact::plain_event(
            id,
            source_name,
            PeerInputClass::PlainEvent,
            meerkat_core::PeerIngressKind::PlainEvent,
        )
    }

    fn test_peer_id() -> PeerId {
        PeerId::parse("22222222-2222-4222-8222-222222222222").expect("canonical test peer id")
    }

    fn peer_kind_for_convention(
        convention: &PeerIngressConvention,
    ) -> meerkat_core::PeerIngressKind {
        match convention {
            PeerIngressConvention::Message => meerkat_core::PeerIngressKind::Message,
            PeerIngressConvention::Request { .. } | PeerIngressConvention::Lifecycle { .. } => {
                meerkat_core::PeerIngressKind::Request
            }
            PeerIngressConvention::Response { .. } => meerkat_core::PeerIngressKind::Response,
            PeerIngressConvention::Ack { .. } => meerkat_core::PeerIngressKind::Ack,
            PeerIngressConvention::PlainEvent { .. } => meerkat_core::PeerIngressKind::PlainEvent,
        }
    }

    fn peer_ingress(
        id: meerkat_core::interaction::InteractionId,
        peer_id: PeerId,
        label: &str,
        class: PeerInputClass,
        convention: PeerIngressConvention,
    ) -> PeerIngressFact {
        let kind = peer_kind_for_convention(&convention);
        PeerIngressFact::peer(
            id,
            class,
            kind,
            Some(meerkat_core::PeerIngressAuthDecision::Required),
            PeerIngressIdentity::new(peer_id, label, convention),
        )
    }

    fn candidate_for_interaction(interaction: InboxInteraction) -> PeerInputCandidate {
        let id = interaction.id;
        let label = interaction.from.clone();
        let peer_id = interaction.from_route.unwrap_or_else(test_peer_id);
        let (class, convention) = match &interaction.content {
            InteractionContent::Message { .. } => (
                PeerInputClass::ActionableMessage,
                PeerIngressConvention::Message,
            ),
            InteractionContent::Request { intent, .. } => (
                PeerInputClass::ActionableRequest,
                PeerIngressConvention::Request {
                    request_id: id.to_string(),
                    intent: intent.clone(),
                },
            ),
            InteractionContent::Response {
                in_reply_to,
                status,
                ..
            } => (
                meerkat_core::PeerIngressMachinePolicy::default()
                    .classify_response(*status)
                    .class,
                PeerIngressConvention::Response {
                    in_reply_to: *in_reply_to,
                    status: *status,
                },
            ),
        };
        let ingress = peer_ingress(id, peer_id, &label, class, convention);
        PeerInputCandidate::new(interaction, ingress, None)
    }

    fn peer_input_for_test(interaction: &InboxInteraction, runtime_id: &LogicalRuntimeId) -> Input {
        let candidate = candidate_for_interaction(interaction.clone());
        peer_input_candidate_to_runtime_input(&candidate, runtime_id)
            .expect("test candidate should project to runtime input")
    }

    #[test]
    fn message_to_peer_input() {
        let interaction = InboxInteraction {
            from_route: None,
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
        let input = peer_input_for_test(&interaction, &LogicalRuntimeId::new("test"));
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
            from_route: None,
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
        let input = peer_input_for_test(&interaction, &LogicalRuntimeId::new("test"));
        if let Input::Peer(p) = &input {
            assert!(matches!(p.convention, Some(PeerConvention::Request { .. })));
            match p.convention.as_ref() {
                Some(PeerConvention::Request { request_id, .. }) => {
                    assert_eq!(request_id, &interaction.id.0.to_string());
                }
                other => panic!("Expected request convention, got {other:?}"),
            }
            assert_eq!(p.header.durability, InputDurability::Durable);
            assert_eq!(
                p.payload,
                Some(serde_json::json!({"name": "agent-1"})),
                "request params must remain structured on PeerInput so runtime prompt projection does not depend on pre-rendered comms prose"
            );
        } else {
            panic!("Expected PeerInput");
        }
    }

    #[test]
    fn classified_request_uses_canonical_peer_id_for_runtime_projection() {
        let source_peer_id =
            PeerId::parse("11111111-1111-4111-8111-111111111111").expect("canonical peer id");
        let request_id = make_interaction_id();
        let classified = PeerInputCandidate {
            interaction: InboxInteraction {
                from_route: None,
                from: "test-mob/lead/l-requester".into(),
                content: InteractionContent::Request {
                    intent: "interpret_image".into(),
                    params: serde_json::json!({"description": "tower with a light"}),
                },
                id: request_id,
                rendered_text: "[COMMS REQUEST stale helper prose]".into(),
                handling_mode: meerkat_core::types::HandlingMode::Steer,
                render_metadata: None,
            },
            ingress: PeerIngressFact::peer(
                request_id,
                PeerInputClass::ActionableRequest,
                meerkat_core::PeerIngressKind::Request,
                Some(meerkat_core::PeerIngressAuthDecision::Required),
                PeerIngressIdentity::new(
                    source_peer_id,
                    "test-mob/lead/l-requester",
                    PeerIngressConvention::Request {
                        request_id: request_id.to_string(),
                        intent: "interpret_image".to_string(),
                    },
                ),
            ),
            lifecycle_peer: None,
            response_terminality: None,
        };

        let input =
            peer_input_candidate_to_runtime_input(&classified, &LogicalRuntimeId::new("worker"))
                .expect("classified request should project to peer input");
        let Input::Peer(peer) = &input else {
            panic!("Expected PeerInput");
        };
        let InputOrigin::Peer { peer_id, .. } = &peer.header.source else {
            panic!("Expected peer source");
        };
        assert_eq!(peer_id, "11111111-1111-4111-8111-111111111111");
        assert_eq!(peer.body, "[COMMS REQUEST stale helper prose]");

        let prompt = crate::input::input_prompt_text(&input);
        assert!(prompt.starts_with(
            "[SYSTEM NOTICE][PEER_REQUEST] Correlated peer request from peer_id 11111111-1111-4111-8111-111111111111 (display_name: test-mob/lead/l-requester)."
        ));
        assert!(prompt.contains("\"peer_id\":\"11111111-1111-4111-8111-111111111111\""));
        assert!(prompt.contains("\"display_name\":\"test-mob/lead/l-requester\""));
        assert!(prompt.contains(&format!("\"in_reply_to\":\"{}\"", request_id.0)));
        assert!(prompt.contains("\"status\":\"completed\""));
        assert!(!prompt.contains("to=\""));
    }

    #[test]
    fn plain_event_to_external_event_input() {
        let id = make_interaction_id();
        let classified = PeerInputCandidate {
            lifecycle_peer: None,
            response_terminality: None,
            ingress: plain_event_ingress(id, "webhook"),
            interaction: InboxInteraction {
                from_route: None,
                from: "event:webhook".into(),
                content: InteractionContent::Message {
                    body: "{\"ok\":true}".into(),
                    blocks: None,
                },
                id,
                rendered_text: String::new(),
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                render_metadata: None,
            },
        };
        let input =
            peer_input_candidate_to_runtime_input(&classified, &LogicalRuntimeId::new("test"))
                .expect("plain event should project to external event input");
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
        let id = make_interaction_id();
        let classified = PeerInputCandidate {
            lifecycle_peer: None,
            response_terminality: None,
            ingress: peer_ingress(
                id,
                test_peer_id(),
                "event:webhook",
                PeerInputClass::ActionableMessage,
                PeerIngressConvention::Message,
            ),
            interaction: InboxInteraction {
                from_route: None,
                from: "event:webhook".into(),
                content: InteractionContent::Message {
                    body: "hello".into(),
                    blocks: None,
                },
                id,
                rendered_text: "[COMMS MESSAGE from event:webhook]\nhello".into(),
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                render_metadata: None,
            },
        };
        let input =
            peer_input_candidate_to_runtime_input(&classified, &LogicalRuntimeId::new("test"))
                .expect("classified peer event should project to peer input");
        match input {
            Input::Peer(peer) => {
                assert_eq!(peer.body, "[COMMS MESSAGE from event:webhook]\nhello");
                match peer.header.source {
                    InputOrigin::Peer { peer_id, .. } => {
                        assert_eq!(peer_id, test_peer_id().as_str())
                    }
                    other => panic!("Expected peer source, got {other:?}"),
                }
            }
            other => panic!("Expected Peer input, got {other:?}"),
        }
    }

    #[test]
    fn classified_peer_projection_uses_ingress_canonical_peer_id_not_display_from() {
        let id = make_interaction_id();
        let canonical_peer_id = meerkat_core::comms::PeerId::new();
        let classified = PeerInputCandidate {
            lifecycle_peer: None,
            response_terminality: None,
            ingress: PeerIngressFact::peer(
                id,
                PeerInputClass::ActionableRequest,
                meerkat_core::PeerIngressKind::Request,
                Some(meerkat_core::PeerIngressAuthDecision::Required),
                PeerIngressIdentity::new(
                    canonical_peer_id,
                    "display-agent",
                    PeerIngressConvention::Request {
                        request_id: id.to_string(),
                        intent: "review".to_string(),
                    },
                ),
            ),
            interaction: InboxInteraction {
                from_route: None,
                from: "display-agent".into(),
                content: InteractionContent::Request {
                    intent: "review".into(),
                    params: serde_json::json!({"pr": 42}),
                },
                id,
                rendered_text: "[COMMS REQUEST from display-agent]".into(),
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                render_metadata: None,
            },
        };

        let input =
            peer_input_candidate_to_runtime_input(&classified, &LogicalRuntimeId::new("test"))
                .expect("classified peer projection should use typed canonical id");
        let Input::Peer(peer) = input else {
            panic!("Expected Peer input");
        };
        match peer.header.source {
            InputOrigin::Peer { peer_id, .. } => {
                assert_eq!(peer_id, canonical_peer_id.as_str());
                assert_ne!(peer_id, "display-agent");
            }
            other => panic!("Expected peer source, got {other:?}"),
        }
        assert_eq!(peer.body, "[COMMS REQUEST from display-agent]");
    }

    #[test]
    fn classified_peer_projection_rejects_display_only_ingress_identity() {
        let id = make_interaction_id();
        let classified = PeerInputCandidate {
            lifecycle_peer: None,
            response_terminality: None,
            ingress: PeerIngressFact::legacy_peer_label(
                id,
                "display-agent",
                PeerInputClass::ActionableMessage,
                meerkat_core::PeerIngressKind::Message,
                Some(meerkat_core::PeerIngressAuthDecision::Required),
                PeerIngressConvention::Message,
            ),
            interaction: InboxInteraction {
                from_route: None,
                from: "display-agent".into(),
                content: InteractionContent::Message {
                    body: "hello".into(),
                    blocks: None,
                },
                id,
                rendered_text: "[COMMS MESSAGE from display-agent]\nhello".into(),
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                render_metadata: None,
            },
        };

        let result =
            peer_input_candidate_to_runtime_input(&classified, &LogicalRuntimeId::new("test"));
        assert!(
            matches!(
                result,
                Err(PeerIngressProjectionError::MissingCanonicalPeerId { interaction_id })
                    if interaction_id == id
            ),
            "display-only ingress must fail closed, got {result:?}"
        );
    }

    #[test]
    fn request_body_prefers_rendered_text_and_preserves_structured_payload() {
        // Comms-owned `rendered_text` is the authoritative projection for
        // request/response conventions too (not just messages): it carries
        // the `[COMMS REQUEST ...]` framing downstream prompt rendering
        // relies on. The structured JSON still flows through `payload` for
        // consumers that need the raw params.
        let interaction = InboxInteraction {
            from_route: None,
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
        let input = peer_input_for_test(&interaction, &LogicalRuntimeId::new("test"));
        if let Input::Peer(peer) = input {
            assert_eq!(
                peer.body,
                "[COMMS REQUEST from event:webhook (id: req)]\nIntent: mob.peer_added"
            );
            assert_eq!(peer.payload, Some(serde_json::json!({"peer":"agent-1"})));
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
            from_route: None,
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
        let input = peer_input_for_test(&interaction, &LogicalRuntimeId::new("test"));
        if let Input::Peer(peer) = input {
            assert_eq!(peer.body, "[COMMS MESSAGE from peer-1]\nsee image");
            assert_eq!(peer.blocks, Some(blocks));
        } else {
            panic!("Expected PeerInput");
        }
    }

    #[test]
    fn multimodal_message_uses_rendered_projection_while_preserving_blocks() {
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
            from_route: None,
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
        let input = peer_input_for_test(&interaction, &LogicalRuntimeId::new("test"));
        if let Input::Peer(peer) = input {
            assert_eq!(
                peer.body,
                "[COMMS MESSAGE from peer-1]\ncaption text\n[image: image/png]"
            );
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
        let id = make_interaction_id();
        let classified = PeerInputCandidate {
            lifecycle_peer: None,
            response_terminality: None,
            ingress: plain_event_ingress(id, "webhook"),
            interaction: InboxInteraction {
                from_route: None,
                from: "event:webhook".into(),
                content: InteractionContent::Message {
                    body: "see image".into(),
                    blocks: Some(blocks.clone()),
                },
                id,
                rendered_text: "[EVENT via webhook] see image".into(),
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                render_metadata: None,
            },
        };
        let input =
            peer_input_candidate_to_runtime_input(&classified, &LogicalRuntimeId::new("test"))
                .expect("plain event with blocks should project");
        match input {
            Input::ExternalEvent(event) => {
                assert_eq!(event.payload["body"], "see image");
                assert!(event.payload.get("blocks").is_none());
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
        let id = make_interaction_id();
        let classified = PeerInputCandidate {
            lifecycle_peer: None,
            response_terminality: None,
            ingress: plain_event_ingress(id, "webhook"),
            interaction: InboxInteraction {
                from_route: None,
                from: "event:webhook".into(),
                content: InteractionContent::Message {
                    body: "urgent".into(),
                    blocks: None,
                },
                id,
                rendered_text: "[EVENT via webhook] urgent".into(),
                handling_mode: meerkat_core::types::HandlingMode::Steer,
                render_metadata: Some(render_metadata.clone()),
            },
        };

        match peer_input_candidate_to_runtime_input(&classified, &LogicalRuntimeId::new("test"))
            .expect("plain event should preserve render metadata")
        {
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
        let route_id = meerkat_core::comms::PeerId::from_uuid(
            uuid::Uuid::parse_str("018f6f79-7a82-7c4e-a552-a3b86f9630f2").unwrap(),
        );
        let interaction = InboxInteraction {
            from_route: Some(route_id),
            from: "Peer One".into(),
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
        let input = peer_input_for_test(&interaction, &LogicalRuntimeId::new("test"));
        if let Input::Peer(p) = &input {
            match &p.header.source {
                InputOrigin::Peer {
                    peer_id,
                    display_identity,
                    ..
                } => {
                    assert_eq!(peer_id, &route_id.to_string());
                    assert_eq!(display_identity.as_deref(), Some("Peer One"));
                }
                other => panic!("Expected Peer source, got {other:?}"),
            }
            assert!(matches!(
                p.convention,
                Some(PeerConvention::ResponseTerminal {
                    status: ResponseTerminalStatus::Completed,
                    ..
                })
            ));
            assert_eq!(p.header.durability, InputDurability::Durable);
            assert_eq!(
                p.payload,
                Some(serde_json::json!({"ok": true})),
                "terminal response result must remain structured on PeerInput so runtime prompt projection stays runtime-owned"
            );
        } else {
            panic!("Expected PeerInput");
        }
        let projection = crate::input::peer_projection(&input).expect("terminal projection");
        let expected_key = format!("peer_response_terminal:{route_id}:{in_reply_to}");
        assert_eq!(
            projection.context_key().as_deref(),
            Some(expected_key.as_str())
        );
        assert!(
            projection.prompt_text().contains("from Peer One"),
            "terminal prompt should use display identity: {}",
            projection.prompt_text()
        );
    }

    #[test]
    fn classified_response_uses_ingress_terminal_class() {
        let in_reply_to = make_interaction_id();
        let id = make_interaction_id();
        let classified = PeerInputCandidate {
            interaction: InboxInteraction {
                from_route: None,
                from: "peer-1".into(),
                content: InteractionContent::Response {
                    status: ResponseStatus::Completed,
                    result: serde_json::json!({"ok": true}),
                    in_reply_to,
                },
                id,
                rendered_text: String::new(),
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                render_metadata: None,
            },
            ingress: PeerIngressFact::peer(
                id,
                PeerInputClass::ResponseProgress,
                meerkat_core::PeerIngressKind::Response,
                Some(meerkat_core::PeerIngressAuthDecision::Required),
                PeerIngressIdentity::new(
                    test_peer_id(),
                    "peer-1",
                    PeerIngressConvention::Response {
                        in_reply_to,
                        status: ResponseStatus::Completed,
                    },
                ),
            ),
            lifecycle_peer: None,
            response_terminality: Some(meerkat_core::TerminalityClass::Progress),
        };

        let input =
            classified_interaction_to_runtime_input(&classified, &LogicalRuntimeId::new("test"))
                .expect("classified response should project");
        if let Input::Peer(peer) = input {
            assert!(
                matches!(
                    peer.convention,
                    Some(PeerConvention::ResponseProgress { .. })
                ),
                "classified bridge must consume ingress-owned response class"
            );
        } else {
            panic!("Expected PeerInput");
        }
    }

    #[test]
    fn response_terminal_without_canonical_peer_id_fails_typed_projection() {
        let in_reply_to = make_interaction_id();
        let interaction_id = make_interaction_id();
        let candidate = PeerInputCandidate {
            interaction: InboxInteraction {
                from_route: None,
                from: "Peer One".into(),
                content: InteractionContent::Response {
                    status: ResponseStatus::Completed,
                    result: serde_json::json!({"ok": true}),
                    in_reply_to,
                },
                id: interaction_id,
                rendered_text: String::new(),
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                render_metadata: None,
            },
            ingress: PeerIngressFact::legacy_peer_label(
                interaction_id,
                "Peer One",
                PeerInputClass::ResponseTerminal,
                meerkat_core::PeerIngressKind::Response,
                Some(meerkat_core::PeerIngressAuthDecision::Required),
                PeerIngressConvention::Response {
                    in_reply_to,
                    status: ResponseStatus::Completed,
                },
            ),
            lifecycle_peer: None,
            response_terminality: Some(meerkat_core::TerminalityClass::Terminal {
                disposition: meerkat_core::TerminalDisposition::Completed,
            }),
        };
        let err = peer_input_candidate_to_runtime_input(&candidate, &LogicalRuntimeId::new("test"))
            .unwrap_err();
        assert!(matches!(
            err,
            PeerIngressProjectionError::MissingCanonicalPeerId { .. }
        ));
    }

    #[test]
    fn response_failed_to_terminal() {
        let in_reply_to = make_interaction_id();
        let route_id = meerkat_core::comms::PeerId::from_uuid(
            uuid::Uuid::parse_str("018f6f79-7a82-7c4e-a552-a3b86f9630f3").unwrap(),
        );
        let interaction = InboxInteraction {
            from_route: Some(route_id),
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
        let input = peer_input_for_test(&interaction, &LogicalRuntimeId::new("test"));
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
            from_route: None,
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
        let input = peer_input_for_test(&interaction, &LogicalRuntimeId::new("test"));
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
    fn classified_response_uses_ingress_terminality_over_raw_status() {
        let in_reply_to = make_interaction_id();
        let id = make_interaction_id();
        let classified = PeerInputCandidate {
            interaction: InboxInteraction {
                from_route: None,
                from: "peer-1".into(),
                content: InteractionContent::Response {
                    status: ResponseStatus::Completed,
                    result: serde_json::json!({"ok": true}),
                    in_reply_to,
                },
                id,
                rendered_text: String::new(),
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                render_metadata: None,
            },
            ingress: PeerIngressFact::peer(
                id,
                PeerInputClass::ResponseProgress,
                meerkat_core::PeerIngressKind::Response,
                Some(meerkat_core::PeerIngressAuthDecision::Required),
                PeerIngressIdentity::new(
                    test_peer_id(),
                    "peer-1",
                    PeerIngressConvention::Response {
                        in_reply_to,
                        status: ResponseStatus::Completed,
                    },
                ),
            ),
            lifecycle_peer: None,
            response_terminality: Some(meerkat_core::TerminalityClass::Progress),
        };

        let input =
            peer_input_candidate_to_runtime_input(&classified, &LogicalRuntimeId::new("test"))
                .expect("classified response should project");

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
            from_route: None,
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
        let input = peer_input_for_test(&interaction, &LogicalRuntimeId::new("agent-runtime-1"));
        if let Input::Peer(p) = &input {
            if let InputOrigin::Peer {
                peer_id,
                display_identity,
                runtime_id,
                ..
            } = &p.header.source
            {
                assert_eq!(peer_id, &test_peer_id().as_str());
                assert_eq!(display_identity.as_deref(), Some("peer-1"));
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
                from_route: None,
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
                from_route: None,
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
                from_route: Some(meerkat_core::comms::PeerId::from_uuid(
                    uuid::Uuid::parse_str("018f6f79-7a82-7c4e-a552-a3b86f9630f6").unwrap(),
                )),
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
            let input = peer_input_for_test(interaction, &rid);
            assert!(matches!(input, Input::Peer(_)));
        }
    }
}
