//! Ingress classification for incoming peer/event traffic.
//!
//! Runs synchronously in the sending task using receiver-owned trust/auth state.

use crate::inproc::InprocRegistry;
use crate::peer_types::ContentShape as PeerContentShape;
use crate::trust::TrustedPeers;
use crate::types::{InboxItem, MessageKind};
use meerkat_core::{
    InteractionId, PeerIngressAdmission, PeerIngressAuthDecision, PeerIngressConvention,
    PeerIngressEnvelopeFacts, PeerIngressEnvelopeKind, PeerIngressFact, PeerIngressIdentity,
    PeerIngressKind, PeerIngressMachinePolicy, PeerIngressPlainEventFacts, PeerInputClass,
    TerminalityClass, handles::PeerCommsHandle,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use uuid::Uuid;

pub(crate) type PeerCommsHandleSlot = Arc<parking_lot::RwLock<Option<Arc<dyn PeerCommsHandle>>>>;

/// Receiver-owned context for synchronous ingress classification.
///
/// Cloned into `InboxSender` so classification runs in the sending task
/// using receiver-owned trust/auth state.
pub(crate) struct IngressClassificationContext {
    pub(crate) require_peer_auth: bool,
    pub(crate) trusted_peers: Arc<parking_lot::RwLock<TrustedPeers>>,
    pub(crate) ingress_policy: Arc<PeerIngressMachinePolicy>,
    pub(crate) peer_comms_handle: PeerCommsHandleSlot,
    pub(crate) require_machine_authority: Arc<AtomicBool>,
    pub(crate) inproc_namespace: Option<String>,
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
    pub(crate) from_peer_id: Option<String>,
    pub(crate) lifecycle_peer: Option<String>,
    pub(crate) response_terminality: Option<TerminalityClass>,
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
            if self.require_machine_authority.load(Ordering::SeqCst) {
                tracing::warn!(
                    "runtime-backed peer ingress has no machine authority; rejecting external envelope"
                );
                return None;
            }
            return Some(self.ingress_policy.classify_external_envelope(&facts));
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
            if self.require_machine_authority.load(Ordering::SeqCst) {
                tracing::warn!(
                    "runtime-backed peer ingress has no machine authority; rejecting plain event"
                );
                return None;
            }
            return Some(self.ingress_policy.classify_plain_event_facts(&facts));
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
    /// facts. Standalone comms runtimes leave `peer_comms_handle` unset and
    /// use the core compatibility policy only while
    /// `require_machine_authority` is false.
    pub(crate) fn prepare(&self, item: InboxItem) -> Option<PreparedIngressItem> {
        match item {
            InboxItem::External { envelope } => {
                let trusted = self.trusted_peers.read();
                let from_peer = trusted.get_peer(&envelope.from).map(|p| p.name.clone());
                let trusted_sender = from_peer.is_some();

                let from_peer_id = envelope.from.to_peer_id();
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
                let request_id = parse_admission_interaction_id(admission.request_id.as_deref());
                let classification = admission.classification;
                let lifecycle_peer = admission.lifecycle_peer.clone();
                let convention = match &envelope.kind {
                    MessageKind::Message { .. } => PeerIngressConvention::Message,
                    MessageKind::Request { intent, params, .. } => {
                        if let Some(kind) = classification.lifecycle_kind {
                            let peer = lifecycle_peer.clone().unwrap_or_else(|| {
                                meerkat_core::peer_lifecycle_subject(params, from_name.as_str())
                            });
                            PeerIngressConvention::Lifecycle { kind, peer }
                        } else {
                            PeerIngressConvention::Request {
                                request_id: admission
                                    .request_id
                                    .clone()
                                    .unwrap_or_else(|| envelope.id.to_string()),
                                intent: intent.clone(),
                            }
                        }
                    }
                    MessageKind::Lifecycle { kind, params } => {
                        let peer = lifecycle_peer.clone().unwrap_or_else(|| {
                            meerkat_core::peer_lifecycle_subject(params, from_name.as_str())
                        });
                        PeerIngressConvention::Lifecycle { kind: *kind, peer }
                    }
                    MessageKind::Response {
                        in_reply_to,
                        status,
                        ..
                    } => PeerIngressConvention::Response {
                        in_reply_to: request_id.unwrap_or(InteractionId(*in_reply_to)),
                        status: (*status).into(),
                    },
                    MessageKind::Ack { in_reply_to } => PeerIngressConvention::Ack {
                        in_reply_to: request_id.unwrap_or(InteractionId(*in_reply_to)),
                    },
                };
                let ingress_fact = PeerIngressFact::peer(
                    InteractionId(envelope.id),
                    classification.class,
                    classification.kind,
                    Some(classification.auth),
                    PeerIngressIdentity::new(from_peer_id, from_name.clone(), convention)
                        .with_signing_pubkey(envelope.from.0),
                );

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
                    raw_item_id: InteractionId(envelope.id),
                    kind: classification.kind,
                    class: classification.class,
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
                from_peer_id: prepared.ingress_fact.canonical_peer_id_string(),
                lifecycle_peer: prepared.lifecycle_peer,
                response_terminality: prepared.response_terminality,
            })
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
    use crate::trust::TrustedPeer;
    use crate::types::Envelope;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use uuid::Uuid;

    struct RecordingPeerCommsHandle {
        external_calls: AtomicUsize,
        plain_calls: AtomicUsize,
        reject_external: bool,
        reject_plain: bool,
        policy: PeerIngressMachinePolicy,
        external_override: Option<PeerIngressAdmission>,
        plain_override: Option<PeerIngressAdmission>,
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
                reject_external: false,
                reject_plain: false,
                policy: PeerIngressMachinePolicy::from_silent_intents(silent_intents),
                external_override: None,
                plain_override: None,
            })
        }

        fn rejecting_external() -> Arc<Self> {
            Arc::new(Self {
                external_calls: AtomicUsize::new(0),
                plain_calls: AtomicUsize::new(0),
                reject_external: true,
                reject_plain: false,
                policy: PeerIngressMachinePolicy::default(),
                external_override: None,
                plain_override: None,
            })
        }

        fn rejecting_plain() -> Arc<Self> {
            Arc::new(Self {
                external_calls: AtomicUsize::new(0),
                plain_calls: AtomicUsize::new(0),
                reject_external: false,
                reject_plain: true,
                policy: PeerIngressMachinePolicy::default(),
                external_override: None,
                plain_override: None,
            })
        }

        fn accepting_with_external_override(admission: PeerIngressAdmission) -> Arc<Self> {
            Arc::new(Self {
                external_calls: AtomicUsize::new(0),
                plain_calls: AtomicUsize::new(0),
                reject_external: false,
                reject_plain: false,
                policy: PeerIngressMachinePolicy::default(),
                external_override: Some(admission),
                plain_override: None,
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
            if self.reject_external {
                Err(meerkat_core::handles::DslTransitionError::guard_rejected(
                    "test_peer_comms::classify_external_envelope",
                    "machine rejected external ingress",
                ))
            } else {
                Ok(self
                    .external_override
                    .clone()
                    .unwrap_or_else(|| self.policy.classify_external_envelope(&facts)))
            }
        }

        fn classify_plain_event(
            &self,
            facts: PeerIngressPlainEventFacts,
        ) -> Result<PeerIngressAdmission, meerkat_core::handles::DslTransitionError> {
            self.plain_calls.fetch_add(1, Ordering::SeqCst);
            if self.reject_plain {
                Err(meerkat_core::handles::DslTransitionError::guard_rejected(
                    "test_peer_comms::classify_plain_event",
                    "machine rejected plain ingress",
                ))
            } else {
                Ok(self
                    .plain_override
                    .clone()
                    .unwrap_or_else(|| self.policy.classify_plain_event_facts(&facts)))
            }
        }

        fn set_peer_ingress_context(
            &self,
            _keep_alive: bool,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            Ok(())
        }
    }

    fn make_keypair() -> Keypair {
        Keypair::generate()
    }

    fn make_context(
        require_peer_auth: bool,
        trusted_peers: TrustedPeers,
        silent_intents: Vec<&str>,
    ) -> IngressClassificationContext {
        make_context_with_machine_and_namespace(
            require_peer_auth,
            trusted_peers,
            silent_intents,
            None,
            None,
        )
    }

    fn make_context_with_namespace(
        require_peer_auth: bool,
        trusted_peers: TrustedPeers,
        silent_intents: Vec<&str>,
        inproc_namespace: Option<String>,
    ) -> IngressClassificationContext {
        make_context_with_machine_and_namespace(
            require_peer_auth,
            trusted_peers,
            silent_intents,
            None,
            inproc_namespace,
        )
    }

    fn make_context_with_machine(
        require_peer_auth: bool,
        trusted_peers: TrustedPeers,
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
        trusted_peers: TrustedPeers,
        silent_intents: Vec<&str>,
        peer_comms_handle: Option<Arc<dyn meerkat_core::handles::PeerCommsHandle>>,
        inproc_namespace: Option<String>,
    ) -> IngressClassificationContext {
        IngressClassificationContext {
            require_peer_auth,
            trusted_peers: Arc::new(parking_lot::RwLock::new(trusted_peers)),
            ingress_policy: Arc::new(PeerIngressMachinePolicy::from_silent_intents(
                silent_intents,
            )),
            peer_comms_handle: Arc::new(parking_lot::RwLock::new(peer_comms_handle)),
            require_machine_authority: Arc::new(AtomicBool::new(false)),
            inproc_namespace,
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

    fn facts_for_envelope(from_peer: &str, envelope: &Envelope) -> PeerIngressEnvelopeFacts {
        PeerIngressEnvelopeFacts {
            item_id: envelope.id.to_string(),
            from_peer: from_peer.to_string(),
            from_peer_id: envelope.from.to_peer_id(),
            kind: match &envelope.kind {
                MessageKind::Message { body, .. } => {
                    PeerIngressEnvelopeKind::Message { body: body.clone() }
                }
                MessageKind::Request { intent, params, .. } => PeerIngressEnvelopeKind::Request {
                    intent: intent.clone(),
                    params: params.clone(),
                },
                MessageKind::Lifecycle { kind, params } => PeerIngressEnvelopeKind::Lifecycle {
                    kind: *kind,
                    params: params.clone(),
                },
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
        }
    }

    fn assert_prepared_matches_admission(
        prepared: &PreparedIngressItem,
        admission: &PeerIngressAdmission,
    ) {
        assert_eq!(prepared.class, admission.classification.class);
        assert_eq!(prepared.kind, admission.classification.kind);
        assert_eq!(prepared.auth, admission.classification.auth);
        assert_eq!(prepared.lifecycle_peer, admission.lifecycle_peer);
        assert_eq!(
            prepared.request_id.map(|id| id.to_string()),
            admission.request_id
        );
        assert_eq!(
            prepared.response_terminality,
            admission.classification.response_terminality
        );
        assert_eq!(prepared.text_projection, admission.rendered_text);
    }

    #[test]
    fn transport_classifier_uses_machine_admission_output_not_local_policy() {
        let sender = make_keypair();
        let trusted = make_trusted_peers("sender-agent", &sender.public_key());
        let machine_request_id = Uuid::new_v4().to_string();
        let override_admission = PeerIngressAdmission {
            classification: meerkat_core::PeerIngressClassification::required(
                PeerInputClass::SilentRequest,
                PeerIngressKind::Request,
            ),
            lifecycle_peer: None,
            request_id: Some(machine_request_id),
            rendered_text: "machine-rendered-request".to_string(),
        };
        let machine =
            RecordingPeerCommsHandle::accepting_with_external_override(override_admission.clone());
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
                params: serde_json::json!({}),
                handling_mode: None,
            },
        );

        let prepared = ctx
            .prepare(InboxItem::External { envelope })
            .expect("machine accepted request should prepare");

        assert_prepared_matches_admission(&prepared, &override_admission);
        assert_eq!(machine.external_calls(), 1);
    }

    #[test]
    fn transport_classifier_parity_matches_machine_policy_for_core_cases() {
        let sender = make_keypair();
        let trusted = make_trusted_peers("sender-agent", &sender.public_key());
        let machine = RecordingPeerCommsHandle::accepting_with_silent(["probe.silent"]);
        let ctx = make_context_with_machine(
            true,
            trusted,
            vec![],
            Some(machine.clone() as Arc<dyn meerkat_core::handles::PeerCommsHandle>),
        );
        let policy = PeerIngressMachinePolicy::from_silent_intents(["probe.silent"]);
        let response_id = Uuid::new_v4();

        let cases = vec![
            make_envelope(
                &sender,
                MessageKind::Message {
                    blocks: None,
                    body: "hello".to_string(),
                    handling_mode: None,
                },
            ),
            make_envelope(
                &sender,
                MessageKind::Request {
                    intent: "review".to_string(),
                    params: serde_json::json!({"pr": 42}),
                    handling_mode: None,
                },
            ),
            make_envelope(
                &sender,
                MessageKind::Request {
                    intent: "probe.silent".to_string(),
                    params: serde_json::json!({}),
                    handling_mode: None,
                },
            ),
            make_envelope(
                &sender,
                MessageKind::Request {
                    intent: "mob.peer_added".to_string(),
                    params: serde_json::json!({"peer": "worker-1"}),
                    handling_mode: None,
                },
            ),
            make_envelope(
                &sender,
                MessageKind::Lifecycle {
                    kind: meerkat_core::comms::PeerLifecycleKind::PeerRetired,
                    params: serde_json::json!({"peer": "worker-2"}),
                },
            ),
            make_envelope(
                &sender,
                MessageKind::Request {
                    intent: meerkat_core::SUPERVISOR_BRIDGE_INTENT.to_string(),
                    params: serde_json::json!({}),
                    handling_mode: None,
                },
            ),
            make_envelope(
                &sender,
                MessageKind::Response {
                    in_reply_to: response_id,
                    status: crate::types::Status::Accepted,
                    result: serde_json::json!(null),
                    handling_mode: None,
                },
            ),
            make_envelope(
                &sender,
                MessageKind::Response {
                    in_reply_to: response_id,
                    status: crate::types::Status::Completed,
                    result: serde_json::json!({"ok": true}),
                    handling_mode: None,
                },
            ),
            make_envelope(
                &sender,
                MessageKind::Ack {
                    in_reply_to: response_id,
                },
            ),
        ];

        for envelope in cases {
            let expected =
                policy.classify_external_envelope(&facts_for_envelope("sender-agent", &envelope));
            let prepared = ctx
                .prepare(InboxItem::External { envelope })
                .expect("machine-accepted ingress should prepare");
            assert_prepared_matches_admission(&prepared, &expected);
        }

        let plain_facts = PeerIngressPlainEventFacts {
            source_name: "tcp".to_string(),
            body: "plain event".to_string(),
        };
        let expected_plain = policy.classify_plain_event_facts(&plain_facts);
        let prepared_plain = ctx
            .prepare(InboxItem::PlainEvent {
                blocks: None,
                body: plain_facts.body,
                source: meerkat_core::PlainEventSource::Tcp,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                interaction_id: None,
                render_metadata: None,
            })
            .expect("machine-accepted plain event should prepare");
        assert_prepared_matches_admission(&prepared_plain, &expected_plain);
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
        let expected_peer_id = sender.public_key().to_peer_id().to_string();
        assert_eq!(result.class, PeerInputClass::ActionableMessage);
        assert_eq!(result.from_peer.as_deref(), Some("sender-agent"));
        assert_eq!(
            result.from_peer_id.as_deref(),
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
                handling_mode: None,
            },
        );
        let item = InboxItem::External { envelope };
        let result = ctx.classify(&item).expect("should classify");
        let expected_peer_id = sender.public_key().to_peer_id().to_string();
        assert_eq!(result.class, PeerInputClass::ActionableRequest);
        assert_eq!(result.from_peer.as_deref(), Some("sender-agent"));
        assert_eq!(
            result.from_peer_id.as_deref(),
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
                handling_mode: None,
            },
        );
        let item = InboxItem::External { envelope };
        let result = ctx.classify(&item).expect("should classify");
        assert_eq!(result.class, PeerInputClass::ResponseTerminal);
        let expected_peer_id = sender.public_key().to_peer_id().to_string();
        assert_eq!(
            result.from_peer_id.as_deref(),
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
        let expected_peer_id = sender.public_key().to_peer_id().to_string();
        assert_eq!(
            result.from_peer_id.as_deref(),
            Some(expected_peer_id.as_str())
        );
    }

    #[test]
    fn auth_exempt_supervisor_intent_cannot_bypass_machine_rejection() {
        let sender = make_keypair();
        let machine = RecordingPeerCommsHandle::rejecting_external();
        let ctx = make_context_with_machine(
            true,
            TrustedPeers::new(),
            vec![],
            Some(machine.clone() as Arc<dyn meerkat_core::handles::PeerCommsHandle>),
        );
        let envelope = make_envelope(
            &sender,
            MessageKind::Request {
                intent: meerkat_core::SUPERVISOR_BRIDGE_INTENT.to_string(),
                params: serde_json::json!({}),
                handling_mode: None,
            },
        );

        let result = ctx.classify(&InboxItem::External { envelope });

        assert!(
            result.is_none(),
            "machine rejection must fail closed before auth exemptions apply"
        );
        assert_eq!(machine.external_calls(), 1);
    }

    #[test]
    fn runtime_backed_ingress_without_machine_handle_fails_closed() {
        let sender = make_keypair();
        let trusted = make_trusted_peers("sender-agent", &sender.public_key());
        let ctx = make_context(true, trusted, vec!["review"]);
        ctx.require_machine_authority.store(true, Ordering::SeqCst);
        let envelope = make_envelope(
            &sender,
            MessageKind::Request {
                intent: "review".to_string(),
                params: serde_json::json!({}),
                handling_mode: None,
            },
        );

        let result = ctx.classify(&InboxItem::External { envelope });

        assert!(
            result.is_none(),
            "runtime-backed ingress must not silently use standalone compatibility policy"
        );
    }

    #[test]
    fn runtime_backed_missing_machine_rejects_auth_lifecycle_response_and_plain_cases() {
        let sender = make_keypair();
        let trusted = make_trusted_peers("sender-agent", &sender.public_key());
        let ctx = make_context(true, trusted, vec!["review"]);
        ctx.require_machine_authority.store(true, Ordering::SeqCst);
        let response_id = Uuid::new_v4();

        let external_cases = [
            MessageKind::Request {
                intent: meerkat_core::SUPERVISOR_BRIDGE_INTENT.to_string(),
                params: serde_json::json!({}),
                handling_mode: None,
            },
            MessageKind::Request {
                intent: "mob.peer_added".to_string(),
                params: serde_json::json!({"peer": "new-agent"}),
                handling_mode: None,
            },
            MessageKind::Response {
                in_reply_to: response_id,
                status: crate::types::Status::Completed,
                result: serde_json::json!({"ok": true}),
                handling_mode: None,
            },
        ];

        for kind in external_cases {
            let result = ctx.classify(&InboxItem::External {
                envelope: make_envelope(&sender, kind),
            });
            assert!(
                result.is_none(),
                "runtime-backed ingress must fail closed instead of letting the compatibility classifier decide"
            );
        }

        let plain_result = ctx.classify(&InboxItem::PlainEvent {
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
    fn machine_result_overrides_local_auth_lifecycle_and_terminality_conventions() {
        let sender = make_keypair();

        let auth_override = PeerIngressAdmission {
            classification: meerkat_core::PeerIngressClassification::required(
                PeerInputClass::ActionableRequest,
                PeerIngressKind::Request,
            ),
            lifecycle_peer: None,
            request_id: None,
            rendered_text: "machine-required-supervisor-bridge".to_string(),
        };
        let auth_machine =
            RecordingPeerCommsHandle::accepting_with_external_override(auth_override);
        let auth_ctx = make_context_with_machine(
            true,
            TrustedPeers::new(),
            vec![],
            Some(auth_machine.clone() as Arc<dyn meerkat_core::handles::PeerCommsHandle>),
        );
        let auth_result = auth_ctx.classify(&InboxItem::External {
            envelope: make_envelope(
                &sender,
                MessageKind::Request {
                    intent: meerkat_core::SUPERVISOR_BRIDGE_INTENT.to_string(),
                    params: serde_json::json!({}),
                    handling_mode: None,
                },
            ),
        });
        assert!(
            auth_result.is_none(),
            "local supervisor-bridge exemption must not admit when machine returned auth-required"
        );
        assert_eq!(auth_machine.external_calls(), 1);

        let lifecycle_override = PeerIngressAdmission {
            classification: meerkat_core::PeerIngressClassification::required(
                PeerInputClass::ActionableRequest,
                PeerIngressKind::Request,
            ),
            lifecycle_peer: None,
            request_id: None,
            rendered_text: "machine-actionable-peer-added".to_string(),
        };
        let lifecycle_machine =
            RecordingPeerCommsHandle::accepting_with_external_override(lifecycle_override);
        let trusted = make_trusted_peers("orchestrator", &sender.public_key());
        let lifecycle_ctx = make_context_with_machine(
            true,
            trusted,
            vec![],
            Some(lifecycle_machine.clone() as Arc<dyn meerkat_core::handles::PeerCommsHandle>),
        );
        let lifecycle = lifecycle_ctx
            .classify(&InboxItem::External {
                envelope: make_envelope(
                    &sender,
                    MessageKind::Request {
                        intent: "mob.peer_added".to_string(),
                        params: serde_json::json!({"peer": "new-agent"}),
                        handling_mode: None,
                    },
                ),
            })
            .expect("machine-accepted request should classify");
        assert_eq!(lifecycle.class, PeerInputClass::ActionableRequest);
        assert_eq!(lifecycle.lifecycle_peer, None);
        assert_eq!(lifecycle_machine.external_calls(), 1);

        let response_override = PeerIngressAdmission {
            classification: meerkat_core::PeerIngressClassification {
                class: PeerInputClass::ResponseProgress,
                kind: PeerIngressKind::Response,
                auth: PeerIngressAuthDecision::Required,
                lifecycle_kind: None,
                response_terminality: Some(TerminalityClass::Progress),
            },
            lifecycle_peer: None,
            request_id: Some(Uuid::new_v4().to_string()),
            rendered_text: "machine-progress-response".to_string(),
        };
        let response_machine =
            RecordingPeerCommsHandle::accepting_with_external_override(response_override);
        let trusted = make_trusted_peers("sender-agent", &sender.public_key());
        let response_ctx = make_context_with_machine(
            true,
            trusted,
            vec![],
            Some(response_machine.clone() as Arc<dyn meerkat_core::handles::PeerCommsHandle>),
        );
        let response = response_ctx
            .classify(&InboxItem::External {
                envelope: make_envelope(
                    &sender,
                    MessageKind::Response {
                        in_reply_to: Uuid::new_v4(),
                        status: crate::types::Status::Completed,
                        result: serde_json::json!({"ok": true}),
                        handling_mode: None,
                    },
                ),
            })
            .expect("machine-accepted response should classify");
        assert_eq!(response.class, PeerInputClass::ResponseProgress);
        assert_eq!(
            response.response_terminality,
            Some(TerminalityClass::Progress)
        );
        assert_eq!(response_machine.external_calls(), 1);
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
            .classify(&InboxItem::External {
                envelope: make_envelope(
                    &sender,
                    MessageKind::Request {
                        intent: "review".to_string(),
                        params: serde_json::json!({}),
                        handling_mode: None,
                    },
                ),
            })
            .expect("machine-accepted silent request should classify");
        assert_eq!(silent.class, PeerInputClass::SilentRequest);

        let lifecycle = ctx
            .classify(&InboxItem::External {
                envelope: make_envelope(
                    &sender,
                    MessageKind::Request {
                        intent: "mob.peer_added".to_string(),
                        params: serde_json::json!({"peer": "new-agent"}),
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
            "[COMMS REQUEST from peer_id {} (display_name: sender-agent) (id: {request_id})]\nIntent: review",
            sender.public_key().to_peer_id()
        )));
        assert!(prepared.text_projection.contains("\"peer_id\""));
        assert!(!prepared.text_projection.contains("to=\""));
        assert_eq!(machine.external_calls(), 1);
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
    fn plain_event_projection_requires_machine_classification() {
        let machine = RecordingPeerCommsHandle::accepting();
        let ctx = make_context_with_machine(
            false,
            TrustedPeers::new(),
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
        assert_eq!(prepared.text_projection, "[EVENT via tcp] event body");
        assert_eq!(machine.plain_calls(), 1);
    }

    #[test]
    fn plain_event_machine_rejection_fails_closed() {
        let machine = RecordingPeerCommsHandle::rejecting_plain();
        let ctx = make_context_with_machine(
            false,
            TrustedPeers::new(),
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
            Uuid::parse_str(&prepared.raw_item_id.to_string()).is_ok(),
            "plain events should receive a stable generated ingress id"
        );
        assert_eq!(
            prepared.text_projection,
            "[EVENT via tcp] event body".to_string()
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
