//! Integration tests for Router that use real Unix sockets.
//!
//! These tests verify actual network I/O behavior and are slower than unit tests.

#![cfg(feature = "integration-real-tests")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use meerkat_comms::{
    CommsConfig, DEFAULT_MAX_MESSAGE_BYTES, Envelope, Keypair, MessageKind, Router, SendError,
    Signature, Status, TrustedPeer, TrustedPeers,
};
use std::{path::Path, sync::LazyLock, time::Duration};
use tempfile::TempDir;
use uuid::Uuid;

static INPROC_REGISTRY_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

fn make_keypair() -> Keypair {
    Keypair::generate()
}

fn bind_uds_or_skip(path: &Path) -> Option<tokio::net::UnixListener> {
    match tokio::net::UnixListener::bind(path) {
        Ok(listener) => Some(listener),
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!(
                "skipping UDS test: PermissionDenied binding to {}",
                path.display()
            );
            None
        }
        Err(err) => panic!("failed to bind UDS listener: {err}"),
    }
}

fn make_trusted_peers_with_addr(
    name: &str,
    pubkey: &meerkat_comms::PubKey,
    addr: &str,
) -> TrustedPeers {
    TrustedPeers {
        peers: vec![TrustedPeer {
            name: name.to_string(),
            pubkey: *pubkey,
            addr: addr.to_string(),
            meta: meerkat_comms::PeerMeta::default(),
        }],
    }
}

async fn read_envelope_async(
    reader: &mut (impl tokio::io::AsyncRead + Unpin),
) -> std::io::Result<Envelope> {
    use tokio::io::AsyncReadExt;
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf);
    if len > DEFAULT_MAX_MESSAGE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "message too large",
        ));
    }
    let mut payload = vec![0u8; len as usize];
    reader.read_exact(&mut payload).await?;
    ciborium::from_reader(&payload[..])
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}

async fn write_envelope_async(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    envelope: &Envelope,
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut payload = Vec::new();
    ciborium::into_writer(envelope, &mut payload)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    let len = payload.len() as u32;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "integration-real: socket integration + timing behavior"]
async fn integration_real_router_send() {
    let tmp = TempDir::new().unwrap();
    let sock_path = tmp.path().join("peer.sock");

    let peer_keypair = make_keypair();
    let our_keypair = make_keypair();

    let trusted_peers = make_trusted_peers_with_addr(
        "test-peer",
        &peer_keypair.public_key(),
        &format!("uds://{}", sock_path.display()),
    );

    let our_pubkey = our_keypair.public_key();
    let (_, inbox_sender) = meerkat_comms::Inbox::new();
    let router = Router::new(
        our_keypair,
        trusted_peers,
        CommsConfig::default(),
        inbox_sender,
        true,
    );

    let listener = match bind_uds_or_skip(&sock_path) {
        Some(listener) => listener,
        None => return,
    };
    let server_handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let envelope = read_envelope_async(&mut stream).await.unwrap();

        let mut ack = Envelope {
            id: Uuid::new_v4(),
            from: peer_keypair.public_key(),
            to: our_pubkey,
            kind: MessageKind::Ack {
                in_reply_to: envelope.id,
            },
            sig: Signature::new([0u8; 64]),
        };
        ack.sign(&peer_keypair);
        write_envelope_async(&mut stream, &ack).await.unwrap();
    });

    let result = router.send_message("test-peer", "hello".to_string()).await;
    assert!(result.is_ok());
    server_handle.await.unwrap();
}

#[tokio::test]
#[ignore = "integration-real: socket integration + timing behavior"]
async fn integration_real_router_resolves_peer_name() {
    let keypair = make_keypair();
    let peer_keypair = make_keypair();
    let trusted_peers = make_trusted_peers_with_addr(
        "my-peer",
        &peer_keypair.public_key(),
        "tcp://127.0.0.1:9999",
    );
    let (_, inbox_sender) = meerkat_comms::Inbox::new();
    let router = Router::new(
        keypair,
        trusted_peers,
        CommsConfig::default(),
        inbox_sender,
        true,
    );

    let result = router
        .send_message("unknown-peer", "hello".to_string())
        .await;

    match result {
        Err(SendError::PeerNotFound(name)) => assert_eq!(name, "unknown-peer"),
        _ => panic!("expected PeerNotFound error"),
    }
}

