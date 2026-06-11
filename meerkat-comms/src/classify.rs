//! Ingress classification for incoming peer/event traffic.
//!
//! Runs synchronously in the sending task using receiver-owned trust/auth state.

use crate::inproc::InprocRegistry;
use crate::peer_types::ContentShape as PeerContentShape;
use crate::trust::TrustStore;
use crate::types::{InboxItem, MessageKind};
use meerkat_core::{
    InteractionId, PeerIngressAdmission, PeerIngressAuthDecision, PeerIngressConvention,
    PeerIngressEnvelopeFacts, PeerIngressEnvelopeKind, PeerIngressFact, PeerIngressIdentity,
    PeerIngressKind, PeerIngressPlainEventFacts, PeerInputClass, TerminalityClass,
    handles::PeerCommsHandle,
};
use std::sync::Arc;
use uuid::Uuid;

pub(crate) type PeerCommsHandleSlot = Arc<parking_lot::RwLock<Option<Arc<dyn PeerCommsHandle>>>>;

/// Receiver-owned context for synchronous ingress classification.
///
/// Cloned into `InboxSender` so classification runs in the sending task
/// using receiver-owned trust/auth state.
pub(crate) struct IngressClassificationContext {
    pub(crate) require_peer_auth: bool,
    pub(crate) trusted_peers: Arc<parking_lot::RwLock<TrustStore>>,
    pub(crate) peer_comms_handle: PeerCommsHandleSlot,
    pub(crate) inproc_namespace: Option<String>,
}

/// Classified ingress descriptor for one inbox item.
///
/// Carries the typed classification produced at ingress so the classified
/// inbox queue can enqueue entries with full correlation context.
pub(crate) struct PreparedIngressItem {
    pub(crate) item: InboxItem,
    pub(crate) raw_item_id: InteractionId,
    pub(crate) kind: PeerIngressKind,
    pub(crate) class: PeerInputClass,
    /// Machine-owned actionable grouping verdict, mirrored from the
    /// MeerkatMachine PeerIngress classification effect. The inbox mirrors this
    /// bit instead of re-deriving the class->actionable grouping locally.
    pub(crate) actionable: bool,
    pub(crate) auth: PeerIngressAuthDecision,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) trusted_sender: bool,
    pub(crate) from_peer: Option<String>,
    pub(crate) lifecycle_peer: Option<String>,
    pub(crate) response_terminality: Option<TerminalityClass>,
    pub(crate) ingress_fact: PeerIngressFact,
    pub(crate) text_projection: String,
    #[allow(dead_code)]
    pub(crate) content_shape: PeerContentShape,
    pub(crate) request_id: Option<InteractionId>,
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

fn parse_admission_interaction_id(raw: Option<&str>) -> Option<InteractionId> {
    raw.and_then(|id| Uuid::parse_str(id).ok())
        .map(InteractionId)
}

