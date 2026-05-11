//! Canonical app-facing peer-directory visibility authority.
//!
//! Trust ownership stays with `TrustedPeers`; this authority owns the separate
//! fact of whether an otherwise trusted peer is visible on user-facing peer
//! directory surfaces.

use meerkat_core::comms::PeerId;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PeerDirectoryVisibility {
    Public,
    Private,
}

impl PeerDirectoryVisibility {
    pub(crate) fn is_visible(self) -> bool {
        matches!(self, Self::Public)
    }
}

#[derive(Debug, Default)]
pub(crate) struct PeerDirectoryVisibilityAuthority {
    private_peer_ids: BTreeSet<PeerId>,
}

impl PeerDirectoryVisibilityAuthority {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn mark_private(&mut self, peer_id: PeerId) {
        self.private_peer_ids.insert(peer_id);
    }

    pub(crate) fn unmark_private(&mut self, peer_id: &PeerId) -> bool {
        self.private_peer_ids.remove(peer_id)
    }

    pub(crate) fn visibility_for(&self, peer_id: &PeerId) -> PeerDirectoryVisibility {
        if self.private_peer_ids.contains(peer_id) {
            PeerDirectoryVisibility::Private
        } else {
            PeerDirectoryVisibility::Public
        }
    }

    pub(crate) fn is_private(&self, peer_id: &PeerId) -> bool {
        !self.visibility_for(peer_id).is_visible()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_authority_owns_private_marker_lifecycle() {
        let mut authority = PeerDirectoryVisibilityAuthority::new();
        let peer_id = PeerId::new();

        assert_eq!(
            authority.visibility_for(&peer_id),
            PeerDirectoryVisibility::Public
        );
        authority.mark_private(peer_id);
        assert_eq!(
            authority.visibility_for(&peer_id),
            PeerDirectoryVisibility::Private
        );
        assert!(authority.is_private(&peer_id));
        assert!(authority.unmark_private(&peer_id));
        assert_eq!(
            authority.visibility_for(&peer_id),
            PeerDirectoryVisibility::Public
        );
    }
}