#[tokio::test]
#[ignore = "integration-real: socket integration + timing behavior"]
async fn integration_real_router_connects_to_peer() {
    let tmp = TempDir::new().unwrap();
    let sock_path = tmp.path().join("peer.sock");

    let peer_keypair = make_keypair();
    let our_keypair = make_keypair();

    let trusted_peers = make_trusted_peers_with_addr(
        "test-peer",
        &peer_keypair.public_key(),
        &format!("uds://{}", sock_path.display()),
    );

    let (_, inbox_sender) = meerkat_comms::Inbox::new();
    let router = Router::new(
        our_keypair,
        trusted_peers,
        CommsConfig::default(),
        inbox_sender,
        true,
    );

    // No server listening - should fail with IO error
    let result = router.send_message("test-peer", "hello".to_string()).await;
    assert!(matches!(result, Err(SendError::Io(_))));
}

#[tokio::test]
#[ignore = "integration-real: socket integration + timing behavior"]
async fn integration_real_router_signs_envelope() {
    let tmp = TempDir::new().unwrap();
    let sock_path = tmp.path().join("peer.sock");

    let peer_keypair = make_keypair();
    let our_keypair = make_keypair();
    let our_pubkey = our_keypair.public_key();

    let trusted_peers = make_trusted_peers_with_addr(
        "test-peer",
        &peer_keypair.public_key(),
        &format!("uds://{}", sock_path.display()),
    );

    let (_, inbox_sender) = meerkat_comms::Inbox::new();
    let router = Router::new(
        our_keypair,
        trusted_peers,
        CommsConfig::default(),
        inbox_sender,
        true,
    );

    let listener = match bind_uds_or_skip(&sock_path) {
        Some(listener) => listener,
        None => return,
    };
    let server_handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let envelope = read_envelope_async(&mut stream).await.unwrap();
        assert!(envelope.verify(), "Envelope should have valid signature");
        assert_eq!(envelope.from, our_pubkey, "Envelope should be from our key");

        let mut ack = Envelope {
            id: Uuid::new_v4(),
            from: peer_keypair.public_key(),
            to: our_pubkey,
            kind: MessageKind::Ack {
                in_reply_to: envelope.id,
            },
            sig: Signature::new([0u8; 64]),
        };
        ack.sign(&peer_keypair);
        write_envelope_async(&mut stream, &ack).await.unwrap();
    });

    let result = router.send_message("test-peer", "hello".to_string()).await;
    assert!(result.is_ok());
    server_handle.await.unwrap();
}

#[tokio::test]
#[ignore = "integration-real: socket integration + timing behavior"]
async fn integration_real_router_waits_for_ack() {
    let tmp = TempDir::new().unwrap();
    let sock_path = tmp.path().join("peer.sock");

    let peer_keypair = make_keypair();
    let our_keypair = make_keypair();
    let our_pubkey = our_keypair.public_key();

    let trusted_peers = make_trusted_peers_with_addr(
        "test-peer",
        &peer_keypair.public_key(),
        &format!("uds://{}", sock_path.display()),
    );

    let config = CommsConfig {
        ack_timeout_secs: 1,
        ..Default::default()
    };
    let (_, inbox_sender) = meerkat_comms::Inbox::new();
    let router = Router::new(our_keypair, trusted_peers, config, inbox_sender, true);

    let listener = match bind_uds_or_skip(&sock_path) {
        Some(listener) => listener,
        None => return,
    };
    let server_handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let envelope = read_envelope_async(&mut stream).await.unwrap();

        let mut ack = Envelope {
            id: Uuid::new_v4(),
            from: peer_keypair.public_key(),
            to: our_pubkey,
            kind: MessageKind::Ack {
                in_reply_to: envelope.id,
            },
            sig: Signature::new([0u8; 64]),
        };
        ack.sign(&peer_keypair);
        write_envelope_async(&mut stream, &ack).await.unwrap();
    });

    let result = router.send_message("test-peer", "hello".to_string()).await;
    assert!(result.is_ok(), "Should receive ack successfully");
    server_handle.await.unwrap();
}

