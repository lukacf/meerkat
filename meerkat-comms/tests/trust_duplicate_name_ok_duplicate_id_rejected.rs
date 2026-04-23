//! Wave-B V5 (B-8): [`TrustStore`] keys by canonical [`PeerId`].
//!
//! Duplicate [`PeerName`] across entries is explicitly allowed — peer names
//! are display metadata, not routing keys. Duplicate [`PeerId`] is a hard
//! error on `insert`: two trust entries claiming the same routing identity
//! is structurally impossible by design.

use meerkat_comms::identity::PubKey;
use meerkat_comms::peer_meta::PeerMeta;
use meerkat_comms::trust::{TrustEntry, TrustError, TrustStore};
use meerkat_core::comms::{PeerAddress, PeerId, PeerName, PeerTransport};

fn entry(peer_id: PeerId, name: &str, pubkey_seed: u8) -> TrustEntry {
    TrustEntry {
        peer_id,
        name: PeerName::new(name).expect("valid peer name"),
        pubkey: PubKey::new([pubkey_seed; 32]),
        address: PeerAddress::new(PeerTransport::Inproc, name),
        meta: PeerMeta::default(),
    }
}

#[test]
fn duplicate_peer_name_is_allowed() {
    let mut store = TrustStore::new();
    let id_a = PeerId::new();
    let id_b = PeerId::new();
    assert_ne!(id_a, id_b, "fresh PeerIds must never collide");

    store.insert(entry(id_a, "reviewer", 1)).expect("insert A");
    store.insert(entry(id_b, "reviewer", 2)).expect("insert B");

    assert_eq!(store.len(), 2);
    assert!(store.contains(&id_a));
    assert!(store.contains(&id_b));
}

#[test]
fn duplicate_peer_id_is_hard_error() {
    let mut store = TrustStore::new();
    let id = PeerId::new();

    store.insert(entry(id, "alice", 1)).expect("first insert");

    let err = store
        .insert(entry(id, "alice-imposter", 2))
        .expect_err("duplicate peer id must be rejected");
    match err {
        TrustError::DuplicatePeerId { peer_id } => assert_eq!(peer_id, id),
        other => panic!("expected DuplicatePeerId, got {other:?}"),
    }

    // Store is unchanged by the rejected insert.
    assert_eq!(store.len(), 1);
    let entry = store.get(&id).expect("first entry still present");
    assert_eq!(entry.name.as_str(), "alice");
}

#[test]
fn upsert_allows_replacement_by_peer_id() {
    let mut store = TrustStore::new();
    let id = PeerId::new();

    assert!(store.upsert(entry(id, "alice", 1)).is_none());
    let prior = store
        .upsert(entry(id, "alice-v2", 2))
        .expect("upsert returns prior entry");
    assert_eq!(prior.name.as_str(), "alice");

    let current = store.get(&id).expect("replaced entry present");
    assert_eq!(current.name.as_str(), "alice-v2");
    assert_eq!(current.pubkey, PubKey::new([2; 32]));
}
