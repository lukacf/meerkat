//! CommsMessage types for session-injectable messages.
//!
//! These types project classified inbox entries into the format needed for
//! injection into an agent's session.

use crate::inproc::InprocRegistry;
use crate::{InboxItem, MessageKind, PubKey};
use meerkat_core::comms::PeerLifecycleKind;
use meerkat_core::types::ContentBlock;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::borrow::Cow;
use uuid::Uuid;

/// Standard message intents for inter-agent communication.
///
/// This enum provides type-safe intent values for common operations,
/// with a `Custom` variant for user-defined extensibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageIntent {
    /// Request to delegate a task to another agent
    Delegate,
    /// Request status update on a task
    Status,
    /// Request to cancel an operation
    Cancel,
    /// Request acknowledgment/confirmation
    Ack,
    /// Request to review something (code, PR, etc.)
    Review,
    /// Request computation or calculation
    Calculate,
    /// Request information or query
    Query,
    /// Peer added lifecycle event (mob.peer_added)
    #[serde(rename = "mob.peer_added")]
    PeerAdded,
    /// Peer retired lifecycle event (mob.peer_retired)
    #[serde(rename = "mob.peer_retired")]
    PeerRetired,
    /// Custom intent for user-defined operations
    #[serde(untagged)]
    Custom(String),
}

impl MessageIntent {
    /// Create a custom intent from a string
    pub fn custom(s: impl Into<String>) -> Self {
        Self::Custom(s.into())
    }

    /// Get the intent as a string (for backward compatibility)
    pub fn as_str(&self) -> &str {
        match self {
            Self::Delegate => "delegate",
            Self::Status => "status",
            Self::Cancel => "cancel",
            Self::Ack => "ack",
            Self::Review => "review",
            Self::Calculate => "calculate",
            Self::Query => "query",
            Self::PeerAdded => "mob.peer_added",
            Self::PeerRetired => "mob.peer_retired",
            Self::Custom(s) => s.as_str(),
        }
    }
}

impl From<String> for MessageIntent {
    fn from(s: String) -> Self {
        match s.as_str() {
            "delegate" => Self::Delegate,
            "status" => Self::Status,
            "cancel" => Self::Cancel,
            "ack" => Self::Ack,
            "review" => Self::Review,
            "calculate" => Self::Calculate,
            "query" => Self::Query,
            "mob.peer_added" => Self::PeerAdded,
            "mob.peer_retired" => Self::PeerRetired,
            _ => Self::Custom(s),
        }
    }
}

impl From<&str> for MessageIntent {
    fn from(s: &str) -> Self {
        Self::from(s.to_string())
    }
}

impl std::fmt::Display for MessageIntent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Content variants for comms messages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommsContent {
    /// A simple text message from a peer.
    Message {
        /// The message body.
        body: String,
        /// Optional multimodal content blocks.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<ContentBlock>>,
    },
    /// A request from a peer asking this agent to perform an action.
    Request {
        /// The request ID (for send_response).
        request_id: Uuid,
        /// The intent/action being requested (now type-safe).
        intent: MessageIntent,
        /// Parameters for the request.
        params: JsonValue,
        /// Optional multimodal content blocks.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<ContentBlock>>,
    },
    /// A one-way peer lifecycle notification (control-plane topology update).
    ///
    /// The lifecycle kind is the typed authority on the notification's
    /// semantics — including dismissal terminality. Consumers read
    /// `kind`, never the rendered body text.
    Lifecycle {
        /// The typed lifecycle kind.
        kind: PeerLifecycleKind,
        /// Parameters carried by the lifecycle notification (e.g. subject peer).
        params: JsonValue,
    },
    /// A response from a peer to a previous request.
    Response {
        /// The ID of the request this is responding to.
        in_reply_to: Uuid,
        /// The status of the response.
        status: CommsStatus,
        /// The result data.
        result: JsonValue,
        /// Optional multimodal content blocks.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<ContentBlock>>,
    },
}

/// Response status (mirrors meerkat_comms::Status).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommsStatus {
    Accepted,
    Completed,
    Failed,
}

impl From<crate::Status> for CommsStatus {
    fn from(s: crate::Status) -> Self {
        match s {
            crate::Status::Accepted => CommsStatus::Accepted,
            crate::Status::Completed => CommsStatus::Completed,
            crate::Status::Failed => CommsStatus::Failed,
        }
    }
}

impl From<CommsStatus> for crate::Status {
    fn from(s: CommsStatus) -> Self {
        match s {
            CommsStatus::Accepted => crate::Status::Accepted,
            CommsStatus::Completed => crate::Status::Completed,
            CommsStatus::Failed => crate::Status::Failed,
        }
    }
}

