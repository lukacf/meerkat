//! Router for Meerkat comms - high-level send API.
//!
//! The router provides a simple API for sending messages to peers,
//! handling envelope creation, signing, connection, and ack waiting.

use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UnixStream};
use uuid::Uuid;

use crate::identity::{Keypair, Signature};
use crate::transport::{PeerAddr, TransportError, MAX_PAYLOAD_SIZE};
use crate::trust::TrustedPeers;
use crate::types::{Envelope, MessageKind, Status};

/// Default ack timeout in seconds.
pub const DEFAULT_ACK_TIMEOUT_SECS: u64 = 30;

/// Default max message size in bytes (1 MB).
pub const DEFAULT_MAX_MESSAGE_BYTES: u32 = MAX_PAYLOAD_SIZE;

/// Configuration for comms.
#[derive(Debug, Clone)]
pub struct CommsConfig {
    /// Timeout for ack wait in seconds.
    pub ack_timeout_secs: u64,
    /// Maximum message size in bytes.
    pub max_message_bytes: u32,
}

impl Default for CommsConfig {
    fn default() -> Self {
        Self {
            ack_timeout_secs: DEFAULT_ACK_TIMEOUT_SECS,
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
        }
    }
}

/// Errors that can occur during send operations.
#[derive(Debug, Error)]
pub enum SendError {
    #[error("Peer not found: {0}")]
    PeerNotFound(String),
    #[error("Peer offline (ack timeout)")]
    PeerOffline,
    #[error("Invalid ack: {0}")]
    InvalidAck(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),
    #[error("Invalid peer address: {0}")]
    InvalidAddress(String),
    #[error("CBOR encoding error: {0}")]
    Cbor(String),
}

/// Router for sending messages to peers.
pub struct Router {
    /// Our keypair for signing messages.
    keypair: Keypair,
    /// List of trusted peers.
    trusted_peers: TrustedPeers,
    /// Configuration.
    config: CommsConfig,
}

impl Router {
    /// Create a new router.
    pub fn new(keypair: Keypair, trusted_peers: TrustedPeers, config: CommsConfig) -> Self {
        Self {
            keypair,
            trusted_peers,
            config,
        }
    }

    /// Send a message to a peer by name.
    ///
    /// Per spec "On Send":
    /// 1. Create envelope with message kind
    /// 2. Compute signable_bytes (canonical CBOR)
    /// 3. Sign with sender's secret key
    /// 4. Connect to peer (UDS or TCP)
    /// 5. Write length-prefix + CBOR payload
    /// 6. Wait for Ack (30s timeout) - unless sending Ack/Response
    /// 7. If timeout, return SendError::PeerOffline
    pub async fn send(&self, peer_name: &str, kind: MessageKind) -> Result<(), SendError> {
        // Look up peer
        let peer = self
            .trusted_peers
            .get_by_name(peer_name)
            .ok_or_else(|| SendError::PeerNotFound(peer_name.to_string()))?;

        // Parse peer address
        let addr =
            PeerAddr::parse(&peer.addr).map_err(|e| SendError::InvalidAddress(e.to_string()))?;

        // Determine if we should wait for ack
        let wait_for_ack = should_wait_for_ack(&kind);

        // Create and sign envelope
        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: self.keypair.public_key(),
            to: peer.pubkey,
            kind,
            sig: Signature::new([0u8; 64]),
        };
        envelope.sign(&self.keypair);

        // Connect and send
        match addr {
            PeerAddr::Uds(path) => {
                let mut stream = UnixStream::connect(&path).await?;
                self.send_on_stream(&mut stream, &envelope, wait_for_ack, &peer.pubkey)
                    .await?;
            }
            PeerAddr::Tcp(addr_str) => {
                // DNS resolution happens here via ToSocketAddrs
                let mut stream = TcpStream::connect(&addr_str).await?;
                self.send_on_stream(&mut stream, &envelope, wait_for_ack, &peer.pubkey)
                    .await?;
            }
        }

