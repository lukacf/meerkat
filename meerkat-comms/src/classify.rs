//! Ingress classification for incoming peer/event traffic.
//!
//! Runs synchronously in the sending task using receiver-owned trust/auth state.

use crate::agent::types::{CommsMessage, MessageIntent};
use crate::inproc::InprocRegistry;
use crate::peer_comms_authority::{ContentShape as PeerContentShape, RawPeerKind};
use crate::trust::TrustedPeers;
use crate::types::{InboxItem, MessageKind};
use meerkat_core::{PeerIngressKind, PeerInputClass};
use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

/// Receiver-owned context for synchronous ingress classification.
///
/// Cloned into `InboxSender` so classification runs in the sending task
/// using receiver-owned trust/auth state.
pub(crate) struct IngressClassificationContext {
    pub(crate) require_peer_auth: bool,
    pub(crate) trusted_peers: Arc<parking_lot::RwLock<TrustedPeers>>,
    pub(crate) silent_intents: Arc<HashSet<String>>,
}

/// Result of classifying an inbox item.
///
/// `None` from `classify()` means the item should be dropped at ingress
/// (e.g., untrusted sender when `require_peer_auth` is enabled).
#[cfg(test)]
pub(crate) struct ClassificationResult {
    pub(crate) class: PeerInputClass,
    pub(crate) from_peer: Option<String>,
    pub(crate) lifecycle_peer: Option<String>,
}

/// Authority-aligned ingress descriptor for one inbox item.
///
/// This keeps the live classification path close to the eventual
/// `PeerCommsAuthority` handoff shape without forcing the authority onto the
/// hot path yet.
pub(crate) struct PreparedIngressItem {
    pub(crate) item: InboxItem,
    pub(crate) raw_item_id: String,
    pub(crate) raw_kind: RawPeerKind,
    pub(crate) class: PeerInputClass,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) trusted_sender: bool,
    pub(crate) from_peer: Option<String>,
    pub(crate) lifecycle_peer: Option<String>,
    pub(crate) text_projection: String,
    #[allow(dead_code)]
    pub(crate) content_shape: PeerContentShape,
    pub(crate) request_id: Option<String>,
}

fn content_shape_for_text_and_blocks(
    body: &str,
    blocks: Option<&[meerkat_core::types::ContentBlock]>,
) -> PeerContentShape {
    match blocks {
        Some(blocks) if !blocks.is_empty() => {
            if body.trim().is_empty() {
                PeerContentShape::Blocks
            } else {
                PeerContentShape::Mixed
            }
        }
        _ => PeerContentShape::Text,
    }
}

fn ack_projection(from_peer: &str, in_reply_to: Uuid) -> String {
    format!("[COMMS ACK from {from_peer} (to request: {in_reply_to})]")
}

impl PreparedIngressItem {
    pub(crate) fn ingress_kind(&self) -> PeerIngressKind {
        match &self.raw_kind {
            RawPeerKind::Message => PeerIngressKind::Message,
            RawPeerKind::Request
            | RawPeerKind::PeerLifecycleAdded
            | RawPeerKind::PeerLifecycleRetired
            | RawPeerKind::SilentRequest => PeerIngressKind::Request,
            RawPeerKind::ResponseTerminal | RawPeerKind::ResponseProgress => {
                PeerIngressKind::Response
            }
            RawPeerKind::Ack => PeerIngressKind::Ack,
            RawPeerKind::PlainEvent => PeerIngressKind::PlainEvent,
        }
    }
}