#[tokio::test]
#[ignore = "integration-real: socket integration + timing behavior"]
async fn integration_real_router_timeout_returns_offline() {
    let tmp = TempDir::new().unwrap();
    let sock_path = tmp.path().join("peer.sock");

    let peer_keypair = make_keypair();
    let our_keypair = make_keypair();

    let trusted_peers = make_trusted_peers_with_addr(
        "test-peer",
        &peer_keypair.public_key(),
        &format!("uds://{}", sock_path.display()),
    );

    let config = CommsConfig {
        ack_timeout_secs: 1,
        ..Default::default()
    };
    let (_, inbox_sender) = meerkat_comms::Inbox::new();
    let router = Router::new(our_keypair, trusted_peers, config, inbox_sender, true);

    let listener = match bind_uds_or_skip(&sock_path) {
        Some(listener) => listener,
        None => return,
    };
    let server_handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _envelope = read_envelope_async(&mut stream).await.unwrap();
        // Do NOT send ack - just sleep
        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    let result = router.send_message("test-peer", "hello".to_string()).await;
    assert!(matches!(result, Err(SendError::PeerOffline)));

    server_handle.abort();
}

#[tokio::test]
#[ignore = "integration-real: socket integration + timing behavior"]
async fn integration_real_send_message() {
    let tmp = TempDir::new().unwrap();
    let sock_path = tmp.path().join("peer.sock");

    let peer_keypair = make_keypair();
    let our_keypair = make_keypair();
    let our_pubkey = our_keypair.public_key();

    let trusted_peers = make_trusted_peers_with_addr(
        "test-peer",
        &peer_keypair.public_key(),
        &format!("uds://{}", sock_path.display()),
    );

    let (_, inbox_sender) = meerkat_comms::Inbox::new();
    let router = Router::new(
        our_keypair,
        trusted_peers,
        CommsConfig::default(),
        inbox_sender,
        true,
    );

    let listener = match bind_uds_or_skip(&sock_path) {
        Some(listener) => listener,
        None => return,
    };
    let server_handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let envelope = read_envelope_async(&mut stream).await.unwrap();

        match envelope.kind {
            MessageKind::Message { body } => {
                assert_eq!(body, "test body");
            }
            _ => panic!("expected Message"),
        }

        let mut ack = Envelope {
            id: Uuid::new_v4(),
            from: peer_keypair.public_key(),
            to: our_pubkey,
            kind: MessageKind::Ack {
                in_reply_to: envelope.id,
            },
            sig: Signature::new([0u8; 64]),
        };
        ack.sign(&peer_keypair);
        write_envelope_async(&mut stream, &ack).await.unwrap();
    });

    let result = router
        .send_message("test-peer", "test body".to_string())
        .await;
    assert!(result.is_ok());
    server_handle.await.unwrap();
}

#[tokio::test]
#[ignore = "integration-real: socket integration + timing behavior"]
async fn integration_real_send_request() {
    let tmp = TempDir::new().unwrap();
    let sock_path = tmp.path().join("peer.sock");

    let peer_keypair = make_keypair();
    let our_keypair = make_keypair();
    let our_pubkey = our_keypair.public_key();

    let trusted_peers = make_trusted_peers_with_addr(
        "test-peer",
        &peer_keypair.public_key(),
        &format!("uds://{}", sock_path.display()),
    );

    let (_, inbox_sender) = meerkat_comms::Inbox::new();
    let router = Router::new(
        our_keypair,
        trusted_peers,
        CommsConfig::default(),
        inbox_sender,
        true,
    );

    let listener = match bind_uds_or_skip(&sock_path) {
        Some(listener) => listener,
        None => return,
    };
    let server_handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let envelope = read_envelope_async(&mut stream).await.unwrap();

        match envelope.kind {
            MessageKind::Request { intent, params } => {
                assert_eq!(intent, "review-pr");
                assert_eq!(params["pr"], 42);
            }
            _ => panic!("expected Request"),
        }

        let mut ack = Envelope {
            id: Uuid::new_v4(),
            from: peer_keypair.public_key(),
            to: our_pubkey,
            kind: MessageKind::Ack {
                in_reply_to: envelope.id,
            },
            sig: Signature::new([0u8; 64]),
        };
        ack.sign(&peer_keypair);
        write_envelope_async(&mut stream, &ack).await.unwrap();
    });

    let result = router
        .send_request(
            "test-peer",
            "review-pr".to_string(),
            serde_json::json!({"pr": 42}),
        )
        .await;
    assert!(result.is_ok());
    server_handle.await.unwrap();
}

