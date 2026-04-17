//! Plain-data domain types for peer ingress classification.
//!
//! These types describe the shape of classified peer traffic as it flows
//! through the ingress pipeline. They carry no authority or transition
//! semantics; DSL-owned classification state lives on the MeerkatMachine
//! dispatched via [`meerkat_core::handles::PeerCommsHandle`], and shell-side
//! mechanics (trust set, queue order, dequeue emission) live directly on
//! `ClassifiedInboxQueue` in `inbox.rs`.

/// Opaque peer identifier (public-key-derived string).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct PeerId(pub String);

/// The raw kind tag from the wire envelope, before classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RawPeerKind {
    Request,
    PeerLifecycleAdded,
    PeerLifecycleRetired,
    ResponseTerminal,
    ResponseProgress,
    PlainEvent,
    SilentRequest,
    Ack,
    Message,
}

/// Shape of the content payload (used for downstream rendering decisions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContentShape {
    Text,
    Blocks,
    Mixed,
}

/// Observable lifecycle phase of the peer-ingress queue.
///
/// Mirrors the shape exposed via `meerkat_core::PeerIngressAuthorityPhase` in
/// runtime snapshots. Transitions are driven inline from the classified inbox:
/// `new` → `Absent`; admitted external envelopes move to `Received`; untrusted
/// external envelopes move to `Dropped` (or `Received` if work was already
/// queued); a full queue drain moves to `Delivered`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PeerIngressState {
    /// No items have been received yet.
    Absent,
    /// At least one trusted item has been received and queued.
    Received,
    /// An untrusted item was dropped at ingress (and the queue is empty).
    Dropped,
    /// All queued items have been submitted and delivered.
    Delivered,
}