/// A message from the comms inbox ready for session injection.
///
/// This is the processed form of an `InboxItem::External` envelope,
/// containing the peer name (resolved from pubkey) and parsed content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommsMessage {
    /// The envelope ID (for reference in responses).
    pub envelope_id: Uuid,
    /// The peer name (ingress-resolved display identity).
    pub from_peer: String,
    /// The peer's public key (for explicit reference).
    pub from_pubkey: PubKey,
    /// The message content.
    pub content: CommsContent,
}

impl CommsMessage {
    pub(crate) fn from_external_with_resolved_peer(
        envelope: &crate::Envelope,
        from_peer: String,
    ) -> Option<Self> {
        let content = match &envelope.kind {
            MessageKind::Message { body, blocks, .. } => CommsContent::Message {
                body: body.clone(),
                blocks: blocks.clone(),
            },
            // This public projection is explicitly session-injectable and has
            // no access to the runtime's member-residency authority. A fenced
            // envelope must therefore stay on the structured interaction
            // drain, where MeerkatMachine checks `expected_recipient` before
            // admitting any work. Flattening it into `Message` here would turn
            // the legacy drain/recv APIs into an authority bypass.
            MessageKind::IncarnationFencedMessage { .. } => return None,
            MessageKind::Request {
                intent,
                params,
                blocks,
                ..
            } => CommsContent::Request {
                request_id: envelope.id,
                intent: MessageIntent::from(intent.as_str()),
                params: params.clone(),
                blocks: blocks.clone(),
            },
            MessageKind::Lifecycle { kind, params } => CommsContent::Lifecycle {
                kind: *kind,
                params: params.clone(),
            },
            MessageKind::Response {
                in_reply_to,
                status,
                result,
                blocks,
                ..
            } => CommsContent::Response {
                in_reply_to: *in_reply_to,
                status: (*status).into(),
                result: result.clone(),
                blocks: blocks.clone(),
            },
            MessageKind::Ack { .. } => return None,
        };

        Some(CommsMessage {
            envelope_id: envelope.id,
            from_peer,
            from_pubkey: envelope.from,
            content,
        })
    }

    /// Create a `CommsMessage` from a classified inbox entry.
    ///
    /// Uses the ingress-stored `from_peer` identity instead of re-resolving
    /// against live trust state, preserving snapshot semantics: a message
    /// accepted at ingress cannot disappear or change identity if the peer
    /// is removed/renamed before drain.
    ///
    /// Returns `None` for non-External items, PlainEvent, Ack messages, and
    /// incarnation-fenced messages. Fenced messages are authority-bearing and
    /// may only be consumed through the structured runtime interaction drain.
    pub(crate) fn from_classified_entry(
        entry: &crate::inbox::ClassifiedInboxEntry,
    ) -> Option<Self> {
        let envelope = match &entry.item {
            InboxItem::External { envelope } => envelope,
            InboxItem::PlainEvent { .. } => return None,
        };

        let from_peer = entry.from_peer.clone().unwrap_or_else(|| {
            InprocRegistry::global()
                .get_name_by_pubkey(&envelope.from)
                .unwrap_or_else(|| envelope.from.to_pubkey_string())
        });

        Self::from_external_with_resolved_peer(envelope, from_peer)
    }

    /// Format this message as text suitable for injection into an LLM session.
    ///
    /// The format is designed to be clear to the LLM about:
    /// - Who sent the message
    /// - What type of message it is
    /// - How to respond (for requests)
    pub fn to_user_message_text(&self) -> String {
        match &self.content {
            CommsContent::Message { body, blocks } => {
                let text = match blocks {
                    Some(b) if !b.is_empty() => meerkat_core::types::text_content(b),
                    _ => body.clone(),
                };
                meerkat_core::format_peer_message_projection(&self.from_peer, &text)
            }
            CommsContent::Request {
                request_id,
                intent,
                params,
                ..
            } => meerkat_core::format_peer_request_projection(
                self.from_pubkey.to_peer_id(),
                Some(&self.from_peer),
                request_id,
                intent.as_str(),
                params,
            ),
            CommsContent::Lifecycle { kind, params } => {
                // One-way lifecycle notices are control-plane notifications, not
                // correlated requests; render them as a plain peer notice so the
                // LLM is never told to `send_response`.
                let params_suffix = if params.is_null()
                    || matches!(params, JsonValue::Object(map) if map.is_empty())
                {
                    String::new()
                } else {
                    format!(" {}", serde_json::to_string(params).unwrap_or_default())
                };
                meerkat_core::format_peer_message_projection(
                    &self.from_peer,
                    &format!("[lifecycle: {}]{params_suffix}", kind.as_str()),
                )
            }
            CommsContent::Response {
                in_reply_to,
                status,
                result,
                blocks,
            } => {
                meerkat_core::format_peer_response_projection(
                    &self.from_peer,
                    in_reply_to,
                    match status {
                        CommsStatus::Accepted => meerkat_core::ResponseStatus::Accepted,
                        CommsStatus::Completed => meerkat_core::ResponseStatus::Completed,
                        CommsStatus::Failed => meerkat_core::ResponseStatus::Failed,
                    },
                    result,
                ) + &blocks
                    .as_ref()
                    .filter(|blocks| !blocks.is_empty())
                    .map(|blocks| format!("\n{}", meerkat_core::types::text_content(blocks)))
                    .unwrap_or_default()
            }
        }
    }
}

