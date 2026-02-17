//! Router for Meerkat comms - high-level send API.

use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
#[cfg(not(unix))]
use std::io::ErrorKind;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
#[cfg(unix)]
use tokio::net::UnixStream;
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio_util::codec::Framed;
use uuid::Uuid;

use crate::identity::{Keypair, Signature};
use crate::inbox::InboxSender;
use crate::inproc::{InprocRegistry, InprocSendError};
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

#[inline]
fn map_inproc_send_error(err: InprocSendError) -> SendError {
    match err {
        InprocSendError::PeerNotFound(peer) => SendError::PeerNotFound(peer),
        InprocSendError::InboxClosed | InprocSendError::InboxFull => SendError::PeerOffline,
    }
}

pub struct Router {
    keypair: Arc<Keypair>,
    trusted_peers: Arc<RwLock<TrustedPeers>>,
    config: CommsConfig,
    require_peer_auth: bool,
    inbox_sender: InboxSender,
    inproc_namespace: Option<String>,
}

impl Router {
    pub fn new(
        keypair: Keypair,
        trusted_peers: TrustedPeers,
        config: CommsConfig,
        inbox_sender: InboxSender,
        require_peer_auth: bool,
    ) -> Self {
        Self {
            keypair: Arc::new(keypair),
            trusted_peers: Arc::new(RwLock::new(trusted_peers)),
            config,
            require_peer_auth,
            inbox_sender,
            inproc_namespace: None,
        }
    }

    pub fn with_shared_peers(
        keypair: Keypair,
        trusted_peers: Arc<RwLock<TrustedPeers>>,
        config: CommsConfig,
        inbox_sender: InboxSender,
        require_peer_auth: bool,
    ) -> Self {
        Self {
            keypair: Arc::new(keypair),
            trusted_peers,
            config,
            require_peer_auth,
            inbox_sender,
            inproc_namespace: None,
        }
    }

    /// Scope in-process routing to a namespace.
    pub fn with_inproc_namespace(mut self, namespace: Option<String>) -> Self {
        self.inproc_namespace = namespace;
        self
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

    pub async fn send(&self, peer_name: &str, kind: MessageKind) -> Result<Uuid, SendError> {
        let inproc_namespace = self.inproc_namespace.as_deref().unwrap_or("");
        let peer = {
            let peers = self.trusted_peers.read().await;
            peers.get_by_name(peer_name).cloned()
        }
        .or_else(|| {
            if self.require_peer_auth {
                None
            } else {
                InprocRegistry::global()
                    .get_by_name_in_namespace(inproc_namespace, peer_name)
                    .map(|(pubkey, _)| crate::TrustedPeer {
                        name: peer_name.to_string(),
                        pubkey,
                        addr: format!("inproc://{peer_name}"),
                        meta: crate::PeerMeta::default(),
                    })
            }
        })
        .ok_or_else(|| SendError::PeerNotFound(peer_name.to_string()))?;
        let wait_for_ack = should_wait_for_ack(&kind);
        let addr = PeerAddr::parse(&peer.addr)?;
        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: self.keypair.public_key(),
            to: peer.pubkey,
            kind,
            sig: Signature::new([0u8; 64]),
        };
        if self.require_peer_auth {
            envelope.sign(&self.keypair);
        }

        match addr {
            #[cfg(unix)]
            PeerAddr::Uds(path) => {
                let mut stream = UnixStream::connect(&path).await?;
                self.send_on_stream(&mut stream, envelope, wait_for_ack)
                    .await
            }
            #[cfg(not(unix))]
            PeerAddr::Uds(_path) => Err(std::io::Error::new(
                ErrorKind::Unsupported,
                "unix domain sockets are not supported on this platform"
            ).into()),
            PeerAddr::Tcp(addr_str) => {
                let mut stream = TcpStream::connect(&addr_str).await?;
                self.send_on_stream(&mut stream, envelope, wait_for_ack)
                    .await
            }
            PeerAddr::Inproc(_) => {
                let registry = InprocRegistry::global();
                registry
                    .send_with_signature_in_namespace(
                        inproc_namespace,
                        &self.keypair,
                        peer_name,
                        envelope.kind,
                        self.require_peer_auth,
                    )
                    .map_err(map_inproc_send_error)
            }
        }
    }

    async fn send_on_stream<S>(
        &self,
        stream: &mut S,
        envelope: Envelope,
        wait_for_ack: bool,
    ) -> Result<Uuid, SendError>
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
                    // Validate ACK: signature, sender, recipient, and in_reply_to
                    if let MessageKind::Ack { in_reply_to } = frame.envelope.kind {
                        if self.require_peer_auth {
                            if !frame.envelope.verify() {
                                return Err(SendError::PeerOffline);
                            }
                            if frame.envelope.from != sent_to {
                                return Err(SendError::PeerOffline);
                            }
                            // Verify ACK is addressed to us (prevents misrouted/injected ACKs)
                            if frame.envelope.to != self.keypair.public_key() {
                                return Err(SendError::PeerOffline);
                            }
                        } else if frame.envelope.to != self.keypair.public_key() {
                            return Err(SendError::PeerOffline);
                        }
                        if in_reply_to != sent_id {
                            return Err(SendError::PeerOffline);
                        }
                        Ok(sent_id)
                    } else {
                        Err(SendError::PeerOffline)
                    }
                }
                _ => Err(SendError::PeerOffline),
            }
        } else {
            Ok(sent_id)
        }
    }
    pub async fn send_request(
        &self,
        peer_name: &str,
        intent: String,
        params: serde_json::Value,
    ) -> Result<Uuid, SendError> {
        self.send(peer_name, MessageKind::Request { intent, params })
            .await
    }

    pub async fn send_response(
        &self,
        peer_name: &str,
        in_reply_to: Uuid,
        status: Status,
        result: serde_json::Value,
    ) -> Result<Uuid, SendError> {
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
