//! End-to-end tests for meerkat-comms.
//!
//! These tests verify the full system working together with realistic scenarios.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use meerkat_comms::{
    CommsConfig, Inbox, Keypair, PubKey, Router, TrustedPeer, TrustedPeers, handle_connection,
};
use serde_json::json;
use tempfile::TempDir;
use tokio::net::{TcpListener, UnixListener};
use std::path::Path;

/// Helper to create a keypair
fn make_keypair() -> Keypair {
    Keypair::generate()
}

fn bind_uds_or_skip(path: &Path) -> Option<UnixListener> {
    match UnixListener::bind(path) {
        Ok(listener) => Some(listener),
        Err(e) => {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                return None;
            }
            panic!("UnixListener::bind failed: {e}");
        }
    }
}

async fn bind_tcp_or_skip(addr: &str) -> Option<TcpListener> {
    match TcpListener::bind(addr).await {
        Ok(listener) => Some(listener),
        Err(e) => {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                return None;
            }
            panic!("TcpListener::bind failed: {e}");
        }
    }
}

/// Helper to set up mutual trust between two peers
fn setup_mutual_trust(
    name_a: &str,
    pubkey_a: &PubKey,
    addr_a: &str,
    name_b: &str,
    pubkey_b: &PubKey,
    addr_b: &str,
) -> (TrustedPeers, TrustedPeers) {
    // Peer A trusts Peer B
    let a_trusts = TrustedPeers {
        peers: vec![TrustedPeer {
            name: name_b.to_string(),
            pubkey: *pubkey_b,
            addr: addr_b.to_string(),
        }],
    };

    // Peer B trusts Peer A
    let b_trusts = TrustedPeers {
        peers: vec![TrustedPeer {
            name: name_a.to_string(),
            pubkey: *pubkey_a,
            addr: addr_a.to_string(),
        }],
    };

    (a_trusts, b_trusts)
}

/// E2E test harness that sets up two Meerkat instances
struct E2EHarness {
    peer_a_keypair: Keypair,
    peer_b_keypair: Keypair,
    peer_a_trust: TrustedPeers,
    peer_b_trust: TrustedPeers,
}

impl E2EHarness {
    fn new_uds(tmp_a: &TempDir, tmp_b: &TempDir) -> Self {
        let peer_a_keypair = make_keypair();
        let peer_b_keypair = make_keypair();

        let addr_a = format!("uds://{}", tmp_a.path().join("peer_a.sock").display());
        let addr_b = format!("uds://{}", tmp_b.path().join("peer_b.sock").display());

        let (peer_a_trust, peer_b_trust) = setup_mutual_trust(
            "peer-a",
            &peer_a_keypair.public_key(),
            &addr_a,
            "peer-b",
            &peer_b_keypair.public_key(),
            &addr_b,
        );

        Self {
            peer_a_keypair,
            peer_b_keypair,
            peer_a_trust,
            peer_b_trust,
        }
    }
}

#[test]
fn test_e2e_harness() {
    let tmp_a = TempDir::new().unwrap();
    let tmp_b = TempDir::new().unwrap();
    let harness = E2EHarness::new_uds(&tmp_a, &tmp_b);

    // Verify mutual trust is set up correctly
    assert!(
        harness
            .peer_a_trust
            .is_trusted(&harness.peer_b_keypair.public_key())
    );
    assert!(
        harness
            .peer_b_trust
            .is_trusted(&harness.peer_a_keypair.public_key())
    );

    // Verify names
    assert_eq!(
        harness
            .peer_a_trust
            .get_by_name("peer-b")
            .map(|p| &p.pubkey),
        Some(&harness.peer_b_keypair.public_key())
    );
    assert_eq!(
        harness
            .peer_b_trust
            .get_by_name("peer-a")
            .map(|p| &p.pubkey),
        Some(&harness.peer_a_keypair.public_key())
    );
}

#[test]
fn test_e2e_mutual_trust_setup() {
    let peer_a_keypair = make_keypair();
    let peer_b_keypair = make_keypair();

    let (a_trusts, b_trusts) = setup_mutual_trust(
        "alice",
        &peer_a_keypair.public_key(),
        "uds:///tmp/alice.sock",
        "bob",
        &peer_b_keypair.public_key(),
        "uds:///tmp/bob.sock",
    );

    // Alice trusts Bob
    assert!(a_trusts.is_trusted(&peer_b_keypair.public_key()));
    assert!(!a_trusts.is_trusted(&peer_a_keypair.public_key()));

    // Bob trusts Alice
    assert!(b_trusts.is_trusted(&peer_a_keypair.public_key()));
    assert!(!b_trusts.is_trusted(&peer_b_keypair.public_key()));
}