#[tokio::test]
#[ignore = "integration-real: socket integration + timing behavior"]
async fn integration_real_send_response() {
    let tmp = TempDir::new().unwrap();
    let sock_path = tmp.path().join("peer.sock");

    let peer_keypair = make_keypair();
    let our_keypair = make_keypair();

    let trusted_peers = make_trusted_peers_with_addr(
        "test-peer",
        &peer_keypair.public_key(),
        &format!("uds://{}", sock_path.display()),
    );

    let (_, inbox_sender) = meerkat_comms::Inbox::new();
    let router = Router::new(
        our_keypair,
        trusted_peers,
        CommsConfig::default(),
        inbox_sender,
        true,
    );
    let request_id = Uuid::new_v4();

    let listener = match bind_uds_or_skip(&sock_path) {
        Some(listener) => listener,
        None => return,
    };
    let server_handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let envelope = read_envelope_async(&mut stream).await.unwrap();

        match envelope.kind {
            MessageKind::Response {
                in_reply_to,
                status,
                result,
            } => {
                assert_eq!(in_reply_to, request_id);
                assert!(matches!(status, Status::Completed));
                assert_eq!(result["approved"], true);
            }
            _ => panic!("expected Response"),
        }
    });

    let result = router
        .send_response(
            "test-peer",
            request_id,
            Status::Completed,
            serde_json::json!({"approved": true}),
        )
        .await;
    assert!(result.is_ok());
    server_handle.await.unwrap();
}

#[tokio::test]
#[ignore = "integration-real: socket integration + timing behavior"]
async fn integration_real_send_response_no_ack_wait() {
    let tmp = TempDir::new().unwrap();
    let sock_path = tmp.path().join("peer.sock");

    let peer_keypair = make_keypair();
    let our_keypair = make_keypair();

    let trusted_peers = make_trusted_peers_with_addr(
        "test-peer",
        &peer_keypair.public_key(),
        &format!("uds://{}", sock_path.display()),
    );

    let config = CommsConfig {
        ack_timeout_secs: 1,
        ..Default::default()
    };
    let (_, inbox_sender) = meerkat_comms::Inbox::new();
    let router = Router::new(our_keypair, trusted_peers, config, inbox_sender, true);

    let listener = match bind_uds_or_skip(&sock_path) {
        Some(listener) => listener,
        None => return,
    };
    let server_handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _envelope = read_envelope_async(&mut stream).await.unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
    });

    let start = std::time::Instant::now();
    let result = router
        .send_response(
            "test-peer",
            Uuid::new_v4(),
            Status::Completed,
            serde_json::json!({}),
        )
        .await;
    let elapsed = start.elapsed();

    assert!(result.is_ok(), "send_response should succeed");
    assert!(
        elapsed < Duration::from_secs(1),
        "send_response should not wait for ack, took {:?}",
        elapsed
    );

    server_handle.abort();
}

// === Inproc transport tests ===

