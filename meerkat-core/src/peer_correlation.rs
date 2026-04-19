//! Peer-interaction correlation identifiers and typed lifecycle state.
//!
//! A [`PeerCorrelationId`] is the canonical UUID that ties a peer request
//! (outbound envelope id, reservation key) to every subsequent progress /
//! terminal response (`in_reply_to`). It is the key of the MeerkatMachine DSL
//! substate maps `pending_peer_requests` and `inbound_peer_requests`.
//!
//! The typed state enums live here so the shell (comms runtime, drain task,
//! consumers of the registries) can read and branch on DSL-owned truth without
//! pulling the catalog DSL types through core's public surface.
//!
//! Added by W1-A (issue #264): collapses the peer request/response lifecycle
//! into a first-class MeerkatMachine substate. The registries that used to be
//! parallel bookkeeping (`subscriber_registry`, `interaction_stream_registry`)
//! become projections of these states, with cleanup driven by a typed DSL
//! effect on terminal transitions.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Canonical correlation identifier for the peer request/response lifecycle.
///
/// On the sender side, `PeerCorrelationId` is both the outbound request
/// envelope id and the reservation key for any stream subscription. On the
/// receiver side, every response envelope carries it as `in_reply_to`. The
/// sender routes a reply back to its originating request by matching on this
/// id — no local id-translation map.
///
/// Wrapping it in a newtype prevents raw-`Uuid` confusion at every site that
/// fires a DSL input, inserts into the subscriber registry, or closes out an
/// inbound request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PeerCorrelationId(pub Uuid);

impl PeerCorrelationId {
    /// Create a fresh correlation id (UUID v4).
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Construct from an existing UUID (e.g., an inbound request's envelope id).
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Return the underlying UUID.
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for PeerCorrelationId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for PeerCorrelationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<Uuid> for PeerCorrelationId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<PeerCorrelationId> for Uuid {
    fn from(id: PeerCorrelationId) -> Self {
        id.0
    }
}

/// Typed lifecycle state of an outbound peer request (local is sender).
///
/// This enum spans both active and terminal states. The active lifetime
/// runs `Sent → AcceptedProgress` while the correlation id lives in
/// `pending_peer_requests`; terminal transitions remove the entry from the
/// map and emit `PeerInteractionStateChanged { new_state: <terminal> }`
/// followed by `PeerInteractionCleanup`. Callers observing via
/// [`crate::handles::PeerInteractionHandle::outbound_state`] therefore see
/// `Some(Sent | AcceptedProgress)` or `None` — terminal variants surface
/// only on the state-change effect, not on the active map.
///
/// Unit-variant on purpose — error detail travels on the typed DSL input
/// that drove the transition, not on the state enum.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[non_exhaustive]
pub enum OutboundPeerRequestState {
    /// Request sent; no response yet.
    #[default]
    Sent,
    /// At least one progress response (`ResponseStatus::Accepted`) has
    /// arrived; waiting on a terminal response.
    AcceptedProgress,
    /// Terminal: response with `Completed` status arrived. Reported on
    /// the state-change effect only; never stored in the active map.
    Completed,
    /// Terminal: response with `Failed` status arrived. Reported on the
    /// state-change effect only; never stored in the active map.
    Failed,
    /// Terminal: timed out before any terminal response arrived. Reported
    /// on the state-change effect only; never stored in the active map.
    TimedOut,
}

impl OutboundPeerRequestState {
    /// Is this a terminal state — i.e., the request lifecycle is done and the
    /// shell should drop any associated subscriber/stream channel?
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::TimedOut)
    }
}

/// Typed lifecycle state of an inbound peer request (local is receiver).
///
/// Mirrors [`OutboundPeerRequestState`] on the receiver side: the request
/// has been received and later replied to (with any status). The inbound
/// map removes the entry on `Replied`, so the active map only ever holds
/// `Received` — `Replied` surfaces as the effect's `new_state`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[non_exhaustive]
pub enum InboundPeerRequestState {
    /// Request received; receiver has not sent a reply yet.
    #[default]
    Received,
    /// Terminal: receiver has replied with a terminal response. Reported
    /// on the state-change effect only; never stored in the active map.
    Replied,
}

impl InboundPeerRequestState {
    /// Is this a terminal state?
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Replied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corr_id_roundtrip_via_uuid() {
        let id = PeerCorrelationId::new();
        let raw: Uuid = id.into();
        assert_eq!(PeerCorrelationId::from(raw), id);
        assert_eq!(PeerCorrelationId::from_uuid(raw), id);
        assert_eq!(id.as_uuid(), raw);
    }

    #[test]
    fn outbound_terminality_covers_every_terminal_variant() {
        assert!(!OutboundPeerRequestState::Sent.is_terminal());
        assert!(!OutboundPeerRequestState::AcceptedProgress.is_terminal());
        assert!(OutboundPeerRequestState::Completed.is_terminal());
        assert!(OutboundPeerRequestState::Failed.is_terminal());
        assert!(OutboundPeerRequestState::TimedOut.is_terminal());
    }

    #[test]
    fn inbound_terminality_is_replied_only() {
        assert!(!InboundPeerRequestState::Received.is_terminal());
        assert!(InboundPeerRequestState::Replied.is_terminal());
    }
}
