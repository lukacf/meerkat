//! Core message types for Meerkat comms.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::identity::{Keypair, PubKey, Signature};
use ciborium::value::{CanonicalValue, Value};

/// Response status for Request messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Accepted,
    Completed,
    Failed,
}

/// The kind of message being sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MessageKind {
    /// A simple text message.
    Message { body: String },
    /// A request for the peer to perform an action.
    Request { intent: String, params: JsonValue },
    /// A response to a previous request.
    Response {
        in_reply_to: Uuid,
        status: Status,
        result: JsonValue,
    },
    /// Acknowledgment of message receipt.
    Ack { in_reply_to: Uuid },
}

/// A signed message envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    /// Unique message identifier.
    pub id: Uuid,
    /// Sender's public key.
    pub from: PubKey,
    /// Recipient's public key.
    pub to: PubKey,
    /// The message content.
    pub kind: MessageKind,
    /// Ed25519 signature over canonical CBOR of [id, from, to, kind].
    pub sig: Signature,
}

impl Envelope {
    /// Compute canonical CBOR bytes to sign: `[id, from, to, kind]`.
    ///
    /// Field order is fixed per spec for cross-implementation compatibility.
    ///
    /// Note: `ciborium`'s serde `into_writer` does **not** sort map keys. To ensure
    /// deterministic (RFC 8949) encoding, we serialize into `ciborium::value::Value`,
    /// then recursively sort all maps by canonical key order before encoding.
    ///
    /// `PubKey` is encoded as a CBOR byte string (major type 2), not an array.
    pub fn signable_bytes(&self) -> Vec<u8> {
        fn canonicalize(value: &mut Value) {
            match value {
                Value::Array(items) => {
                    for item in items {
                        canonicalize(item);
                    }
                }
                Value::Map(entries) => {
                    for (key, val) in entries.iter_mut() {
                        canonicalize(key);
                        canonicalize(val);
                    }

                    entries.sort_by(|(k1, _), (k2, _)| match (k1, k2) {
                        (Value::Text(a), Value::Text(b)) => match a.len().cmp(&b.len()) {
                            std::cmp::Ordering::Equal => a.cmp(b),
                            ord => ord,
                        },
                        _ => {
                            CanonicalValue::from(k1.clone()).cmp(&CanonicalValue::from(k2.clone()))
                        }
                    });
                }
                Value::Tag(_, inner) => canonicalize(inner),
                _ => {}
            }
        }

        // Create a tuple of (id, from, to, kind) for signing.
        let signable = (&self.id, &self.from, &self.to, &self.kind);

        let mut value = match Value::serialized(&signable) {
            Ok(value) => value,
            Err(_) => return Vec::new(),
        };
        canonicalize(&mut value);

        let mut buf = Vec::new();
        if ciborium::into_writer(&value, &mut buf).is_err() {
            return Vec::new();
        }
        buf
    }

    /// Sign the envelope with the given keypair, updating the sig field.
    pub fn sign(&mut self, keypair: &Keypair) {
        let bytes = self.signable_bytes();
        if bytes.is_empty() {
            self.sig = Signature::new([0u8; 64]);
            return;
        }
        self.sig = keypair.sign(&bytes);
    }

    /// Verify the signature using the sender's public key.
    pub fn verify(&self) -> bool {
        let bytes = self.signable_bytes();
        if bytes.is_empty() {
            return false;
        }
        self.from.verify(&bytes, &self.sig)
    }
}

