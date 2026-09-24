#![allow(clippy::expect_used, clippy::panic)]

//! Wave-B V5 (B-8): name-to-PeerId resolution returns a typed ambiguity
//! error when more than one [`TrustEntry`] shares a [`PeerName`].
//!
//! The router never guesses. Callers that hold only a display name must
//! handle the [`TrustResolveError::Ambiguous`] case explicitly; the trust
//! store will not collapse multiple candidates onto a single routing key.

use meerkat_comms::identity::PubKey;
use meerkat_comms::peer_meta::PeerMeta;
use meerkat_comms::trust::{TrustEntry, TrustResolveError, TrustStore};
use meerkat_core::comms::{PeerAddress, PeerName, PeerTransport};

fn entry(name: &str, pubkey_seed: u8) -> TrustEntry {
    let pubkey = PubKey::new([pubkey_seed; 32]);
    TrustEntry {
        peer_id: pubkey.to_peer_id(),
        name: PeerName::new(name).expect("valid peer name"),
        pubkey,
        address: PeerAddress::new(PeerTransport::Inproc, name),
        meta: PeerMeta::default(),
    }
}

#[test]
fn resolves_unique_name_to_peer_id() {
    let mut store = TrustStore::new();
    let id = PubKey::new([1; 32]).to_peer_id();
    store.insert(entry("coding-meerkat", 1)).expect("insert");

    let name = PeerName::new("coding-meerkat").expect("name");
    let resolved = store.resolve_name(&name).expect("unique name resolves");
    assert_eq!(resolved, id);
}

#[test]
fn ambiguous_name_returns_typed_error_with_candidates() {
    let mut store = TrustStore::new();
    let id_a = PubKey::new([1; 32]).to_peer_id();
    let id_b = PubKey::new([2; 32]).to_peer_id();
    store.insert(entry("reviewer", 1)).expect("insert A");
    store.insert(entry("reviewer", 2)).expect("insert B");

    let name = PeerName::new("reviewer").expect("name");
    let err = store
        .resolve_name(&name)
        .expect_err("ambiguous name must not resolve silently");
    match err {
        TrustResolveError::Ambiguous {
            name: n,
            candidates,
        } => {
            assert_eq!(n.as_str(), "reviewer");
            assert_eq!(candidates.len(), 2);
            assert!(candidates.contains(&id_a));
            assert!(candidates.contains(&id_b));
        }
        other => panic!("expected Ambiguous, got {other:?}"),
    }
}

#[test]
fn unknown_name_returns_not_found() {
    let store = TrustStore::new();
    let name = PeerName::new("ghost").expect("name");
    match store.resolve_name(&name) {
        Err(TrustResolveError::NotFound(n)) => assert_eq!(n.as_str(), "ghost"),
        other => panic!("expected NotFound, got {other:?}"),
    }
}