impl IngressClassificationContext {
    /// Prepare an inbox item for classified ingress.
    ///
    /// This normalizes plain-event identities and computes the same
    /// high-level ingress facts that the peer machine will need later.
    pub(crate) fn prepare(&self, item: InboxItem) -> Option<PreparedIngressItem> {
        match item {
            InboxItem::External { envelope } => {
                let trusted = self.trusted_peers.read();
                let from_peer = trusted.get_peer(&envelope.from).map(|p| p.name.clone());
                let trusted_sender = from_peer.is_some();

                let from_name = from_peer.unwrap_or_else(|| {
                    InprocRegistry::global()
                        .get_name_by_pubkey(&envelope.from)
                        .unwrap_or_else(|| envelope.from.to_peer_id())
                });

                let (raw_kind, class, lifecycle_peer, request_id) = match &envelope.kind {
                    MessageKind::Message { .. } => (
                        RawPeerKind::Message,
                        PeerInputClass::ActionableMessage,
                        None,
                        None,
                    ),
                    MessageKind::Request { intent, params, .. } => {
                        let typed_intent = MessageIntent::from(intent.as_str());
                        match typed_intent {
                            MessageIntent::PeerAdded => {
                                let peer = params
                                    .get("peer")
                                    .and_then(|v| v.as_str())
                                    .filter(|s| !s.is_empty())
                                    .unwrap_or(from_name.as_str())
                                    .to_string();
                                (
                                    RawPeerKind::PeerLifecycleAdded,
                                    PeerInputClass::PeerLifecycleAdded,
                                    Some(peer),
                                    Some(envelope.id.to_string()),
                                )
                            }
                            MessageIntent::PeerRetired => {
                                let peer = params
                                    .get("peer")
                                    .and_then(|v| v.as_str())
                                    .filter(|s| !s.is_empty())
                                    .unwrap_or(from_name.as_str())
                                    .to_string();
                                (
                                    RawPeerKind::PeerLifecycleRetired,
                                    PeerInputClass::PeerLifecycleRetired,
                                    Some(peer),
                                    Some(envelope.id.to_string()),
                                )
                            }
                            _ if self.silent_intents.contains(intent.as_str()) => (
                                RawPeerKind::SilentRequest,
                                PeerInputClass::SilentRequest,
                                None,
                                Some(envelope.id.to_string()),
                            ),
                            _ => (
                                RawPeerKind::Request,
                                PeerInputClass::ActionableRequest,
                                None,
                                Some(envelope.id.to_string()),
                            ),
                        }
                    }
                    MessageKind::Response {
                        in_reply_to,
                        status,
                        ..
                    } => (
                        match status {
                            crate::types::Status::Accepted => RawPeerKind::ResponseProgress,
                            crate::types::Status::Completed | crate::types::Status::Failed => {
                                RawPeerKind::ResponseTerminal
                            }
                        },
                        PeerInputClass::Response,
                        None,
                        Some(in_reply_to.to_string()),
                    ),
                    MessageKind::Ack { in_reply_to } => (
                        RawPeerKind::Ack,
                        PeerInputClass::Ack,
                        None,
                        Some(in_reply_to.to_string()),
                    ),
                };

                let text_projection = match &envelope.kind {
                    MessageKind::Message { body, .. } => {
                        format!("[COMMS MESSAGE from {from_name}]\n{body}")
                    }
                    MessageKind::Ack { in_reply_to } => ack_projection(&from_name, *in_reply_to),
                    _ => {
                        CommsMessage::from_external_with_resolved_peer(&envelope, from_name.clone())
                            .map(|message| message.to_user_message_text())
                            .unwrap_or_default()
                    }
                };

                let content_shape = match &envelope.kind {
                    MessageKind::Message { body, blocks, .. } => {
                        content_shape_for_text_and_blocks(body, blocks.as_deref())
                    }
                    MessageKind::Request { .. }
                    | MessageKind::Response { .. }
                    | MessageKind::Ack { .. } => PeerContentShape::Text,
                };

                Some(PreparedIngressItem {
                    raw_item_id: envelope.id.to_string(),
                    raw_kind,
                    class,
                    trusted_sender,
                    from_peer: Some(from_name),
                    lifecycle_peer,
                    text_projection,
                    content_shape,
                    request_id,
                    item: InboxItem::External { envelope },
                })
            }
            InboxItem::PlainEvent {
                body,
                source,
                handling_mode,
                interaction_id,
                blocks,
                render_metadata,
            } => {
                let interaction_id = interaction_id.unwrap_or_else(Uuid::new_v4);
                let text_projection = meerkat_core::interaction::format_external_event_projection(
                    &source.to_string(),
                    Some(&body),
                );
                let content_shape = content_shape_for_text_and_blocks(&body, blocks.as_deref());
                Some(PreparedIngressItem {
                    raw_item_id: interaction_id.to_string(),
                    raw_kind: RawPeerKind::PlainEvent,
                    class: PeerInputClass::PlainEvent,
                    trusted_sender: true,
                    from_peer: None,
                    lifecycle_peer: None,
                    text_projection,
                    content_shape,
                    request_id: None,
                    item: InboxItem::PlainEvent {
                        body,
                        source,
                        handling_mode,
                        interaction_id: Some(interaction_id),
                        blocks,
                        render_metadata,
                    },
                })
            }
        }
    }

