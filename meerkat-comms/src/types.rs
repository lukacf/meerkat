//! Core message types for Meerkat comms.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::identity::{Keypair, PubKey, Signature};
use ciborium::value::{CanonicalValue, Value};
use meerkat_core::comms::PeerLifecycleKind;
pub use meerkat_core::comms::SenderContentTaint;
use meerkat_core::types::{ContentBlock, HandlingMode, RenderMetadata};

/// Response status for Request messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Accepted,
    Completed,
    Failed,
}

impl From<Status> for meerkat_core::interaction::ResponseStatus {
    fn from(s: Status) -> Self {
        match s {
            Status::Accepted => meerkat_core::interaction::ResponseStatus::Accepted,
            Status::Completed => meerkat_core::interaction::ResponseStatus::Completed,
            Status::Failed => meerkat_core::interaction::ResponseStatus::Failed,
        }
    }
}

impl From<meerkat_core::interaction::ResponseStatus> for Status {
    fn from(s: meerkat_core::interaction::ResponseStatus) -> Self {
        match s {
            meerkat_core::interaction::ResponseStatus::Accepted => Status::Accepted,
            meerkat_core::interaction::ResponseStatus::Completed => Status::Completed,
            meerkat_core::interaction::ResponseStatus::Failed => Status::Failed,
        }
    }
}

/// The kind of message being sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MessageKind {
    /// A simple text message.
    Message {
        body: String,
        /// Optional multimodal content blocks. When present, carries full
        /// multimodal content; `body` remains the canonical text projection.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<ContentBlock>>,
        /// Sender-declared content taint. Lives INSIDE the signed region
        /// (`kind` is part of the signable tuple): omitted-when-`None` keeps
        /// old envelopes byte-identical and verifying; present-when-`Some` is
        /// covered by the signature. `None` = no declaration — a real third
        /// state receivers must never coalesce into
        /// [`SenderContentTaint::Clean`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_taint: Option<SenderContentTaint>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handling_mode: Option<HandlingMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        objective_id: Option<meerkat_core::interaction::ObjectiveId>,
    },
    /// A peer message with a mandatory exact recipient residency. The
    /// authenticated member sender is unchanged; the fence is inside the
    /// signed envelope and is checked before receiver work admission.
    #[serde(rename = "incarnation_fenced_message")]
    IncarnationFencedMessage {
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<ContentBlock>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_taint: Option<SenderContentTaint>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handling_mode: Option<HandlingMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        objective_id: Option<meerkat_core::interaction::ObjectiveId>,
        expected_recipient: meerkat_core::comms::PeerRecipientIncarnation,
    },
    /// A request for the peer to perform an action.
    Request {
        intent: String,
        params: JsonValue,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<ContentBlock>>,
        /// Sender-declared socket endpoint for the response to this exact
        /// request. The field lives inside the signed `kind` region, so an
        /// ingress classifier may treat it as authenticated sender metadata
        /// after verifying the envelope. Omission preserves the canonical
        /// bytes of requests produced before this field existed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_endpoint: Option<meerkat_core::comms::PeerAddress>,
        /// Sender-declared content taint (signed-when-present; see
        /// [`MessageKind::Message::content_taint`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_taint: Option<SenderContentTaint>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handling_mode: Option<HandlingMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        objective_id: Option<meerkat_core::interaction::ObjectiveId>,
    },
    /// A one-way peer lifecycle notification.
    Lifecycle {
        kind: PeerLifecycleKind,
        params: JsonValue,
    },
    /// A response to a previous request.
    Response {
        in_reply_to: Uuid,
        status: Status,
        result: JsonValue,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<ContentBlock>>,
        /// Sender-declared content taint (signed-when-present; see
        /// [`MessageKind::Message::content_taint`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_taint: Option<SenderContentTaint>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handling_mode: Option<HandlingMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        objective_id: Option<meerkat_core::interaction::ObjectiveId>,
    },
    /// Acknowledgment of message receipt.
    Ack { in_reply_to: Uuid },
}

