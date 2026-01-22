//! Core message types for Meerkat comms.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::identity::{Keypair, PubKey, Signature};

/// Response status for Request messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Compute the canonical CBOR bytes to sign: [id, from, to, kind]
    /// Field order is fixed per spec for cross-implementation compatibility.
    pub fn signable_bytes(&self) -> Vec<u8> {
        // Create a tuple of (id, from, to, kind) for canonical encoding
        let signable = (&self.id, &self.from, &self.to, &self.kind);
        let mut buf = Vec::new();
        ciborium::into_writer(&signable, &mut buf).expect("CBOR serialization failed");
        buf
    }

    /// Sign the envelope with the given keypair, updating the sig field.
    pub fn sign(&mut self, keypair: &Keypair) {
        let bytes = self.signable_bytes();
        self.sig = keypair.sign(&bytes);
    }

    /// Verify the signature using the sender's public key.
    pub fn verify(&self) -> bool {
        let bytes = self.signable_bytes();
        self.from.verify(&bytes, &self.sig)
    }
}

/// An item in the agent's inbox.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InboxItem {
    /// A message received from an external peer.
    External { envelope: Envelope },
    /// A result from a completed subagent.
    SubagentResult {
        subagent_id: Uuid,
        result: JsonValue,
        summary: String,
    },
}

#[cfg(test)]
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
            "Status should encode as string 'accepted', got: {:?}",
            buf
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
            "MessageKind should use string tags, got: {:?}",
            buf
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
}
