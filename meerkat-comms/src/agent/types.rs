//! CommsMessage types for session-injectable messages.
//!
//! These types bridge the gap between raw `InboxItem` envelopes and
//! the format needed for injection into an agent's session.

use crate::{InboxItem, MessageKind, PubKey, TrustedPeers};
use meerkat_core::PlainEventSource;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::borrow::Cow;
use uuid::Uuid;

/// A plain (unauthenticated) event message from an external source.
#[derive(Debug, Clone, PartialEq)]
pub struct PlainMessage {
    /// The event body (plain text or serialized JSON).
    pub body: String,
    /// Where the event originated from.
    pub source: PlainEventSource,
    /// Optional interaction id for subscriber correlation.
    pub interaction_id: Option<uuid::Uuid>,
}

impl PlainMessage {
    /// Format this message for injection into an LLM session.
    pub fn to_user_message_text(&self) -> String {
        format!("[EVENT via {}] {}", self.source, self.body)
    }
}

/// A message drained from the inbox, distinguishing authenticated peer
/// messages from unauthenticated external events.
///
/// The compiler enforces this distinction — authenticated messages have
/// verified sender identity (`CommsMessage`), plain events do not (`PlainMessage`).
#[derive(Debug, Clone)]
pub enum DrainedMessage {
    /// A message from an authenticated peer (Ed25519-verified envelope).
    Authenticated(CommsMessage),
    /// A plain event from an external (unauthenticated) source.
    Plain(PlainMessage),
}

/// Convert an inbox item into a `DrainedMessage`.
///
/// Returns `None` for:
/// - `SubagentResult` items (handled separately)
/// - Ack messages (not injected into session)
/// - Unknown peers (for authenticated messages)
pub fn drain_inbox_item(item: &InboxItem, trusted_peers: &TrustedPeers) -> Option<DrainedMessage> {
    match item {
        InboxItem::External { .. } => {
            CommsMessage::from_inbox_item(item, trusted_peers).map(DrainedMessage::Authenticated)
        }
        InboxItem::PlainEvent {
            body,
            source,
            interaction_id,
        } => Some(DrainedMessage::Plain(PlainMessage {
            body: body.clone(),
            source: *source,
            interaction_id: *interaction_id,
        })),
        InboxItem::SubagentResult { .. } => None,
    }
}

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
    },
    /// A request from a peer asking this agent to perform an action.
    Request {
        /// The request ID (for send_response).
        request_id: Uuid,
        /// The intent/action being requested (now type-safe).
        intent: MessageIntent,
        /// Parameters for the request.
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
    /// The peer name (resolved from pubkey via TrustedPeers).
    pub from_peer: String,
    /// The peer's public key (for explicit reference).
    pub from_pubkey: PubKey,
    /// The message content.
    pub content: CommsContent,
}

impl CommsMessage {
    /// Create a CommsMessage from an InboxItem and trusted peers list.
    ///
    /// Returns `None` if:
    /// - The item is not an External envelope
    /// - The sender is not in trusted_peers (unknown peer)
    /// - The message kind is Ack (acks are not injected into session)
    pub fn from_inbox_item(item: &InboxItem, trusted_peers: &TrustedPeers) -> Option<Self> {
        let envelope = match item {
            InboxItem::External { envelope } => envelope,
            InboxItem::SubagentResult { .. } | InboxItem::PlainEvent { .. } => return None,
        };

        // Resolve peer name from pubkey
        let peer = trusted_peers.get_peer(&envelope.from)?;
        let from_peer = peer.name.clone();

        // Convert MessageKind to CommsContent
        let content = match &envelope.kind {
            MessageKind::Message { body } => CommsContent::Message { body: body.clone() },
            MessageKind::Request { intent, params } => CommsContent::Request {
                request_id: envelope.id,
                intent: MessageIntent::from(intent.as_str()),
                params: params.clone(),
            },
            MessageKind::Response {
                in_reply_to,
                status,
                result,
            } => CommsContent::Response {
                in_reply_to: *in_reply_to,
                status: (*status).into(),
                result: result.clone(),
            },
            MessageKind::Ack { .. } => return None, // Don't inject acks
        };

        Some(CommsMessage {
            envelope_id: envelope.id,
            from_peer,
            from_pubkey: envelope.from,
            content,
        })
    }