impl MessageKind {
    #[must_use]
    pub fn objective_id(&self) -> Option<meerkat_core::interaction::ObjectiveId> {
        match self {
            Self::Message { objective_id, .. }
            | Self::IncarnationFencedMessage { objective_id, .. }
            | Self::Request { objective_id, .. }
            | Self::Response { objective_id, .. } => *objective_id,
            Self::Lifecycle { .. } | Self::Ack { .. } => None,
        }
    }
    /// Sender-declared content taint carried by this kind, when it is a
    /// content-bearing kind (`Message` / `Request` / `Response`) and a
    /// declaration is present.
    #[must_use]
    pub fn content_taint(&self) -> Option<SenderContentTaint> {
        match self {
            Self::Message { content_taint, .. }
            | Self::IncarnationFencedMessage { content_taint, .. }
            | Self::Request { content_taint, .. }
            | Self::Response { content_taint, .. } => *content_taint,
            Self::Lifecycle { .. } | Self::Ack { .. } => None,
        }
    }

    /// Stamp the resolved outbound taint declaration onto content-bearing
    /// kinds. Non-content kinds (`Lifecycle` / `Ack`) are returned unchanged.
    ///
    /// This is the router's single outbound stamping hook: construction sites
    /// leave `content_taint` as `None` and [`crate::router::Router`] resolves
    /// the effective declaration at the send choke point.
    pub(crate) fn with_content_taint(mut self, taint: Option<SenderContentTaint>) -> Self {
        match &mut self {
            Self::Message { content_taint, .. }
            | Self::IncarnationFencedMessage { content_taint, .. }
            | Self::Request { content_taint, .. }
            | Self::Response { content_taint, .. } => *content_taint = taint,
            Self::Lifecycle { .. } | Self::Ack { .. } => {}
        }
        self
    }
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
    ///
    /// TWIN ALGORITHM: `meerkat-contracts/src/wire/spec_digest.rs` carries a
    /// byte-identical copy of this canonicalizer for the portable-member-spec
    /// digest (the crates cannot share code without a dependency cycle). Any
    /// RFC 8949 edge-case fix must land in BOTH; the contracts side pins the
    /// semantics with a known-answer vector test.
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
    /// A plain-text event from an external (unauthenticated) source.
    PlainEvent {
        body: String,
        source: meerkat_core::PlainEventSource,
        #[serde(default)]
        handling_mode: HandlingMode,
        /// Optional interaction ID for subscription correlation.
        /// Set by `inject_with_subscription`, `None` for untracked events.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interaction_id: Option<Uuid>,
        /// Optional multimodal content blocks.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<ContentBlock>>,
        /// Optional normalized rendering metadata.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        render_metadata: Option<RenderMetadata>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        objective_id: Option<meerkat_core::interaction::ObjectiveId>,
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
            objective_id: None,
            body: "hello".to_string(),
            blocks: None,
            content_taint: None,
            handling_mode: None,
        };
        if let MessageKind::Message {
            body,
            blocks,
            content_taint,
            handling_mode,
            ..
        } = msg
        {
            assert_eq!(body, "hello");
            assert_eq!(blocks, None);
            assert_eq!(content_taint, None);
            assert_eq!(handling_mode, None);
        } else {
            panic!("Expected Message variant");
        }
    }

    #[test]
    fn incarnation_fenced_message_objective_is_optional_and_signed() {
        use crate::identity::Keypair;

        let expected_recipient = meerkat_core::comms::PeerRecipientIncarnation {
            mob_id: "mob".to_string(),
            agent_identity: "worker".to_string(),
            host_id: "host".to_string(),
            binding_generation: 1,
            member_session_id: "session".to_string(),
            generation: 1,
            fence_token: 7,
        };
        let without_objective = MessageKind::IncarnationFencedMessage {
            body: "fenced work".to_string(),
            blocks: None,
            content_taint: None,
            handling_mode: Some(HandlingMode::Queue),
            objective_id: None,
            expected_recipient: expected_recipient.clone(),
        };
        let legacy_json = serde_json::to_value(&without_objective).unwrap();
        assert!(legacy_json.get("objective_id").is_none());
        let decoded: MessageKind = serde_json::from_value(legacy_json).unwrap();
        assert_eq!(decoded, without_objective);
        assert_eq!(decoded.objective_id(), None);

        let keypair = Keypair::generate();
        let mut legacy_envelope = Envelope {
            id: Uuid::new_v4(),
            from: keypair.public_key(),
            to: PubKey::new([2u8; 32]),
            kind: without_objective,
            sig: Signature::new([0u8; 64]),
        };
        legacy_envelope.sign(&keypair);
        let legacy_envelope_json = serde_json::to_value(&legacy_envelope).unwrap();
        assert!(legacy_envelope_json["kind"].get("objective_id").is_none());
        let legacy_envelope_roundtrip: Envelope =
            serde_json::from_value(legacy_envelope_json).unwrap();
        assert!(legacy_envelope_roundtrip.verify());
        assert_eq!(legacy_envelope_roundtrip.kind.objective_id(), None);

        let objective_id = meerkat_core::interaction::ObjectiveId::new();
        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: keypair.public_key(),
            to: PubKey::new([2u8; 32]),
            kind: MessageKind::IncarnationFencedMessage {
                body: "fenced work".to_string(),
                blocks: None,
                content_taint: None,
                handling_mode: Some(HandlingMode::Queue),
                objective_id: Some(objective_id),
                expected_recipient,
            },
            sig: Signature::new([0u8; 64]),
        };
        assert_eq!(envelope.kind.objective_id(), Some(objective_id));
        envelope.sign(&keypair);
        assert!(envelope.verify());
        let envelope_json = serde_json::to_value(&envelope).unwrap();
        let mut envelope: Envelope = serde_json::from_value(envelope_json).unwrap();
        assert!(envelope.verify());
        assert_eq!(envelope.kind.objective_id(), Some(objective_id));

        let MessageKind::IncarnationFencedMessage { objective_id, .. } = &mut envelope.kind else {
            panic!("expected incarnation-fenced message");
        };
        *objective_id = None;
        assert!(
            !envelope.verify(),
            "objective_id must remain signature-bound"
        );
    }

    #[test]
    fn test_message_kind_request_fields() {
        let req = MessageKind::Request {
            objective_id: None,
            intent: "review-pr".to_string(),
            params: serde_json::json!({"pr": 42}),
            blocks: None,
            reply_endpoint: None,
            content_taint: None,
            handling_mode: None,
        };
        if let MessageKind::Request {
            intent,
            params,
            blocks,
            reply_endpoint,
            content_taint,
            handling_mode,
            ..
        } = req
        {
            assert_eq!(intent, "review-pr");
            assert_eq!(params["pr"], 42);
            assert_eq!(blocks, None);
            assert_eq!(reply_endpoint, None);
            assert_eq!(content_taint, None);
            assert_eq!(handling_mode, None);
        } else {
            panic!("Expected Request variant");
        }
    }

    #[test]
    fn request_reply_endpoint_is_signed_and_absent_is_byte_identical() {
        #[derive(Serialize)]
        #[serde(tag = "type", rename_all = "lowercase")]
        enum LegacyMessageKind {
            Request {
                intent: String,
                params: serde_json::Value,
                #[serde(default, skip_serializing_if = "Option::is_none")]
                blocks: Option<Vec<ContentBlock>>,
                #[serde(default, skip_serializing_if = "Option::is_none")]
                content_taint: Option<SenderContentTaint>,
                #[serde(default, skip_serializing_if = "Option::is_none")]
                handling_mode: Option<HandlingMode>,
            },
        }

        let request = MessageKind::Request {
            intent: "probe".to_string(),
            params: serde_json::json!({"seq": 1}),
            blocks: None,
            reply_endpoint: None,
            content_taint: None,
            handling_mode: Some(HandlingMode::Queue),
            objective_id: None,
        };
        let legacy = LegacyMessageKind::Request {
            intent: "probe".to_string(),
            params: serde_json::json!({"seq": 1}),
            blocks: None,
            content_taint: None,
            handling_mode: Some(HandlingMode::Queue),
        };
        let mut request_bytes = Vec::new();
        ciborium::into_writer(&request, &mut request_bytes).unwrap();
        let mut legacy_bytes = Vec::new();
        ciborium::into_writer(&legacy, &mut legacy_bytes).unwrap();
        assert_eq!(
            request_bytes, legacy_bytes,
            "omitting reply_endpoint must preserve the legacy Request bytes"
        );

        let keypair = Keypair::generate();
        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: keypair.public_key(),
            to: PubKey::new([2u8; 32]),
            kind: MessageKind::Request {
                intent: "probe".to_string(),
                params: serde_json::json!({}),
                blocks: None,
                reply_endpoint: Some(
                    meerkat_core::comms::PeerAddress::parse("tcp://127.0.0.1:4311").unwrap(),
                ),
                content_taint: None,
                handling_mode: Some(HandlingMode::Queue),
                objective_id: None,
            },
            sig: Signature::new([0u8; 64]),
        };
        envelope.sign(&keypair);
        assert!(envelope.verify(), "signed reply endpoint must verify");
        let MessageKind::Request { reply_endpoint, .. } = &mut envelope.kind else {
            panic!("expected Request");
        };
        *reply_endpoint =
            Some(meerkat_core::comms::PeerAddress::parse("tcp://127.0.0.1:4312").unwrap());
        assert!(
            !envelope.verify(),
            "tampering with the reply endpoint must break the envelope signature"
        );
    }

    #[test]
    fn test_message_kind_response_fields() {
        let id = Uuid::new_v4();
        let resp = MessageKind::Response {
            objective_id: None,
            in_reply_to: id,
            status: Status::Completed,
            result: serde_json::json!({"approved": true}),
            blocks: None,
            content_taint: None,
            handling_mode: None,
        };
        if let MessageKind::Response {
            in_reply_to,
            status,
            result,
            ..
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
                objective_id: None,
                body: "test".to_string(),
                blocks: None,
                content_taint: None,
                handling_mode: None,
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
                objective_id: None,
                body: "hello".to_string(),
                blocks: None,
                content_taint: None,
                handling_mode: None,
            },
            MessageKind::Request {
                objective_id: None,
                intent: "test".to_string(),
                params: serde_json::json!({}),
                blocks: None,
                reply_endpoint: None,
                content_taint: None,
                handling_mode: None,
            },
            MessageKind::Response {
                objective_id: None,
                in_reply_to: Uuid::new_v4(),
                status: Status::Completed,
                result: serde_json::json!(null),
                blocks: None,
                content_taint: None,
                handling_mode: None,
            },
            MessageKind::Message {
                objective_id: None,
                body: "declared".to_string(),
                blocks: None,
                content_taint: Some(SenderContentTaint::Tainted),
                handling_mode: None,
            },
            MessageKind::Response {
                objective_id: None,
                in_reply_to: Uuid::new_v4(),
                status: Status::Completed,
                result: serde_json::json!(null),
                blocks: None,
                content_taint: Some(SenderContentTaint::Clean),
                handling_mode: None,
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
                objective_id: None,
                body: "test".to_string(),
                blocks: None,
                content_taint: None,
                handling_mode: None,
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
            InboxItem::PlainEvent {
                objective_id: None,
                body: "event".to_string(),
                source: meerkat_core::PlainEventSource::Tcp,
                handling_mode: HandlingMode::Queue,
                interaction_id: Some(Uuid::new_v4()),
                blocks: None,
                render_metadata: None,
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
            objective_id: None,
            body: "test".to_string(),
            blocks: None,
            content_taint: None,
            handling_mode: None,
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
                objective_id: None,
                body: "test".to_string(),
                blocks: None,
                content_taint: None,
                handling_mode: None,
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
                objective_id: None,
                body: "test".to_string(),
                blocks: None,
                content_taint: None,
                handling_mode: None,
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
                objective_id: None,
                body: "test".to_string(),
                blocks: None,
                content_taint: None,
                handling_mode: None,
            },
            sig: Signature::new([0u8; 64]),
        };
        envelope.sign(&keypair);
        assert!(envelope.verify(), "signed envelope should verify");
    }

    #[test]
    fn test_rct_contracts_envelope_signable_bytes_are_canonical() {
        let envelope = Envelope {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            from: PubKey::new([1u8; 32]),
            to: PubKey::new([2u8; 32]),
            kind: MessageKind::Message {
                objective_id: None,
                body: "hello".to_string(),
                blocks: None,
                content_taint: None,
                handling_mode: None,
            },
            sig: Signature::new([0u8; 64]),
        };

        let bytes = envelope.signable_bytes();

        let mut expected = Vec::new();
        expected.extend_from_slice(&[
            0x84, 0x50, 0x55, 0x0e, 0x84, 0x00, 0xe2, 0x9b, 0x41, 0xd4, 0xa7, 0x16, 0x44, 0x66,
            0x55, 0x44, 0x00, 0x00, 0x58, 0x20,
        ]);
        expected.extend(std::iter::repeat_n(0x01, 32));
        expected.extend_from_slice(&[0x58, 0x20]);
        expected.extend(std::iter::repeat_n(0x02, 32));
        expected.extend_from_slice(&[
            0xa2, 0x64, b'b', b'o', b'd', b'y', 0x65, b'h', b'e', b'l', b'l', b'o', 0x64, b't',
            b'y', b'p', b'e', 0x67, b'm', b'e', b's', b's', b'a', b'g', b'e',
        ]);

        assert_eq!(bytes, expected);
    }

    #[test]
    fn test_regression_ack_must_match_sent_message() {
        use crate::identity::Keypair;

        let sender_keypair = Keypair::generate();
        let receiver_keypair = Keypair::generate();

        let sent_id = Uuid::new_v4();
        let sent_envelope = Envelope {
            id: sent_id,
            from: sender_keypair.public_key(),
            to: receiver_keypair.public_key(),
            kind: MessageKind::Message {
                objective_id: None,
                body: "hello".to_string(),
                blocks: None,
                content_taint: None,
                handling_mode: None,
            },
            sig: Signature::new([0u8; 64]),
        };

        let mut valid_ack = Envelope {
            id: Uuid::new_v4(),
            from: receiver_keypair.public_key(),
            to: sender_keypair.public_key(),
            kind: MessageKind::Ack {
                in_reply_to: sent_id,
            },
            sig: Signature::new([0u8; 64]),
        };
        valid_ack.sign(&receiver_keypair);

        assert!(valid_ack.verify(), "valid ACK should verify");
        assert_eq!(
            valid_ack.from, sent_envelope.to,
            "ACK from should match sent to"
        );
        if let MessageKind::Ack { in_reply_to } = valid_ack.kind {
            assert_eq!(in_reply_to, sent_id, "ACK in_reply_to should match sent id");
        }
    }

    #[test]
    fn test_regression_ack_wrong_in_reply_to_is_invalid() {
        use crate::identity::Keypair;

        let sender_keypair = Keypair::generate();
        let receiver_keypair = Keypair::generate();

        let sent_id = Uuid::new_v4();
        let wrong_id = Uuid::new_v4();

        let mut wrong_ack = Envelope {
            id: Uuid::new_v4(),
            from: receiver_keypair.public_key(),
            to: sender_keypair.public_key(),
            kind: MessageKind::Ack {
                in_reply_to: wrong_id,
            },
            sig: Signature::new([0u8; 64]),
        };
        wrong_ack.sign(&receiver_keypair);

        assert!(wrong_ack.verify(), "signature should still verify");
        if let MessageKind::Ack { in_reply_to } = wrong_ack.kind {
            assert_ne!(
                in_reply_to, sent_id,
                "wrong ACK in_reply_to should not match sent id"
            );
        }
    }

    #[test]
    fn test_regression_ack_from_wrong_peer_is_invalid() {
        use crate::identity::Keypair;

        let sender_keypair = Keypair::generate();
        let receiver_keypair = Keypair::generate();
        let imposter_keypair = Keypair::generate();

        let sent_id = Uuid::new_v4();
        let sent_to = receiver_keypair.public_key();

        let mut imposter_ack = Envelope {
            id: Uuid::new_v4(),
            from: imposter_keypair.public_key(),
            to: sender_keypair.public_key(),
            kind: MessageKind::Ack {
                in_reply_to: sent_id,
            },
            sig: Signature::new([0u8; 64]),
        };
        imposter_ack.sign(&imposter_keypair);

        assert!(imposter_ack.verify(), "imposter signature should verify");
        assert_ne!(
            imposter_ack.from, sent_to,
            "imposter ACK from should not match sent to"
        );
    }

    #[test]
    fn test_regression_ack_to_wrong_recipient_is_invalid() {
        use crate::identity::Keypair;

        let sender_keypair = Keypair::generate();
        let receiver_keypair = Keypair::generate();
        let wrong_recipient_keypair = Keypair::generate();

        let sent_id = Uuid::new_v4();

        let mut misrouted_ack = Envelope {
            id: Uuid::new_v4(),
            from: receiver_keypair.public_key(),
            to: wrong_recipient_keypair.public_key(),
            kind: MessageKind::Ack {
                in_reply_to: sent_id,
            },
            sig: Signature::new([0u8; 64]),
        };
        misrouted_ack.sign(&receiver_keypair);

        assert!(
            misrouted_ack.verify(),
            "misrouted ACK signature should verify"
        );
        assert_ne!(
            misrouted_ack.to,
            sender_keypair.public_key(),
            "misrouted ACK 'to' should not match sender's public key"
        );
    }

    // === PlainEvent tests ===

    #[test]
    fn test_inbox_item_plain_event_serde_roundtrip() {
        use meerkat_core::PlainEventSource;

        let item = InboxItem::PlainEvent {
            objective_id: None,
            body: "New email from john@example.com".to_string(),
            source: PlainEventSource::Tcp,
            handling_mode: HandlingMode::Queue,
            interaction_id: None,
            blocks: None,
            render_metadata: None,
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
                objective_id: None,
                body: "hello".to_string(),
                source: PlainEventSource::Tcp,
                handling_mode: HandlingMode::Queue,
                interaction_id: None,
                blocks: None,
                render_metadata: None,
            },
            InboxItem::PlainEvent {
                objective_id: None,
                body: r#"{"event":"email"}"#.to_string(),
                source: PlainEventSource::Stdin,
                handling_mode: HandlingMode::Queue,
                interaction_id: None,
                blocks: None,
                render_metadata: None,
            },
            InboxItem::PlainEvent {
                objective_id: None,
                body: "webhook payload".to_string(),
                source: PlainEventSource::Webhook,
                handling_mode: HandlingMode::Queue,
                interaction_id: None,
                blocks: None,
                render_metadata: None,
            },
            InboxItem::PlainEvent {
                objective_id: None,
                body: "rpc event".to_string(),
                source: PlainEventSource::Rpc,
                handling_mode: HandlingMode::Queue,
                interaction_id: None,
                blocks: None,
                render_metadata: None,
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
        // Old format (no interaction_id, no blocks) should deserialize with defaults
        let old_json = r#"{"type":"plain_event","body":"hello","source":"tcp"}"#;
        let parsed: InboxItem = serde_json::from_str(old_json).unwrap();
        match parsed {
            InboxItem::PlainEvent {
                body,
                source,
                handling_mode: _,
                interaction_id,
                blocks,
                render_metadata,
                ..
            } => {
                assert_eq!(body, "hello");
                assert_eq!(source, meerkat_core::PlainEventSource::Tcp);
                assert_eq!(interaction_id, None);
                assert_eq!(blocks, None);
                assert_eq!(render_metadata, None);
            }
            other => panic!("Expected PlainEvent, got {other:?}"),
        }
    }

    #[test]
    fn test_inbox_item_plain_event_with_interaction_id_json_roundtrip() {
        let id = Uuid::new_v4();
        let item = InboxItem::PlainEvent {
            objective_id: None,
            body: "tracked event".to_string(),
            source: meerkat_core::PlainEventSource::Rpc,
            handling_mode: HandlingMode::Queue,
            interaction_id: Some(id),
            blocks: None,
            render_metadata: None,
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
            objective_id: None,
            body: "tracked event".to_string(),
            source: meerkat_core::PlainEventSource::Rpc,
            handling_mode: HandlingMode::Queue,
            interaction_id: Some(id),
            blocks: None,
            render_metadata: None,
        };

        let mut buf = Vec::new();
        ciborium::into_writer(&item, &mut buf).unwrap();
        let decoded: InboxItem = ciborium::from_reader(&buf[..]).unwrap();
        assert_eq!(item, decoded);
    }

    #[test]
    fn test_inbox_item_plain_event_none_interaction_id_skipped_in_json() {
        let item = InboxItem::PlainEvent {
            objective_id: None,
            body: "untracked".to_string(),
            source: meerkat_core::PlainEventSource::Tcp,
            handling_mode: HandlingMode::Queue,
            interaction_id: None,
            blocks: None,
            render_metadata: None,
        };

        let json = serde_json::to_string(&item).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        // interaction_id should not be present (skip_serializing_if = "Option::is_none")
        assert!(
            value.get("interaction_id").is_none(),
            "interaction_id: None should not be serialized"
        );
    }

    // === Multimodal blocks tests ===

    #[test]
    fn message_kind_with_blocks_cbor_roundtrip() {
        let kind = MessageKind::Message {
            objective_id: None,
            body: "hello".to_string(),
            blocks: Some(vec![
                ContentBlock::Text {
                    text: "hello".to_string(),
                },
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: "iVBORw0KGgo=".into(),
                },
            ]),
            content_taint: None,
            handling_mode: None,
        };
        let mut buf = Vec::new();
        ciborium::into_writer(&kind, &mut buf).unwrap();
        let decoded: MessageKind = ciborium::from_reader(&buf[..]).unwrap();
        assert_eq!(kind, decoded);
    }

    #[test]
    fn message_kind_without_blocks_backwards_compat() {
        use crate::identity::Keypair;

        // Serialize a Message without blocks, sign it, verify it.
        // Then deserialize and verify blocks defaults to None.
        let keypair = Keypair::generate();
        let kind = MessageKind::Message {
            objective_id: None,
            body: "test".to_string(),
            blocks: None,
            content_taint: None,
            handling_mode: None,
        };

        // CBOR roundtrip preserves None
        let mut buf = Vec::new();
        ciborium::into_writer(&kind, &mut buf).unwrap();
        let decoded: MessageKind = ciborium::from_reader(&buf[..]).unwrap();
        assert_eq!(kind, decoded);

        // Envelope sign+verify still works
        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: keypair.public_key(),
            to: PubKey::new([2u8; 32]),
            kind,
            sig: Signature::new([0u8; 64]),
        };
        envelope.sign(&keypair);
        assert!(
            envelope.verify(),
            "blocks=None envelope should still verify"
        );

        // JSON roundtrip omits blocks field entirely
        let json = serde_json::to_string(&envelope.kind).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(
            value.get("blocks").is_none(),
            "blocks: None should not appear in JSON"
        );
        // content_taint: None is omitted from the wire exactly like blocks —
        // undeclared taint keeps old envelopes byte-identical and verifying.
        assert!(
            value.get("content_taint").is_none(),
            "content_taint: None should not appear in JSON"
        );
        let cbor_value: ciborium::value::Value = {
            let mut buf = Vec::new();
            ciborium::into_writer(&envelope.kind, &mut buf).unwrap();
            ciborium::from_reader(&buf[..]).unwrap()
        };
        let ciborium::value::Value::Map(entries) = cbor_value else {
            panic!("MessageKind should encode as a CBOR map");
        };
        assert!(
            !entries.iter().any(|(key, _)| matches!(
                key,
                ciborium::value::Value::Text(text) if text == "content_taint"
            )),
            "content_taint: None should not appear in CBOR"
        );
    }

    #[test]
    fn test_rct_contracts_signable_bytes_with_content_taint_are_canonical() {
        // Exact-byte pin for a DECLARED taint: `content_taint` (len 13) sorts
        // immediately before `handling_mode` (len 13, lexicographically later)
        // in the RFC 8949 length-then-lex canonical key order. Any change to
        // the signed encoding of the taint declaration breaks this pin.
        let envelope = Envelope {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            from: PubKey::new([1u8; 32]),
            to: PubKey::new([2u8; 32]),
            kind: MessageKind::Message {
                objective_id: None,
                body: "hello".to_string(),
                blocks: None,
                content_taint: Some(SenderContentTaint::Tainted),
                handling_mode: Some(HandlingMode::Queue),
            },
            sig: Signature::new([0u8; 64]),
        };

        let bytes = envelope.signable_bytes();

        let mut expected = Vec::new();
        expected.extend_from_slice(&[
            0x84, 0x50, 0x55, 0x0e, 0x84, 0x00, 0xe2, 0x9b, 0x41, 0xd4, 0xa7, 0x16, 0x44, 0x66,
            0x55, 0x44, 0x00, 0x00, 0x58, 0x20,
        ]);
        expected.extend(std::iter::repeat_n(0x01, 32));
        expected.extend_from_slice(&[0x58, 0x20]);
        expected.extend(std::iter::repeat_n(0x02, 32));
        // Map of 4 entries in canonical order:
        //   "body" -> "hello", "type" -> "message",
        //   "content_taint" -> "tainted", "handling_mode" -> "queue".
        expected.push(0xa4);
        expected.extend_from_slice(&[0x64, b'b', b'o', b'd', b'y']);
        expected.extend_from_slice(&[0x65, b'h', b'e', b'l', b'l', b'o']);
        expected.extend_from_slice(&[0x64, b't', b'y', b'p', b'e']);
        expected.extend_from_slice(&[0x67, b'm', b'e', b's', b's', b'a', b'g', b'e']);
        expected.extend_from_slice(&[
            0x6d, b'c', b'o', b'n', b't', b'e', b'n', b't', b'_', b't', b'a', b'i', b'n', b't',
        ]);
        expected.extend_from_slice(&[0x67, b't', b'a', b'i', b'n', b't', b'e', b'd']);
        expected.extend_from_slice(&[
            0x6d, b'h', b'a', b'n', b'd', b'l', b'i', b'n', b'g', b'_', b'm', b'o', b'd', b'e',
        ]);
        expected.extend_from_slice(&[0x65, b'q', b'u', b'e', b'u', b'e']);

        assert_eq!(bytes, expected);
    }

    #[test]
    fn envelope_with_content_taint_signs_and_verifies() {
        use crate::identity::Keypair;

        for taint in [SenderContentTaint::Clean, SenderContentTaint::Tainted] {
            let keypair = Keypair::generate();
            let mut envelope = Envelope {
                id: Uuid::new_v4(),
                from: keypair.public_key(),
                to: PubKey::new([2u8; 32]),
                kind: MessageKind::Request {
                    objective_id: None,
                    intent: "review".to_string(),
                    params: serde_json::json!({"pr": 7}),
                    blocks: None,
                    reply_endpoint: None,
                    content_taint: Some(taint),
                    handling_mode: None,
                },
                sig: Signature::new([0u8; 64]),
            };
            envelope.sign(&keypair);
            assert!(
                envelope.verify(),
                "envelope with content_taint {taint} should verify"
            );

            // The declaration is inside the signed region: flipping it after
            // signing must break verification.
            envelope.kind = envelope.kind.with_content_taint(Some(match taint {
                SenderContentTaint::Clean => SenderContentTaint::Tainted,
                SenderContentTaint::Tainted => SenderContentTaint::Clean,
            }));
            assert!(
                !envelope.verify(),
                "tampered content_taint must fail signature verification"
            );
        }
    }

    #[test]
    fn content_taint_none_and_clean_are_distinct_wire_states() {
        // None (no declaration) omits the field; Clean is an affirmative,
        // serialized declaration. A receiver coalescing the two would erase
        // the sender's claim; the wire keeps them distinct.
        let undeclared = MessageKind::Message {
            objective_id: None,
            body: "hello".to_string(),
            blocks: None,
            content_taint: None,
            handling_mode: None,
        };
        let clean = MessageKind::Message {
            objective_id: None,
            body: "hello".to_string(),
            blocks: None,
            content_taint: Some(SenderContentTaint::Clean),
            handling_mode: None,
        };
        assert_ne!(undeclared, clean);

        let undeclared_json = serde_json::to_value(&undeclared).unwrap();
        let clean_json = serde_json::to_value(&clean).unwrap();
        assert!(undeclared_json.get("content_taint").is_none());
        assert_eq!(clean_json["content_taint"], "clean");

        let undeclared_roundtrip: MessageKind = serde_json::from_value(undeclared_json).unwrap();
        let clean_roundtrip: MessageKind = serde_json::from_value(clean_json).unwrap();
        assert_eq!(undeclared_roundtrip.content_taint(), None);
        assert_eq!(
            clean_roundtrip.content_taint(),
            Some(SenderContentTaint::Clean)
        );
    }
}