/// An item in the agent's inbox.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InboxItem {
    /// A message received from an external peer (signed CBOR envelope).
    External { envelope: Envelope },
    /// A result from a completed subagent.
    SubagentResult {
        subagent_id: Uuid,
        result: JsonValue,
        summary: String,
    },
    /// A plain-text event from an external (unauthenticated) source.
    PlainEvent {
        body: String,
        source: meerkat_core::PlainEventSource,
        /// Optional interaction ID for subscription correlation.
        /// Set by `inject_with_subscription`, `None` for untracked events.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interaction_id: Option<Uuid>,
    },
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_status_variants() {
        // Verify all 3 variants exist
        let _ = Status::Accepted;
        let _ = Status::Completed;
        let _ = Status::Failed;
    }

    #[test]
    fn test_message_kind_message_fields() {
        let msg = MessageKind::Message {
            body: "hello".to_string(),
        };
        if let MessageKind::Message { body } = msg {
            assert_eq!(body, "hello");
        } else {
            panic!("Expected Message variant");
        }
    }

    #[test]
    fn test_message_kind_request_fields() {
        let req = MessageKind::Request {
            intent: "review-pr".to_string(),
            params: serde_json::json!({"pr": 42}),
        };
        if let MessageKind::Request { intent, params } = req {
            assert_eq!(intent, "review-pr");
            assert_eq!(params["pr"], 42);
        } else {
            panic!("Expected Request variant");
        }
    }

    #[test]
    fn test_message_kind_response_fields() {
        let id = Uuid::new_v4();
        let resp = MessageKind::Response {
            in_reply_to: id,
            status: Status::Completed,
            result: serde_json::json!({"approved": true}),
        };
        if let MessageKind::Response {
            in_reply_to,
            status,
            result,
        } = resp
        {
            assert_eq!(in_reply_to, id);
            assert_eq!(status, Status::Completed);
            assert_eq!(result["approved"], true);
        } else {
            panic!("Expected Response variant");
        }
    }

    #[test]
    fn test_message_kind_ack_fields() {
        let id = Uuid::new_v4();
        let ack = MessageKind::Ack { in_reply_to: id };
        if let MessageKind::Ack { in_reply_to } = ack {
            assert_eq!(in_reply_to, id);
        } else {
            panic!("Expected Ack variant");
        }
    }

    #[test]
    fn test_envelope_fields() {
        let envelope = Envelope {
            id: Uuid::new_v4(),
            from: PubKey::new([1u8; 32]),
            to: PubKey::new([2u8; 32]),
            kind: MessageKind::Message {
                body: "test".to_string(),
            },
            sig: Signature::new([0u8; 64]),
        };
        assert!(envelope.id != Uuid::nil());
        assert_eq!(envelope.from.as_bytes()[0], 1);
        assert_eq!(envelope.to.as_bytes()[0], 2);
    }

    #[test]
    fn test_status_cbor_roundtrip() {
        for status in [Status::Accepted, Status::Completed, Status::Failed] {
            let mut buf = Vec::new();
            ciborium::into_writer(&status, &mut buf).unwrap();
            let decoded: Status = ciborium::from_reader(&buf[..]).unwrap();
            assert_eq!(status, decoded);
        }
    }

    #[test]
    fn test_message_kind_cbor_roundtrip() {
        let kinds = vec![
            MessageKind::Message {
                body: "hello".to_string(),
            },
            MessageKind::Request {
                intent: "test".to_string(),
                params: serde_json::json!({}),
            },
            MessageKind::Response {
                in_reply_to: Uuid::new_v4(),
                status: Status::Completed,
                result: serde_json::json!(null),
            },
            MessageKind::Ack {
                in_reply_to: Uuid::new_v4(),
            },
        ];
        for kind in kinds {
            let mut buf = Vec::new();
            ciborium::into_writer(&kind, &mut buf).unwrap();
            let decoded: MessageKind = ciborium::from_reader(&buf[..]).unwrap();
            assert_eq!(kind, decoded);
        }
    }

    #[test]
    fn test_envelope_cbor_roundtrip() {
        let envelope = Envelope {
            id: Uuid::new_v4(),
            from: PubKey::new([1u8; 32]),
            to: PubKey::new([2u8; 32]),
            kind: MessageKind::Message {
                body: "test".to_string(),
            },
            sig: Signature::new([0u8; 64]),
        };
        let mut buf = Vec::new();
        ciborium::into_writer(&envelope, &mut buf).unwrap();
        let decoded: Envelope = ciborium::from_reader(&buf[..]).unwrap();
        assert_eq!(envelope, decoded);
    }

    #[test]
    fn test_inbox_item_external_fields() {
        let envelope = Envelope {
            id: Uuid::new_v4(),
            from: PubKey::new([1u8; 32]),
            to: PubKey::new([2u8; 32]),
            kind: MessageKind::Ack {
                in_reply_to: Uuid::new_v4(),
            },
            sig: Signature::new([0u8; 64]),
        };
        let item = InboxItem::External {
            envelope: envelope.clone(),
        };
        if let InboxItem::External { envelope: e } = item {
            assert_eq!(e.id, envelope.id);
        } else {
            panic!("Expected External variant");
        }
    }

    #[test]
    fn test_inbox_item_subagent_fields() {
        let item = InboxItem::SubagentResult {
            subagent_id: Uuid::new_v4(),
            result: serde_json::json!({"done": true}),
            summary: "Task completed".to_string(),
        };
        if let InboxItem::SubagentResult {
            subagent_id,
            result,
            summary,
        } = item
        {
            assert!(subagent_id != Uuid::nil());
            assert_eq!(result["done"], true);
            assert_eq!(summary, "Task completed");
        } else {
            panic!("Expected SubagentResult variant");
        }
    }

    #[test]
    fn test_inbox_item_cbor_roundtrip() {
        let items = vec![
            InboxItem::External {
                envelope: Envelope {
                    id: Uuid::new_v4(),
                    from: PubKey::new([1u8; 32]),
                    to: PubKey::new([2u8; 32]),
                    kind: MessageKind::Ack {
                        in_reply_to: Uuid::new_v4(),
                    },
                    sig: Signature::new([0u8; 64]),
                },
            },
            InboxItem::SubagentResult {
                subagent_id: Uuid::new_v4(),
                result: serde_json::json!({}),
                summary: "done".to_string(),
            },
        ];
        for item in items {
            let mut buf = Vec::new();
            ciborium::into_writer(&item, &mut buf).unwrap();
            let decoded: InboxItem = ciborium::from_reader(&buf[..]).unwrap();
            assert_eq!(item, decoded);
        }
    }

    #[test]
    fn test_status_encodes_as_string() {
        // Verify Status encodes as string, not ordinal
        let status = Status::Accepted;
        let mut buf = Vec::new();
        ciborium::into_writer(&status, &mut buf).unwrap();
        // CBOR text string starts with 0x60-0x7f for short strings
        // "accepted" is 8 chars, so it should be 0x68 followed by "accepted"
        let cbor_str = String::from_utf8_lossy(&buf);
        assert!(
            cbor_str.contains("accepted"),
            "Status should encode as string 'accepted', got: {buf:?}"
        );
    }

    #[test]
    fn test_message_kind_tags_are_strings() {
        // Verify MessageKind uses string tags, not ordinals
        let msg = MessageKind::Message {
            body: "test".to_string(),
        };
        let mut buf = Vec::new();
        ciborium::into_writer(&msg, &mut buf).unwrap();
        let cbor_str = String::from_utf8_lossy(&buf);
        // Should contain "type" and "message" as strings
        assert!(
            cbor_str.contains("type") && cbor_str.contains("message"),
            "MessageKind should use string tags, got: {buf:?}"
        );
    }

    // Phase 1: Envelope signing tests

    #[test]
    fn test_signable_bytes_deterministic() {
        let envelope = Envelope {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            from: PubKey::new([1u8; 32]),
            to: PubKey::new([2u8; 32]),
            kind: MessageKind::Message {
                body: "test".to_string(),
            },
            sig: Signature::new([0u8; 64]),
        };
        let bytes1 = envelope.signable_bytes();
        let bytes2 = envelope.signable_bytes();
        assert_eq!(bytes1, bytes2, "signable_bytes must be deterministic");
    }

    #[test]
    fn test_envelope_sign() {
        use crate::identity::Keypair;

        let keypair = Keypair::generate();
        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: keypair.public_key(),
            to: PubKey::new([2u8; 32]),
            kind: MessageKind::Message {
                body: "test".to_string(),
            },
            sig: Signature::new([0u8; 64]),
        };
        envelope.sign(&keypair);
        // Verify sig field is no longer all zeros
        assert_ne!(envelope.sig, Signature::new([0u8; 64]));
    }

    #[test]
    fn test_envelope_verify() {
        use crate::identity::Keypair;

        let keypair = Keypair::generate();
        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: keypair.public_key(),
            to: PubKey::new([2u8; 32]),
            kind: MessageKind::Message {
                body: "test".to_string(),
            },
            sig: Signature::new([0u8; 64]),
        };
        envelope.sign(&keypair);
        assert!(envelope.verify(), "signed envelope should verify");
    }

    // === PlainEvent tests ===

    #[test]
    fn test_inbox_item_plain_event_serde_roundtrip() {
        use meerkat_core::PlainEventSource;

        let item = InboxItem::PlainEvent {
            body: "New email from john@example.com".to_string(),
            source: PlainEventSource::Tcp,
            interaction_id: None,
        };

        // JSON round-trip
        let json = serde_json::to_string(&item).unwrap();
        let parsed: InboxItem = serde_json::from_str(&json).unwrap();
        assert_eq!(item, parsed);

        // Verify tag
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["type"], "plain_event");
        assert_eq!(value["body"], "New email from john@example.com");
        assert_eq!(value["source"], "tcp");
    }

    #[test]
    fn test_inbox_item_plain_event_cbor_roundtrip() {
        use meerkat_core::PlainEventSource;

        let items = vec![
            InboxItem::PlainEvent {
                body: "hello".to_string(),
                source: PlainEventSource::Tcp,
                interaction_id: None,
            },
            InboxItem::PlainEvent {
                body: r#"{"event":"email"}"#.to_string(),
                source: PlainEventSource::Stdin,
                interaction_id: None,
            },
            InboxItem::PlainEvent {
                body: "webhook payload".to_string(),
                source: PlainEventSource::Webhook,
                interaction_id: None,
            },
            InboxItem::PlainEvent {
                body: "rpc event".to_string(),
                source: PlainEventSource::Rpc,
                interaction_id: None,
            },
        ];
        for item in items {
            let mut buf = Vec::new();
            ciborium::into_writer(&item, &mut buf).unwrap();
            let decoded: InboxItem = ciborium::from_reader(&buf[..]).unwrap();
            assert_eq!(item, decoded);
        }
    }

    #[test]
    fn test_inbox_item_plain_event_backward_compat_json() {
        // Old format (no interaction_id) should deserialize with interaction_id: None
        let old_json = r#"{"type":"plain_event","body":"hello","source":"tcp"}"#;
        let parsed: InboxItem = serde_json::from_str(old_json).unwrap();
        match parsed {
            InboxItem::PlainEvent {
                body,
                source,
                interaction_id,
            } => {
                assert_eq!(body, "hello");
                assert_eq!(source, meerkat_core::PlainEventSource::Tcp);
                assert_eq!(interaction_id, None);
            }
            other => panic!("Expected PlainEvent, got {other:?}"),
        }
    }

    #[test]
    fn test_inbox_item_plain_event_with_interaction_id_json_roundtrip() {
        let id = Uuid::new_v4();
        let item = InboxItem::PlainEvent {
            body: "tracked event".to_string(),
            source: meerkat_core::PlainEventSource::Rpc,
            interaction_id: Some(id),
        };

        let json = serde_json::to_string(&item).unwrap();
        let parsed: InboxItem = serde_json::from_str(&json).unwrap();
        assert_eq!(item, parsed);

        // Verify interaction_id is present in JSON
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["interaction_id"], id.to_string());
    }

    #[test]
    fn test_inbox_item_plain_event_with_interaction_id_cbor_roundtrip() {
        let id = Uuid::new_v4();
        let item = InboxItem::PlainEvent {
            body: "tracked event".to_string(),
            source: meerkat_core::PlainEventSource::Rpc,
            interaction_id: Some(id),
        };

        let mut buf = Vec::new();
        ciborium::into_writer(&item, &mut buf).unwrap();
        let decoded: InboxItem = ciborium::from_reader(&buf[..]).unwrap();
        assert_eq!(item, decoded);
    }

    #[test]
    fn test_inbox_item_plain_event_none_interaction_id_skipped_in_json() {
        let item = InboxItem::PlainEvent {
            body: "untracked".to_string(),
            source: meerkat_core::PlainEventSource::Tcp,
            interaction_id: None,
        };

        let json = serde_json::to_string(&item).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        // interaction_id should not be present (skip_serializing_if = "Option::is_none")
        assert!(
            value.get("interaction_id").is_none(),
            "interaction_id: None should not be serialized"
        );
    }
}