impl IngressClassificationContext {
    fn machine_classify_external_envelope(
        &self,
        facts: PeerIngressEnvelopeFacts,
    ) -> Option<PeerIngressAdmission> {
        let handle = self.peer_comms_handle.read().clone();
        let Some(handle) = handle else {
            tracing::warn!(
                "classified peer ingress has no machine authority; rejecting external envelope"
            );
            return None;
        };

        match handle.classify_external_envelope(facts) {
            Ok(admission) => Some(admission),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "classified ingress rejected external envelope before shell auth/routing classification"
                );
                None
            }
        }
    }

    fn machine_classify_plain_event(
        &self,
        facts: PeerIngressPlainEventFacts,
    ) -> Option<PeerIngressAdmission> {
        let handle = self.peer_comms_handle.read().clone();
        let Some(handle) = handle else {
            tracing::warn!(
                "classified peer ingress has no machine authority; rejecting plain event"
            );
            return None;
        };

        match handle.classify_plain_event(facts) {
            Ok(admission) => Some(admission),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "classified ingress rejected plain event before shell routing classification"
                );
                None
            }
        }
    }

    fn inproc_display_name_for(&self, pubkey: &crate::identity::PubKey) -> Option<String> {
        let registry = InprocRegistry::global();
        self.inproc_namespace
            .as_deref()
            .and_then(|namespace| registry.get_name_by_pubkey_in_namespace(namespace, pubkey))
            .or_else(|| registry.get_name_by_pubkey(pubkey))
    }

    /// Prepare an inbox item for classified ingress.
    ///
    /// Runtime-backed inboxes hand parsed transport facts to the
    /// MeerkatMachine DSL handle and enqueue the returned machine admission
    /// facts. Without an installed machine handle, classified ingress fails
    /// closed instead of deriving machine facts locally.
    pub(crate) fn prepare(&self, item: InboxItem) -> Option<PreparedIngressItem> {
        match item {
            InboxItem::External { envelope } => {
                let from_peer_id = envelope.from.to_peer_id();
                let from_peer = self
                    .trusted_peers
                    .read()
                    .get(&from_peer_id)
                    .map(|entry| entry.name.as_string());
                let trusted_sender = from_peer.is_some();

                let registry_name = self.inproc_display_name_for(&envelope.from);

                let from_name = registry_name
                    .or(from_peer)
                    .unwrap_or_else(|| envelope.from.to_pubkey_string());

                let facts = PeerIngressEnvelopeFacts {
                    item_id: envelope.id.to_string(),
                    from_peer: from_name.clone(),
                    from_peer_id,
                    kind: match &envelope.kind {
                        MessageKind::Message { body, .. } => {
                            PeerIngressEnvelopeKind::Message { body: body.clone() }
                        }
                        MessageKind::Request { intent, params, .. } => {
                            PeerIngressEnvelopeKind::Request {
                                intent: intent.clone(),
                                params: params.clone(),
                            }
                        }
                        MessageKind::Lifecycle { kind, params } => {
                            PeerIngressEnvelopeKind::Lifecycle {
                                kind: *kind,
                                params: params.clone(),
                            }
                        }
                        MessageKind::Response {
                            in_reply_to,
                            status,
                            result,
                            ..
                        } => PeerIngressEnvelopeKind::Response {
                            in_reply_to: in_reply_to.to_string(),
                            status: (*status).into(),
                            result: result.clone(),
                        },
                        MessageKind::Ack { in_reply_to } => PeerIngressEnvelopeKind::Ack {
                            in_reply_to: in_reply_to.to_string(),
                        },
                    },
                };
                let admission = self.machine_classify_external_envelope(facts)?;
                // R084: the admitted sender identity is the machine-echoed
                // canonical peer id from the classification effect, not the
                // shell-local transport copy. Fail closed if it is missing.
                let Some(canonical_from_peer_id) = admission.from_peer_id else {
                    tracing::warn!(
                        "machine classified peer envelope without canonical sender peer id; rejecting ingress"
                    );
                    return None;
                };
                let request_id = parse_admission_interaction_id(admission.request_id.as_deref());
                let classification = admission.classification;
                let lifecycle_peer = admission.lifecycle_peer.clone();
                let convention = match &envelope.kind {
                    MessageKind::Message { .. } => PeerIngressConvention::Message,
                    MessageKind::Request {
                        intent, params: _, ..
                    } => {
                        if let Some(kind) = classification.lifecycle_kind {
                            let Some(peer) = lifecycle_peer.clone() else {
                                tracing::warn!(
                                    intent = intent.as_str(),
                                    "machine classified lifecycle request without lifecycle peer; rejecting ingress"
                                );
                                return None;
                            };
                            PeerIngressConvention::Lifecycle { kind, peer }
                        } else {
                            let Some(machine_request_id) = admission.request_id.clone() else {
                                tracing::warn!(
                                    intent = intent.as_str(),
                                    "machine classified peer request without request id; rejecting ingress"
                                );
                                return None;
                            };
                            PeerIngressConvention::Request {
                                request_id: machine_request_id,
                                intent: intent.clone(),
                            }
                        }
                    }
                    MessageKind::Lifecycle { kind, params: _ } => {
                        let Some(peer) = lifecycle_peer.clone() else {
                            tracing::warn!(
                                kind = ?kind,
                                "machine classified lifecycle event without lifecycle peer; rejecting ingress"
                            );
                            return None;
                        };
                        PeerIngressConvention::Lifecycle { kind: *kind, peer }
                    }
                    MessageKind::Response { status, .. } => PeerIngressConvention::Response {
                        in_reply_to: request_id?,
                        status: (*status).into(),
                    },
                    MessageKind::Ack { in_reply_to: _ } => PeerIngressConvention::Ack {
                        in_reply_to: request_id?,
                    },
                };
                let ingress_fact = PeerIngressFact::peer(
                    InteractionId(envelope.id),
                    classification.class,
                    classification.kind,
                    Some(classification.auth),
                    PeerIngressIdentity::new(canonical_from_peer_id, from_name.clone(), convention)
                        .with_signing_pubkey(envelope.from.0),
                );

                let content_shape = match &envelope.kind {
                    MessageKind::Message { body, blocks, .. } => {
                        content_shape_for_text_and_blocks(body, blocks.as_deref())
                    }
                    MessageKind::Request { blocks, .. } => {
                        content_shape_for_text_and_blocks("", blocks.as_deref())
                    }
                    MessageKind::Lifecycle { .. }
                    | MessageKind::Response { .. }
                    | MessageKind::Ack { .. } => PeerContentShape::Text,
                };

                Some(PreparedIngressItem {
                    raw_item_id: InteractionId(envelope.id),
                    kind: classification.kind,
                    class: classification.class,
                    actionable: classification.actionable,
                    auth: classification.auth,
                    trusted_sender,
                    from_peer: Some(from_name),
                    lifecycle_peer,
                    response_terminality: classification.response_terminality,
                    text_projection: admission.rendered_text,
                    ingress_fact,
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
                let facts = PeerIngressPlainEventFacts {
                    source_name: source.to_string(),
                    body: body.clone(),
                };
                let admission = self.machine_classify_plain_event(facts)?;
                let classification = admission.classification;
                let content_shape = content_shape_for_text_and_blocks(&body, blocks.as_deref());
                let ingress_fact = PeerIngressFact::plain_event(
                    meerkat_core::InteractionId(interaction_id),
                    source.to_string(),
                    classification.class,
                    classification.kind,
                );
                Some(PreparedIngressItem {
                    raw_item_id: InteractionId(interaction_id),
                    kind: classification.kind,
                    class: classification.class,
                    actionable: classification.actionable,
                    auth: classification.auth,
                    trusted_sender: true,
                    from_peer: None,
                    lifecycle_peer: None,
                    response_terminality: classification.response_terminality,
                    text_projection: admission.rendered_text,
                    ingress_fact,
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
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::sync::Arc;

    pub(crate) fn runtime_peer_comms_handle_with_silent<I, S>(
        silent_intents: I,
    ) -> Arc<dyn meerkat_core::handles::PeerCommsHandle>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        meerkat_runtime::test_peer_comms_handle_with_silent(silent_intents)
    }

    pub(crate) fn runtime_peer_comms_handle() -> Arc<dyn meerkat_core::handles::PeerCommsHandle> {
        runtime_peer_comms_handle_with_silent(std::iter::empty::<String>())
    }

    pub(crate) fn classification_context(
        trusted_peers: TrustStore,
        require_peer_auth: bool,
    ) -> Arc<IngressClassificationContext> {
        classification_context_shared(
            Arc::new(parking_lot::RwLock::new(trusted_peers)),
            require_peer_auth,
        )
    }

    pub(crate) fn classification_context_shared(
        trusted_peers: Arc<parking_lot::RwLock<TrustStore>>,
        require_peer_auth: bool,
    ) -> Arc<IngressClassificationContext> {
        Arc::new(IngressClassificationContext {
            require_peer_auth,
            trusted_peers,
            peer_comms_handle: Arc::new(parking_lot::RwLock::new(
                Some(runtime_peer_comms_handle()),
            )),
            inproc_namespace: None,
        })
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    clippy::redundant_clone,
    clippy::unwrap_used
)]
mod tests {
    use super::*;
    use crate::identity::{Keypair, PubKey, Signature};
    use crate::trust::TrustEntry;
    use crate::types::Envelope;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use uuid::Uuid;

    struct RecordingPeerCommsHandle {
        external_calls: AtomicUsize,
        plain_calls: AtomicUsize,
        inner: Arc<dyn meerkat_core::handles::PeerCommsHandle>,
    }

    impl RecordingPeerCommsHandle {
        fn accepting() -> Arc<Self> {
            Self::accepting_with_silent(std::iter::empty::<&str>())
        }

        fn accepting_with_silent<I, S>(silent_intents: I) -> Arc<Self>
        where
            I: IntoIterator<Item = S>,
            S: Into<String>,
        {
            Arc::new(Self {
                external_calls: AtomicUsize::new(0),
                plain_calls: AtomicUsize::new(0),
                inner: test_support::runtime_peer_comms_handle_with_silent(silent_intents),
            })
        }

        fn rejecting_external() -> Arc<Self> {
            Arc::new(Self {
                external_calls: AtomicUsize::new(0),
                plain_calls: AtomicUsize::new(0),
                inner: Arc::new(RejectingPeerCommsHandle::external()),
            })
        }

        fn rejecting_plain() -> Arc<Self> {
            Arc::new(Self {
                external_calls: AtomicUsize::new(0),
                plain_calls: AtomicUsize::new(0),
                inner: Arc::new(RejectingPeerCommsHandle::plain()),
            })
        }

        fn external_calls(&self) -> usize {
            self.external_calls.load(Ordering::SeqCst)
        }

        fn plain_calls(&self) -> usize {
            self.plain_calls.load(Ordering::SeqCst)
        }
    }

    impl meerkat_core::handles::PeerCommsHandle for RecordingPeerCommsHandle {
        fn classify_external_envelope(
            &self,
            facts: PeerIngressEnvelopeFacts,
        ) -> Result<PeerIngressAdmission, meerkat_core::handles::DslTransitionError> {
            self.external_calls.fetch_add(1, Ordering::SeqCst);
            self.inner.classify_external_envelope(facts)
        }

        fn classify_plain_event(
            &self,
            facts: PeerIngressPlainEventFacts,
        ) -> Result<PeerIngressAdmission, meerkat_core::handles::DslTransitionError> {
            self.plain_calls.fetch_add(1, Ordering::SeqCst);
            self.inner.classify_plain_event(facts)
        }

        fn resolve_peer_ingress_receive(
            &self,
            facts: meerkat_core::PeerIngressReceiveFacts,
        ) -> Result<
            meerkat_core::PeerIngressReceiveAuthority,
            meerkat_core::handles::DslTransitionError,
        > {
            self.inner.resolve_peer_ingress_receive(facts)
        }

        fn resolve_peer_ingress_dequeue(
            &self,
            facts: meerkat_core::PeerIngressDequeueFacts,
        ) -> Result<
            meerkat_core::PeerIngressDequeueAuthority,
            meerkat_core::handles::DslTransitionError,
        > {
            self.inner.resolve_peer_ingress_dequeue(facts)
        }

        fn set_peer_ingress_context(
            &self,
            keep_alive: bool,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            self.inner.set_peer_ingress_context(keep_alive)
        }
    }

    enum RejectKind {
        External,
        Plain,
    }

    struct RejectingPeerCommsHandle {
        kind: RejectKind,
        delegate: Arc<dyn meerkat_core::handles::PeerCommsHandle>,
    }

    impl RejectingPeerCommsHandle {
        fn external() -> Self {
            Self {
                kind: RejectKind::External,
                delegate: test_support::runtime_peer_comms_handle(),
            }
        }

        fn plain() -> Self {
            Self {
                kind: RejectKind::Plain,
                delegate: test_support::runtime_peer_comms_handle(),
            }
        }
    }

    impl meerkat_core::handles::PeerCommsHandle for RejectingPeerCommsHandle {
        fn classify_external_envelope(
            &self,
            facts: PeerIngressEnvelopeFacts,
        ) -> Result<PeerIngressAdmission, meerkat_core::handles::DslTransitionError> {
            if matches!(self.kind, RejectKind::External) {
                return Err(meerkat_core::handles::DslTransitionError::guard_rejected(
                    "test_peer_comms::classify_external_envelope",
                    "machine rejected external ingress",
                ));
            }
            self.delegate.classify_external_envelope(facts)
        }

        fn classify_plain_event(
            &self,
            facts: PeerIngressPlainEventFacts,
        ) -> Result<PeerIngressAdmission, meerkat_core::handles::DslTransitionError> {
            if matches!(self.kind, RejectKind::Plain) {
                return Err(meerkat_core::handles::DslTransitionError::guard_rejected(
                    "test_peer_comms::classify_plain_event",
                    "machine rejected plain ingress",
                ));
            }
            self.delegate.classify_plain_event(facts)
        }

        fn resolve_peer_ingress_receive(
            &self,
            facts: meerkat_core::PeerIngressReceiveFacts,
        ) -> Result<
            meerkat_core::PeerIngressReceiveAuthority,
            meerkat_core::handles::DslTransitionError,
        > {
            self.delegate.resolve_peer_ingress_receive(facts)
        }

        fn resolve_peer_ingress_dequeue(
            &self,
            facts: meerkat_core::PeerIngressDequeueFacts,
        ) -> Result<
            meerkat_core::PeerIngressDequeueAuthority,
            meerkat_core::handles::DslTransitionError,
        > {
            self.delegate.resolve_peer_ingress_dequeue(facts)
        }

        fn set_peer_ingress_context(
            &self,
            keep_alive: bool,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            self.delegate.set_peer_ingress_context(keep_alive)
        }
    }

    fn make_keypair() -> Keypair {
        Keypair::generate()
    }

    fn make_context(
        require_peer_auth: bool,
        trusted_peers: TrustStore,
        silent_intents: Vec<&str>,
    ) -> IngressClassificationContext {
        let handle =
            RecordingPeerCommsHandle::accepting_with_silent(silent_intents.iter().copied());
        make_context_with_machine_and_namespace(
            require_peer_auth,
            trusted_peers,
            silent_intents,
            Some(handle as Arc<dyn meerkat_core::handles::PeerCommsHandle>),
            None,
        )
    }

    fn make_context_with_namespace(
        require_peer_auth: bool,
        trusted_peers: TrustStore,
        silent_intents: Vec<&str>,
        inproc_namespace: Option<String>,
    ) -> IngressClassificationContext {
        let handle =
            RecordingPeerCommsHandle::accepting_with_silent(silent_intents.iter().copied());
        make_context_with_machine_and_namespace(
            require_peer_auth,
            trusted_peers,
            silent_intents,
            Some(handle as Arc<dyn meerkat_core::handles::PeerCommsHandle>),
            inproc_namespace,
        )
    }

    fn make_context_without_machine(
        require_peer_auth: bool,
        trusted_peers: TrustStore,
    ) -> IngressClassificationContext {
        make_context_with_machine_and_namespace(
            require_peer_auth,
            trusted_peers,
            vec![],
            None,
            None,
        )
    }

    fn make_context_with_machine(
        require_peer_auth: bool,
        trusted_peers: TrustStore,
        silent_intents: Vec<&str>,
        peer_comms_handle: Option<Arc<dyn meerkat_core::handles::PeerCommsHandle>>,
    ) -> IngressClassificationContext {
        make_context_with_machine_and_namespace(
            require_peer_auth,
            trusted_peers,
            silent_intents,
            peer_comms_handle,
            None,
        )
    }

    fn make_context_with_machine_and_namespace(
        require_peer_auth: bool,
        trusted_peers: TrustStore,
        _silent_intents: Vec<&str>,
        peer_comms_handle: Option<Arc<dyn meerkat_core::handles::PeerCommsHandle>>,
        inproc_namespace: Option<String>,
    ) -> IngressClassificationContext {
        IngressClassificationContext {
            require_peer_auth,
            trusted_peers: Arc::new(parking_lot::RwLock::new(trusted_peers)),
            peer_comms_handle: Arc::new(parking_lot::RwLock::new(peer_comms_handle)),
            inproc_namespace,
        }
    }

    fn make_trusted_peers(name: &str, pubkey: &PubKey) -> TrustStore {
        let mut store = TrustStore::new();
        store
            .insert(TrustEntry {
                peer_id: pubkey.to_peer_id(),
                name: meerkat_core::comms::PeerName::new(name).expect("valid peer name"),
                pubkey: *pubkey,
                address: meerkat_core::comms::PeerAddress::parse("tcp://127.0.0.1:4200")
                    .expect("valid peer address"),
                meta: crate::PeerMeta::default(),
            })
            .expect("trusted test peer should insert");
        store
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
    fn transport_classifier_uses_machine_admission_output_not_local_decision() {
        let sender = make_keypair();
        let trusted = make_trusted_peers("sender-agent", &sender.public_key());
        let machine = RecordingPeerCommsHandle::accepting_with_silent(["review"]);
        let ctx = make_context_with_machine(
            true,
            trusted,
            vec!["review"],
            Some(machine.clone() as Arc<dyn meerkat_core::handles::PeerCommsHandle>),
        );
        let envelope = make_envelope(
            &sender,
            MessageKind::Request {
                intent: "review".to_string(),
                params: serde_json::json!({}),
                blocks: None,
                handling_mode: None,
            },
        );
        let request_id = envelope.id;

        let prepared = ctx
            .prepare(InboxItem::External { envelope })
            .expect("machine accepted request should prepare");

        assert_eq!(prepared.class, PeerInputClass::SilentRequest);
        assert_eq!(prepared.kind, PeerIngressKind::Request);
        assert_eq!(
            prepared.request_id,
            Some(meerkat_core::InteractionId(request_id))
        );
        assert!(prepared.text_projection.starts_with(&format!(
            "Peer request from peer_id {} (display_name: sender-agent) (id: {request_id})\nIntent: review",
            sender.public_key().to_peer_id()
        )));
        assert_eq!(machine.external_calls(), 1);
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
        let result = ctx.prepare(item).expect("should classify");
        let expected_peer_id = sender.public_key().to_peer_id().to_string();
        assert_eq!(result.class, PeerInputClass::ActionableMessage);
        assert_eq!(result.from_peer.as_deref(), Some("sender-agent"));
        assert_eq!(
            result.ingress_fact.canonical_peer_id_string().as_deref(),
            Some(expected_peer_id.as_str())
        );
        assert!(result.lifecycle_peer.is_none());
    }

    #[test]
    fn classify_message_prefers_inproc_source_label_for_prompt_display() {
        let sender = make_keypair();
        let sender_pubkey = sender.public_key();
        let canonical_label = sender_pubkey.to_peer_id().to_string();
        let source_label = format!("sender-source-{}", Uuid::new_v4().simple());
        let namespace = format!("classify-{}", Uuid::new_v4().simple());
        let trusted = make_trusted_peers(&canonical_label, &sender_pubkey);
        let ctx = make_context_with_namespace(true, trusted, vec![], Some(namespace.clone()));
        let (_, inbox_sender) = crate::Inbox::new();
        InprocRegistry::global().register_with_meta_in_namespace(
            &namespace,
            source_label.clone(),
            sender_pubkey,
            inbox_sender,
            crate::PeerMeta::default(),
        );

        let envelope = make_envelope(
            &sender,
            MessageKind::Message {
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
            },
        );
        let item = InboxItem::External { envelope };
        let prepared = ctx.prepare(item).expect("should prepare");
        assert!(prepared.trusted_sender);
        assert_eq!(prepared.from_peer.as_deref(), Some(source_label.as_str()));
        assert_eq!(
            prepared.text_projection,
            meerkat_core::format_peer_message_projection(&source_label, "hello")
        );
        assert_eq!(
            prepared.ingress_fact.canonical_peer_id_string().as_deref(),
            Some(canonical_label.as_str())
        );

        assert!(InprocRegistry::global().unregister_in_namespace(&namespace, &sender_pubkey));
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
                blocks: None,
                handling_mode: None,
            },
        );
        let item = InboxItem::External { envelope };
        let result = ctx.prepare(item).expect("should classify");
        let expected_peer_id = sender.public_key().to_peer_id().to_string();
        assert_eq!(result.class, PeerInputClass::ActionableRequest);
        assert_eq!(result.from_peer.as_deref(), Some("sender-agent"));
        assert_eq!(
            result.ingress_fact.canonical_peer_id_string().as_deref(),
            Some(expected_peer_id.as_str())
        );
    }

    #[test]
    fn classify_completed_response_as_terminal_response() {
        let sender = make_keypair();
        let trusted = make_trusted_peers("sender-agent", &sender.public_key());
        let ctx = make_context(true, trusted, vec![]);
        let envelope = make_envelope(
            &sender,
            MessageKind::Response {
                in_reply_to: Uuid::new_v4(),
                status: crate::types::Status::Completed,
                result: serde_json::json!({}),
                blocks: None,
                handling_mode: None,
            },
        );
        let item = InboxItem::External { envelope };
        let result = ctx.prepare(item).expect("should classify");
        assert_eq!(result.class, PeerInputClass::ResponseTerminal);
        let expected_peer_id = sender.public_key().to_peer_id().to_string();
        assert_eq!(
            result.ingress_fact.canonical_peer_id_string().as_deref(),
            Some(expected_peer_id.as_str())
        );
        assert_eq!(
            result.response_terminality,
            Some(TerminalityClass::Terminal {
                disposition: meerkat_core::TerminalDisposition::Completed
            })
        );
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
        let result = ctx.prepare(item).expect("should classify");
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
                blocks: None,
                handling_mode: None,
            },
        );
        let item = InboxItem::External { envelope };
        let result = ctx.prepare(item).expect("should classify");
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
                blocks: None,
                handling_mode: None,
            },
        );
        let item = InboxItem::External { envelope };
        let result = ctx.prepare(item).expect("should classify");
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
                blocks: None,
                handling_mode: None,
            },
        );
        let item = InboxItem::External { envelope };
        let result = ctx.prepare(item).expect("should classify");
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
                blocks: None,
                handling_mode: None,
            },
        );
        let item = InboxItem::External { envelope };
        let result = ctx.prepare(item).expect("should classify");
        assert_eq!(result.class, PeerInputClass::SilentRequest);
    }

    #[test]
    fn classify_untrusted_supervisor_bridge_request_remains_admissible() {
        let sender = make_keypair();
        let trusted = TrustStore::new(); // sender NOT trusted yet
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
                blocks: None,
                handling_mode: None,
            },
        );
        let item = InboxItem::External { envelope };
        let result = ctx
            .prepare(item)
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
        let expected_peer_id = sender.public_key().to_peer_id().to_string();
        assert_eq!(
            result.ingress_fact.canonical_peer_id_string().as_deref(),
            Some(expected_peer_id.as_str())
        );
    }

    #[test]
    fn auth_exempt_supervisor_intent_cannot_bypass_machine_rejection() {
        let sender = make_keypair();
        let machine = RecordingPeerCommsHandle::rejecting_external();
        let ctx = make_context_with_machine(
            true,
            TrustStore::new(),
            vec![],
            Some(machine.clone() as Arc<dyn meerkat_core::handles::PeerCommsHandle>),
        );
        let envelope = make_envelope(
            &sender,
            MessageKind::Request {
                intent: meerkat_core::SUPERVISOR_BRIDGE_INTENT.to_string(),
                params: serde_json::json!({}),
                blocks: None,
                handling_mode: None,
            },
        );

        let result = ctx.prepare(InboxItem::External { envelope });

        assert!(
            result.is_none(),
            "machine rejection must fail closed before auth exemptions apply"
        );
        assert_eq!(machine.external_calls(), 1);
    }

    #[test]
    fn classified_ingress_without_machine_handle_fails_closed() {
        let sender = make_keypair();
        let trusted = make_trusted_peers("sender-agent", &sender.public_key());
        let ctx = make_context_without_machine(true, trusted);
        let envelope = make_envelope(
            &sender,
            MessageKind::Request {
                intent: "review".to_string(),
                params: serde_json::json!({}),
                blocks: None,
                handling_mode: None,
            },
        );

        let result = ctx.prepare(InboxItem::External { envelope });

        assert!(
            result.is_none(),
            "classified ingress must not derive machine facts without an installed machine handle"
        );
    }

    #[test]
    fn missing_machine_rejects_auth_lifecycle_response_and_plain_cases() {
        let sender = make_keypair();
        let trusted = make_trusted_peers("sender-agent", &sender.public_key());
        let ctx = make_context_without_machine(true, trusted);
        let response_id = Uuid::new_v4();

        let external_cases = [
            MessageKind::Request {
                intent: meerkat_core::SUPERVISOR_BRIDGE_INTENT.to_string(),
                params: serde_json::json!({}),
                blocks: None,
                handling_mode: None,
            },
            MessageKind::Request {
                intent: "mob.peer_added".to_string(),
                params: serde_json::json!({"peer": "new-agent"}),
                blocks: None,
                handling_mode: None,
            },
            MessageKind::Response {
                in_reply_to: response_id,
                status: crate::types::Status::Completed,
                result: serde_json::json!({"ok": true}),
                blocks: None,
                handling_mode: None,
            },
        ];

        for kind in external_cases {
            let result = ctx.prepare(InboxItem::External {
                envelope: make_envelope(&sender, kind),
            });
            assert!(
                result.is_none(),
                "classified ingress must fail closed instead of deriving local ingress semantics"
            );
        }

        let plain_result = ctx.prepare(InboxItem::PlainEvent {
            blocks: None,
            body: "event body".to_string(),
            source: meerkat_core::PlainEventSource::Tcp,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            interaction_id: None,
            render_metadata: None,
        });
        assert!(
            plain_result.is_none(),
            "plain event projection must also require machine authority on runtime-backed ingress"
        );
    }

    #[test]
    fn machine_classification_controls_auth_lifecycle_and_terminality() {
        let sender = make_keypair();

        let auth_machine = RecordingPeerCommsHandle::accepting();
        let auth_ctx = make_context_with_machine(
            true,
            TrustStore::new(),
            vec![],
            Some(auth_machine.clone() as Arc<dyn meerkat_core::handles::PeerCommsHandle>),
        );
        let auth_result = auth_ctx.prepare(InboxItem::External {
            envelope: make_envelope(
                &sender,
                MessageKind::Request {
                    intent: meerkat_core::SUPERVISOR_BRIDGE_INTENT.to_string(),
                    params: serde_json::json!({}),
                    blocks: None,
                    handling_mode: None,
                },
            ),
        });
        let auth_result = auth_result.expect("machine auth exemption should classify");
        assert_eq!(
            auth_result.auth,
            meerkat_core::PeerIngressAuthDecision::Exempt(
                meerkat_core::PeerIngressAuthExemption::SupervisorBridge
            )
        );
        assert_eq!(auth_machine.external_calls(), 1);

        let lifecycle_machine = RecordingPeerCommsHandle::accepting();
        let trusted = make_trusted_peers("orchestrator", &sender.public_key());
        let lifecycle_ctx = make_context_with_machine(
            true,
            trusted,
            vec![],
            Some(lifecycle_machine.clone() as Arc<dyn meerkat_core::handles::PeerCommsHandle>),
        );
        let lifecycle = lifecycle_ctx
            .prepare(InboxItem::External {
                envelope: make_envelope(
                    &sender,
                    MessageKind::Request {
                        intent: "mob.peer_added".to_string(),
                        params: serde_json::json!({"peer": "new-agent"}),
                        blocks: None,
                        handling_mode: None,
                    },
                ),
            })
            .expect("machine-accepted request should classify");
        assert_eq!(lifecycle.class, PeerInputClass::PeerLifecycleAdded);
        assert_eq!(lifecycle.lifecycle_peer.as_deref(), Some("new-agent"));
        assert_eq!(lifecycle_machine.external_calls(), 1);

        let response_machine = RecordingPeerCommsHandle::accepting();
        let trusted = make_trusted_peers("sender-agent", &sender.public_key());
        let response_ctx = make_context_with_machine(
            true,
            trusted,
            vec![],
            Some(response_machine.clone() as Arc<dyn meerkat_core::handles::PeerCommsHandle>),
        );
        let response = response_ctx
            .prepare(InboxItem::External {
                envelope: make_envelope(
                    &sender,
                    MessageKind::Response {
                        in_reply_to: Uuid::new_v4(),
                        status: crate::types::Status::Completed,
                        result: serde_json::json!({"ok": true}),
                        blocks: None,
                        handling_mode: None,
                    },
                ),
            })
            .expect("machine-accepted response should classify");
        assert_eq!(response.class, PeerInputClass::ResponseTerminal);
        assert_eq!(
            response.response_terminality,
            Some(TerminalityClass::Terminal {
                disposition: meerkat_core::TerminalDisposition::Completed
            })
        );
        assert_eq!(response_machine.external_calls(), 1);
    }

    #[test]
    fn machine_handoff_precedes_silent_routing_and_lifecycle_extraction() {
        let sender = make_keypair();
        let trusted = make_trusted_peers("orchestrator", &sender.public_key());
        let machine = RecordingPeerCommsHandle::accepting_with_silent(["review"]);
        let ctx = make_context_with_machine(
            true,
            trusted,
            vec!["review"],
            Some(machine.clone() as Arc<dyn meerkat_core::handles::PeerCommsHandle>),
        );

        let silent = ctx
            .prepare(InboxItem::External {
                envelope: make_envelope(
                    &sender,
                    MessageKind::Request {
                        intent: "review".to_string(),
                        params: serde_json::json!({}),
                        blocks: None,
                        handling_mode: None,
                    },
                ),
            })
            .expect("machine-accepted silent request should classify");
        assert_eq!(silent.class, PeerInputClass::SilentRequest);

        let lifecycle = ctx
            .prepare(InboxItem::External {
                envelope: make_envelope(
                    &sender,
                    MessageKind::Request {
                        intent: "mob.peer_added".to_string(),
                        params: serde_json::json!({"peer": "new-agent"}),
                        blocks: None,
                        handling_mode: None,
                    },
                ),
            })
            .expect("machine-accepted lifecycle request should classify");
        assert_eq!(lifecycle.class, PeerInputClass::PeerLifecycleAdded);
        assert_eq!(lifecycle.lifecycle_peer.as_deref(), Some("new-agent"));
        assert_eq!(machine.external_calls(), 2);
    }

    #[test]
    fn rendered_request_projection_is_created_only_after_machine_accepts() {
        let sender = make_keypair();
        let trusted = make_trusted_peers("sender-agent", &sender.public_key());
        let machine = RecordingPeerCommsHandle::accepting();
        let ctx = make_context_with_machine(
            true,
            trusted,
            vec![],
            Some(machine.clone() as Arc<dyn meerkat_core::handles::PeerCommsHandle>),
        );
        let envelope = make_envelope(
            &sender,
            MessageKind::Request {
                intent: "review".to_string(),
                params: serde_json::json!({"pr": 42}),
                blocks: None,
                handling_mode: None,
            },
        );
        let request_id = envelope.id.to_string();

        let prepared = ctx
            .prepare(InboxItem::External { envelope })
            .expect("machine accepted request should prepare");

        assert_eq!(prepared.class, PeerInputClass::ActionableRequest);
        let prepared_request_id = prepared.request_id.map(|id| id.to_string());
        assert_eq!(prepared_request_id.as_deref(), Some(request_id.as_str()));
        assert!(prepared.text_projection.starts_with(&format!(
            "Peer request from peer_id {} (display_name: sender-agent) (id: {request_id})\nIntent: review",
            sender.public_key().to_peer_id()
        )));
        assert!(prepared.text_projection.contains("\"peer_id\""));
        assert!(!prepared.text_projection.contains("to=\""));
        assert_eq!(machine.external_calls(), 1);
    }

    #[test]
    fn classify_plain_event() {
        let ctx = make_context(false, TrustStore::new(), vec![]);
        let item = InboxItem::PlainEvent {
            blocks: None,
            body: "event".to_string(),
            source: meerkat_core::PlainEventSource::Tcp,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            interaction_id: None,
            render_metadata: None,
        };
        let result = ctx.prepare(item).expect("should classify");
        assert_eq!(result.class, PeerInputClass::PlainEvent);
        assert!(result.from_peer.is_none());
    }

    #[test]
    fn plain_event_projection_requires_machine_classification() {
        let machine = RecordingPeerCommsHandle::accepting();
        let ctx = make_context_with_machine(
            false,
            TrustStore::new(),
            vec![],
            Some(machine.clone() as Arc<dyn meerkat_core::handles::PeerCommsHandle>),
        );

        let prepared = ctx
            .prepare(InboxItem::PlainEvent {
                blocks: None,
                body: "event body".to_string(),
                source: meerkat_core::PlainEventSource::Tcp,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                interaction_id: None,
                render_metadata: None,
            })
            .expect("machine accepted plain event should prepare");

        assert_eq!(prepared.class, PeerInputClass::PlainEvent);
        assert_eq!(
            prepared.text_projection,
            "External event via tcp: event body"
        );
        assert_eq!(machine.plain_calls(), 1);
    }

    #[test]
    fn plain_event_machine_rejection_fails_closed() {
        let machine = RecordingPeerCommsHandle::rejecting_plain();
        let ctx = make_context_with_machine(
            false,
            TrustStore::new(),
            vec![],
            Some(machine.clone() as Arc<dyn meerkat_core::handles::PeerCommsHandle>),
        );

        let prepared = ctx.prepare(InboxItem::PlainEvent {
            blocks: None,
            body: "event body".to_string(),
            source: meerkat_core::PlainEventSource::Tcp,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            interaction_id: None,
            render_metadata: None,
        });

        assert!(
            prepared.is_none(),
            "machine rejection must fail closed before plain-event routing"
        );
        assert_eq!(machine.plain_calls(), 1);
    }

    #[test]
    fn no_lifecycle_leakage_plain_events_remain_the_only_non_peer_inbox_class() {
        let ctx = make_context(false, TrustStore::new(), vec![]);
        let item = InboxItem::PlainEvent {
            body: "event".to_string(),
            source: meerkat_core::PlainEventSource::Tcp,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            interaction_id: None,
            blocks: None,
            render_metadata: None,
        };
        let result = ctx.prepare(item).expect("should classify");
        assert_eq!(result.class, PeerInputClass::PlainEvent);
    }

    #[test]
    fn classify_lifecycle_without_peer_subject_is_rejected_at_ingress() {
        // K15: a peer lifecycle notice without a valid typed peer subject is
        // a rejected input at classify ingress — it is never attributed to
        // the sender name. (Fails-old: the missing subject silently fell back
        // to the sender's display name.)
        let sender = make_keypair();
        let trusted = make_trusted_peers("orchestrator", &sender.public_key());
        let ctx = make_context(true, trusted, vec![]);
        for params in [
            serde_json::json!({}),
            serde_json::json!({ "peer": "" }),
            serde_json::json!({ "peer": 42 }),
        ] {
            let envelope = make_envelope(
                &sender,
                MessageKind::Request {
                    intent: "mob.peer_added".to_string(),
                    params,
                    blocks: None,
                    handling_mode: None,
                },
            );
            let result = ctx.prepare(InboxItem::External { envelope });
            assert!(
                result.is_none(),
                "lifecycle request without a valid peer subject must fail closed at ingress"
            );
        }
    }

    #[test]
    fn classify_lifecycle_prefers_typed_peer_spec_subject() {
        // K15: `peer_spec` (the canonical typed identity from the wire
        // contract) wins over the presentation `peer` field.
        let sender = make_keypair();
        let trusted = make_trusted_peers("orchestrator", &sender.public_key());
        let ctx = make_context(true, trusted, vec![]);
        let envelope = make_envelope(
            &sender,
            MessageKind::Request {
                intent: "mob.peer_added".to_string(),
                params: serde_json::json!({
                    "peer": "new-agent",
                    "peer_spec": {
                        "name": "mob/new-agent",
                        "peer_id": "pid-new-agent",
                        "address": "inproc://mob/new-agent"
                    }
                }),
                blocks: None,
                handling_mode: None,
            },
        );
        let result = ctx
            .prepare(InboxItem::External { envelope })
            .expect("typed lifecycle params should classify");
        assert_eq!(result.class, PeerInputClass::PeerLifecycleAdded);
        assert_eq!(result.lifecycle_peer.as_deref(), Some("mob/new-agent"));
    }

    #[test]
    fn prepare_plain_event_assigns_interaction_id_and_projection() {
        let ctx = make_context(false, TrustStore::new(), vec![]);
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
            Uuid::parse_str(&prepared.raw_item_id.to_string()).is_ok(),
            "plain events should receive a stable generated ingress id"
        );
        assert_eq!(
            prepared.text_projection,
            "External event via tcp: event body".to_string()
        );

        match prepared.item {
            InboxItem::PlainEvent { interaction_id, .. } => {
                assert_eq!(
                    interaction_id.map(meerkat_core::InteractionId),
                    Some(prepared.raw_item_id)
                );
            }
            other => panic!("expected normalized plain event, got {other:?}"),
        }
    }
}