        Ok(())
    }

    /// Send message, wait for body text.
    pub async fn send_message(&self, peer: &str, body: String) -> Result<(), SendError> {
        self.send(peer, MessageKind::Message { body }).await
    }

    /// Send request, wait for ack.
    pub async fn send_request(
        &self,
        peer: &str,
        intent: String,
        params: serde_json::Value,
    ) -> Result<(), SendError> {
        self.send(peer, MessageKind::Request { intent, params })
            .await
    }

    /// Send response - does NOT wait for ack.
    pub async fn send_response(
        &self,
        peer: &str,
        in_reply_to: Uuid,
        status: Status,
        result: serde_json::Value,
    ) -> Result<(), SendError> {
        self.send(
            peer,
            MessageKind::Response {
                in_reply_to,
                status,
                result,
            },
        )
        .await
    }

    /// Send envelope on stream and optionally wait for ack.
    async fn send_on_stream<S>(
        &self,
        stream: &mut S,
        envelope: &Envelope,
        wait_for_ack: bool,
        expected_peer: &crate::identity::PubKey,
    ) -> Result<(), SendError>
    where
        S: AsyncReadExt + AsyncWriteExt + Unpin,
    {
        // Write envelope
        write_envelope_async(stream, envelope, self.config.max_message_bytes).await?;

        // Wait for ack if required
        if wait_for_ack {
            let timeout = Duration::from_secs(self.config.ack_timeout_secs);
            let ack_result = tokio::time::timeout(
                timeout,
                read_envelope_async(stream, self.config.max_message_bytes),
            )
            .await;

            match ack_result {
                Ok(Ok(ack)) => {
                    // Verify ack signature
                    if !ack.verify() {
                        return Err(SendError::InvalidAck(
                            "signature verification failed".into(),
                        ));
                    }

                    // Verify ack is from the expected peer
                    if ack.from != *expected_peer {
                        return Err(SendError::InvalidAck("ack from unexpected sender".into()));
                    }

                    // Verify ack is addressed to us
                    if ack.to != self.keypair.public_key() {
                        return Err(SendError::InvalidAck("ack not addressed to us".into()));
                    }

                    // Verify it's an ack for our message
                    match ack.kind {
                        MessageKind::Ack { in_reply_to } if in_reply_to == envelope.id => {
                            // Valid ack received
                        }
                        MessageKind::Ack { in_reply_to } => {
                            return Err(SendError::InvalidAck(format!(
                                "ack for wrong message: expected {}, got {}",
                                envelope.id, in_reply_to
                            )));
                        }
                        _ => {
                            return Err(SendError::InvalidAck("received non-ack response".into()));
                        }
                    }
                }
                Ok(Err(_)) => {
                    // Read error - peer closed connection or bad data
                    return Err(SendError::PeerOffline);
                }
                Err(_) => {
                    // Timeout - peer offline
                    return Err(SendError::PeerOffline);
                }
            }
        }

        Ok(())
    }
}

/// Determine if we should wait for an ack for this message kind.
///
/// Per spec:
/// - Message: Yes (wait for ack)
/// - Request: Yes (wait for ack)
/// - Response: No (no ack expected)
/// - Ack: No (never ack an ack)
fn should_wait_for_ack(kind: &MessageKind) -> bool {
    matches!(
        kind,
        MessageKind::Message { .. } | MessageKind::Request { .. }
    )
}

/// Read an envelope from an async stream with length-prefix framing.
async fn read_envelope_async<R: AsyncReadExt + Unpin>(
    reader: &mut R,
    max_size: u32,
) -> Result<Envelope, SendError> {
    let mut len_bytes = [0u8; 4];
    reader.read_exact(&mut len_bytes).await?;
    let len = u32::from_be_bytes(len_bytes);

    // Use configured max (clamped to hard limit)
    let effective_max = max_size.min(MAX_PAYLOAD_SIZE);
    if len > effective_max {
        return Err(SendError::Transport(TransportError::MessageTooLarge {
            size: len,
        }));
    }

    let mut payload = vec![0u8; len as usize];
    reader.read_exact(&mut payload).await?;

    let envelope: Envelope =
        ciborium::from_reader(&payload[..]).map_err(|e| SendError::Cbor(e.to_string()))?;

    Ok(envelope)
}

