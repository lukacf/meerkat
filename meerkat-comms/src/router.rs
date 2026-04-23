//! Router for Meerkat comms - high-level send API.

#[cfg(not(target_arch = "wasm32"))]
use futures::{SinkExt, StreamExt};
use parking_lot::RwLock;
#[cfg(all(not(unix), not(target_arch = "wasm32")))]
use std::io::ErrorKind;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;
use thiserror::Error;
#[cfg(not(target_arch = "wasm32"))]
use tokio::io::{AsyncRead, AsyncWrite};
#[cfg(not(target_arch = "wasm32"))]
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;
#[cfg(not(target_arch = "wasm32"))]
use tokio_util::codec::Framed;
use uuid::Uuid;

use crate::identity::Keypair;
use crate::inbox::InboxSender;
use crate::inproc::InprocSendError;
#[cfg(not(target_arch = "wasm32"))]
use crate::transport::codec::{EnvelopeFrame, TransportCodec};
use crate::transport::TransportError;
use crate::trust::{TrustedPeer, TrustedPeers};
use crate::types::{Envelope, MessageKind};
use meerkat_core::comms::PeerId;

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
#[non_exhaustive]
pub enum SendError {
    #[error("Peer not found: {0}")]
    PeerNotFound(String),
    #[error("Peer offline (no ack received)")]
    PeerOffline,
    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    /// The peer admitted the transport layer but rejected our envelope at
    /// its ingress admission gate. Semantically distinct from `PeerOffline`
    /// — transport worked, policy refused. Carries the typed `DropReason`
    /// so callers can render e.g. `untrusted_sender` rather than masquerade
    /// as "peer unreachable".
    #[error("Peer dropped envelope at admission: {reason:?}")]
    AdmissionDropped { reason: crate::inbox::DropReason },
    /// Routing by [`PeerId`] landed in Wave-B V5 as a typed signature but
    /// transport dispatch is reinstated in Wave-C. Until then, callers that
    /// reach the router-level send path receive this typed error rather
    /// than `PeerOffline` so the "not yet wired" condition is observable
    /// at the call site instead of silently masquerading as a live peer
    /// being unreachable.
    #[error("router send path is awaiting Wave-C transport dispatcher")]
    DispatcherPending,
}

#[inline]
fn map_inproc_send_error(err: InprocSendError) -> SendError {
    match err {
        InprocSendError::PeerNotFound(peer) => SendError::PeerNotFound(peer),
        InprocSendError::InboxClosed | InprocSendError::InboxFull => SendError::PeerOffline,
        // Preserve the typed ingress-drop reason all the way through to
        // REST/RPC/MCP payloads. Do NOT collapse into `PeerOffline` — the
        // transport worked, the receiver's admission policy rejected us.
        InprocSendError::IngressDropped(reason) => SendError::AdmissionDropped { reason },
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
    /// Directory-filter side-channel for private (control-plane) trust
    /// edges. Membership here is additive to `trusted_peers`: the peer
    /// is still admitted AND send-resolvable, but `resolve_peer_directory()`
    /// filters it out of the `comms.peers` REST/RPC/MCP surface. Used e.g.
    /// for the supervisor bridge in session-backed mob members.
    private_pubkeys: Arc<RwLock<std::collections::HashSet<crate::identity::PubKey>>>,
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
            private_pubkeys: Arc::new(RwLock::new(std::collections::HashSet::new())),
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
            private_pubkeys: Arc::new(RwLock::new(std::collections::HashSet::new())),
            config,
            require_peer_auth,
            inbox_sender,
            inproc_namespace: None,
        }
    }

    /// Mark a peer as private (hidden from `resolve_peer_directory`).
    pub fn mark_private(&self, pubkey: crate::identity::PubKey) {
        self.private_pubkeys.write().insert(pubkey);
    }

    /// Remove the private marker for a peer. Returns `true` if the marker
    /// was present and removed.
    pub fn unmark_private(&self, pubkey: &crate::identity::PubKey) -> bool {
        self.private_pubkeys.write().remove(pubkey)
    }

    /// Returns `true` if the peer is currently marked private.
    pub fn is_private(&self, pubkey: &crate::identity::PubKey) -> bool {
        self.private_pubkeys.read().contains(pubkey)
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

    /// Canonical send: routing identity is a [`PeerId`], never a [`PeerName`].
    ///
    /// Wave-B V5: `PeerName` is display metadata; it is not safe as a routing
    /// key (duplicate names are legal). Callers that hold only a name must
    /// resolve it to a `PeerId` via [`TrustStore::resolve_name`] at the
    /// boundary and handle the typed
    /// [`TrustResolveError::Ambiguous`](crate::trust::TrustResolveError::Ambiguous)
    /// case explicitly — the router will not guess.
    ///
    /// Transport dispatch (inproc / uds / tcp fan-out, signing, ack waiting)
    /// is reinstated in Wave-C. In Wave-B the signature exists and is the
    /// sole entry point, but calls return [`SendError::DispatcherPending`]
    /// until Wave-C lands — this way the presence of the dead code path is
    /// visible at runtime rather than masquerading as a reachable peer.
    pub async fn send(&self, dest: PeerId, kind: MessageKind) -> Result<Uuid, SendError> {
        // The `dest`/`kind` values are the typed inputs Wave-C will consume.
        let _ = (dest, kind);
        Err(SendError::DispatcherPending)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn should_wait_for_ack(kind: &MessageKind) -> bool {
    !matches!(kind, MessageKind::Ack { .. } | MessageKind::Response { .. })
}