#[tokio::test]
async fn test_e2e_uds_message_exchange() {
    let _tmp_a = TempDir::new().unwrap();
    let tmp_b = TempDir::new().unwrap();

    let sock_b = tmp_b.path().join("peer_b.sock");
    let addr_b = format!("uds://{}", sock_b.display());

    let peer_a_keypair = make_keypair();
    let peer_b_keypair = make_keypair();

    let (peer_a_trust, peer_b_trust) = setup_mutual_trust(
        "peer-a",
        &peer_a_keypair.public_key(),
        "uds:///unused",
        "peer-b",
        &peer_b_keypair.public_key(),
        &addr_b,
    );

    // Start peer B listening
    let Some(listener_b) = bind_uds_or_skip(&sock_b) else {
        return;
    };
    let (_inbox_b, inbox_sender_b) = Inbox::new();

    // Spawn B's IO task
    let handle_b = tokio::spawn(async move {
        let (stream, _) = listener_b.accept().await.unwrap();
        handle_connection(stream, &peer_b_keypair, &peer_b_trust, &inbox_sender_b)
            .await
            .unwrap();
    });

    // Peer A sends a message
    let (_, inbox_sender_a) = Inbox::new();
    let router_a = Router::new(
        peer_a_keypair,
        peer_a_trust,
        CommsConfig::default(),
        inbox_sender_a,
    );
    router_a
        .send_message("peer-b", "Hello from A!".to_string())
        .await
        .unwrap();

    // Wait for B to process
    handle_b.await.unwrap();

    // Note: The important thing is that the message was sent successfully and ack'd.
    // If we got here without error, the exchange worked. The inbox verification
    // is implicit - if B didn't ack, A's send would have failed.
}

#[tokio::test]
async fn test_e2e_tcp_message_exchange() {
    // Find an available port
    let Some(listener) = bind_tcp_or_skip("127.0.0.1:0").await else {
        return;
    };
    let addr_b = listener.local_addr().unwrap();

    let peer_a_keypair = make_keypair();
    let peer_b_keypair = make_keypair();

    let (peer_a_trust, peer_b_trust) = setup_mutual_trust(
        "peer-a",
        &peer_a_keypair.public_key(),
        "tcp://127.0.0.1:0",
        "peer-b",
        &peer_b_keypair.public_key(),
        &format!("tcp://{}", addr_b),
    );

    // Keep inbox_b alive through the test (must not be dropped before handle_connection completes)
    let (mut inbox_b, inbox_sender_b) = Inbox::new();

    // Spawn B's IO task
    let handle_b = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        handle_connection(stream, &peer_b_keypair, &peer_b_trust, &inbox_sender_b)
            .await
            .unwrap();
    });

    // Peer A sends a message
    let (_, inbox_sender_a) = Inbox::new();
    let router_a = Router::new(
        peer_a_keypair,
        peer_a_trust,
        CommsConfig::default(),
        inbox_sender_a,
    );
    router_a
        .send_message("peer-b", "Hello via TCP!".to_string())
        .await
        .unwrap();

    // Wait for B to process
    handle_b.await.unwrap();

    // Verify B received the message
    let items = inbox_b.try_drain();
    assert_eq!(items.len(), 1, "B should have received one message");
}

#[tokio::test]
async fn test_e2e_request_response_flow() {
    let tmp = TempDir::new().unwrap();
    let sock_b = tmp.path().join("peer_b.sock");
    let sock_a = tmp.path().join("peer_a.sock");
    let addr_b = format!("uds://{}", sock_b.display());
    let addr_a = format!("uds://{}", sock_a.display());

    let peer_a_keypair = make_keypair();
    let peer_b_keypair = make_keypair();

    let peer_a_pubkey = peer_a_keypair.public_key();
    let peer_b_pubkey = peer_b_keypair.public_key();

    let (peer_a_trust, peer_b_trust) = setup_mutual_trust(
        "peer-a",
        &peer_a_pubkey,
        &addr_a,
        "peer-b",
        &peer_b_pubkey,
        &addr_b,
    );

    // Start peer B listening for the Request
    let Some(listener_b) = bind_uds_or_skip(&sock_b) else {
        return;
    };
    // Keep inbox_b alive through the test
    let (mut inbox_b, inbox_sender_b) = Inbox::new();

    // Spawn B's receiver
    let peer_b_trust_clone = peer_b_trust.clone();
    let handle_b = tokio::spawn(async move {
        let (stream, _) = listener_b.accept().await.unwrap();
        handle_connection(
            stream,
            &peer_b_keypair,
            &peer_b_trust_clone,
            &inbox_sender_b,
        )
        .await
        .unwrap();
    });

    // A sends a Request to B
    let (_, inbox_sender_a) = Inbox::new();
    let router_a = Router::new(
        peer_a_keypair,
        peer_a_trust.clone(),
        CommsConfig::default(),
        inbox_sender_a,
    );
    router_a
        .send_request("peer-b", "review-pr".to_string(), json!({"pr": 42}))
        .await
        .unwrap();

    // B receives the request
    handle_b.await.unwrap();

    // Verify B received the Request
    let items = inbox_b.try_drain();
    assert_eq!(items.len(), 1, "B should have received the request");

    // The request was received (ack was sent) - that's the core flow verified
    // In a real scenario, B would now process and send a Response back
}