    /// Format this message as text suitable for injection into an LLM session.
    ///
    /// The format is designed to be clear to the LLM about:
    /// - Who sent the message
    /// - What type of message it is
    /// - How to respond (for requests)
    pub fn to_user_message_text(&self) -> String {
        match &self.content {
            CommsContent::Message { body } => {
                format!("[COMMS MESSAGE from {}]\n{}", self.from_peer, body)
            }
            CommsContent::Request {
                request_id,
                intent,
                params,
            } => {
                let params_str =
                    if params.is_null() || params == &JsonValue::Object(Default::default()) {
                        String::new()
                    } else {
                        format!(
                            "\nParams: {}",
                            serde_json::to_string_pretty(params).unwrap_or_default()
                        )
                    };
                format!(
                    "[COMMS REQUEST from {} (id: {})]\n\
                     Intent: {}{}\n\
                     \n\
                     To respond, use send_response with peer=\"{}\", request_id=\"{}\"",
                    self.from_peer, request_id, intent, params_str, self.from_peer, request_id
                )
            }
            CommsContent::Response {
                in_reply_to,
                status,
                result,
            } => {
                let status_str = match status {
                    CommsStatus::Accepted => "accepted",
                    CommsStatus::Completed => "completed",
                    CommsStatus::Failed => "failed",
                };
                let result_str =
                    if result.is_null() || result == &JsonValue::Object(Default::default()) {
                        String::new()
                    } else {
                        format!(
                            "\nResult: {}",
                            serde_json::to_string_pretty(result).unwrap_or_default()
                        )
                    };
                format!(
                    "[COMMS RESPONSE from {} (to request: {})]\n\
                     Status: {}{}",
                    self.from_peer, in_reply_to, status_str, result_str
                )
            }
        }
    }
}

impl meerkat_core::TurnBoundaryMessage for CommsMessage {
    fn render_for_prompt(&self) -> Cow<'_, str> {
        Cow::Owned(self.to_user_message_text())
    }
}