    /// Classify an inbox item. Returns `None` when the item should be dropped
    /// at ingress (untrusted sender with `require_peer_auth` enabled).
    #[cfg(test)]
    pub(crate) fn classify(&self, item: &InboxItem) -> Option<ClassificationResult> {
        self.prepare(item.clone()).and_then(|prepared| {
            let drop_untrusted_external = matches!(prepared.item, InboxItem::External { .. })
                && self.require_peer_auth
                && !prepared.trusted_sender;
            if drop_untrusted_external {
                return None;
            }
            Some(ClassificationResult {
                class: prepared.class,
                from_peer: prepared.from_peer,
                lifecycle_peer: prepared.lifecycle_peer,
            })
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::identity::{Keypair, PubKey, Signature};
    use crate::peer_comms_authority::{
        PeerCommsAuthority, PeerCommsEffect, PeerCommsInput, PeerCommsMutator, PeerId, RawItemId,
        RequestId,
    };
    use crate::trust::TrustedPeer;
    use crate::types::Envelope;
    use uuid::Uuid;

    fn make_keypair() -> Keypair {
        Keypair::generate()
    }

    fn make_context(
        require_peer_auth: bool,
        trusted_peers: TrustedPeers,
        silent_intents: Vec<&str>,
    ) -> IngressClassificationContext {
        IngressClassificationContext {
            require_peer_auth,
            trusted_peers: Arc::new(parking_lot::RwLock::new(trusted_peers)),
            silent_intents: Arc::new(silent_intents.into_iter().map(String::from).collect()),
        }
    }

    fn make_trusted_peers(name: &str, pubkey: &PubKey) -> TrustedPeers {
        TrustedPeers {
            peers: vec![TrustedPeer {
                name: name.to_string(),
                pubkey: *pubkey,
                addr: "tcp://127.0.0.1:4200".to_string(),
                meta: crate::PeerMeta::default(),
            }],
        }
    }

    fn make_envelope(from: &Keypair, kind: MessageKind) -> Envelope {
        Envelope {
            id: Uuid::new_v4(),
            from: from.public_key(),
            to: PubKey::new([2u8; 32]),
            kind,
            sig: Signature::new([0u8; 64]),
        }
    }

    #[test]
    fn classify_message_as_actionable_message() {
        let sender = make_keypair();
        let trusted = make_trusted_peers("sender-agent", &sender.public_key());
        let ctx = make_context(true, trusted, vec![]);
        let envelope = make_envelope(
            &sender,
            MessageKind::Message {
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
            },
        );
        let item = InboxItem::External { envelope };
        let result = ctx.classify(&item).expect("should classify");
        assert_eq!(result.class, PeerInputClass::ActionableMessage);
        assert_eq!(result.from_peer.as_deref(), Some("sender-agent"));
        assert!(result.lifecycle_peer.is_none());
    }

    #[test]
    fn classify_request_as_actionable_request() {
        let sender = make_keypair();
        let trusted = make_trusted_peers("sender-agent", &sender.public_key());
        let ctx = make_context(true, trusted, vec![]);
        let envelope = make_envelope(
            &sender,
            MessageKind::Request {
                intent: "review".to_string(),
                params: serde_json::json!({}),
                handling_mode: None,
            },
        );
        let item = InboxItem::External { envelope };
        let result = ctx.classify(&item).expect("should classify");
        assert_eq!(result.class, PeerInputClass::ActionableRequest);
        assert_eq!(result.from_peer.as_deref(), Some("sender-agent"));
    }

    #[test]
    fn classify_response_as_response() {
        let sender = make_keypair();
        let trusted = make_trusted_peers("sender-agent", &sender.public_key());
        let ctx = make_context(true, trusted, vec![]);
        let envelope = make_envelope(
            &sender,
            MessageKind::Response {
                in_reply_to: Uuid::new_v4(),
                status: crate::types::Status::Completed,
                result: serde_json::json!({}),
                handling_mode: None,
            },
        );
        let item = InboxItem::External { envelope };
        let result = ctx.classify(&item).expect("should classify");
        assert_eq!(result.class, PeerInputClass::Response);
    }

    #[test]
    fn classify_ack_as_ack() {
        let sender = make_keypair();
        let trusted = make_trusted_peers("sender-agent", &sender.public_key());
        let ctx = make_context(true, trusted, vec![]);
        let envelope = make_envelope(
            &sender,
            MessageKind::Ack {
                in_reply_to: Uuid::new_v4(),
            },
        );
        let item = InboxItem::External { envelope };
        let result = ctx.classify(&item).expect("should classify");
        assert_eq!(result.class, PeerInputClass::Ack);
    }

    #[test]
    fn classify_peer_added_lifecycle() {
        let sender = make_keypair();
        let trusted = make_trusted_peers("orchestrator", &sender.public_key());
        let ctx = make_context(true, trusted, vec![]);
        let envelope = make_envelope(
            &sender,
            MessageKind::Request {
                intent: "mob.peer_added".to_string(),
                params: serde_json::json!({"peer": "new-agent"}),
                handling_mode: None,
            },
        );
        let item = InboxItem::External { envelope };
        let result = ctx.classify(&item).expect("should classify");
        assert_eq!(result.class, PeerInputClass::PeerLifecycleAdded);
        assert_eq!(result.lifecycle_peer.as_deref(), Some("new-agent"));
    }

    #[test]
    fn classify_peer_retired_lifecycle() {
        let sender = make_keypair();
        let trusted = make_trusted_peers("orchestrator", &sender.public_key());
        let ctx = make_context(true, trusted, vec![]);
        let envelope = make_envelope(
            &sender,
            MessageKind::Request {
                intent: "mob.peer_retired".to_string(),
                params: serde_json::json!({"peer": "old-agent"}),
                handling_mode: None,
            },
        );
        let item = InboxItem::External { envelope };
        let result = ctx.classify(&item).expect("should classify");
        assert_eq!(result.class, PeerInputClass::PeerLifecycleRetired);
        assert_eq!(result.lifecycle_peer.as_deref(), Some("old-agent"));
    }

    #[test]
    fn classify_silent_request() {
        let sender = make_keypair();
        let trusted = make_trusted_peers("sender-agent", &sender.public_key());
        let ctx = make_context(true, trusted, vec!["my-silent-intent"]);
        let envelope = make_envelope(
            &sender,
            MessageKind::Request {
                intent: "my-silent-intent".to_string(),
                params: serde_json::json!({}),
                handling_mode: None,
            },
        );
        let item = InboxItem::External { envelope };
        let result = ctx.classify(&item).expect("should classify");
        assert_eq!(result.class, PeerInputClass::SilentRequest);
    }

    #[test]
    fn classify_builtin_intent_in_silent_list() {
        // Silent matching must work for built-in intent names (e.g. "review"),
        // not just Custom variants. Regression test for P2 fix.
        let sender = make_keypair();
        let trusted = make_trusted_peers("sender-agent", &sender.public_key());
        let ctx = make_context(true, trusted, vec!["review"]);
        let envelope = make_envelope(
            &sender,
            MessageKind::Request {
                intent: "review".to_string(),
                params: serde_json::json!({}),
                handling_mode: None,
            },
        );
        let item = InboxItem::External { envelope };
        let result = ctx.classify(&item).expect("should classify");
        assert_eq!(result.class, PeerInputClass::SilentRequest);
    }

    #[test]
    fn classify_untrusted_sender_auth_required_drops_at_ingress() {
        let sender = make_keypair();
        let trusted = TrustedPeers::new(); // sender NOT trusted
        let ctx = make_context(true, trusted, vec![]);
        let envelope = make_envelope(
            &sender,
            MessageKind::Message {
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
            },
        );
        let item = InboxItem::External { envelope };
        let result = ctx.classify(&item);
        assert!(
            result.is_none(),
            "untrusted input must be dropped at ingress"
        );
    }

    #[test]
    fn classify_plain_event() {
        let ctx = make_context(false, TrustedPeers::new(), vec![]);
        let item = InboxItem::PlainEvent {
            blocks: None,
            body: "event".to_string(),
            source: meerkat_core::PlainEventSource::Tcp,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            interaction_id: None,
            render_metadata: None,
        };
        let result = ctx.classify(&item).expect("should classify");
        assert_eq!(result.class, PeerInputClass::PlainEvent);
        assert!(result.from_peer.is_none());
    }

    #[test]
    fn no_lifecycle_leakage_plain_events_remain_the_only_non_peer_inbox_class() {
        let ctx = make_context(false, TrustedPeers::new(), vec![]);
        let item = InboxItem::PlainEvent {
            body: "event".to_string(),
            source: meerkat_core::PlainEventSource::Tcp,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            interaction_id: None,
            blocks: None,
            render_metadata: None,
        };
        let result = ctx.classify(&item).expect("should classify");
        assert_eq!(result.class, PeerInputClass::PlainEvent);
    }

    #[test]
    fn classify_lifecycle_without_peer_param_falls_back_to_sender() {
        let sender = make_keypair();
        let trusted = make_trusted_peers("orchestrator", &sender.public_key());
        let ctx = make_context(true, trusted, vec![]);
        let envelope = make_envelope(
            &sender,
            MessageKind::Request {
                intent: "mob.peer_added".to_string(),
                params: serde_json::json!({}),
                handling_mode: None,
            },
        );
        let item = InboxItem::External { envelope };
        let result = ctx.classify(&item).expect("should classify");
        assert_eq!(result.class, PeerInputClass::PeerLifecycleAdded);
        assert_eq!(result.lifecycle_peer.as_deref(), Some("orchestrator"));
    }

    #[test]
    fn prepare_plain_event_assigns_interaction_id_and_projection() {
        let ctx = make_context(false, TrustedPeers::new(), vec![]);
        let prepared = ctx
            .prepare(InboxItem::PlainEvent {
                body: "event body".to_string(),
                source: meerkat_core::PlainEventSource::Tcp,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                interaction_id: None,
                blocks: None,
                render_metadata: None,
            })
            .expect("plain event should prepare");

        assert_eq!(prepared.class, PeerInputClass::PlainEvent);
        assert_eq!(prepared.raw_kind, RawPeerKind::PlainEvent);
        assert!(
            Uuid::parse_str(&prepared.raw_item_id).is_ok(),
            "plain events should receive a stable generated ingress id"
        );
        assert_eq!(
            prepared.text_projection,
            "[EVENT via tcp] event body".to_string()
        );

        match prepared.item {
            InboxItem::PlainEvent { interaction_id, .. } => {
                assert_eq!(
                    interaction_id.map(|id| id.to_string()),
                    Some(prepared.raw_item_id)
                );
            }
            other => panic!("expected normalized plain event, got {other:?}"),
        }
    }

    #[test]
    fn prepared_lifecycle_request_round_trips_to_peer_authority() {
        let sender = make_keypair();
        let trusted = make_trusted_peers("orchestrator", &sender.public_key());
        let ctx = make_context(true, trusted, vec![]);
        let envelope = make_envelope(
            &sender,
            MessageKind::Request {
                intent: "mob.peer_added".to_string(),
                params: serde_json::json!({"peer": "new-agent"}),
                handling_mode: None,
            },
        );
        let prepared = ctx
            .prepare(InboxItem::External {
                envelope: envelope.clone(),
            })
            .expect("trusted lifecycle request should prepare");

        assert_eq!(prepared.class, PeerInputClass::PeerLifecycleAdded);
        assert_eq!(prepared.raw_kind, RawPeerKind::PeerLifecycleAdded);

        let mut authority = PeerCommsAuthority::new();
        authority
            .apply(PeerCommsInput::TrustPeer {
                peer_id: PeerId(sender.public_key().to_peer_id()),
            })
            .expect("trust peer");
        authority
            .apply(PeerCommsInput::ReceivePeerEnvelope {
                raw_item_id: RawItemId(prepared.raw_item_id.clone()),
                peer_id: PeerId(sender.public_key().to_peer_id()),
                raw_kind: prepared.raw_kind.clone(),
                text_projection: prepared.text_projection.clone(),
                content_shape: prepared.content_shape.clone(),
                request_id: prepared.request_id.clone().map(RequestId),
                reservation_key: None,
                lifecycle_peer: prepared.lifecycle_peer.clone(),
            })
            .expect("receive prepared envelope");

        let transition = authority
            .apply(PeerCommsInput::SubmitTypedPeerInput {
                raw_item_id: RawItemId(prepared.raw_item_id),
            })
            .expect("submit prepared envelope");

        match &transition.effects[0] {
            PeerCommsEffect::SubmitPeerInputCandidate(effect) => {
                assert_eq!(effect.peer_input_class, PeerInputClass::PeerLifecycleAdded);
                assert_eq!(effect.lifecycle_peer.as_deref(), Some("new-agent"));
                assert_eq!(effect.request_id, Some(RequestId(envelope.id.to_string())));
                assert!(
                    effect.text_projection.contains("mob.peer_added"),
                    "request projection should survive the authority round-trip"
                );
            }
        }
    }

    #[test]
    fn prepared_request_round_trips_to_peer_authority_when_auth_is_open() {
        let sender = make_keypair();
        let ctx = make_context(false, TrustedPeers::new(), vec![]);
        let envelope = make_envelope(
            &sender,
            MessageKind::Request {
                intent: "review".to_string(),
                params: serde_json::json!({"scope": "peer"}),
                handling_mode: None,
            },
        );
        let prepared = ctx
            .prepare(InboxItem::External {
                envelope: envelope.clone(),
            })
            .expect("auth-open request should prepare even without trusted peer entry");

        let mut authority = PeerCommsAuthority::new_with_auth_required(false);
        authority
            .apply(PeerCommsInput::ReceivePeerEnvelope {
                raw_item_id: RawItemId(prepared.raw_item_id.clone()),
                peer_id: PeerId(sender.public_key().to_peer_id()),
                raw_kind: prepared.raw_kind.clone(),
                text_projection: prepared.text_projection.clone(),
                content_shape: prepared.content_shape.clone(),
                request_id: prepared.request_id.clone().map(RequestId),
                reservation_key: None,
                lifecycle_peer: prepared.lifecycle_peer.clone(),
            })
            .expect("receive prepared auth-open envelope");

        let transition = authority
            .apply(PeerCommsInput::SubmitTypedPeerInput {
                raw_item_id: RawItemId(prepared.raw_item_id),
            })
            .expect("submit prepared auth-open envelope");

        match &transition.effects[0] {
            PeerCommsEffect::SubmitPeerInputCandidate(effect) => {
                assert_eq!(effect.peer_input_class, PeerInputClass::ActionableRequest);
                assert_eq!(effect.request_id, Some(RequestId(envelope.id.to_string())));
                assert!(
                    effect.text_projection.contains("Intent: review"),
                    "request projection should survive the auth-open authority round-trip"
                );
            }
        }
    }

    #[test]
    fn message_intent_peer_added_roundtrip() {
        let intent = MessageIntent::from("mob.peer_added");
        assert_eq!(intent, MessageIntent::PeerAdded);
        assert_eq!(intent.as_str(), "mob.peer_added");
    }

    #[test]
    fn message_intent_peer_retired_roundtrip() {
        let intent = MessageIntent::from("mob.peer_retired");
        assert_eq!(intent, MessageIntent::PeerRetired);
        assert_eq!(intent.as_str(), "mob.peer_retired");
    }
}
