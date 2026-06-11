//! Integration tests for Router routing behavior.
//!
//! Trust rows enter a live router exclusively through the generated
//! `CommsRuntime::apply_trust_mutation` seam; a bare `Router` therefore has an
//! empty trust projection and must fail closed on every send. Full
//! transport-level exchange (framing, ACK verification, admission) is covered
//! by the `CommsRuntime` integration tests, which install trust through the
//! generated authority path.

#![cfg(feature = "integration-real-tests")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use meerkat_comms::{CommsConfig, Keypair, MessageKind, Router, SendError};
use std::sync::LazyLock;

static INPROC_REGISTRY_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

fn make_keypair() -> Keypair {
    Keypair::generate()
}

#[tokio::test]
#[ignore = "integration-real: socket integration + timing behavior"]
async fn integration_real_router_unknown_peer_fails_closed() {
    let keypair = make_keypair();
    let (_, inbox_sender) = meerkat_comms::Inbox::new();
    let router = Router::new(keypair, CommsConfig::default(), inbox_sender, true);

    let unknown_pubkey = make_keypair().public_key();
    let unknown_peer_id = meerkat_comms::router::peer_id_from_pubkey(&unknown_pubkey);
    let result = router
        .send(
            unknown_peer_id,
            MessageKind::Message {
                body: "hello".to_string(),
                blocks: None,
                handling_mode: None,
            },
        )
        .await;

    match result {
        Err(SendError::PeerNotFound(id)) => assert_eq!(id, unknown_peer_id),
        _ => panic!("expected PeerNotFound error"),
    }
}

#[tokio::test]
#[ignore = "integration-real: socket integration + timing behavior"]
async fn integration_real_router_inproc_peer_not_found() {
    use meerkat_comms::{Inbox, InprocRegistry};

    let _lock = INPROC_REGISTRY_LOCK.lock().await;

    // A registered inproc peer that is NOT trusted must not be sendable:
    // registration is transport presence, trust is the routing authority.
    let receiver_keypair = make_keypair();
    let receiver_pubkey = receiver_keypair.public_key();
    let (_inbox, receiver_sender) = Inbox::new();
    InprocRegistry::global().register("untrusted-receiver", receiver_pubkey, receiver_sender);

    let sender_keypair = make_keypair();
    let (_, inbox_sender) = meerkat_comms::Inbox::new();
    let router = Router::new(sender_keypair, CommsConfig::default(), inbox_sender, true);

    let result = router
        .send(
            meerkat_comms::router::peer_id_from_pubkey(&receiver_pubkey),
            MessageKind::Message {
                body: "hello".to_string(),
                blocks: None,
                handling_mode: None,
            },
        )
        .await;
    assert!(
        matches!(result, Err(SendError::PeerNotFound(_))),
        "untrusted inproc peer must fail closed: {result:?}"
    );

    InprocRegistry::global().unregister(&receiver_pubkey);
}