/// Create a CommsMessage from an Envelope directly (for testing).
#[cfg(test)]
impl CommsMessage {
    pub fn from_envelope(envelope: &crate::Envelope, trusted_peers: &TrustedPeers) -> Option<Self> {
        Self::from_inbox_item(
            &InboxItem::External {
                envelope: envelope.clone(),
            },
            trusted_peers,
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::{Envelope, Keypair, Signature, TrustedPeer};

    fn make_keypair() -> Keypair {
        Keypair::generate()
    }

    fn make_trusted_peers(name: &str, pubkey: &PubKey) -> TrustedPeers {
        TrustedPeers {
            peers: vec![TrustedPeer {
                name: name.to_string(),
                pubkey: *pubkey,
                addr: "tcp://127.0.0.1:4200".to_string(),
            }],
        }
    }

    fn make_envelope(from: &Keypair, to: PubKey, kind: MessageKind) -> Envelope {
        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: from.public_key(),
            to,
            kind,
            sig: Signature::new([0u8; 64]),
        };
        envelope.sign(from);
        envelope
    }

    #[test]
    fn test_comms_message_struct() {
        let msg = CommsMessage {
            envelope_id: Uuid::new_v4(),
            from_peer: "test-peer".to_string(),
            from_pubkey: PubKey::new([1u8; 32]),
            content: CommsContent::Message {
                body: "hello".to_string(),
            },
        };
        assert_eq!(msg.from_peer, "test-peer");
    }

    #[test]
    fn test_comms_content_variants() {
        // Message
        let _ = CommsContent::Message {
            body: "hello".to_string(),
        };
        // Request
        let _ = CommsContent::Request {
            request_id: Uuid::new_v4(),
            intent: MessageIntent::Review,
            params: serde_json::json!({"pr": 42}),
        };
        // Response
        let _ = CommsContent::Response {
            in_reply_to: Uuid::new_v4(),
            status: CommsStatus::Completed,
            result: serde_json::json!({"approved": true}),
        };
    }

    #[test]
    fn test_comms_message_from_inbox_item() {
        let sender = make_keypair();
        let receiver = make_keypair();
        let trusted = make_trusted_peers("sender-agent", &sender.public_key());

        let envelope = make_envelope(
            &sender,
            receiver.public_key(),
            MessageKind::Message {
                body: "hello world".to_string(),
            },
        );

        let item = InboxItem::External { envelope };
        let msg = CommsMessage::from_inbox_item(&item, &trusted);

        assert!(msg.is_some());
        let msg = msg.unwrap();
        assert_eq!(msg.from_peer, "sender-agent");
        match msg.content {
            CommsContent::Message { body } => assert_eq!(body, "hello world"),
            _ => unreachable!("expected Message"),
        }
    }

    #[test]
    fn test_comms_message_from_inbox_item_request() {
        let sender = make_keypair();
        let receiver = make_keypair();
        let trusted = make_trusted_peers("requester", &sender.public_key());

        // Use standard "review" intent to test enum mapping
        let envelope = make_envelope(
            &sender,
            receiver.public_key(),
            MessageKind::Request {
                intent: "review".to_string(),
                params: serde_json::json!({"pr": 123}),
            },
        );
        let request_id = envelope.id;

        let item = InboxItem::External { envelope };
        let msg = CommsMessage::from_inbox_item(&item, &trusted).unwrap();

        match msg.content {
            CommsContent::Request {
                request_id: rid,
                intent,
                params,
            } => {
                assert_eq!(rid, request_id);
                assert_eq!(intent, MessageIntent::Review);
                assert_eq!(params["pr"], 123);
            }
            _ => unreachable!("expected Request"),
        }
    }

    #[test]
    fn test_comms_message_from_inbox_item_request_custom_intent() {
        let sender = make_keypair();
        let receiver = make_keypair();
        let trusted = make_trusted_peers("requester", &sender.public_key());

        // Custom intent should be preserved as Custom variant
        let envelope = make_envelope(
            &sender,
            receiver.public_key(),
            MessageKind::Request {
                intent: "review-pr".to_string(),
                params: serde_json::json!({"pr": 456}),
            },
        );

        let item = InboxItem::External { envelope };
        let msg = CommsMessage::from_inbox_item(&item, &trusted).unwrap();

        match msg.content {
            CommsContent::Request { intent, params, .. } => {
                assert_eq!(intent, MessageIntent::Custom("review-pr".to_string()));
                assert_eq!(params["pr"], 456);
            }
            _ => unreachable!("expected Request"),
        }
    }

    #[test]
    fn test_comms_message_from_inbox_item_response() {
        let sender = make_keypair();
        let receiver = make_keypair();
        let trusted = make_trusted_peers("responder", &sender.public_key());

        let orig_request_id = Uuid::new_v4();
        let envelope = make_envelope(
            &sender,
            receiver.public_key(),
            MessageKind::Response {
                in_reply_to: orig_request_id,
                status: crate::Status::Completed,
                result: serde_json::json!({"approved": true}),
            },
        );

        let item = InboxItem::External { envelope };
        let msg = CommsMessage::from_inbox_item(&item, &trusted).unwrap();

        match msg.content {
            CommsContent::Response {
                in_reply_to,
                status,
                result,
            } => {
                assert_eq!(in_reply_to, orig_request_id);
                assert_eq!(status, CommsStatus::Completed);
                assert_eq!(result["approved"], true);
            }
            _ => unreachable!("expected Response"),
        }
    }

    #[test]
    fn test_comms_message_from_inbox_item_ignores_ack() {
        let sender = make_keypair();
        let receiver = make_keypair();
        let trusted = make_trusted_peers("sender", &sender.public_key());

        let envelope = make_envelope(
            &sender,
            receiver.public_key(),
            MessageKind::Ack {
                in_reply_to: Uuid::new_v4(),
            },
        );

        let item = InboxItem::External { envelope };
        let msg = CommsMessage::from_inbox_item(&item, &trusted);

        assert!(
            msg.is_none(),
            "Acks should not be converted to CommsMessage"
        );
    }

    #[test]
    fn test_comms_message_from_inbox_item_ignores_unknown_peer() {
        let sender = make_keypair();
        let other = make_keypair();
        let receiver = make_keypair();
        let trusted = make_trusted_peers("other", &other.public_key()); // sender NOT trusted

        let envelope = make_envelope(
            &sender,
            receiver.public_key(),
            MessageKind::Message {
                body: "hello".to_string(),
            },
        );

        let item = InboxItem::External { envelope };
        let msg = CommsMessage::from_inbox_item(&item, &trusted);

        assert!(msg.is_none(), "Unknown peers should return None");
    }

    #[test]
    fn test_comms_message_from_inbox_item_ignores_subagent_result() {
        let trusted = TrustedPeers::new();

        let item = InboxItem::SubagentResult {
            subagent_id: Uuid::new_v4(),
            result: serde_json::json!({"done": true}),
            summary: "Task completed".to_string(),
        };

        let msg = CommsMessage::from_inbox_item(&item, &trusted);
        assert!(msg.is_none(), "SubagentResult should return None");
    }

    #[test]
    fn test_comms_message_formatting() {
        let msg = CommsMessage {
            envelope_id: Uuid::new_v4(),
            from_peer: "review-agent".to_string(),
            from_pubkey: PubKey::new([1u8; 32]),
            content: CommsContent::Message {
                body: "Please review PR #42".to_string(),
            },
        };

        let text = msg.to_user_message_text();
        assert!(text.contains("COMMS MESSAGE"));
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
            },
        };

