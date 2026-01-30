//! Router for Meerkat comms - high-level send API.
//!
//! The router provides a simple API for sending messages to peers,
//! handling envelope creation, signing, connection, and ack waiting.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpStream, UnixStream};
use tokio::sync::RwLock;
use tokio_util::codec::Framed;
use uuid::Uuid;

use crate::identity::{Keypair, Signature};
use crate::inbox::InboxError;
use crate::inproc::InprocRegistry;
use crate::transport::codec::{EnvelopeFrame, TransportCodec};
use crate::transport::{PeerAddr, TransportError, MAX_PAYLOAD_SIZE};
use crate::trust::{TrustedPeer, TrustedPeers};
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
    #[error("Peer inbox is full")]
    InboxFull,
}

/// Router for sending messages to peers.
pub struct Router {
    /// Our keypair for signing messages.
    keypair: Arc<Keypair>,
    /// Shared list of trusted peers (allows dynamic updates).
    trusted_peers: Arc<RwLock<TrustedPeers>>,
    /// Configuration.
    config: CommsConfig,
}

impl Router {
    /// Create a new router.
    pub fn new(keypair: Keypair, trusted_peers: TrustedPeers, config: CommsConfig) -> Self {
        Self {
            keypair: Arc::new(keypair),
            trusted_peers: Arc::new(RwLock::new(trusted_peers)),
            config,
        }
    }

    /// Create a new router with shared trusted peers.
    ///
    /// Use this when you need to share the trusted peers with other components
    /// (e.g., listeners that validate incoming connections, or dynamic peer updates).
    pub fn with_shared_peers(
        keypair: Keypair,
        trusted_peers: Arc<RwLock<TrustedPeers>>,
        config: CommsConfig,
    ) -> Self {
        Self {
            keypair: Arc::new(keypair),
            trusted_peers,
            config,
        }
    }

    pub fn keypair_arc(&self) -> Arc<Keypair> {
        self.keypair.clone()
    }

    /// Get a reference to the shared trusted peers.
    pub fn shared_trusted_peers(&self) -> Arc<RwLock<TrustedPeers>> {
        self.trusted_peers.clone()
    }

    /// Check if there are any trusted peers (requires async read lock).
    ///
    /// For hot-path usage, consider using `shared_trusted_peers()` with
    /// `try_read()` to avoid blocking.
    pub async fn has_peers(&self) -> bool {
        self.trusted_peers.read().await.has_peers()
    }

    /// Check if there are any trusted peers (non-blocking).
    ///
    /// Returns `None` if the lock couldn't be acquired immediately.
    /// Returns `Some(true)` if peers exist, `Some(false)` if no peers.
    pub fn try_has_peers(&self) -> Option<bool> {
        self.trusted_peers.try_read().ok().map(|g| g.has_peers())
    }

    /// Add or update a trusted peer dynamically.
    pub async fn add_trusted_peer(&self, peer: TrustedPeer) {
        let mut peers = self.trusted_peers.write().await;
        peers.upsert(peer);
    }

    /// Remove a trusted peer by pubkey.
    pub async fn remove_trusted_peer(&self, pubkey: &crate::identity::PubKey) -> bool {
        let mut peers = self.trusted_peers.write().await;
        peers.remove(pubkey)
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
        // Look up peer (read lock)
        let (peer_pubkey, peer_addr) = {
            let peers = self.trusted_peers.read().await;
            let peer = peers
                .get_by_name(peer_name)
                .ok_or_else(|| SendError::PeerNotFound(peer_name.to_string()))?;
            (peer.pubkey, peer.addr.clone())
        };

        // Parse peer address
        let addr =
            PeerAddr::parse(&peer_addr).map_err(|e| SendError::InvalidAddress(e.to_string()))?;

        // Determine if we should wait for ack
        let wait_for_ack = should_wait_for_ack(&kind);

        // Create and sign envelope
        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: self.keypair.public_key(),
            to: peer_pubkey,
            kind,
            sig: Signature::new([0u8; 64]),
        };
        envelope.sign(&self.keypair);

