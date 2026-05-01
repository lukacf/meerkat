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
use crate::inproc::{InprocRegistry, InprocSendError};
#[cfg(not(target_arch = "wasm32"))]
use crate::transport::codec::{EnvelopeFrame, TransportCodec};
use crate::transport::{PeerAddr, TransportError};
use crate::trust::{TrustError, TrustedPeer, TrustedPeers};
use crate::types::{Envelope, MessageKind};
use meerkat_core::comms::PeerId;

/// Derive the canonical [`PeerId`] for a signing [`crate::identity::PubKey`].
///
/// Thin wrapper over [`crate::identity::PubKey::to_peer_id`] — kept for
/// callers that prefer the free-function form. The UUIDv5 namespace lives
/// with [`crate::identity::PubKey`] so it travels with the type being
/// hashed.
pub fn peer_id_from_pubkey(pubkey: &crate::identity::PubKey) -> PeerId {
    pubkey.to_peer_id()
}

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
    /// The destination [`PeerId`] does not resolve to any trusted-peer
    /// entry. Carries the typed [`PeerId`] so callers can distinguish it
    /// from a display-name mismatch.
    #[error("Peer not found: {0}")]
    PeerNotFound(PeerId),
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
}

#[inline]
fn map_inproc_send_error(err: InprocSendError, dest: PeerId) -> SendError {
    match err {
        InprocSendError::PeerNotFound(_) => SendError::PeerNotFound(dest),
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
    /// Canonical peer IDs supplied at descriptor registration time, keyed by
    /// signing pubkey. This preserves explicit `PeerId`s for descriptor
    /// registrations where the runtime owns a stable routing id in addition
    /// to the peer's signing key.
    trusted_peer_ids: Arc<RwLock<std::collections::HashMap<crate::identity::PubKey, PeerId>>>,
    /// Directory-filter side-channel for private (control-plane) trust
    /// edges. Membership here is additive to `trusted_peers`: the peer
    /// is still admitted AND send-resolvable, but `resolve_peer_directory()`
    /// filters it out of the `comms.peers` REST/RPC/MCP surface. Used e.g.
    /// for the supervisor bridge in session-backed mob members.
    private_peer_ids: Arc<RwLock<std::collections::HashSet<PeerId>>>,
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
            trusted_peer_ids: Arc::new(RwLock::new(std::collections::HashMap::new())),
            private_peer_ids: Arc::new(RwLock::new(std::collections::HashSet::new())),
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
            trusted_peer_ids: Arc::new(RwLock::new(std::collections::HashMap::new())),
            private_peer_ids: Arc::new(RwLock::new(std::collections::HashSet::new())),
            config,
            require_peer_auth,
            inbox_sender,
            inproc_namespace: None,
        }
    }

    /// Mark a peer as private (hidden from `resolve_peer_directory`).
    pub fn mark_private(&self, pubkey: crate::identity::PubKey) {
        self.mark_private_peer_id(self.peer_id_for_pubkey(&pubkey));
    }

    /// Mark a peer as private by canonical `PeerId`.
    pub(crate) fn mark_private_peer_id(&self, peer_id: PeerId) {
        self.private_peer_ids.write().insert(peer_id);
    }

    /// Remove the private marker for a peer. Returns `true` if the marker
    /// was present and removed.
    pub fn unmark_private(&self, peer_id: &PeerId) -> bool {
        self.private_peer_ids.write().remove(peer_id)
    }

    /// Returns `true` if the peer is currently marked private.
    pub fn is_private(&self, pubkey: &crate::identity::PubKey) -> bool {
        self.private_peer_ids
            .read()
            .contains(&peer_id_from_pubkey(pubkey))
    }

    pub(crate) fn private_peer_ids(&self) -> std::collections::HashSet<PeerId> {
        self.private_peer_ids.read().clone()
    }

    pub(crate) fn peer_id_for_pubkey(&self, pubkey: &crate::identity::PubKey) -> PeerId {
        self.trusted_peer_ids
            .read()
            .get(pubkey)
            .copied()
            .unwrap_or_else(|| peer_id_from_pubkey(pubkey))
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

    pub fn add_trusted_peer(&self, peer: TrustedPeer) -> Result<(), TrustError> {
        let peer_id = peer_id_from_pubkey(&peer.pubkey);
        self.add_trusted_peer_with_peer_id(peer_id, peer)
    }

    pub(crate) fn add_trusted_peer_with_peer_id(
        &self,
        peer_id: PeerId,
        peer: TrustedPeer,
    ) -> Result<(), TrustError> {
        let pubkey = peer.pubkey;
        self.trusted_peers.write().upsert(peer)?;
        self.trusted_peer_ids.write().insert(pubkey, peer_id);
        Ok(())
    }

    pub fn remove_trusted_peer(&self, peer_id: &PeerId) -> bool {
        let peer_ids = self.trusted_peer_ids.read().clone();
        let removed = {
            let mut trusted = self.trusted_peers.write();
            let Some(index) = trusted.peers.iter().position(|peer| {
                peer_ids
                    .get(&peer.pubkey)
                    .copied()
                    .unwrap_or_else(|| peer_id_from_pubkey(&peer.pubkey))
                    == *peer_id
            }) else {
                return false;
            };
            trusted.peers.remove(index)
        };
        self.trusted_peer_ids.write().remove(&removed.pubkey);
        self.private_peer_ids.write().remove(peer_id);
        true
    }

    fn trusted_peer_by_peer_id(&self, peer_id: &PeerId) -> Option<TrustedPeer> {
        let peer_ids = self.trusted_peer_ids.read().clone();
        self.trusted_peers
            .read()
            .peers
            .iter()
            .find(|peer| {
                !peer.pubkey.is_zero()
                    && peer_ids
                        .get(&peer.pubkey)
                        .copied()
                        .unwrap_or_else(|| peer_id_from_pubkey(&peer.pubkey))
                        == *peer_id
            })
            .cloned()
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
    /// resolve it to a `PeerId` via [`crate::trust::TrustStore::resolve_name`]
    /// at the boundary and handle the typed
    /// [`TrustResolveError::Ambiguous`](crate::trust::TrustResolveError::Ambiguous)
    /// case explicitly — the router will not guess.
    pub async fn send(&self, dest: PeerId, kind: MessageKind) -> Result<Uuid, SendError> {
        self.send_with_id(dest, Uuid::new_v4(), kind).await
    }

    /// Canonical send with a caller-supplied envelope id.
    ///
    /// Used by the correlated request/response path, which reserves a
    /// stream key before the envelope goes out so replies can correlate
    /// via `in_reply_to` without an extra local id map.
    pub async fn send_with_id(
        &self,
        dest: PeerId,
        envelope_id: Uuid,
        kind: MessageKind,
    ) -> Result<Uuid, SendError> {
        let inproc_namespace = self.inproc_namespace.as_deref().unwrap_or("");
        let peer = self
            .trusted_peer_by_peer_id(&dest)
            .or_else(|| {
                if self.require_peer_auth {
                    None
                } else {
                    // Auth-disabled fallback: scan the inproc registry for an
                    // entry whose derived PeerId matches. Display names remain
                    // the inproc lookup key, but the routing match is PeerId.
                    InprocRegistry::global()
                        .peers_in_namespace(inproc_namespace)
                        .into_iter()
                        .find(|p| peer_id_from_pubkey(&p.pubkey) == dest)
                        .map(|p| TrustedPeer {
                            name: p.name.clone(),
                            pubkey: p.pubkey,
                            addr: format!("inproc://{}", p.name),
                            meta: p.meta,
                        })
                }
            })
            .ok_or(SendError::PeerNotFound(dest))?;
        let addr = PeerAddr::parse(&peer.addr)?;
        let mut envelope = Envelope {
            id: envelope_id,
            from: self.keypair.public_key(),
            to: peer.pubkey,
            kind,
            sig: crate::identity::Signature::new([0u8; 64]),
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
                    let mut stream = TcpStream::connect(&addr_str).await?;
                    self.send_on_stream(&mut stream, envelope, wait_for_ack)
                        .await
                }
                PeerAddr::Inproc(_) => {
                    let registry = InprocRegistry::global();
                    // Inproc delivery is constrained by the resolved peer's
                    // pubkey: the trust store already pinned it to `dest`,
                    // so the registry lookup must agree or we refuse the
                    // send rather than fall through to a name collision.
                    match registry.send_with_signature_in_namespace_with_id(
                        inproc_namespace,
                        &self.keypair,
                        &peer.name,
                        envelope.id,
                        envelope.kind.clone(),
                        self.require_peer_auth,
                    ) {
                        Ok(uuid) => Ok(uuid),
                        Err(InprocSendError::PeerNotFound(_)) => registry
                            .send_cross_namespace_with_id(
                                &self.keypair,
                                &peer.name,
                                &peer.pubkey,
                                envelope.id,
                                envelope.kind,
                                self.require_peer_auth,
                            )
                            .map_err(|err| map_inproc_send_error(err, dest)),
                        Err(other) => Err(map_inproc_send_error(other, dest)),
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
                        &peer.name,
                        envelope.kind.clone(),
                        self.require_peer_auth,
                    ) {
                        Ok(uuid) => Ok(uuid),
                        Err(InprocSendError::PeerNotFound(_)) => registry
                            .send_cross_namespace(
                                &self.keypair,
                                &peer.name,
                                &peer.pubkey,
                                envelope.kind,
                                self.require_peer_auth,
                            )
                            .map_err(|err| map_inproc_send_error(err, dest)),
                        Err(other) => Err(map_inproc_send_error(other, dest)),
                    }
                }
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn should_wait_for_ack(kind: &MessageKind) -> bool {
    !matches!(kind, MessageKind::Ack { .. } | MessageKind::Response { .. })
}
