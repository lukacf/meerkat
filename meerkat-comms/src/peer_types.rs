//! Plain-data domain types for peer ingress classification.
//!
//! These types describe classified peer traffic as it flows through the
//! ingress pipeline. DSL-owned classification state lives on the
//! MeerkatMachine dispatched via [`meerkat_core::handles::PeerCommsHandle`].
//! The submission queue keeps mechanical ordering only; the observable
//! peer-ingress authority phase is owned by [`PeerIngressQueueAuthority`].

/// Shape of the content payload (used for downstream rendering decisions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContentShape {
    Text,
    Blocks,
    Mixed,
}

/// Observable lifecycle phase of peer-ingress authority.
///
/// Mirrors the shape exposed via `meerkat_core::PeerIngressAuthorityPhase` in
/// runtime snapshots.
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

/// Single owner for peer-ingress authority phase.
///
/// `ClassifiedInboxQueue` owns queue mechanics: capacity, order, and dequeue.
/// This authority owns the semantic phase transitions that runtime snapshots
/// expose, so enqueue/dequeue mechanics cannot independently reinterpret the
/// peer-ingress lifecycle.
#[derive(Debug, Clone)]
pub(crate) struct PeerIngressQueueAuthority {
    phase: PeerIngressState,
}

impl PeerIngressQueueAuthority {
    pub(crate) fn new() -> Self {
        Self {
            phase: PeerIngressState::Absent,
        }
    }

    pub(crate) fn phase(&self) -> PeerIngressState {
        self.phase
    }

    /// An admitted external envelope has reached the authority-owned
    /// submission queue.
    pub(crate) fn external_received(&mut self) {
        self.phase = PeerIngressState::Received;
    }

    /// An external envelope was rejected by admission.
    ///
    /// If work is already queued, the authority stays in `Received`; the
    /// dropped item did not erase the earlier received work.
    pub(crate) fn external_dropped(&mut self, had_queued_work: bool) {
        self.phase = if had_queued_work {
            PeerIngressState::Received
        } else {
            PeerIngressState::Dropped
        };
    }

    /// One external envelope was submitted to the agent loop.
    pub(crate) fn external_submitted(&mut self, still_queued: bool) {
        self.phase = if still_queued {
            PeerIngressState::Received
        } else {
            PeerIngressState::Delivered
        };
    }
}

impl Default for PeerIngressQueueAuthority {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_ingress_authority_owns_phase_transitions() {
        let mut authority = PeerIngressQueueAuthority::new();
        assert_eq!(authority.phase(), PeerIngressState::Absent);

        authority.external_dropped(false);
        assert_eq!(authority.phase(), PeerIngressState::Dropped);

        authority.external_received();
        assert_eq!(authority.phase(), PeerIngressState::Received);

        authority.external_dropped(true);
        assert_eq!(authority.phase(), PeerIngressState::Received);

        authority.external_submitted(true);
        assert_eq!(authority.phase(), PeerIngressState::Received);

        authority.external_submitted(false);
        assert_eq!(authority.phase(), PeerIngressState::Delivered);
    }
}