/// Write an envelope to an async stream with length-prefix framing.
async fn write_envelope_async<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    envelope: &Envelope,
    max_size: u32,
) -> Result<(), SendError> {
    let mut payload = Vec::new();
    ciborium::into_writer(envelope, &mut payload).map_err(|e| SendError::Cbor(e.to_string()))?;

    let len = payload.len() as u32;
    // Use configured max (clamped to hard limit)
    let effective_max = max_size.min(MAX_PAYLOAD_SIZE);
    if len > effective_max {
        return Err(SendError::Transport(TransportError::MessageTooLarge {
            size: len,
        }));
    }

    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::PubKey;
    use crate::trust::TrustedPeer;
    use std::io;
    use tempfile::TempDir;

    fn make_keypair() -> Keypair {
        Keypair::generate()
    }

    fn make_trusted_peers_with_addr(name: &str, pubkey: &PubKey, addr: &str) -> TrustedPeers {
        TrustedPeers {
            peers: vec![TrustedPeer {
                name: name.to_string(),
                pubkey: *pubkey,
                addr: addr.to_string(),
            }],
        }
    }

    #[test]
    fn test_router_struct() {
        let keypair = make_keypair();
        let trusted_peers = TrustedPeers::new();
        let config = CommsConfig::default();
        let router = Router::new(keypair, trusted_peers, config);
        // Router exists with all fields
        let _ = router.keypair.public_key();
        let _ = router.trusted_peers.peers.len();
        let _ = router.config.ack_timeout_secs;
    }

    #[test]
    fn test_send_error_peer_not_found() {
        let err = SendError::PeerNotFound("unknown".to_string());
        assert!(err.to_string().contains("Peer not found"));
        assert!(err.to_string().contains("unknown"));
    }

    #[test]
    fn test_send_error_peer_offline() {
        let err = SendError::PeerOffline;
        assert!(err.to_string().contains("offline"));
    }

    #[test]
    fn test_send_error_io() {
        let err = SendError::Io(io::Error::new(io::ErrorKind::NotFound, "not found"));
        assert!(err.to_string().contains("IO error"));
    }

    #[test]
    fn test_comms_config() {
        let config = CommsConfig {
            ack_timeout_secs: 60,
            max_message_bytes: 2_000_000,
        };
        assert_eq!(config.ack_timeout_secs, 60);
        assert_eq!(config.max_message_bytes, 2_000_000);

        let default_config = CommsConfig::default();
        assert_eq!(default_config.ack_timeout_secs, DEFAULT_ACK_TIMEOUT_SECS);
        assert_eq!(default_config.max_message_bytes, DEFAULT_MAX_MESSAGE_BYTES);
    }

    #[tokio::test]
    async fn test_router_send() {
        // Set up a mock peer that will send an ack
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
        let router = Router::new(our_keypair, trusted_peers, CommsConfig::default());

        // Start mock peer server
        let listener = tokio::net::UnixListener::bind(&sock_path).unwrap();
        let server_handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();

            // Read the envelope
            let envelope = read_envelope_async(&mut stream, DEFAULT_MAX_MESSAGE_BYTES)
                .await
                .unwrap();

            // Send ack
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
            write_envelope_async(&mut stream, &ack, DEFAULT_MAX_MESSAGE_BYTES)
                .await
                .unwrap();
        });

        // Send message
        let result = router.send_message("test-peer", "hello".to_string()).await;

        assert!(result.is_ok());
        server_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_router_resolves_peer_name() {
        let keypair = make_keypair();
        let peer_keypair = make_keypair();
        let trusted_peers = make_trusted_peers_with_addr(
            "my-peer",
            &peer_keypair.public_key(),
            "tcp://127.0.0.1:9999",
        );
        let router = Router::new(keypair, trusted_peers, CommsConfig::default());

        // Unknown peer should fail with PeerNotFound
        let result = router
            .send_message("unknown-peer", "hello".to_string())
            .await;

        match result {
            Err(SendError::PeerNotFound(name)) => assert_eq!(name, "unknown-peer"),
            _ => panic!("expected PeerNotFound error"),
        }
    }

    #[tokio::test]
    async fn test_router_connects_to_peer() {
        // This test verifies the router attempts to connect
        let tmp = TempDir::new().unwrap();
        let sock_path = tmp.path().join("peer.sock");

        let peer_keypair = make_keypair();
        let our_keypair = make_keypair();

        let trusted_peers = make_trusted_peers_with_addr(
            "test-peer",
            &peer_keypair.public_key(),
            &format!("uds://{}", sock_path.display()),
        );

        let router = Router::new(our_keypair, trusted_peers, CommsConfig::default());

        // No server listening - should fail with IO error
        let result = router.send_message("test-peer", "hello".to_string()).await;

        assert!(matches!(result, Err(SendError::Io(_))));
    }

    #[tokio::test]
    async fn test_router_signs_envelope() {
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

        let router = Router::new(our_keypair, trusted_peers, CommsConfig::default());

        // Start mock peer server that verifies signature
        let listener = tokio::net::UnixListener::bind(&sock_path).unwrap();
        let server_handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();

            // Read the envelope and verify signature
            let envelope = read_envelope_async(&mut stream, DEFAULT_MAX_MESSAGE_BYTES)
                .await
                .unwrap();
            assert!(envelope.verify(), "Envelope should have valid signature");
            assert_eq!(envelope.from, our_pubkey, "Envelope should be from our key");

            // Send ack
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
            write_envelope_async(&mut stream, &ack, DEFAULT_MAX_MESSAGE_BYTES)
                .await
                .unwrap();
        });

        let result = router.send_message("test-peer", "hello".to_string()).await;
        assert!(result.is_ok());
        server_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_router_waits_for_ack() {
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

        // Short timeout for testing
        let config = CommsConfig {
            ack_timeout_secs: 1,
            ..Default::default()
        };
        let router = Router::new(our_keypair, trusted_peers, config);

        // Start server that sends ack
        let listener = tokio::net::UnixListener::bind(&sock_path).unwrap();
        let server_handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let envelope = read_envelope_async(&mut stream, DEFAULT_MAX_MESSAGE_BYTES)
                .await
                .unwrap();

            // Send ack
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
            write_envelope_async(&mut stream, &ack, DEFAULT_MAX_MESSAGE_BYTES)
                .await
                .unwrap();
        });

        let result = router.send_message("test-peer", "hello".to_string()).await;
        assert!(result.is_ok(), "Should receive ack successfully");
        server_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_router_timeout_returns_offline() {
        let tmp = TempDir::new().unwrap();
        let sock_path = tmp.path().join("peer.sock");

        let peer_keypair = make_keypair();
        let our_keypair = make_keypair();

        let trusted_peers = make_trusted_peers_with_addr(
            "test-peer",
            &peer_keypair.public_key(),
            &format!("uds://{}", sock_path.display()),
        );

        // Very short timeout
        let config = CommsConfig {
            ack_timeout_secs: 1,
            ..Default::default()
        };
        let router = Router::new(our_keypair, trusted_peers, config);

        // Start server that does NOT send ack
        let listener = tokio::net::UnixListener::bind(&sock_path).unwrap();
        let server_handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _envelope = read_envelope_async(&mut stream, DEFAULT_MAX_MESSAGE_BYTES)
                .await
                .unwrap();
            // Do NOT send ack - just sleep
            tokio::time::sleep(Duration::from_secs(5)).await;
        });

        let result = router.send_message("test-peer", "hello".to_string()).await;
        assert!(matches!(result, Err(SendError::PeerOffline)));

        // Cancel the server
        server_handle.abort();
    }

    #[tokio::test]
    async fn test_send_message() {
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

        let router = Router::new(our_keypair, trusted_peers, CommsConfig::default());

        let listener = tokio::net::UnixListener::bind(&sock_path).unwrap();
        let server_handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let envelope = read_envelope_async(&mut stream, DEFAULT_MAX_MESSAGE_BYTES)
                .await
                .unwrap();

            // Verify it's a Message
            match envelope.kind {
                MessageKind::Message { body } => {
                    assert_eq!(body, "test body");
                }
                _ => panic!("expected Message"),
            }

            // Send ack
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
            write_envelope_async(&mut stream, &ack, DEFAULT_MAX_MESSAGE_BYTES)
                .await
                .unwrap();
        });

        let result = router
            .send_message("test-peer", "test body".to_string())
            .await;
        assert!(result.is_ok());
        server_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_send_request() {
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

        let router = Router::new(our_keypair, trusted_peers, CommsConfig::default());

        let listener = tokio::net::UnixListener::bind(&sock_path).unwrap();
        let server_handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let envelope = read_envelope_async(&mut stream, DEFAULT_MAX_MESSAGE_BYTES)
                .await
                .unwrap();

            // Verify it's a Request
            match envelope.kind {
                MessageKind::Request { intent, params } => {
                    assert_eq!(intent, "review-pr");
                    assert_eq!(params["pr"], 42);
                }
                _ => panic!("expected Request"),
            }

            // Send ack
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
            write_envelope_async(&mut stream, &ack, DEFAULT_MAX_MESSAGE_BYTES)
                .await
                .unwrap();
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
    async fn test_send_response() {
        let tmp = TempDir::new().unwrap();
        let sock_path = tmp.path().join("peer.sock");

        let peer_keypair = make_keypair();
        let our_keypair = make_keypair();

        let trusted_peers = make_trusted_peers_with_addr(
            "test-peer",
            &peer_keypair.public_key(),
            &format!("uds://{}", sock_path.display()),
        );

        let router = Router::new(our_keypair, trusted_peers, CommsConfig::default());
        let request_id = Uuid::new_v4();

        let listener = tokio::net::UnixListener::bind(&sock_path).unwrap();
        let server_handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let envelope = read_envelope_async(&mut stream, DEFAULT_MAX_MESSAGE_BYTES)
                .await
                .unwrap();

            // Verify it's a Response
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
            // No ack needed for Response
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
    async fn test_send_response_no_ack_wait() {
        let tmp = TempDir::new().unwrap();
        let sock_path = tmp.path().join("peer.sock");

        let peer_keypair = make_keypair();
        let our_keypair = make_keypair();

        let trusted_peers = make_trusted_peers_with_addr(
            "test-peer",
            &peer_keypair.public_key(),
            &format!("uds://{}", sock_path.display()),
        );

        // Short timeout to verify we don't wait
        let config = CommsConfig {
            ack_timeout_secs: 1,
            ..Default::default()
        };
        let router = Router::new(our_keypair, trusted_peers, config);

        // Server that never sends ack
        let listener = tokio::net::UnixListener::bind(&sock_path).unwrap();
        let server_handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _envelope = read_envelope_async(&mut stream, DEFAULT_MAX_MESSAGE_BYTES)
                .await
                .unwrap();
            // No ack sent - if router waited, it would timeout
            tokio::time::sleep(Duration::from_secs(3)).await;
        });

        // Time the send_response call
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

        // Should return immediately (< 1s), not wait for timeout
        assert!(result.is_ok(), "send_response should succeed");
        assert!(
            elapsed < Duration::from_secs(1),
            "send_response should not wait for ack, took {:?}",
            elapsed
        );

        server_handle.abort();
    }
}