impl meerkat_core::TurnBoundaryMessage for CommsMessage {
    fn render_for_prompt(&self) -> Cow<'_, str> {
        Cow::Owned(self.to_user_message_text())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_comms_message_struct() {
        let msg = CommsMessage {
            envelope_id: Uuid::new_v4(),
            from_peer: "test-peer".to_string(),
            from_pubkey: PubKey::new([1u8; 32]),
            content: CommsContent::Message {
                body: "hello".to_string(),
                blocks: None,
            },
        };
        assert_eq!(msg.from_peer, "test-peer");
    }

    #[test]
    fn incarnation_fenced_envelope_is_not_session_injectable_comms_message() {
        let sender = crate::Keypair::generate();
        let mut envelope = crate::Envelope {
            id: Uuid::new_v4(),
            from: sender.public_key(),
            to: crate::Keypair::generate().public_key(),
            kind: MessageKind::IncarnationFencedMessage {
                body: "must remain fenced".to_string(),
                blocks: None,
                content_taint: None,
                handling_mode: None,
                objective_id: None,
                expected_recipient: meerkat_core::comms::PeerRecipientIncarnation {
                    mob_id: "mob".to_string(),
                    agent_identity: "worker".to_string(),
                    host_id: "host".to_string(),
                    binding_generation: 2,
                    member_session_id: "session".to_string(),
                    generation: 3,
                    fence_token: 5,
                },
            },
            sig: crate::Signature::new([0_u8; 64]),
        };
        envelope.sign(&sender);

        assert!(
            CommsMessage::from_external_with_resolved_peer(&envelope, "sender".to_string())
                .is_none(),
            "an authority-bearing fenced envelope must not flatten into the legacy session-injectable projection"
        );
    }

    #[test]
    fn test_comms_content_variants() {
        // Message
        let _ = CommsContent::Message {
            body: "hello".to_string(),
            blocks: None,
        };
        // Request
        let _ = CommsContent::Request {
            request_id: Uuid::new_v4(),
            intent: MessageIntent::Review,
            params: serde_json::json!({"pr": 42}),
            blocks: None,
        };
        // Response
        let _ = CommsContent::Response {
            in_reply_to: Uuid::new_v4(),
            status: CommsStatus::Completed,
            result: serde_json::json!({"approved": true}),
            blocks: None,
        };
    }

    #[test]
    fn test_comms_message_formatting() {
        let msg = CommsMessage {
            envelope_id: Uuid::new_v4(),
            from_peer: "review-agent".to_string(),
            from_pubkey: PubKey::new([1u8; 32]),
            content: CommsContent::Message {
                body: "Please review PR #42".to_string(),
                blocks: None,
            },
        };

        let text = msg.to_user_message_text();
        assert!(text.contains("Peer message from review-agent"));
        assert!(text.contains("review-agent"));
        assert!(text.contains("Please review PR #42"));
    }

    #[test]
    fn test_comms_message_formatting_request() {
        let request_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let msg = CommsMessage {
            envelope_id: request_id,
            from_peer: "coding-agent".to_string(),
            from_pubkey: PubKey::new([1u8; 32]),
            content: CommsContent::Request {
                request_id,
                intent: MessageIntent::Review,
                params: serde_json::json!({"pr": 42}),
                blocks: None,
            },
        };

        let text = msg.to_user_message_text();
        assert!(text.contains("Peer request from peer_id"));
        assert!(text.contains("coding-agent"));
        assert!(text.contains("review"));
        assert!(text.contains("550e8400-e29b-41d4-a716-446655440000"));
        assert!(text.contains("send_response"));
        assert!(text.contains(&format!(
            "\"peer_id\":\"{}\"",
            PubKey::new([1u8; 32]).to_peer_id()
        )));
        assert!(text.contains("\"display_name\":\"coding-agent\""));
        assert!(text.contains("\"in_reply_to\":\"550e8400-e29b-41d4-a716-446655440000\""));
        assert!(text.contains("\"status\":\"completed\""));
        assert!(text.contains("Do not answer this request with send_message"));
    }