        let text = msg.to_user_message_text();
        assert!(text.contains("COMMS REQUEST"));
        assert!(text.contains("coding-agent"));
        assert!(text.contains("review"));
        assert!(text.contains("550e8400-e29b-41d4-a716-446655440000"));
        assert!(text.contains("send_response"));
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
            },
        };

        let text = msg.to_user_message_text();
        assert!(text.contains("COMMS RESPONSE"));
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

    // === DrainedMessage / PlainMessage tests ===

    #[test]
    fn test_drained_message_from_external() {
        let sender = make_keypair();
        let receiver = make_keypair();
        let trusted = make_trusted_peers("sender-agent", &sender.public_key());

        let envelope = make_envelope(
            &sender,
            receiver.public_key(),
            MessageKind::Message {
                body: "hello".to_string(),
            },
        );
        let item = InboxItem::External { envelope };
        let drained = drain_inbox_item(&item, &trusted);

        assert!(drained.is_some());
        match drained.unwrap() {
            DrainedMessage::Authenticated(msg) => {
                assert_eq!(msg.from_peer, "sender-agent");
            }
            DrainedMessage::Plain(_) => panic!("Expected Authenticated"),
        }
    }

    #[test]
    fn test_drained_message_from_plain_event() {
        use meerkat_core::PlainEventSource;

        let trusted = TrustedPeers::new();
        let item = InboxItem::PlainEvent {
            body: "New email arrived".to_string(),
            source: PlainEventSource::Tcp,
            interaction_id: None,
        };
        let drained = drain_inbox_item(&item, &trusted);

        assert!(drained.is_some());
        match drained.unwrap() {
            DrainedMessage::Plain(msg) => {
                assert_eq!(msg.body, "New email arrived");
                assert_eq!(msg.source, PlainEventSource::Tcp);
                assert_eq!(msg.interaction_id, None);
            }
            DrainedMessage::Authenticated(_) => panic!("Expected Plain"),
        }
    }

    #[test]
    fn test_plain_message_to_user_message_text() {
        use meerkat_core::PlainEventSource;

        let msg = PlainMessage {
            body: "CPU > 95% on prod-3".to_string(),
            source: PlainEventSource::Webhook,
            interaction_id: None,
        };
        let text = msg.to_user_message_text();
        assert_eq!(text, "[EVENT via webhook] CPU > 95% on prod-3");
    }

    #[test]
    fn test_plain_message_formatting_all_sources() {
        use meerkat_core::PlainEventSource;

        for (source, label) in [
            (PlainEventSource::Tcp, "tcp"),
            (PlainEventSource::Uds, "uds"),
            (PlainEventSource::Stdin, "stdin"),
            (PlainEventSource::Webhook, "webhook"),
            (PlainEventSource::Rpc, "rpc"),
        ] {
            let msg = PlainMessage {
                body: "test".to_string(),
                source,
                interaction_id: None,
            };
            assert!(
                msg.to_user_message_text().contains(label),
                "PlainMessage should contain source label '{}'",
                label
            );
        }
    }
}
