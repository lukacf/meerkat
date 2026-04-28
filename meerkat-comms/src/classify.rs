//! Ingress classification for incoming peer/event traffic.
//!
//! Runs synchronously in the sending task using receiver-owned trust/auth state.

use crate::inproc::InprocRegistry;
use crate::peer_types::ContentShape as PeerContentShape;
use crate::trust::TrustedPeers;
use crate::types::{InboxItem, MessageKind};
use meerkat_core::{
    PeerIngressAuthDecision, PeerIngressKind, PeerIngressMachinePolicy, PeerInputClass,
};
use std::sync::Arc;
use uuid::Uuid;

/// Receiver-owned context for synchronous ingress classification.
///
/// Cloned into `InboxSender` so classification runs in the sending task
/// using receiver-owned trust/auth state.
pub(crate) struct IngressClassificationContext {
    pub(crate) require_peer_auth: bool,
    pub(crate) trusted_peers: Arc<parking_lot::RwLock<TrustedPeers>>,
    pub(crate) ingress_policy: Arc<PeerIngressMachinePolicy>,
}

/// Result of classifying an inbox item.
///
/// `None` from `classify()` means the item should be dropped at ingress
/// (e.g., untrusted sender when `require_peer_auth` is enabled).
#[cfg(test)]
pub(crate) struct ClassificationResult {
    pub(crate) class: PeerInputClass,
    pub(crate) auth: PeerIngressAuthDecision,
    pub(crate) from_peer: Option<String>,
    pub(crate) lifecycle_peer: Option<String>,
}

/// Classified ingress descriptor for one inbox item.
///
/// Carries the typed classification produced at ingress so the classified
/// inbox queue can enqueue entries with full correlation context.
pub(crate) struct PreparedIngressItem {
    pub(crate) item: InboxItem,
    pub(crate) raw_item_id: String,
    pub(crate) kind: PeerIngressKind,
    pub(crate) class: PeerInputClass,
    pub(crate) auth: PeerIngressAuthDecision,
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
                        .unwrap_or_else(|| envelope.from.to_pubkey_string())
                });

                let (classification, lifecycle_peer, request_id) = match &envelope.kind {
                    MessageKind::Message { .. } => {
                        (self.ingress_policy.classify_message(), None, None)
                    }
                    MessageKind::Request { intent, params, .. } => {
                        let classification = self.ingress_policy.classify_request_intent(intent);
                        let lifecycle_peer = classification.lifecycle_kind.map(|_| {
                            meerkat_core::peer_lifecycle_subject(params, from_name.as_str())
                        });
                        (
                            classification,
                            lifecycle_peer,
                            Some(envelope.id.to_string()),
                        )
                    }
                    MessageKind::Lifecycle { kind, params } => {
                        let classification = self.ingress_policy.classify_lifecycle(*kind);
                        let peer = meerkat_core::peer_lifecycle_subject(params, from_name.as_str());
                        (classification, Some(peer), None)
                    }
                    MessageKind::Response {
                        in_reply_to,
                        status,
                        ..
                    } => (
                        self.ingress_policy.classify_response((*status).into()),
                        None,
                        Some(in_reply_to.to_string()),
                    ),
                    MessageKind::Ack { in_reply_to } => (
                        self.ingress_policy.classify_ack(),
                        None,
                        Some(in_reply_to.to_string()),
                    ),
                };

                let text_projection = match &envelope.kind {
                    MessageKind::Message { body, .. } => {
                        meerkat_core::format_peer_message_projection(&from_name, body)
                    }
                    MessageKind::Lifecycle { .. } => String::new(),
                    MessageKind::Request { intent, params, .. } => {
                        meerkat_core::format_peer_request_projection(
                            &from_name,
                            envelope.id,
                            intent,
                            params,
                        )
                    }
                    MessageKind::Response {
                        in_reply_to,
                        status,
                        result,
                        ..
                    } => meerkat_core::format_peer_response_projection(
                        &from_name,
                        *in_reply_to,
                        (*status).into(),
                        result,
                    ),
                    MessageKind::Ack { in_reply_to } => {
                        meerkat_core::format_peer_ack_projection(&from_name, *in_reply_to)
                    }
                };

                let content_shape = match &envelope.kind {
                    MessageKind::Message { body, blocks, .. } => {
                        content_shape_for_text_and_blocks(body, blocks.as_deref())
                    }
                    MessageKind::Lifecycle { .. } => PeerContentShape::Text,
                    MessageKind::Request { .. }
                    | MessageKind::Response { .. }
                    | MessageKind::Ack { .. } => PeerContentShape::Text,
                };

                Some(PreparedIngressItem {
                    raw_item_id: envelope.id.to_string(),
                    kind: classification.kind,
                    class: classification.class,
                    auth: classification.auth,
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
                let classification = self.ingress_policy.classify_plain_event();
                Some(PreparedIngressItem {
                    raw_item_id: interaction_id.to_string(),
                    kind: classification.kind,
                    class: classification.class,
                    auth: classification.auth,
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
                && !prepared.trusted_sender
                && !prepared.auth.is_exempt();
            if drop_untrusted_external {
                return None;
            }
            Some(ClassificationResult {
                class: prepared.class,
                auth: prepared.auth,
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
            ingress_policy: Arc::new(PeerIngressMachinePolicy::from_silent_intents(
                silent_intents,
            )),
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
    fn classify_untrusted_supervisor_bridge_request_remains_admissible() {
        let sender = make_keypair();
        let trusted = TrustedPeers::new(); // sender NOT trusted yet
        let ctx = make_context(true, trusted, vec![]);
        let envelope = make_envelope(
            &sender,
            MessageKind::Request {
                intent: meerkat_core::SUPERVISOR_BRIDGE_INTENT.to_string(),
                params: serde_json::json!({
                    "command": "bind_member",
                    "supervisor": {
                        "name": "mob/__mob_supervisor__",
                        "peer_id": sender.public_key().to_peer_id(),
                        "address": "inproc://mob/__mob_supervisor__"
                    },
                    "epoch": 1,
                    "protocol_version": 1,
                    "expected_peer_id": "peer-id",
                    "expected_address": "inproc://peer"
                }),
                handling_mode: None,
            },
        );
        let item = InboxItem::External { envelope };
        let result = ctx
            .classify(&item)
            .expect("bind_member should remain admissible");
        assert_eq!(result.class, PeerInputClass::ActionableRequest);
        assert_eq!(
            result.auth,
            meerkat_core::PeerIngressAuthDecision::Exempt(
                meerkat_core::PeerIngressAuthExemption::SupervisorBridge
            )
        );
        assert_eq!(
            result.from_peer,
            Some(sender.public_key().to_pubkey_string())
        );
    }

    #[test]
    fn classify_untrusted_legacy_bridge_intent_drops_at_ingress() {
        let sender = make_keypair();
        let trusted = TrustedPeers::new(); // sender NOT trusted yet
        let ctx = make_context(true, trusted, vec![]);
        let envelope = make_envelope(
            &sender,
            MessageKind::Request {
                intent: "mob.runtime.bind_member".to_string(),
                params: serde_json::json!({}),
                handling_mode: None,
            },
        );
        let item = InboxItem::External { envelope };
        let result = ctx.classify(&item);
        assert!(
            result.is_none(),
            "legacy bridge intents should no longer bypass auth at ingress"
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
        assert_eq!(prepared.kind, PeerIngressKind::PlainEvent);
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
}