    #[test]
    fn test_comms_message_formatting_response() {
        let in_reply_to = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let msg = CommsMessage {
            envelope_id: Uuid::new_v4(),
            from_peer: "review-agent".to_string(),
            from_pubkey: PubKey::new([1u8; 32]),
            content: CommsContent::Response {
                in_reply_to,
                status: CommsStatus::Completed,
                result: serde_json::json!({"approved": true}),
                blocks: None,
            },
        };

        let text = msg.to_user_message_text();
        assert!(text.contains("Peer response from review-agent"));
        assert!(text.contains("review-agent"));
        assert!(text.contains("completed"));
        assert!(text.contains("550e8400-e29b-41d4-a716-446655440000"));
    }

    // ========================================================================
    // MessageIntent type-safety tests
    // ========================================================================

    #[test]
    fn test_message_intent_standard_variants() {
        // All standard variants should serialize as snake_case strings
        assert_eq!(MessageIntent::Delegate.as_str(), "delegate");
        assert_eq!(MessageIntent::Status.as_str(), "status");
        assert_eq!(MessageIntent::Cancel.as_str(), "cancel");
        assert_eq!(MessageIntent::Ack.as_str(), "ack");
        assert_eq!(MessageIntent::Review.as_str(), "review");
        assert_eq!(MessageIntent::Calculate.as_str(), "calculate");
        assert_eq!(MessageIntent::Query.as_str(), "query");
    }

    #[test]
    fn test_message_intent_custom_variant() {
        let custom = MessageIntent::custom("my-custom-intent");
        assert_eq!(custom.as_str(), "my-custom-intent");
        assert_eq!(
            custom,
            MessageIntent::Custom("my-custom-intent".to_string())
        );
    }

    #[test]
    fn test_message_intent_from_string() {
        // Standard intents should map to enum variants
        assert_eq!(MessageIntent::from("delegate"), MessageIntent::Delegate);
        assert_eq!(MessageIntent::from("status"), MessageIntent::Status);
        assert_eq!(MessageIntent::from("cancel"), MessageIntent::Cancel);
        assert_eq!(MessageIntent::from("ack"), MessageIntent::Ack);
        assert_eq!(MessageIntent::from("review"), MessageIntent::Review);
        assert_eq!(MessageIntent::from("calculate"), MessageIntent::Calculate);
        assert_eq!(MessageIntent::from("query"), MessageIntent::Query);

        // Unknown strings should become Custom
        assert_eq!(
            MessageIntent::from("unknown-intent"),
            MessageIntent::Custom("unknown-intent".to_string())
        );
    }

    #[test]
    fn test_message_intent_display() {
        assert_eq!(format!("{}", MessageIntent::Review), "review");
        assert_eq!(
            format!("{}", MessageIntent::Custom("foo".to_string())),
            "foo"
        );
    }

    #[test]
    fn test_message_intent_serialization() {
        // Standard variants serialize to their string representations
        let json = serde_json::to_string(&MessageIntent::Delegate).unwrap();
        assert_eq!(json, "\"delegate\"");

        let json = serde_json::to_string(&MessageIntent::Review).unwrap();
        assert_eq!(json, "\"review\"");

        // Custom variant serializes to the inner string
        let json = serde_json::to_string(&MessageIntent::Custom("custom-op".to_string())).unwrap();
        assert_eq!(json, "\"custom-op\"");
    }

    #[test]
    fn test_message_intent_deserialization() {
        // Standard strings deserialize to enum variants
        let intent: MessageIntent = serde_json::from_str("\"delegate\"").unwrap();
        assert_eq!(intent, MessageIntent::Delegate);

        let intent: MessageIntent = serde_json::from_str("\"review\"").unwrap();
        assert_eq!(intent, MessageIntent::Review);

        // Unknown strings deserialize to Custom
        let intent: MessageIntent = serde_json::from_str("\"my-custom\"").unwrap();
        assert_eq!(intent, MessageIntent::Custom("my-custom".to_string()));
    }
}
