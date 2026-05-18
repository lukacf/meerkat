//! Plain-data domain types for peer ingress classification.
//!
//! These types describe the shape of classified peer traffic as it flows
//! through the ingress pipeline. They carry no authority or transition
//! semantics; DSL-owned classification state lives on the MeerkatMachine
//! dispatched via [`meerkat_core::handles::PeerCommsHandle`], and shell-side
//! mechanics (trust-set reads, queue order, dequeue emission) live directly on
//! `ClassifiedInboxQueue` in `inbox.rs`.

/// Shape of the content payload (used for downstream rendering decisions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContentShape {
    Text,
    Blocks,
    Mixed,
}

/// Projection of the generated peer-ingress authority phase.
///
/// Mirrors the shape exposed via `meerkat_core::PeerIngressAuthorityPhase` in
/// runtime snapshots. The classified queue updates this copy only from
/// `PeerCommsHandle` receive/dequeue authority effects; standalone runtimes
/// without a required session handle use the generated core authority.
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

impl From<meerkat_core::PeerIngressAuthorityPhase> for PeerIngressState {
    fn from(phase: meerkat_core::PeerIngressAuthorityPhase) -> Self {
        match phase {
            meerkat_core::PeerIngressAuthorityPhase::Absent => Self::Absent,
            meerkat_core::PeerIngressAuthorityPhase::Received => Self::Received,
            meerkat_core::PeerIngressAuthorityPhase::Dropped => Self::Dropped,
            meerkat_core::PeerIngressAuthorityPhase::Delivered => Self::Delivered,
        }
    }
}

impl From<PeerIngressState> for meerkat_core::PeerIngressAuthorityPhase {
    fn from(phase: PeerIngressState) -> Self {
        match phase {
            PeerIngressState::Absent => Self::Absent,
            PeerIngressState::Received => Self::Received,
            PeerIngressState::Dropped => Self::Dropped,
            PeerIngressState::Delivered => Self::Delivered,
        }
    }
}
