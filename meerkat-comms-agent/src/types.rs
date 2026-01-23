//! CommsMessage types for session-injectable messages.
//!
//! These types bridge the gap between raw `InboxItem` envelopes and
//! the format needed for injection into an agent's session.

use meerkat_comms::{InboxItem, MessageKind, PubKey, TrustedPeers};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

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
        /// The intent/action being requested.
        intent: String,
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

impl From<meerkat_comms::Status> for CommsStatus {
    fn from(s: meerkat_comms::Status) -> Self {
        match s {
            meerkat_comms::Status::Accepted => CommsStatus::Accepted,
            meerkat_comms::Status::Completed => CommsStatus::Completed,
            meerkat_comms::Status::Failed => CommsStatus::Failed,
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
            InboxItem::SubagentResult { .. } => return None,
        };

        // Resolve peer name from pubkey
        let peer = trusted_peers.get_peer(&envelope.from)?;
        let from_peer = peer.name.clone();

        // Convert MessageKind to CommsContent
        let content = match &envelope.kind {
            MessageKind::Message { body } => CommsContent::Message { body: body.clone() },
            MessageKind::Request { intent, params } => CommsContent::Request {
                request_id: envelope.id,
                intent: intent.clone(),
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

/// Create a CommsMessage from an Envelope directly (for testing).
#[cfg(test)]
impl CommsMessage {
    pub fn from_envelope(
        envelope: &meerkat_comms::Envelope,
        trusted_peers: &TrustedPeers,
    ) -> Option<Self> {
        Self::from_inbox_item(
            &InboxItem::External {
                envelope: envelope.clone(),
            },
            trusted_peers,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_comms::{Envelope, Keypair, Signature, TrustedPeer};

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
            intent: "review-pr".to_string(),
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
            _ => panic!("expected Message"),
        }
    }

    #[test]
    fn test_comms_message_from_inbox_item_request() {
        let sender = make_keypair();
        let receiver = make_keypair();
        let trusted = make_trusted_peers("requester", &sender.public_key());

        let envelope = make_envelope(
            &sender,
            receiver.public_key(),
            MessageKind::Request {
                intent: "review-pr".to_string(),
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
                assert_eq!(intent, "review-pr");
                assert_eq!(params["pr"], 123);
            }
            _ => panic!("expected Request"),
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
                status: meerkat_comms::Status::Completed,
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
            _ => panic!("expected Response"),
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
                intent: "review-pr".to_string(),
                params: serde_json::json!({"pr": 42}),
            },
        };

        let text = msg.to_user_message_text();
        assert!(text.contains("COMMS REQUEST"));
        assert!(text.contains("coding-agent"));
        assert!(text.contains("review-pr"));
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
}
