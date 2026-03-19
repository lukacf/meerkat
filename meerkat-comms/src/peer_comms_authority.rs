//! Sealed authority module for the PeerComms machine.
//!
//! This module provides typed enums and a sealed mutator trait that enforces
//! all PeerComms state mutations flow through the machine authority.
//! Handwritten shell code (classify.rs, inbox.rs) calls
//! [`PeerCommsAuthority::apply`] and executes returned effects; it cannot
//! mutate canonical state directly.
//!
//! The transition table encoded here is the single source of truth, matching
//! the machine schema in `meerkat-machine-schema/src/catalog/peer_comms.rs`:
//!
//! - 4 states: Absent, Received, Dropped, Delivered
//! - 3 inputs: TrustPeer, ReceivePeerEnvelope, SubmitTypedPeerInput
//! - 5 transitions with guards (peer_is_trusted, peer_is_not_trusted,
//!   item_was_queued, item_was_classified, delivery_drains_queue,
//!   delivery_leaves_more_work)
//! - 1 effect: SubmitPeerInputCandidate
//! - 1 helper: ClassFor (maps RawPeerKind → PeerInputClass)

use meerkat_core::PeerInputClass;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

// ---------------------------------------------------------------------------
// Domain newtypes — mirror the abstract schema type refs
// ---------------------------------------------------------------------------

/// Opaque identifier for a raw inbox item tracked through the ingress pipeline.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct RawItemId(pub String);

/// Opaque peer identifier (public-key-derived string).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct PeerId(pub String);

/// The raw kind tag from the wire envelope, before classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RawPeerKind {
    Request,
    ResponseTerminal,
    ResponseProgress,
    PlainEvent,
    SilentRequest,
    Message,
}

/// Shape of the content payload (used for downstream rendering decisions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContentShape {
    Text,
    Blocks,
    Mixed,
}

/// Optional request correlation identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequestId(pub String);

/// Optional reservation key for interaction-scoped streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReservationKey(pub String);

// ---------------------------------------------------------------------------
// Phase enum — mirrors the machine schema's state variants
// ---------------------------------------------------------------------------

/// Canonical phases for the PeerComms ingress pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PeerIngressState {
    /// No items have been received yet.
    Absent,
    /// At least one trusted item has been received and queued.
    Received,
    /// An untrusted item was dropped at ingress.
    Dropped,
    /// All queued items have been submitted and delivered.
    Delivered,
}

impl PeerIngressState {
    /// Terminal phases cannot accept further inputs (except new items
    /// which would start a fresh machine instance).
    pub(crate) fn is_terminal(self) -> bool {
        matches!(self, Self::Dropped | Self::Delivered)
    }
}

// ---------------------------------------------------------------------------
// Typed input enum — mirrors the machine schema's input variants
// ---------------------------------------------------------------------------

/// Typed inputs for the PeerComms machine.
///
/// Shell code classifies raw commands into these typed inputs, then calls
/// [`PeerCommsAuthority::apply`]. The authority decides transition legality.
#[derive(Debug, Clone)]
pub(crate) enum PeerCommsInput {
    /// Register a peer as trusted.
    TrustPeer {
        peer_id: PeerId,
    },
    /// A raw peer envelope has arrived from the transport layer.
    ReceivePeerEnvelope {
        raw_item_id: RawItemId,
        peer_id: PeerId,
        raw_kind: RawPeerKind,
        text_projection: String,
        content_shape: ContentShape,
        request_id: Option<RequestId>,
        reservation_key: Option<ReservationKey>,
    },
    /// Submit a classified item for delivery to the agent loop.
    SubmitTypedPeerInput {
        raw_item_id: RawItemId,
    },
}

// ---------------------------------------------------------------------------
// Typed effect enum — mirrors the machine schema's effect variants
// ---------------------------------------------------------------------------

/// Effects emitted by PeerComms transitions.
///
/// Shell code receives these from [`PeerCommsAuthority::apply`] and is
/// responsible for executing the side effects (e.g., delivering classified
/// items to the agent loop).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubmitPeerInputCandidate {
    pub raw_item_id: RawItemId,
    pub peer_input_class: PeerInputClass,
    pub text_projection: String,
    pub content_shape: ContentShape,
    pub request_id: Option<RequestId>,
    pub reservation_key: Option<ReservationKey>,
}

