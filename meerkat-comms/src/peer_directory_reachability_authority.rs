//! Canonical transient reachability state for resolved peers.
//!
//! This authority is intentionally outbound and per-peer. It complements the
//! inbound raw-item `PeerComms` authority instead of extending it.

use meerkat_core::comms::{PeerReachability, PeerReachabilityReason};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ReachabilityKey {
    pub peer_name: String,
    pub peer_id: String,
}

impl ReachabilityKey {
    pub(crate) fn new(peer_name: impl Into<String>, peer_id: impl Into<String>) -> Self {
        Self {
            peer_name: peer_name.into(),
            peer_id: peer_id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReachabilitySnapshot {
    pub reachability: PeerReachability,
    pub last_unreachable_reason: Option<PeerReachabilityReason>,
}

#[derive(Debug, Default)]
pub(crate) struct PeerDirectoryReachabilityAuthority {
    resolved_keys: BTreeSet<ReachabilityKey>,
    reachability: BTreeMap<ReachabilityKey, PeerReachability>,
    last_reason: BTreeMap<ReachabilityKey, Option<PeerReachabilityReason>>,
}

impl PeerDirectoryReachabilityAuthority {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn reconcile_resolved_directory(
        &mut self,
        keys: impl IntoIterator<Item = ReachabilityKey>,
    ) {
        let next_keys: BTreeSet<_> = keys.into_iter().collect();
        self.reachability.retain(|key, _| next_keys.contains(key));
        self.last_reason.retain(|key, _| next_keys.contains(key));
        for key in &next_keys {
            self.reachability
                .entry(key.clone())
                .or_insert(PeerReachability::Unknown);
            self.last_reason.entry(key.clone()).or_insert(None);
        }
        self.resolved_keys = next_keys;
    }

    pub(crate) fn record_send_succeeded(&mut self, key: &ReachabilityKey) {
        if self.resolved_keys.contains(key) {
            self.reachability
                .insert(key.clone(), PeerReachability::Reachable);
            self.last_reason.insert(key.clone(), None);
        }
    }

    pub(crate) fn record_send_failed(
        &mut self,
        key: &ReachabilityKey,
        reason: PeerReachabilityReason,
    ) {
        if self.resolved_keys.contains(key) {
            self.reachability
                .insert(key.clone(), PeerReachability::Unreachable);
            self.last_reason.insert(key.clone(), Some(reason));
        }
    }

    pub(crate) fn snapshot_for(&self, key: &ReachabilityKey) -> ReachabilitySnapshot {
        ReachabilitySnapshot {
            reachability: self
                .reachability
                .get(key)
                .copied()
                .unwrap_or(PeerReachability::Unknown),
            last_unreachable_reason: self.last_reason.get(key).copied().flatten(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconcile_initializes_new_keys_as_unknown() {
        let mut authority = PeerDirectoryReachabilityAuthority::new();
        let key = ReachabilityKey::new("agent-a", "ed25519:a");
        authority.reconcile_resolved_directory([key.clone()]);

        let snapshot = authority.snapshot_for(&key);
        assert_eq!(snapshot.reachability, PeerReachability::Unknown);
        assert_eq!(snapshot.last_unreachable_reason, None);
    }

    #[test]
    fn reconcile_prunes_removed_keys_and_preserves_survivors() {
        let mut authority = PeerDirectoryReachabilityAuthority::new();
        let keep = ReachabilityKey::new("agent-a", "ed25519:a");
        let drop_key = ReachabilityKey::new("agent-b", "ed25519:b");
        authority.reconcile_resolved_directory([keep.clone(), drop_key.clone()]);
        authority.record_send_succeeded(&keep);
        authority.record_send_failed(&drop_key, PeerReachabilityReason::TransportError);

        authority.reconcile_resolved_directory([keep.clone()]);

        assert_eq!(
            authority.snapshot_for(&keep).reachability,
            PeerReachability::Reachable
        );
        assert_eq!(
            authority.snapshot_for(&drop_key).reachability,
            PeerReachability::Unknown
        );
    }
}
