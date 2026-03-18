//! Ingress classification for incoming peer/event traffic.
//!
//! Runs synchronously in the sending task using receiver-owned trust/auth state.

use crate::agent::types::MessageIntent;
use crate::inproc::InprocRegistry;
use crate::trust::TrustedPeers;
use crate::types::{InboxItem, MessageKind};
use meerkat_core::PeerInputClass;
use std::collections::HashSet;
use std::sync::Arc;

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
pub(crate) struct ClassificationResult {
    pub(crate) class: PeerInputClass,
    pub(crate) from_peer: Option<String>,
    pub(crate) lifecycle_peer: Option<String>,
}

impl IngressClassificationContext {
    /// Classify an inbox item. Returns `None` when the item should be dropped
    /// at ingress (untrusted sender with `require_peer_auth` enabled).
    pub(crate) fn classify(&self, item: &InboxItem) -> Option<ClassificationResult> {
        match item {
            InboxItem::External { envelope } => {
                let trusted = self.trusted_peers.read();
                let from_peer = trusted.get_peer(&envelope.from).map(|p| p.name.clone());

                if self.require_peer_auth && from_peer.is_none() {
                    // Drop untrusted input at ingress. Snapshot semantics:
                    // later trust additions do not resurrect this item.
                    return None;
                }

                let from_name = from_peer.unwrap_or_else(|| {
                    InprocRegistry::global()
                        .get_name_by_pubkey(&envelope.from)
                        .unwrap_or_else(|| envelope.from.to_peer_id())
                });

                Some(match &envelope.kind {
                    MessageKind::Message { .. } => ClassificationResult {
                        class: PeerInputClass::ActionableMessage,
                        from_peer: Some(from_name),
                        lifecycle_peer: None,
                    },
                    MessageKind::Request { intent, params } => {
                        let typed_intent = MessageIntent::from(intent.as_str());
                        match typed_intent {
                            MessageIntent::PeerAdded => {
                                let peer = params
                                    .get("peer")
                                    .and_then(|v| v.as_str())
                                    .filter(|s| !s.is_empty())
                                    .unwrap_or(from_name.as_str())
                                    .to_string();
                                ClassificationResult {
                                    class: PeerInputClass::PeerLifecycleAdded,
                                    from_peer: Some(from_name),
                                    lifecycle_peer: Some(peer),
                                }
                            }
                            MessageIntent::PeerRetired => {
                                let peer = params
                                    .get("peer")
                                    .and_then(|v| v.as_str())
                                    .filter(|s| !s.is_empty())
                                    .unwrap_or(from_name.as_str())
                                    .to_string();
                                ClassificationResult {
                                    class: PeerInputClass::PeerLifecycleRetired,
                                    from_peer: Some(from_name),
                                    lifecycle_peer: Some(peer),
                                }
                            }
                            // Check silent intents against the wire string, not
                            // just the Custom variant. This preserves the pre-0.4.10
                            // behavior where silent_comms_intents matched built-in
                            // intent names like "review", "status", etc.
                            _ if self.silent_intents.contains(intent.as_str()) => {
                                ClassificationResult {
                                    class: PeerInputClass::SilentRequest,
                                    from_peer: Some(from_name),
                                    lifecycle_peer: None,
                                }
                            }
                            _ => ClassificationResult {
                                class: PeerInputClass::ActionableRequest,
                                from_peer: Some(from_name),
                                lifecycle_peer: None,
                            },
                        }
                    }
                    MessageKind::Response { .. } => ClassificationResult {
                        class: PeerInputClass::Response,
                        from_peer: Some(from_name),
                        lifecycle_peer: None,
                    },
                    MessageKind::Ack { .. } => ClassificationResult {
                        class: PeerInputClass::Ack,
                        from_peer: Some(from_name),
                        lifecycle_peer: None,
                    },
                })
            }
            InboxItem::PlainEvent { .. } => Some(ClassificationResult {
                class: PeerInputClass::PlainEvent,
                from_peer: None,
                lifecycle_peer: None,
            }),
        }
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
            },
        );
        let item = InboxItem::External { envelope };
        let result = ctx.classify(&item).expect("should classify");
        assert_eq!(result.class, PeerInputClass::PeerLifecycleAdded);
        assert_eq!(result.lifecycle_peer.as_deref(), Some("orchestrator"));
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