        // Connect and send
        match (addr, envelope) {
            (PeerAddr::Uds(path), envelope) => {
                let mut stream = UnixStream::connect(&path).await?;
                self.send_on_stream(&mut stream, envelope, wait_for_ack, &peer_pubkey)
                    .await?;
            }
            (PeerAddr::Tcp(addr_str), envelope) => {
                // DNS resolution happens here via ToSocketAddrs
                let mut stream = TcpStream::connect(&addr_str).await?;
                self.send_on_stream(&mut stream, envelope, wait_for_ack, &peer_pubkey)
                    .await?;
            }
            (PeerAddr::Inproc(name), envelope) => {
                // In-process delivery via global registry
                // The envelope is already signed, deliver directly to the peer's inbox
                //
                // IMPORTANT: We strictly require the pubkey to match. This preserves
                // trust invariants - we only deliver to the identity specified in
                // trusted_peers, not whatever happens to be registered under that name.
                let registry = InprocRegistry::global();
                if let Some(sender) = registry.get_by_pubkey(&peer_pubkey) {
                    sender
                        .send(crate::types::InboxItem::External { envelope })
                        .map_err(|err| match err {
                            InboxError::Closed => SendError::PeerOffline,
                            InboxError::Full => SendError::InboxFull,
                        })?;
                    // No ack needed for inproc - delivery is synchronous
                } else {
                    return Err(SendError::PeerNotFound(format!(
                        "Inproc peer '{}' (pubkey {:?}) not found in registry",
                        name,
                        &peer_pubkey.as_bytes()[..8]
                    )));
                }
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
        envelope: Envelope,
        wait_for_ack: bool,
        expected_peer: &crate::identity::PubKey,
    ) -> Result<(), SendError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let mut framed = Framed::new(
            stream,
            TransportCodec::with_max_payload_size(self.config.max_message_bytes),
        );

        let envelope_id = envelope.id;
        let frame = EnvelopeFrame {
            envelope,
            raw: Arc::new(Bytes::new()),
        };
        framed.send(frame).await?;

        // Wait for ack if required
        if wait_for_ack {
            let timeout = Duration::from_secs(self.config.ack_timeout_secs);
            let ack_result = tokio::time::timeout(timeout, framed.next()).await;

            match ack_result {
                Ok(Some(Ok(frame))) => {
                    let ack = frame.envelope;
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
                        MessageKind::Ack { in_reply_to } if in_reply_to == envelope_id => {
                            // Valid ack received
                        }
                        MessageKind::Ack { in_reply_to } => {
                            return Err(SendError::InvalidAck(format!(
                                "ack for wrong message: expected {}, got {}",
                                envelope_id, in_reply_to
                            )));
                        }
                        _ => {
                            return Err(SendError::InvalidAck("received non-ack response".into()));
                        }
                    }
                }
                Ok(Some(Err(_)) | None) => {
                    // Read error / peer closed connection / bad data
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::identity::PubKey;
    use std::io;

    fn make_keypair() -> Keypair {
        Keypair::generate()
    }

    #[test]
    fn test_router_struct() {
        let keypair = make_keypair();
        let trusted_peers = TrustedPeers::new();
        let config = CommsConfig::default();
        let router = Router::new(keypair, trusted_peers, config);
        // Router exists with all fields
        let _ = router.keypair.public_key();
        let _ = router.shared_trusted_peers(); // Access via shared_trusted_peers()
        let _ = router.config.ack_timeout_secs;
    }

    #[tokio::test]
    async fn test_router_dynamic_trust_updates() {
        let keypair = make_keypair();
        let trusted_peers = TrustedPeers::new();
        let config = CommsConfig::default();
        let router = Router::new(keypair, trusted_peers, config);

        // Initially no peers
        {
            let peers = router.shared_trusted_peers();
            let guard = peers.read().await;
            assert_eq!(guard.peers.len(), 0);
        }

        // Add a peer dynamically
        let peer = TrustedPeer {
            name: "dynamic-peer".to_string(),
            pubkey: PubKey::new([42u8; 32]),
            addr: "uds:///tmp/dynamic.sock".to_string(),
        };
        router.add_trusted_peer(peer.clone()).await;

        // Verify peer was added
        {
            let peers = router.shared_trusted_peers();
            let guard = peers.read().await;
            assert_eq!(guard.peers.len(), 1);
            assert_eq!(guard.peers[0].name, "dynamic-peer");
        }

        // Update the peer (same pubkey, different addr)
        let updated_peer = TrustedPeer {
            name: "dynamic-peer-updated".to_string(),
            pubkey: PubKey::new([42u8; 32]),
            addr: "uds:///tmp/updated.sock".to_string(),
        };
        router.add_trusted_peer(updated_peer).await;

        // Verify peer was updated (still 1 peer)
        {
            let peers = router.shared_trusted_peers();
            let guard = peers.read().await;
            assert_eq!(guard.peers.len(), 1);
            assert_eq!(guard.peers[0].name, "dynamic-peer-updated");
            assert_eq!(guard.peers[0].addr, "uds:///tmp/updated.sock");
        }

        // Remove the peer
        let removed = router.remove_trusted_peer(&PubKey::new([42u8; 32])).await;
        assert!(removed);

        // Verify peer was removed
        {
            let peers = router.shared_trusted_peers();
            let guard = peers.read().await;
            assert_eq!(guard.peers.len(), 0);
        }
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
}
