//! Router for Meerkat comms - high-level send API.

use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpStream, UnixStream};
use tokio::sync::RwLock;
use tokio_util::codec::Framed;
use uuid::Uuid;

use crate::identity::{Keypair, Signature};
use crate::inbox::InboxSender;
use crate::inproc::InprocRegistry;
use crate::transport::codec::{EnvelopeFrame, TransportCodec};
use crate::transport::{MAX_PAYLOAD_SIZE, PeerAddr, TransportError};
use crate::trust::{TrustedPeer, TrustedPeers};
use crate::types::{Envelope, MessageKind, Status};

pub const DEFAULT_ACK_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_MAX_MESSAGE_BYTES: u32 = MAX_PAYLOAD_SIZE;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CommsConfig {
    pub ack_timeout_secs: u64,
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

#[derive(Debug, Error)]
pub enum SendError {
    #[error("Peer not found: {0}")]
    PeerNotFound(String),
    #[error("Peer offline (no ack received)")]
    PeerOffline,
    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub struct Router {
    keypair: Arc<Keypair>,
    trusted_peers: Arc<RwLock<TrustedPeers>>,
    config: CommsConfig,
    inbox_sender: InboxSender,
}

impl Router {
    pub fn new(
        keypair: Keypair,
        trusted_peers: TrustedPeers,
        config: CommsConfig,
        inbox_sender: InboxSender,
    ) -> Self {
        Self {
            keypair: Arc::new(keypair),
            trusted_peers: Arc::new(RwLock::new(trusted_peers)),
            config,
            inbox_sender,
        }
    }

    pub fn with_shared_peers(
        keypair: Keypair,
        trusted_peers: Arc<RwLock<TrustedPeers>>,
        config: CommsConfig,
        inbox_sender: InboxSender,
    ) -> Self {
        Self {
            keypair: Arc::new(keypair),
            trusted_peers,
            config,
            inbox_sender,
        }
    }

    pub fn keypair_arc(&self) -> Arc<Keypair> {
        self.keypair.clone()
    }
    pub fn shared_trusted_peers(&self) -> Arc<RwLock<TrustedPeers>> {
        self.trusted_peers.clone()
    }
    pub fn inbox_sender(&self) -> &InboxSender {
        &self.inbox_sender
    }

    pub async fn has_peers(&self) -> bool {
        self.trusted_peers.read().await.has_peers()
    }
    pub fn try_has_peers(&self) -> Option<bool> {
        self.trusted_peers.try_read().ok().map(|g| g.has_peers())
    }

    pub async fn add_trusted_peer(&self, peer: TrustedPeer) {
        let mut peers = self.trusted_peers.write().await;
        peers.upsert(peer);
    }

    pub async fn remove_trusted_peer(&self, pubkey: &crate::identity::PubKey) -> bool {
        let mut peers = self.trusted_peers.write().await;
        peers.remove(pubkey)
    }

    pub async fn send(&self, peer_name: &str, kind: MessageKind) -> Result<(), SendError> {
        let (peer_pubkey, peer_addr) = {
            let peers = self.trusted_peers.read().await;
            let peer = peers
                .get_by_name(peer_name)
                .ok_or_else(|| SendError::PeerNotFound(peer_name.to_string()))?;
            (peer.pubkey, peer.addr.clone())
        };

        let addr = PeerAddr::parse(&peer_addr)?;
        let wait_for_ack = should_wait_for_ack(&kind);

        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: self.keypair.public_key(),
            to: peer_pubkey,
            kind,
            sig: Signature::new([0u8; 64]),
        };
        envelope.sign(&self.keypair);

        match addr {
            PeerAddr::Uds(path) => {
                let mut stream = UnixStream::connect(&path).await?;
                self.send_on_stream(&mut stream, envelope, wait_for_ack)
                    .await
            }
            PeerAddr::Tcp(addr_str) => {
                let mut stream = TcpStream::connect(&addr_str).await?;
                self.send_on_stream(&mut stream, envelope, wait_for_ack)
                    .await
            }
            PeerAddr::Inproc(_) => {
                let registry = InprocRegistry::global();
                registry
                    .send(&self.keypair, peer_name, envelope.kind)
                    .map_err(|e| match e {
                        crate::inproc::InprocSendError::PeerNotFound(n) => {
                            SendError::PeerNotFound(n)
                        }
                        _ => SendError::PeerOffline,
                    })
            }
        }
    }

    async fn send_on_stream<S>(
        &self,
        stream: &mut S,
        envelope: Envelope,
        wait_for_ack: bool,
    ) -> Result<(), SendError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let sent_id = envelope.id;
        let sent_to = envelope.to;

        let mut framed = Framed::new(stream, TransportCodec::new(self.config.max_message_bytes));
        framed.send(EnvelopeFrame::from_envelope(envelope)).await?;
        if wait_for_ack {
            match tokio::time::timeout(
                Duration::from_secs(self.config.ack_timeout_secs),
                framed.next(),
            )
            .await
            {
                Ok(Some(Ok(frame))) => {
                    // Validate ACK: signature, sender matches recipient, in_reply_to matches sent id
                    if let MessageKind::Ack { in_reply_to } = frame.envelope.kind {
                        if !frame.envelope.verify() {
                            return Err(SendError::PeerOffline);
                        }
                        if frame.envelope.from != sent_to {
                            return Err(SendError::PeerOffline);
                        }
                        if in_reply_to != sent_id {
                            return Err(SendError::PeerOffline);
                        }
                        Ok(())
                    } else {
                        Err(SendError::PeerOffline)
                    }
                }
                _ => Err(SendError::PeerOffline),
            }
        } else {
            Ok(())
        }
    }

    pub async fn send_message(&self, peer_name: &str, body: String) -> Result<(), SendError> {
        self.send(peer_name, MessageKind::Message { body }).await
    }

    pub async fn send_request(
        &self,
        peer_name: &str,
        intent: String,
        params: serde_json::Value,
    ) -> Result<(), SendError> {
        self.send(peer_name, MessageKind::Request { intent, params })
            .await
    }

    pub async fn send_response(
        &self,
        peer_name: &str,
        in_reply_to: Uuid,
        status: Status,
        result: serde_json::Value,
    ) -> Result<(), SendError> {
        self.send(
            peer_name,
            MessageKind::Response {
                in_reply_to,
                status,
                result,
            },
        )
        .await
    }
}

fn should_wait_for_ack(kind: &MessageKind) -> bool {
    !matches!(kind, MessageKind::Ack { .. } | MessageKind::Response { .. })
}