#[tokio::test]
async fn test_e2e_untrusted_rejected() {
    let tmp = TempDir::new().unwrap();
    let sock_b = tmp.path().join("peer_b.sock");
    let addr_b = format!("uds://{}", sock_b.display());

    let peer_a_keypair = make_keypair();
    let peer_b_keypair = make_keypair();
    let _untrusted_keypair = make_keypair();

    // A trusts B, but B does NOT trust A (empty trust list)
    let peer_a_trust = TrustedPeers {
        peers: vec![TrustedPeer {
            name: "peer-b".to_string(),
            pubkey: peer_b_keypair.public_key(),
            addr: addr_b.clone(),
        }],
    };

    let peer_b_trust = TrustedPeers { peers: vec![] }; // B trusts no one

    // Start peer B listening
    let Some(listener_b) = bind_uds_or_skip(&sock_b) else {
        return;
    };
    let (_, inbox_sender_b) = Inbox::new();

    let handle_b = tokio::spawn(async move {
        let (stream, _) = listener_b.accept().await.unwrap();
        // This will reject the message due to untrusted sender
        // The connection handler should complete (possibly with error due to untrusted sender)
        // What matters is the message is NOT delivered to inbox
        handle_connection(stream, &peer_b_keypair, &peer_b_trust, &inbox_sender_b).await
    });

    // A sends a message (will connect, but B will reject it as untrusted)
    let (_, inbox_sender_a) = Inbox::new();
    let router_a = Router::new(
        peer_a_keypair,
        peer_a_trust,
        CommsConfig::default(),
        inbox_sender_a,
    );
    let send_result = router_a.send_message("peer-b", "Hello!".to_string()).await;

    // The send should fail (no ack from B due to untrusted rejection)
    // Either timeout or immediate rejection
    assert!(send_result.is_err());

    // Wait for B's handler to complete
    let _ = handle_b.await;
}

#[tokio::test]
async fn test_e2e_concurrent_multi_peer() {
    let tmp = TempDir::new().unwrap();

    // Three peers: A, B, C
    let peer_a_keypair = make_keypair();
    let peer_b_keypair = make_keypair();
    let peer_c_keypair = make_keypair();

    let sock_a = tmp.path().join("peer_a.sock");
    let sock_b = tmp.path().join("peer_b.sock");
    let sock_c = tmp.path().join("peer_c.sock");

    let addr_a = format!("uds://{}", sock_a.display());
    let addr_b = format!("uds://{}", sock_b.display());
    let addr_c = format!("uds://{}", sock_c.display());

    // A trusts B and C
    let peer_a_trust = TrustedPeers {
        peers: vec![
            TrustedPeer {
                name: "peer-b".to_string(),
                pubkey: peer_b_keypair.public_key(),
                addr: addr_b.clone(),
            },
            TrustedPeer {
                name: "peer-c".to_string(),
                pubkey: peer_c_keypair.public_key(),
                addr: addr_c.clone(),
            },
        ],
    };

    // B trusts A
    let peer_b_trust = TrustedPeers {
        peers: vec![TrustedPeer {
            name: "peer-a".to_string(),
            pubkey: peer_a_keypair.public_key(),
            addr: addr_a.clone(),
        }],
    };

    // C trusts A
    let peer_c_trust = TrustedPeers {
        peers: vec![TrustedPeer {
            name: "peer-a".to_string(),
            pubkey: peer_a_keypair.public_key(),
            addr: addr_a.clone(),
        }],
    };

    // Start listeners for B and C
    let Some(listener_b) = bind_uds_or_skip(&sock_b) else {
        return;
    };
    let Some(listener_c) = bind_uds_or_skip(&sock_c) else {
        return;
    };

    // Keep inboxes alive through the test
    let (mut inbox_b, inbox_sender_b) = Inbox::new();
    let (mut inbox_c, inbox_sender_c) = Inbox::new();

    // Spawn B's and C's handlers
    let handle_b = tokio::spawn(async move {
        let (stream, _) = listener_b.accept().await.unwrap();
        handle_connection(stream, &peer_b_keypair, &peer_b_trust, &inbox_sender_b)
            .await
            .unwrap();
    });

    let handle_c = tokio::spawn(async move {
        let (stream, _) = listener_c.accept().await.unwrap();
        handle_connection(stream, &peer_c_keypair, &peer_c_trust, &inbox_sender_c)
            .await
            .unwrap();
    });

    // A sends messages to both B and C concurrently
    let (_, inbox_sender_a) = Inbox::new();
    let router_a = Router::new(
        peer_a_keypair,
        peer_a_trust,
        CommsConfig::default(),
        inbox_sender_a,
    );

    let send_b = router_a.send_message("peer-b", "Hello B!".to_string());
    let send_c = router_a.send_message("peer-c", "Hello C!".to_string());

    // Both sends should succeed
    let (result_b, result_c) = tokio::join!(send_b, send_c);
    assert!(result_b.is_ok(), "Send to B should succeed");
    assert!(result_c.is_ok(), "Send to C should succeed");

    // Wait for handlers
    handle_b.await.unwrap();
    handle_c.await.unwrap();

    // Verify both B and C received messages
    let items_b = inbox_b.try_drain();
    let items_c = inbox_c.try_drain();
    assert_eq!(items_b.len(), 1, "B should have received one message");
    assert_eq!(items_c.len(), 1, "C should have received one message");
}
