//! Router for Meerkat comms - high-level send API.

#[cfg(not(target_arch = "wasm32"))]
use futures::{SinkExt, StreamExt};
use parking_lot::RwLock;
#[cfg(all(not(unix), not(target_arch = "wasm32")))]
use std::io::ErrorKind;
#[cfg(target_os = "espidf")]
use std::io::{Read as _, Write as _};
#[cfg(target_os = "espidf")]
use std::net::TcpStream as StdTcpStream;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;
use thiserror::Error;
#[cfg(not(target_arch = "wasm32"))]
use tokio::io::{AsyncRead, AsyncWrite};
#[cfg(all(not(target_arch = "wasm32"), not(target_os = "espidf")))]
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;
#[cfg(not(target_arch = "wasm32"))]
use tokio_util::codec::Framed;
use uuid::Uuid;

use crate::identity::{Keypair, Signature};
use crate::inbox::InboxSender;
use crate::inproc::{InprocRegistry, InprocSendError};
#[cfg(not(target_arch = "wasm32"))]
use crate::transport::codec::{EnvelopeFrame, TransportCodec};
use crate::transport::{PeerAddr, TransportError};
use crate::trust::{TrustedPeer, TrustedPeers};
use crate::types::{Envelope, MessageKind, Status};

pub const DEFAULT_ACK_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_MAX_MESSAGE_BYTES: u32 = crate::transport::MAX_PAYLOAD_SIZE;

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

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub struct Router {
    keypair: Arc<Keypair>,
    /// Single source of truth for trusted peers. Shared by the Router,
    /// CommsRuntime, IngressClassificationContext, and any callers of
    /// `trusted_peers_shared()`. Uses `parking_lot::RwLock` so ingress
    /// classification can read synchronously.
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
    pub fn shared_trusted_peers(&self) -> Arc<parking_lot::RwLock<TrustedPeers>> {
        self.trusted_peers.clone()
    }
    pub fn inbox_sender(&self) -> &InboxSender {
        &self.inbox_sender
    }

    pub fn has_peers(&self) -> bool {
        self.trusted_peers.read().has_peers()
    }

    pub fn add_trusted_peer(&self, peer: TrustedPeer) {
        self.trusted_peers.write().upsert(peer);
    }

    pub fn remove_trusted_peer(&self, pubkey: &crate::identity::PubKey) -> bool {
        self.trusted_peers.write().remove(pubkey)
    }

    /// Get the trusted peers Arc.
    ///
    /// This is the single source of truth for trust state. The same Arc is
    /// shared with IngressClassificationContext, so any mutation is
    /// immediately visible to ingress classification.
    pub fn classification_peers_arc(&self) -> Arc<parking_lot::RwLock<TrustedPeers>> {
        self.trusted_peers.clone()
    }

    pub async fn send(&self, peer_name: &str, kind: MessageKind) -> Result<Uuid, SendError> {
        let inproc_namespace = self.inproc_namespace.as_deref().unwrap_or("");
        let peer = {
            let peers = self.trusted_peers.read();
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

        #[cfg(not(target_arch = "wasm32"))]
        {
            let wait_for_ack = should_wait_for_ack(&envelope.kind);
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
                    "unix domain sockets are not supported on this platform",
                )
                .into()),
                PeerAddr::Tcp(addr_str) => {
                    #[cfg(target_os = "espidf")]
                    {
                        let mut stream = StdTcpStream::connect(&addr_str)?;
                        stream.set_nodelay(true)?;
                        self.send_on_blocking_tcp_stream(&mut stream, envelope, wait_for_ack)
                    }
                    #[cfg(not(target_os = "espidf"))]
                    {
                        let mut stream = TcpStream::connect(&addr_str).await?;
                        self.send_on_stream(&mut stream, envelope, wait_for_ack)
                            .await
                    }
                }
                PeerAddr::Inproc(_) => {
                    let registry = InprocRegistry::global();
                    match registry.send_with_signature_in_namespace(
                        inproc_namespace,
                        &self.keypair,
                        peer_name,
                        envelope.kind.clone(),
                        self.require_peer_auth,
                    ) {
                        Ok(uuid) => Ok(uuid),
                        Err(InprocSendError::PeerNotFound(_)) => {
                            tracing::debug!(
                                peer = peer_name,
                                namespace = inproc_namespace,
                                "same-namespace lookup failed, falling back to cross-namespace send"
                            );
                            registry
                                .send_cross_namespace(
                                    &self.keypair,
                                    peer_name,
                                    &peer.pubkey,
                                    envelope.kind,
                                    self.require_peer_auth,
                                )
                                .map_err(map_inproc_send_error)
                        }
                        Err(other) => Err(map_inproc_send_error(other)),
                    }
                }
            }
        }

        #[cfg(target_arch = "wasm32")]
        {
            match addr {
                PeerAddr::Tcp(_) => Err(SendError::Transport(TransportError::InvalidAddress(
                    "TCP transport is not available on wasm32".to_string(),
                ))),
                PeerAddr::Inproc(_) => {
                    let registry = InprocRegistry::global();
                    match registry.send_with_signature_in_namespace(
                        inproc_namespace,
                        &self.keypair,
                        peer_name,
                        envelope.kind.clone(),
                        self.require_peer_auth,
                    ) {
                        Ok(uuid) => Ok(uuid),
                        Err(InprocSendError::PeerNotFound(_)) => {
                            tracing::debug!(
                                peer = peer_name,
                                namespace = inproc_namespace,
                                "same-namespace lookup failed, falling back to cross-namespace send"
                            );
                            registry
                                .send_cross_namespace(
                                    &self.keypair,
                                    peer_name,
                                    &peer.pubkey,
                                    envelope.kind,
                                    self.require_peer_auth,
                                )
                                .map_err(map_inproc_send_error)
                        }
                        Err(other) => Err(map_inproc_send_error(other)),
                    }
                }
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
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

    #[cfg(target_os = "espidf")]
    fn send_on_blocking_tcp_stream(
        &self,
        stream: &mut StdTcpStream,
        envelope: Envelope,
        wait_for_ack: bool,
    ) -> Result<Uuid, SendError> {
        let sent_id = envelope.id;
        let sent_to = envelope.to;

        write_blocking_envelope(stream, &envelope)?;
        if wait_for_ack {
            stream.set_read_timeout(Some(Duration::from_secs(self.config.ack_timeout_secs)))?;
            let frame = read_blocking_envelope(stream)?;
            if let MessageKind::Ack { in_reply_to } = frame.kind {
                if self.require_peer_auth {
                    if !frame.verify() {
                        return Err(SendError::PeerOffline);
                    }
                    if frame.from != sent_to {
                        return Err(SendError::PeerOffline);
                    }
                    if frame.to != self.keypair.public_key() {
                        return Err(SendError::PeerOffline);
                    }
                } else if frame.to != self.keypair.public_key() {
                    return Err(SendError::PeerOffline);
                }
                if in_reply_to != sent_id {
                    return Err(SendError::PeerOffline);
                }
                Ok(sent_id)
            } else {
                Err(SendError::PeerOffline)
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

#[cfg(not(target_arch = "wasm32"))]
fn should_wait_for_ack(kind: &MessageKind) -> bool {
    !matches!(kind, MessageKind::Ack { .. } | MessageKind::Response { .. })
}

#[cfg(target_os = "espidf")]
fn write_blocking_envelope(
    stream: &mut StdTcpStream,
    envelope: &Envelope,
) -> Result<(), SendError> {
    let mut payload = Vec::new();
    ciborium::into_writer(envelope, &mut payload).map_err(|err| {
        SendError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            err.to_string(),
        ))
    })?;
    let len = payload.len() as u32;
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(&payload)?;
    stream.flush()?;
    Ok(())
}

#[cfg(target_os = "espidf")]
fn read_blocking_envelope(stream: &mut StdTcpStream) -> Result<Envelope, SendError> {
    let mut len_bytes = [0u8; 4];
    stream.read_exact(&mut len_bytes)?;
    let len = u32::from_be_bytes(len_bytes) as usize;
    if len > crate::transport::MAX_PAYLOAD_SIZE as usize {
        return Err(SendError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame too large: {len}"),
        )));
    }
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload)?;
    ciborium::from_reader(payload.as_slice()).map_err(|err| {
        SendError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            err.to_string(),
        ))
    })
}