/// Effects produced by PeerComms transitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PeerCommsEffect {
    SubmitPeerInputCandidate(SubmitPeerInputCandidate),
}

// ---------------------------------------------------------------------------
// Transition result
// ---------------------------------------------------------------------------

/// Successful transition outcome from the PeerComms authority.
#[derive(Debug)]
pub(crate) struct PeerCommsTransition {
    /// The phase after the transition.
    pub next_phase: PeerIngressState,
    /// Effects to be executed by shell code.
    pub effects: Vec<PeerCommsEffect>,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors from the PeerComms authority.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PeerCommsError {
    #[error("illegal transition from {from:?}: {reason}")]
    IllegalTransition {
        from: PeerIngressState,
        reason: String,
    },
    #[error("guard failed: {guard} (from {from:?})")]
    GuardFailed {
        from: PeerIngressState,
        guard: String,
    },
    #[error("missing state for raw item {0:?}")]
    MissingItemState(RawItemId),
}

// ---------------------------------------------------------------------------
// ClassFor helper — maps RawPeerKind → PeerInputClass
// ---------------------------------------------------------------------------

/// Schema helper: classify a raw peer kind into a `PeerInputClass`.
///
/// This is the authority's version of the classification logic. The existing
/// `classify.rs` does richer classification with silent-intent sets and
/// lifecycle detection — that remains shell work. This helper encodes only
/// the base mapping from the machine schema.
fn class_for(raw_kind: &RawPeerKind) -> PeerInputClass {
    match raw_kind {
        RawPeerKind::Request => PeerInputClass::ActionableRequest,
        RawPeerKind::ResponseTerminal => PeerInputClass::Response,
        RawPeerKind::ResponseProgress => PeerInputClass::Response,
        RawPeerKind::PlainEvent => PeerInputClass::PlainEvent,
        RawPeerKind::SilentRequest => PeerInputClass::SilentRequest,
        RawPeerKind::Message => PeerInputClass::ActionableMessage,
    }
}

// ---------------------------------------------------------------------------
// Canonical machine state (fields)
// ---------------------------------------------------------------------------

/// Canonical machine-owned fields for the PeerComms ingress pipeline.
#[derive(Debug, Clone)]
struct PeerCommsFields {
    trusted_peers: BTreeSet<PeerId>,
    raw_item_peer: BTreeMap<RawItemId, PeerId>,
    raw_item_kind: BTreeMap<RawItemId, RawPeerKind>,
    classified_as: BTreeMap<RawItemId, PeerInputClass>,
    text_projection: BTreeMap<RawItemId, String>,
    content_shape: BTreeMap<RawItemId, ContentShape>,
    request_id: BTreeMap<RawItemId, Option<RequestId>>,
    reservation_key: BTreeMap<RawItemId, Option<ReservationKey>>,
    trusted_snapshot: BTreeMap<RawItemId, bool>,
    submission_queue: VecDeque<RawItemId>,
}

impl PeerCommsFields {
    fn new() -> Self {
        Self {
            trusted_peers: BTreeSet::new(),
            raw_item_peer: BTreeMap::new(),
            raw_item_kind: BTreeMap::new(),
            classified_as: BTreeMap::new(),
            text_projection: BTreeMap::new(),
            content_shape: BTreeMap::new(),
            request_id: BTreeMap::new(),
            reservation_key: BTreeMap::new(),
            trusted_snapshot: BTreeMap::new(),
            submission_queue: VecDeque::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Sealed mutator trait — only the authority implements this
// ---------------------------------------------------------------------------

mod sealed {
    pub trait Sealed {}
}

/// Sealed trait for PeerComms state mutation.
///
/// Only [`PeerCommsAuthority`] implements this. Handwritten code cannot
/// create alternative implementations, ensuring single-source-of-truth
/// semantics for peer ingress state.
pub(crate) trait PeerCommsMutator: sealed::Sealed {
    /// Apply a typed input to the current machine state.
    ///
    /// Returns the transition result including next state and effects,
    /// or an error if the transition is not legal from the current state.
    fn apply(&mut self, input: PeerCommsInput) -> Result<PeerCommsTransition, PeerCommsError>;
}

// ---------------------------------------------------------------------------
// Authority implementation
// ---------------------------------------------------------------------------

/// The canonical authority for PeerComms ingress state.
///
/// Holds the canonical phase + fields and delegates all transitions through
/// the encoded transition table. Shell code (classify.rs, inbox.rs) prepares
/// typed inputs and executes returned effects.
#[derive(Debug, Clone)]
pub(crate) struct PeerCommsAuthority {
    phase: PeerIngressState,
    fields: PeerCommsFields,
}

impl sealed::Sealed for PeerCommsAuthority {}

impl PeerCommsAuthority {
    /// Create an authority in the initial `Absent` state.
    pub(crate) fn new() -> Self {
        Self {
            phase: PeerIngressState::Absent,
            fields: PeerCommsFields::new(),
        }
    }

    /// Create an authority initialized to a specific phase.
    ///
    /// Used for recovery or testing.
    #[cfg(test)]
    pub(crate) fn with_phase(phase: PeerIngressState) -> Self {
        Self {
            phase,
            fields: PeerCommsFields::new(),
        }
    }

    /// Current phase.
    pub(crate) fn phase(&self) -> PeerIngressState {
        self.phase
    }

    /// Number of items in the submission queue.
    pub(crate) fn queue_len(&self) -> usize {
        self.fields.submission_queue.len()
    }

    /// Whether a peer is currently trusted.
    pub(crate) fn is_peer_trusted(&self, peer_id: &PeerId) -> bool {
        self.fields.trusted_peers.contains(peer_id)
    }

    /// Evaluate a transition without committing it.
    fn evaluate(
        &self,
        input: &PeerCommsInput,
    ) -> Result<(PeerIngressState, PeerCommsFields, Vec<PeerCommsEffect>), PeerCommsError> {
        let phase = self.phase;
        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        let next_phase = match input {
            // -----------------------------------------------------------------
            // TrustPeer: Absent|Received → Absent
            // Updates: set_insert trusted_peers
            // -----------------------------------------------------------------
            PeerCommsInput::TrustPeer { peer_id } => {
                match phase {
                    PeerIngressState::Absent | PeerIngressState::Received => {}
                    _ => {
                        return Err(PeerCommsError::IllegalTransition {
                            from: phase,
                            reason: "TrustPeer only valid from Absent or Received".into(),
                        });
                    }
                }
                fields.trusted_peers.insert(peer_id.clone());
                PeerIngressState::Absent
            }

            // -----------------------------------------------------------------
            // ReceivePeerEnvelope (trusted): Absent|Received → Received
            // Guard: peer_is_trusted
            // -----------------------------------------------------------------
            // ReceivePeerEnvelope (untrusted): Absent|Received → Dropped
            // Guard: peer_is_not_trusted
            // -----------------------------------------------------------------
            PeerCommsInput::ReceivePeerEnvelope {
                raw_item_id,
                peer_id,
                raw_kind,
                text_projection,
                content_shape,
                request_id,
                reservation_key,
            } => {
                match phase {
                    PeerIngressState::Absent | PeerIngressState::Received => {}
                    _ => {
                        return Err(PeerCommsError::IllegalTransition {
                            from: phase,
                            reason: "ReceivePeerEnvelope only valid from Absent or Received".into(),
                        });
                    }
                }

                let trusted = fields.trusted_peers.contains(peer_id);

                // Common updates for both trusted and untrusted paths
                fields
                    .raw_item_peer
                    .insert(raw_item_id.clone(), peer_id.clone());
                fields
                    .raw_item_kind
                    .insert(raw_item_id.clone(), raw_kind.clone());
                fields
                    .text_projection
                    .insert(raw_item_id.clone(), text_projection.clone());
                fields
                    .content_shape
                    .insert(raw_item_id.clone(), content_shape.clone());
                fields
                    .request_id
                    .insert(raw_item_id.clone(), request_id.clone());
                fields
                    .reservation_key
                    .insert(raw_item_id.clone(), reservation_key.clone());

                if trusted {
                    // ReceiveTrustedPeerEnvelope transition
                    fields
                        .classified_as
                        .insert(raw_item_id.clone(), class_for(raw_kind));
                    fields
                        .trusted_snapshot
                        .insert(raw_item_id.clone(), true);
                    fields.submission_queue.push_back(raw_item_id.clone());
                    PeerIngressState::Received
                } else {
                    // DropUntrustedPeerEnvelope transition
                    fields
                        .trusted_snapshot
                        .insert(raw_item_id.clone(), false);
                    PeerIngressState::Dropped
                }
            }

            // -----------------------------------------------------------------
            // SubmitTypedPeerInput: Received → Delivered (if queue drains)
            //                       Received → Received (if more work remains)
            // Guards: item_was_queued, item_was_classified,
            //         delivery_drains_queue / delivery_leaves_more_work
            // Emits: SubmitPeerInputCandidate
            // -----------------------------------------------------------------
            PeerCommsInput::SubmitTypedPeerInput { raw_item_id } => {
                if phase != PeerIngressState::Received {
                    return Err(PeerCommsError::IllegalTransition {
                        from: phase,
                        reason: "SubmitTypedPeerInput only valid from Received".into(),
                    });
                }

                // Guard: item_was_queued
                if !fields.submission_queue.contains(raw_item_id) {
                    return Err(PeerCommsError::GuardFailed {
                        from: phase,
                        guard: "item_was_queued".into(),
                    });
                }

                // Guard: item_was_classified
                if !fields.classified_as.contains_key(raw_item_id) {
                    return Err(PeerCommsError::GuardFailed {
                        from: phase,
                        guard: "item_was_classified".into(),
                    });
                }

                // Build effect from state maps
                let peer_input_class = fields
                    .classified_as
                    .get(raw_item_id)
                    .ok_or_else(|| PeerCommsError::MissingItemState(raw_item_id.clone()))?
                    .clone();
                let text_projection = fields
                    .text_projection
                    .get(raw_item_id)
                    .ok_or_else(|| PeerCommsError::MissingItemState(raw_item_id.clone()))?
                    .clone();
                let content_shape_val = fields
                    .content_shape
                    .get(raw_item_id)
                    .ok_or_else(|| PeerCommsError::MissingItemState(raw_item_id.clone()))?
                    .clone();
                let request_id_val = fields
                    .request_id
                    .get(raw_item_id)
                    .ok_or_else(|| PeerCommsError::MissingItemState(raw_item_id.clone()))?
                    .clone();
                let reservation_key_val = fields
                    .reservation_key
                    .get(raw_item_id)
                    .ok_or_else(|| PeerCommsError::MissingItemState(raw_item_id.clone()))?
                    .clone();

                effects.push(PeerCommsEffect::SubmitPeerInputCandidate(
                    SubmitPeerInputCandidate {
                        raw_item_id: raw_item_id.clone(),
                        peer_input_class,
                        text_projection,
                        content_shape: content_shape_val,
                        request_id: request_id_val,
                        reservation_key: reservation_key_val,
                    },
                ));

                // Remove from queue
                fields
                    .submission_queue
                    .retain(|id| id != raw_item_id);

                // Guard: delivery_drains_queue vs delivery_leaves_more_work
                if fields.submission_queue.is_empty() {
                    PeerIngressState::Delivered
                } else {
                    PeerIngressState::Received
                }
            }
        };

        Ok((next_phase, fields, effects))
    }
}

impl PeerCommsMutator for PeerCommsAuthority {
    fn apply(&mut self, input: PeerCommsInput) -> Result<PeerCommsTransition, PeerCommsError> {
        let (next_phase, next_fields, effects) = self.evaluate(&input)?;

        // Commit: update canonical state.
        self.phase = next_phase;
        self.fields = next_fields;

        Ok(PeerCommsTransition {
            next_phase,
            effects,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn make_authority() -> PeerCommsAuthority {
        PeerCommsAuthority::new()
    }

    fn peer(name: &str) -> PeerId {
        PeerId(name.to_string())
    }

    fn item(name: &str) -> RawItemId {
        RawItemId(name.to_string())
    }

    fn receive_envelope(
        raw_item_id: RawItemId,
        peer_id: PeerId,
        raw_kind: RawPeerKind,
    ) -> PeerCommsInput {
        PeerCommsInput::ReceivePeerEnvelope {
            raw_item_id,
            peer_id,
            raw_kind,
            text_projection: "test text".to_string(),
            content_shape: ContentShape::Text,
            request_id: None,
            reservation_key: None,
        }
    }

    // === TrustPeer transitions ===

    #[test]
    fn trust_peer_from_absent() {
        let mut auth = make_authority();
        let t = auth
            .apply(PeerCommsInput::TrustPeer {
                peer_id: peer("alice"),
            })
            .expect("trust from absent");
        assert_eq!(t.next_phase, PeerIngressState::Absent);
        assert!(t.effects.is_empty());
        assert!(auth.is_peer_trusted(&peer("alice")));
    }

    #[test]
    fn trust_peer_from_received() {
        let mut auth = make_authority();
        // First trust and receive to get to Received
        auth.apply(PeerCommsInput::TrustPeer {
            peer_id: peer("alice"),
        })
        .unwrap();
        auth.apply(receive_envelope(
            item("item-1"),
            peer("alice"),
            RawPeerKind::Message,
        ))
        .unwrap();
        assert_eq!(auth.phase(), PeerIngressState::Received);

        // Trust another peer from Received
        let t = auth
            .apply(PeerCommsInput::TrustPeer {
                peer_id: peer("bob"),
            })
            .expect("trust from received");
        // TrustPeer always transitions to Absent per schema
        assert_eq!(t.next_phase, PeerIngressState::Absent);
        assert!(auth.is_peer_trusted(&peer("bob")));
    }

    #[test]
    fn trust_peer_from_dropped_is_rejected() {
        let mut auth = PeerCommsAuthority::with_phase(PeerIngressState::Dropped);
        let result = auth.apply(PeerCommsInput::TrustPeer {
            peer_id: peer("alice"),
        });
        assert!(result.is_err());
    }

    #[test]
    fn trust_peer_from_delivered_is_rejected() {
        let mut auth = PeerCommsAuthority::with_phase(PeerIngressState::Delivered);
        let result = auth.apply(PeerCommsInput::TrustPeer {
            peer_id: peer("alice"),
        });
        assert!(result.is_err());
    }

    // === ReceivePeerEnvelope — trusted path ===

    #[test]
    fn receive_trusted_envelope_transitions_to_received() {
        let mut auth = make_authority();
        auth.apply(PeerCommsInput::TrustPeer {
            peer_id: peer("alice"),
        })
        .unwrap();
        let t = auth
            .apply(receive_envelope(
                item("item-1"),
                peer("alice"),
                RawPeerKind::Message,
            ))
            .expect("receive trusted");
        assert_eq!(t.next_phase, PeerIngressState::Received);
        assert!(t.effects.is_empty());
        assert_eq!(auth.queue_len(), 1);
    }

    #[test]
    fn receive_trusted_envelope_classifies_message() {
        let mut auth = make_authority();
        auth.apply(PeerCommsInput::TrustPeer {
            peer_id: peer("alice"),
        })
        .unwrap();
        auth.apply(receive_envelope(
            item("item-1"),
            peer("alice"),
            RawPeerKind::Message,
        ))
        .unwrap();
        assert_eq!(
            auth.fields.classified_as.get(&item("item-1")),
            Some(&PeerInputClass::ActionableMessage)
        );
    }

    #[test]
    fn receive_trusted_envelope_classifies_request() {
        let mut auth = make_authority();
        auth.apply(PeerCommsInput::TrustPeer {
            peer_id: peer("alice"),
        })
        .unwrap();
        auth.apply(receive_envelope(
            item("item-1"),
            peer("alice"),
            RawPeerKind::Request,
        ))
        .unwrap();
        assert_eq!(
            auth.fields.classified_as.get(&item("item-1")),
            Some(&PeerInputClass::ActionableRequest)
        );
    }

    #[test]
    fn receive_trusted_envelope_classifies_response_terminal() {
        let mut auth = make_authority();
        auth.apply(PeerCommsInput::TrustPeer {
            peer_id: peer("alice"),
        })
        .unwrap();
        auth.apply(receive_envelope(
            item("item-1"),
            peer("alice"),
            RawPeerKind::ResponseTerminal,
        ))
        .unwrap();
        assert_eq!(
            auth.fields.classified_as.get(&item("item-1")),
            Some(&PeerInputClass::Response)
        );
    }

    #[test]
    fn receive_trusted_envelope_classifies_plain_event() {
        let mut auth = make_authority();
        auth.apply(PeerCommsInput::TrustPeer {
            peer_id: peer("alice"),
        })
        .unwrap();
        auth.apply(receive_envelope(
            item("item-1"),
            peer("alice"),
            RawPeerKind::PlainEvent,
        ))
        .unwrap();
        assert_eq!(
            auth.fields.classified_as.get(&item("item-1")),
            Some(&PeerInputClass::PlainEvent)
        );
    }

    #[test]
    fn receive_trusted_envelope_classifies_silent_request() {
        let mut auth = make_authority();
        auth.apply(PeerCommsInput::TrustPeer {
            peer_id: peer("alice"),
        })
        .unwrap();
        auth.apply(receive_envelope(
            item("item-1"),
            peer("alice"),
            RawPeerKind::SilentRequest,
        ))
        .unwrap();
        assert_eq!(
            auth.fields.classified_as.get(&item("item-1")),
            Some(&PeerInputClass::SilentRequest)
        );
    }

    #[test]
    fn receive_multiple_trusted_envelopes_accumulates_queue() {
        let mut auth = make_authority();
        auth.apply(PeerCommsInput::TrustPeer {
            peer_id: peer("alice"),
        })
        .unwrap();
        auth.apply(receive_envelope(
            item("item-1"),
            peer("alice"),
            RawPeerKind::Message,
        ))
        .unwrap();
        auth.apply(receive_envelope(
            item("item-2"),
            peer("alice"),
            RawPeerKind::Request,
        ))
        .unwrap();
        assert_eq!(auth.phase(), PeerIngressState::Received);
        assert_eq!(auth.queue_len(), 2);
    }

    // === ReceivePeerEnvelope — untrusted path ===

    #[test]
    fn receive_untrusted_envelope_transitions_to_dropped() {
        let mut auth = make_authority();
        // Do NOT trust the peer
        let t = auth
            .apply(receive_envelope(
                item("item-1"),
                peer("untrusted"),
                RawPeerKind::Message,
            ))
            .expect("receive untrusted");
        assert_eq!(t.next_phase, PeerIngressState::Dropped);
        assert!(t.effects.is_empty());
        assert_eq!(auth.queue_len(), 0);
    }

    #[test]
    fn receive_untrusted_records_trusted_snapshot_false() {
        let mut auth = make_authority();
        auth.apply(receive_envelope(
            item("item-1"),
            peer("untrusted"),
            RawPeerKind::Message,
        ))
        .unwrap();
        assert_eq!(
            auth.fields.trusted_snapshot.get(&item("item-1")),
            Some(&false)
        );
    }

    #[test]
    fn receive_untrusted_does_not_classify() {
        let mut auth = make_authority();
        auth.apply(receive_envelope(
            item("item-1"),
            peer("untrusted"),
            RawPeerKind::Message,
        ))
        .unwrap();
        assert!(auth.fields.classified_as.get(&item("item-1")).is_none());
    }

    #[test]
    fn receive_from_terminal_dropped_is_rejected() {
        let mut auth = make_authority();
        auth.apply(receive_envelope(
            item("item-1"),
            peer("untrusted"),
            RawPeerKind::Message,
        ))
        .unwrap();
        assert_eq!(auth.phase(), PeerIngressState::Dropped);

        let result = auth.apply(receive_envelope(
            item("item-2"),
            peer("untrusted"),
            RawPeerKind::Message,
        ));
        assert!(result.is_err());
    }

    // === SubmitTypedPeerInput transitions ===

    #[test]
    fn submit_single_item_drains_to_delivered() {
        let mut auth = make_authority();
        auth.apply(PeerCommsInput::TrustPeer {
            peer_id: peer("alice"),
        })
        .unwrap();
        auth.apply(receive_envelope(
            item("item-1"),
            peer("alice"),
            RawPeerKind::Message,
        ))
        .unwrap();

        let t = auth
            .apply(PeerCommsInput::SubmitTypedPeerInput {
                raw_item_id: item("item-1"),
            })
            .expect("submit");
        assert_eq!(t.next_phase, PeerIngressState::Delivered);
        assert_eq!(t.effects.len(), 1);

        match &t.effects[0] {
            PeerCommsEffect::SubmitPeerInputCandidate(effect) => {
                assert_eq!(effect.raw_item_id, item("item-1"));
                assert_eq!(effect.peer_input_class, PeerInputClass::ActionableMessage);
                assert_eq!(effect.text_projection, "test text");
            }
        }
    }

    #[test]
    fn submit_with_more_work_stays_received() {
        let mut auth = make_authority();
        auth.apply(PeerCommsInput::TrustPeer {
            peer_id: peer("alice"),
        })
        .unwrap();
        auth.apply(receive_envelope(
            item("item-1"),
            peer("alice"),
            RawPeerKind::Message,
        ))
        .unwrap();
        auth.apply(receive_envelope(
            item("item-2"),
            peer("alice"),
            RawPeerKind::Request,
        ))
        .unwrap();

        let t = auth
            .apply(PeerCommsInput::SubmitTypedPeerInput {
                raw_item_id: item("item-1"),
            })
            .expect("submit first");
        assert_eq!(t.next_phase, PeerIngressState::Received);
        assert_eq!(auth.queue_len(), 1);
    }

    #[test]
    fn submit_all_items_transitions_to_delivered() {
        let mut auth = make_authority();
        auth.apply(PeerCommsInput::TrustPeer {
            peer_id: peer("alice"),
        })
        .unwrap();
        auth.apply(receive_envelope(
            item("item-1"),
            peer("alice"),
            RawPeerKind::Message,
        ))
        .unwrap();
        auth.apply(receive_envelope(
            item("item-2"),
            peer("alice"),
            RawPeerKind::Request,
        ))
        .unwrap();

        auth.apply(PeerCommsInput::SubmitTypedPeerInput {
            raw_item_id: item("item-1"),
        })
        .unwrap();
        let t = auth
            .apply(PeerCommsInput::SubmitTypedPeerInput {
                raw_item_id: item("item-2"),
            })
            .expect("submit second");
        assert_eq!(t.next_phase, PeerIngressState::Delivered);
        assert_eq!(auth.queue_len(), 0);
    }

    #[test]
    fn submit_from_absent_is_rejected() {
        let mut auth = make_authority();
        let result = auth.apply(PeerCommsInput::SubmitTypedPeerInput {
            raw_item_id: item("item-1"),
        });
        assert!(result.is_err());
    }

    #[test]
    fn submit_unqueued_item_is_rejected() {
        let mut auth = make_authority();
        auth.apply(PeerCommsInput::TrustPeer {
            peer_id: peer("alice"),
        })
        .unwrap();
        auth.apply(receive_envelope(
            item("item-1"),
            peer("alice"),
            RawPeerKind::Message,
        ))
        .unwrap();

        // Try to submit an item that was never received
        let result = auth.apply(PeerCommsInput::SubmitTypedPeerInput {
            raw_item_id: item("item-unknown"),
        });
        assert!(
            result.is_err(),
            "submit of unqueued item should be rejected"
        );
    }

    // === Guard: phase does not change on failure ===

    #[test]
    fn phase_unchanged_on_failed_transition() {
        let mut auth = make_authority();
        assert_eq!(auth.phase(), PeerIngressState::Absent);

        let result = auth.apply(PeerCommsInput::SubmitTypedPeerInput {
            raw_item_id: item("item-1"),
        });
        assert!(result.is_err());
        assert_eq!(auth.phase(), PeerIngressState::Absent);
    }

    // === ClassFor helper ===

    #[test]
    fn class_for_request() {
        assert_eq!(class_for(&RawPeerKind::Request), PeerInputClass::ActionableRequest);
    }

    #[test]
    fn class_for_response_terminal() {
        assert_eq!(class_for(&RawPeerKind::ResponseTerminal), PeerInputClass::Response);
    }

    #[test]
    fn class_for_response_progress() {
        assert_eq!(class_for(&RawPeerKind::ResponseProgress), PeerInputClass::Response);
    }

    #[test]
    fn class_for_plain_event() {
        assert_eq!(class_for(&RawPeerKind::PlainEvent), PeerInputClass::PlainEvent);
    }

    #[test]
    fn class_for_silent_request() {
        assert_eq!(class_for(&RawPeerKind::SilentRequest), PeerInputClass::SilentRequest);
    }

    #[test]
    fn class_for_message() {
        assert_eq!(class_for(&RawPeerKind::Message), PeerInputClass::ActionableMessage);
    }

    // === Effect content validation ===

    #[test]
    fn submit_effect_carries_correlation_fields() {
        let mut auth = make_authority();
        auth.apply(PeerCommsInput::TrustPeer {
            peer_id: peer("alice"),
        })
        .unwrap();
        auth.apply(PeerCommsInput::ReceivePeerEnvelope {
            raw_item_id: item("item-1"),
            peer_id: peer("alice"),
            raw_kind: RawPeerKind::Request,
            text_projection: "hello world".to_string(),
            content_shape: ContentShape::Blocks,
            request_id: Some(RequestId("req-42".into())),
            reservation_key: Some(ReservationKey("rsv-7".into())),
        })
        .unwrap();

        let t = auth
            .apply(PeerCommsInput::SubmitTypedPeerInput {
                raw_item_id: item("item-1"),
            })
            .unwrap();

        match &t.effects[0] {
            PeerCommsEffect::SubmitPeerInputCandidate(effect) => {
                assert_eq!(effect.peer_input_class, PeerInputClass::ActionableRequest);
                assert_eq!(effect.text_projection, "hello world");
                assert_eq!(effect.content_shape, ContentShape::Blocks);
                assert_eq!(effect.request_id, Some(RequestId("req-42".into())));
                assert_eq!(
                    effect.reservation_key,
                    Some(ReservationKey("rsv-7".into()))
                );
            }
        }
    }

    // === Invariant: queued items are always classified ===

    #[test]
    fn invariant_queued_items_are_classified() {
        let mut auth = make_authority();
        auth.apply(PeerCommsInput::TrustPeer {
            peer_id: peer("alice"),
        })
        .unwrap();
        auth.apply(receive_envelope(
            item("item-1"),
            peer("alice"),
            RawPeerKind::Message,
        ))
        .unwrap();
        auth.apply(receive_envelope(
            item("item-2"),
            peer("alice"),
            RawPeerKind::Request,
        ))
        .unwrap();

        // Verify every queued item has a classification entry
        for queued_id in &auth.fields.submission_queue {
            assert!(
                auth.fields.classified_as.contains_key(queued_id),
                "queued item {queued_id:?} must have classification"
            );
        }
    }

    // === Invariant: queued items preserve content shape / text projection ===

    #[test]
    fn invariant_queued_items_preserve_shape_and_text() {
        let mut auth = make_authority();
        auth.apply(PeerCommsInput::TrustPeer {
            peer_id: peer("alice"),
        })
        .unwrap();
        auth.apply(receive_envelope(
            item("item-1"),
            peer("alice"),
            RawPeerKind::Message,
        ))
        .unwrap();

        for queued_id in &auth.fields.submission_queue {
            assert!(
                auth.fields.content_shape.contains_key(queued_id),
                "queued item {queued_id:?} must have content_shape"
            );
            assert!(
                auth.fields.text_projection.contains_key(queued_id),
                "queued item {queued_id:?} must have text_projection"
            );
            assert!(
                auth.fields.request_id.contains_key(queued_id),
                "queued item {queued_id:?} must have request_id slot"
            );
            assert!(
                auth.fields.reservation_key.contains_key(queued_id),
                "queued item {queued_id:?} must have reservation_key slot"
            );
        }
    }
}