#[tokio::test]
#[ignore = "integration-real: socket integration + timing behavior"]
async fn integration_real_router_inproc_send() {
    use meerkat_comms::{Inbox, InprocRegistry};

    let _lock = INPROC_REGISTRY_LOCK.lock().await;

    // Clear any existing state
    InprocRegistry::global().clear();

    // Create receiver
    let receiver_keypair = make_keypair();
    let receiver_pubkey = receiver_keypair.public_key();
    let (mut inbox, inbox_sender) = Inbox::new();

    // Register receiver in inproc registry
    InprocRegistry::global().register("receiver-agent", receiver_pubkey, inbox_sender);

    // Create sender with inproc peer
    let sender_keypair = make_keypair();
    let sender_pubkey = sender_keypair.public_key();
    let trusted_peers = make_trusted_peers_with_addr(
        "receiver-agent",
        &receiver_pubkey,
        "inproc://receiver-agent",
    );
    let (_, inbox_sender) = meerkat_comms::Inbox::new();
    let router = Router::new(
        sender_keypair,
        trusted_peers,
        CommsConfig::default(),
        inbox_sender,
        true,
    );

    // Send via router
    let result = router
        .send_message("receiver-agent", "hello via inproc".to_string())
        .await;
    assert!(result.is_ok(), "inproc send should succeed: {:?}", result);

    // Verify message was received
    let items = inbox.try_drain();
    assert_eq!(items.len(), 1, "should have received one message");

    match &items[0] {
        meerkat_comms::InboxItem::External { envelope } => {
            assert_eq!(envelope.from, sender_pubkey);
            assert_eq!(envelope.to, receiver_pubkey);
            match &envelope.kind {
                MessageKind::Message { body } => {
                    assert_eq!(body, "hello via inproc");
                }
                _ => panic!("expected Message kind"),
            }
            assert!(envelope.verify(), "signature should be valid");
        }
        _ => panic!("expected External inbox item"),
    }

    // Cleanup
    InprocRegistry::global().clear();
}

#[tokio::test]
#[ignore = "integration-real: socket integration + timing behavior"]
async fn integration_real_router_inproc_peer_not_found() {
    use meerkat_comms::InprocRegistry;

    let _lock = INPROC_REGISTRY_LOCK.lock().await;

    // Clear any existing state
    InprocRegistry::global().clear();

    // Create sender with inproc peer that's NOT registered
    let sender_keypair = make_keypair();
    let fake_pubkey = make_keypair().public_key();
    let trusted_peers =
        make_trusted_peers_with_addr("missing-agent", &fake_pubkey, "inproc://missing-agent");
    let (_, inbox_sender) = meerkat_comms::Inbox::new();
    let router = Router::new(
        sender_keypair,
        trusted_peers,
        CommsConfig::default(),
        inbox_sender,
        true,
    );

    // Send should fail - peer not in registry
    let result = router
        .send_message("missing-agent", "hello".to_string())
        .await;
    assert!(result.is_err(), "should fail for missing inproc peer");
    assert!(
        matches!(result, Err(SendError::PeerNotFound(_))),
        "should be PeerNotFound error"
    );

    // Cleanup
    InprocRegistry::global().clear();
}

#[tokio::test]
#[ignore = "integration-real: socket integration + timing behavior"]
async fn integration_real_router_inproc_request_response() {
    use meerkat_comms::{Inbox, InprocRegistry};

    let _lock = INPROC_REGISTRY_LOCK.lock().await;

    // Clear any existing state
    InprocRegistry::global().clear();

    // Create receiver
    let receiver_keypair = make_keypair();
    let receiver_pubkey = receiver_keypair.public_key();
    let (mut inbox, inbox_sender) = Inbox::new();

    // Register receiver
    InprocRegistry::global().register("service-agent", receiver_pubkey, inbox_sender);

    // Create sender
    let sender_keypair = make_keypair();
    let trusted_peers =
        make_trusted_peers_with_addr("service-agent", &receiver_pubkey, "inproc://service-agent");
    let (_, inbox_sender) = meerkat_comms::Inbox::new();
    let router = Router::new(
        sender_keypair,
        trusted_peers,
        CommsConfig::default(),
        inbox_sender,
        true,
    );

    // Send request
    let result = router
        .send_request(
            "service-agent",
            "analyze".to_string(),
            serde_json::json!({"file": "main.rs"}),
        )
        .await;
    assert!(result.is_ok());

    // Verify request was received
    let items = inbox.try_drain();
    assert_eq!(items.len(), 1);

    match &items[0] {
        meerkat_comms::InboxItem::External { envelope } => match &envelope.kind {
            MessageKind::Request { intent, params } => {
                assert_eq!(intent, "analyze");
                assert_eq!(params["file"], "main.rs");
            }
            _ => panic!("expected Request kind"),
        },
        _ => panic!("expected External inbox item"),
    }

    // Cleanup
    InprocRegistry::global().clear();
}
