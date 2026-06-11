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
use crate::trust::{TrustEntry, TrustError, TrustStore, TrustedPeersView};
use crate::types::{Envelope, MessageKind};
use meerkat_core::comms::{GeneratedCommsTrustAuthoritySourceKind, PeerId};

type TrustSourceSet = std::collections::BTreeSet<GeneratedCommsTrustAuthoritySourceKind>;
type PeerIdSet = std::collections::BTreeSet<PeerId>;
type TrustSourceMap = std::collections::BTreeMap<PeerId, TrustSourceSet>;
type TrustDescriptorMap = std::collections::BTreeMap<
    PeerId,
    std::collections::BTreeMap<GeneratedCommsTrustAuthoritySourceKind, TrustEntry>,
>;

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

/// Outcome of a successful router send.
///
/// Carries the verified delivery-acknowledgement truth alongside the envelope
/// id. `acked` is `true` only when the send waited for and cryptographically
/// verified a peer ACK over a stream transport. Inproc delivery has no ACK
/// round-trip (direct inbox handoff), and non-ack-bearing kinds (Ack/Response)
/// never wait — both report `acked == false`. The public receipt must reflect
/// this truth rather than hardcoding it.
#[derive(Debug, Clone, Copy)]
pub struct SendOutcome {
    pub envelope_id: Uuid,
    pub acked: bool,
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
    /// Single source of truth for trusted peers, keyed by canonical
    /// [`PeerId`]. Shared internally by the Router, CommsRuntime, and
    /// IngressClassificationContext. Uses `parking_lot::RwLock` so ingress
    /// classification can read synchronously, but the mutable lock is not
    /// exposed outside this crate.
    trusted_peers: Arc<RwLock<TrustStore>>,
    /// Mechanical projection of which generated source currently owns each
    /// trust row. Admission uses the union; generated removals are scoped to
    /// the source that authorized them.
    trusted_peer_sources: Arc<RwLock<TrustSourceMap>>,
    /// Source-owned descriptors for generated trust rows. The public
    /// `trusted_peers` set remains the send/admission union, but source-scoped
    /// snapshots read this map so one generated owner cannot rewrite another
    /// owner's descriptor projection.
    trusted_peer_descriptors_by_source: Arc<RwLock<TrustDescriptorMap>>,
    /// Source-attributed directory visibility for private (control-plane)
    /// trust edges. A peer is private iff some generated source authorized it
    /// privately. This is the single visibility authority: `is_private`,
    /// `is_private_peer_id`, and directory resolution derive from it, so
    /// rebuilding the Router from the generated trust-authority snapshot
    /// reproduces identical `comms.peers` visibility with no separate flat
    /// side-map to repopulate. Private peers are still admitted AND
    /// send-resolvable, but `resolve_peer_directory()` filters them out of the
    /// `comms.peers` REST/RPC/MCP surface (e.g. the supervisor bridge in
    /// session-backed mob members).
    private_peer_sources: Arc<RwLock<TrustSourceMap>>,
    config: CommsConfig,
    require_peer_auth: bool,
    inbox_sender: InboxSender,
    inproc_namespace: Option<String>,
}

impl Router {
    /// Construct a router with an empty live trust projection.
    ///
    /// Live trust rows are installed only by generated machine/composition
    /// mutation authority.
    pub fn new(
        keypair: Keypair,
        config: CommsConfig,
        inbox_sender: InboxSender,
        require_peer_auth: bool,
    ) -> Self {
        Self::with_shared_peers(
            keypair,
            Arc::new(RwLock::new(TrustStore::new())),
            config,
            inbox_sender,
            require_peer_auth,
        )
    }

    pub(crate) fn with_shared_peers(
        keypair: Keypair,
        trusted_peers: Arc<RwLock<TrustStore>>,
        config: CommsConfig,
        inbox_sender: InboxSender,
        require_peer_auth: bool,
    ) -> Self {
        *trusted_peers.write() = TrustStore::new();
        Self {
            keypair: Arc::new(keypair),
            trusted_peers,
            trusted_peer_sources: Arc::new(RwLock::new(std::collections::BTreeMap::new())),
            trusted_peer_descriptors_by_source: Arc::new(RwLock::new(
                std::collections::BTreeMap::new(),
            )),
            private_peer_sources: Arc::new(RwLock::new(std::collections::BTreeMap::new())),
            config,
            require_peer_auth,
            inbox_sender,
            inproc_namespace: None,
        }
    }

    /// Returns `true` if the peer is currently marked private.
    pub fn is_private(&self, pubkey: &crate::identity::PubKey) -> bool {
        self.is_private_peer_id(&peer_id_from_pubkey(pubkey))
    }

    /// Snapshot of the peer IDs that are currently directory-private, derived
    /// from the source-attributed `private_peer_sources` authority (a peer is
    /// private iff some generated source authorized it privately).
    pub(crate) fn private_peer_ids(&self) -> PeerIdSet {
        self.private_peer_sources
            .read()
            .iter()
            .filter(|(_, sources)| !sources.is_empty())
            .map(|(peer_id, _)| *peer_id)
            .collect()
    }

    pub(crate) fn has_trust_source(
        &self,
        peer_id: &PeerId,
        source_kind: GeneratedCommsTrustAuthoritySourceKind,
    ) -> bool {
        self.trusted_peer_sources
            .read()
            .get(peer_id)
            .is_some_and(|sources| sources.contains(&source_kind))
    }

    pub(crate) fn trusted_peers_for_source(
        &self,
        source_kind: GeneratedCommsTrustAuthoritySourceKind,
    ) -> Vec<(PeerId, TrustEntry)> {
        self.trusted_peer_descriptors_by_source
            .read()
            .iter()
            .filter_map(|(peer_id, descriptors)| {
                descriptors
                    .get(&source_kind)
                    .cloned()
                    .map(|peer| (*peer_id, peer))
            })
            .collect()
    }

    /// Scope in-process routing to a namespace.
    pub fn with_inproc_namespace(mut self, namespace: Option<String>) -> Self {
        self.inproc_namespace = namespace;
        self
    }

    pub fn keypair_arc(&self) -> Arc<Keypair> {
        self.keypair.clone()
    }

    pub fn trusted_peers_view(&self) -> TrustedPeersView {
        TrustedPeersView::new(self.trusted_peers.clone())
    }

    pub fn trusted_peers_snapshot(&self) -> TrustStore {
        self.trusted_peers.read().clone()
    }
    pub fn inbox_sender(&self) -> &InboxSender {
        &self.inbox_sender
    }

    pub fn has_peers(&self) -> bool {
        self.trusted_peers.read().has_peers()
    }

    /// Apply a generated trust-projection add to the router's mechanical
    /// trust store. Semantic trust authority lives at the machine seam that
    /// called into `CommsRuntime::apply_trust_mutation`.
    pub(crate) fn add_trusted_peer_for_source(
        &self,
        entry: TrustEntry,
        source_kind: GeneratedCommsTrustAuthoritySourceKind,
        private: bool,
    ) -> Result<bool, TrustError> {
        let peer_id = entry.peer_id;
        let mut trusted_peer_sources = self.trusted_peer_sources.write();
        let mut descriptors_by_source = self.trusted_peer_descriptors_by_source.write();
        let mut private_peer_sources = self.private_peer_sources.write();

        if let Some(existing) = descriptors_by_source
            .get(&peer_id)
            .and_then(|descriptors| descriptors.get(&source_kind))
        {
            let source_private = private_peer_sources
                .get(&peer_id)
                .is_some_and(|sources| sources.contains(&source_kind));
            // Trust material is the routing identity (pubkey + address);
            // `name` and discovery `meta` are display-only (a peer-only
            // member's name is derived heuristically per projection source —
            // an inproc address embeds the comms name, a non-inproc address
            // falls back to a synthetic backend-peer label — so it can differ
            // for one peer). Only a divergent pubkey or address is a genuine
            // re-add of different routing material.
            let same_routing_material =
                existing.pubkey == entry.pubkey && existing.address == entry.address;
            if same_routing_material && source_private == private {
                return Ok(false);
            }
            return Err(TrustError::ConflictingGeneratedTrustSource {
                peer_id,
                source_kind,
            });
        }
        if trusted_peer_sources
            .get(&peer_id)
            .is_some_and(|sources| sources.contains(&source_kind))
        {
            return Err(TrustError::ConflictingGeneratedTrustSource {
                peer_id,
                source_kind,
            });
        }

        self.trusted_peers.write().upsert(entry.clone())?;
        descriptors_by_source
            .entry(peer_id)
            .or_default()
            .insert(source_kind, entry);
        let created = trusted_peer_sources
            .entry(peer_id)
            .or_default()
            .insert(source_kind);
        if private {
            private_peer_sources
                .entry(peer_id)
                .or_default()
                .insert(source_kind);
        }
        Ok(created)
    }

    /// Apply a generated trust-projection removal to the router's mechanical
    /// trust store. Semantic trust authority lives at the machine seam that
    /// called into `CommsRuntime::apply_trust_mutation`.
    pub(crate) fn remove_trusted_peer_for_source(
        &self,
        peer_id: &PeerId,
        source_kind: GeneratedCommsTrustAuthoritySourceKind,
    ) -> bool {
        let source_removed = {
            let mut sources = self.trusted_peer_sources.write();
            let Some(peer_sources) = sources.get_mut(peer_id) else {
                return false;
            };
            if !peer_sources.remove(&source_kind) {
                return false;
            }
            if peer_sources.is_empty() {
                sources.remove(peer_id);
            }
            true
        };
        if !source_removed {
            return false;
        }
        let remaining_descriptor = {
            let mut descriptors_by_source = self.trusted_peer_descriptors_by_source.write();
            let Some(descriptors) = descriptors_by_source.get_mut(peer_id) else {
                return false;
            };
            descriptors.remove(&source_kind);
            let remaining = descriptors.values().next().cloned();
            if descriptors.is_empty() {
                descriptors_by_source.remove(peer_id);
            }
            remaining
        };
        {
            let mut private_sources = self.private_peer_sources.write();
            if let Some(peer_sources) = private_sources.get_mut(peer_id) {
                peer_sources.remove(&source_kind);
                if peer_sources.is_empty() {
                    private_sources.remove(peer_id);
                }
            }
        }
        if let Some(peer) = remaining_descriptor {
            // Another generated source still owns this trust row; the union
            // projection becomes that source's descriptor.
            return self.trusted_peers.write().upsert(peer).is_ok();
        }
        if self.trusted_peer_sources.read().contains_key(peer_id) {
            return true;
        }
        if self.trusted_peers.write().remove(peer_id).is_none() {
            return false;
        }
        self.private_peer_sources.write().remove(peer_id);
        true
    }

    /// Directory-private iff some generated source authorized the peer
    /// privately. Derived from the source-attributed `private_peer_sources`
    /// authority — there is no separate flat side-map to consult.
    pub(crate) fn is_private_peer_id(&self, peer_id: &PeerId) -> bool {
        self.private_peer_sources
            .read()
            .get(peer_id)
            .is_some_and(|sources| !sources.is_empty())
    }

    fn trusted_peer_by_peer_id(&self, peer_id: &PeerId) -> Option<TrustEntry> {
        self.trusted_peers.read().get(peer_id).cloned()
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn send_on_stream<S>(
        &self,
        stream: &mut S,
        envelope: Envelope,
        wait_for_ack: bool,
    ) -> Result<SendOutcome, SendError>
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
                        // A peer ACK was received and verified: this is the only
                        // path that may report `acked == true`.
                        Ok(SendOutcome {
                            envelope_id: sent_id,
                            acked: true,
                        })
                    } else {
                        Err(SendError::PeerOffline)
                    }
                }
                _ => Err(SendError::PeerOffline),
            }
        } else {
            // No ACK was awaited (Ack/Response kinds), so we cannot claim one
            // was received.
            Ok(SendOutcome {
                envelope_id: sent_id,
                acked: false,
            })
        }
    }

    /// Deliver an inproc envelope, honoring namespace isolation when it has
    /// been opted into.
    ///
    /// `CoreCommsConfig.inproc_namespace` is the namespace-isolation OPT-IN.
    /// When it is configured (`Some(ns)`) the namespace is the delivery
    /// authority: the destination is resolved exactly once, *inside* that
    /// namespace, and that resolution is the delivery target — or we fail
    /// closed with [`SendError::PeerNotFound`] rather than crossing namespaces
    /// (#242 — no cross-namespace delivery). Resolution is not re-derived from
    /// the global registry after the namespace check (a second any-namespace
    /// lookup would open a re-registration window where delivery crosses the
    /// boundary the check just enforced). When it is NOT configured (`None`)
    /// no isolation was promised, so none is imposed — the legacy
    /// any-namespace resolution stands, failing closed on ambiguity. Coercing
    /// an absent namespace to a synthetic `""` namespace here would wrongly
    /// partition default-namespace peers that legitimately share a process
    /// (e.g. an in-process mob member and its runtime bridge), so the typed
    /// `None` is respected as "unconstrained", not as a distinct namespace.
    async fn send_inproc_in_namespace(
        &self,
        peer: &TrustEntry,
        envelope: Envelope,
        dest: PeerId,
    ) -> Result<SendOutcome, SendError> {
        let registry = InprocRegistry::global();
        let envelope_id = match self.inproc_namespace.as_deref() {
            Some(namespace) => registry
                .send_to_pubkey_in_namespace_with_id_wait(
                    namespace,
                    &self.keypair,
                    &peer.pubkey,
                    envelope.id,
                    envelope.kind,
                    self.require_peer_auth,
                )
                .await
                .map_err(|err| map_inproc_send_error(err, dest))?,
            None => registry
                .send_to_pubkey_any_namespace_with_id_wait(
                    &self.keypair,
                    &peer.pubkey,
                    envelope.id,
                    envelope.kind,
                    self.require_peer_auth,
                )
                .await
                .map_err(|err| map_inproc_send_error(err, dest))?,
        };
        // Inproc delivery is a direct inbox handoff: there is no ACK round-trip,
        // so we never claim the envelope was acked.
        Ok(SendOutcome {
            envelope_id,
            acked: false,
        })
    }

    /// Canonical send: routing identity is a [`PeerId`], never a [`PeerName`].
    ///
    /// Wave-B V5: `PeerName` is display metadata; it is not safe as a routing
    /// key (duplicate names are legal). Callers that hold only a name must
    /// resolve it to a `PeerId` via [`crate::trust::TrustStore::resolve_name`]
    /// at the boundary and handle the typed
    /// [`TrustResolveError::Ambiguous`](crate::trust::TrustResolveError::Ambiguous)
    /// case explicitly — the router will not guess.
    pub async fn send(&self, dest: PeerId, kind: MessageKind) -> Result<SendOutcome, SendError> {
        self.send_with_id(dest, Uuid::new_v4(), kind).await
    }

    /// Canonical send with a caller-supplied envelope id.
    ///
    /// Used by the correlated request/response path, which reserves a
    /// stream key before the envelope goes out so replies can correlate
    /// via `in_reply_to` without an extra local id map.
    ///
    /// The returned [`SendOutcome`] carries the verified ACK truth: `acked` is
    /// `true` only when a peer ACK was received and cryptographically verified
    /// over a stream transport.
    pub async fn send_with_id(
        &self,
        dest: PeerId,
        envelope_id: Uuid,
        kind: MessageKind,
    ) -> Result<SendOutcome, SendError> {
        let Some(peer) = self.trusted_peer_by_peer_id(&dest) else {
            return Err(SendError::PeerNotFound(dest));
        };
        let addr = PeerAddr::parse(&peer.address.to_string())?;
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
                PeerAddr::Inproc(_) => self.send_inproc_in_namespace(&peer, envelope, dest).await,
            }
        }

        #[cfg(target_arch = "wasm32")]
        {
            match addr {
                PeerAddr::Tcp(_) => Err(SendError::Transport(TransportError::InvalidAddress(
                    "TCP transport is not available on wasm32".to_string(),
                ))),
                PeerAddr::Inproc(_) => self.send_inproc_in_namespace(&peer, envelope, dest).await,
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn should_wait_for_ack(kind: &MessageKind) -> bool {
    !matches!(kind, MessageKind::Ack { .. } | MessageKind::Response { .. })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::identity::PubKey;
    use crate::inbox::Inbox;
    use crate::peer_meta::PeerMeta;

    fn test_router() -> Router {
        let (_inbox, inbox_sender) = Inbox::new();
        Router::new(
            Keypair::generate(),
            CommsConfig::default(),
            inbox_sender,
            true,
        )
    }

    fn peer(name: &str, pubkey: [u8; 32], addr: &str) -> TrustEntry {
        let pubkey = PubKey::new(pubkey);
        TrustEntry {
            peer_id: pubkey.to_peer_id(),
            name: meerkat_core::comms::PeerName::new(name).expect("valid peer name"),
            pubkey,
            address: meerkat_core::comms::PeerAddress::parse(addr).expect("valid peer address"),
            meta: PeerMeta::default(),
        }
    }

    /// ROW #268 gate: `acked` reflects the verified ACK from the delivery path.
    /// A stream send that receives and verifies a peer ACK reports
    /// `acked == true`; a send that does not await an ACK reports
    /// `acked == false`. The truth is never hardcoded.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn send_outcome_acked_reflects_verified_ack() {
        use crate::transport::codec::{EnvelopeFrame, TransportCodec};
        use crate::types::MessageKind;
        use futures::{SinkExt, StreamExt};
        use tokio_util::codec::Framed;

        let router_kp = Keypair::generate();
        let router_pubkey = router_kp.public_key();
        let (_inbox, inbox_sender) = Inbox::new();
        let router = Router::new(router_kp, CommsConfig::default(), inbox_sender, true);

        let peer_kp = Keypair::generate();
        let peer_pubkey = peer_kp.public_key();

        // Outgoing Message envelope addressed to the peer, signed by the router.
        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: router_pubkey,
            to: peer_pubkey,
            kind: MessageKind::Message {
                body: "needs ack".to_string(),
                blocks: None,
                handling_mode: None,
            },
            sig: crate::identity::Signature::new([0u8; 64]),
        };
        envelope.sign(&router.keypair);
        let sent_id = envelope.id;

        let (mut client, server) = tokio::io::duplex(4096);

        // Peer side: read the framed message, then reply with a verified ACK.
        let peer_task = tokio::spawn(async move {
            let mut framed = Framed::new(server, TransportCodec::new(DEFAULT_MAX_MESSAGE_BYTES));
            let incoming = framed
                .next()
                .await
                .expect("peer receives a frame")
                .expect("frame decodes");
            let mut ack = Envelope {
                id: Uuid::new_v4(),
                from: peer_pubkey,
                to: router_pubkey,
                kind: MessageKind::Ack {
                    in_reply_to: incoming.envelope.id,
                },
                sig: crate::identity::Signature::new([0u8; 64]),
            };
            ack.sign(&peer_kp);
            framed
                .send(EnvelopeFrame::from_envelope(ack))
                .await
                .expect("peer sends ack");
        });

        let outcome = router
            .send_on_stream(&mut client, envelope, true)
            .await
            .expect("send_on_stream succeeds with a verified ack");
        peer_task.await.expect("peer task completes");

        assert_eq!(outcome.envelope_id, sent_id);
        assert!(
            outcome.acked,
            "a verified peer ACK must produce acked == true, not a hardcoded false"
        );

        // A kind that does not await an ACK (Response) must report acked=false.
        let (mut client2, _server2) = tokio::io::duplex(4096);
        let mut response_env = Envelope {
            id: Uuid::new_v4(),
            from: router_pubkey,
            to: peer_pubkey,
            kind: MessageKind::Response {
                in_reply_to: Uuid::new_v4(),
                status: crate::types::Status::Completed,
                result: serde_json::json!({"ok": true}),
                blocks: None,
                handling_mode: None,
            },
            sig: crate::identity::Signature::new([0u8; 64]),
        };
        response_env.sign(&router.keypair);
        let no_ack = router
            .send_on_stream(&mut client2, response_env, false)
            .await
            .expect("non-ack-bearing send succeeds");
        assert!(
            !no_ack.acked,
            "a send that does not await an ACK must report acked == false"
        );
    }

    /// ROW #242 gate: a trusted inproc send from a runtime in namespace A to a
    /// peer registered ONLY in namespace B must NOT be delivered — the
    /// configured `inproc_namespace` is the delivery authority, not a
    /// cross-namespace pubkey lookup.
    #[tokio::test]
    async fn inproc_send_does_not_cross_namespace() {
        use crate::classify::test_support;

        // Use the process-global registry, but isolate by unique namespaces and
        // a fresh keypair rather than `clear()` so we never disturb peers other
        // tests registered concurrently (the registry is process-global).
        let registry = InprocRegistry::global();
        let suffix = Uuid::new_v4().simple().to_string();
        let namespace_a = format!("realm-a-{suffix}");
        let namespace_b = format!("realm-b-{suffix}");

        // Receiver lives only in namespace B.
        let receiver_kp = Keypair::generate();
        let receiver_pubkey = receiver_kp.public_key();
        let receiver_pubkey_bytes = *receiver_pubkey.as_bytes();
        let (mut receiver_inbox, receiver_sender) = Inbox::new_classified(
            test_support::classification_context(TrustStore::new(), false),
        );
        registry.register_with_meta_in_namespace(
            &namespace_b,
            "receiver",
            receiver_pubkey,
            receiver_sender,
            PeerMeta::default(),
        );

        // Router is scoped to namespace "realm-a" and trusts the receiver.
        let (_inbox, inbox_sender) = Inbox::new();
        let router = Router::new(
            Keypair::generate(),
            CommsConfig::default(),
            inbox_sender,
            false,
        )
        .with_inproc_namespace(Some(namespace_a.clone()));

        let peer_id = PeerId::from_ed25519_pubkey(&receiver_pubkey_bytes);
        router
            .add_trusted_peer_for_source(
                peer("receiver", receiver_pubkey_bytes, "inproc://receiver"),
                GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection,
                false,
            )
            .expect("trust add");

        // Trusted send resolves the peer (trust is namespace-agnostic) but the
        // delivery authority is the router's namespace ("realm-a"), where the
        // receiver is NOT registered. Must fail closed, not deliver.
        let result = router
            .send(
                peer_id,
                MessageKind::Message {
                    body: "cross-namespace must not deliver".to_string(),
                    blocks: None,
                    handling_mode: None,
                },
            )
            .await;
        assert!(
            matches!(result, Err(SendError::PeerNotFound(_))),
            "send to a peer registered only in another namespace must fail closed, got {result:?}"
        );
        assert!(
            receiver_inbox.try_drain_classified().is_empty(),
            "envelope must not be delivered across namespaces"
        );

        // Clean up only our own registration; never `clear()` the shared
        // process-global registry.
        registry.unregister_in_namespace(&namespace_b, &receiver_pubkey);
    }

    /// Single-resolution invariant: when the router is namespace-scoped, the
    /// destination is resolved exactly once *inside* that namespace and that
    /// resolution is the delivery target. A peer whose pubkey is live in OUR
    /// namespace AND in a foreign namespace must receive the envelope on the
    /// in-namespace inbox (and never on the foreign one). The former
    /// precheck-then-re-resolve shape failed closed on this ambiguity and
    /// opened a re-registration window between check and delivery.
    #[tokio::test]
    async fn inproc_send_delivers_to_own_namespace_when_pubkey_live_elsewhere_too() {
        use crate::classify::test_support;

        let registry = InprocRegistry::global();
        let suffix = Uuid::new_v4().simple().to_string();
        let namespace_a = format!("realm-a-{suffix}");
        let namespace_b = format!("realm-b-{suffix}");

        // ONE keypair registered in BOTH namespaces with distinct inboxes.
        let receiver_kp = Keypair::generate();
        let receiver_pubkey = receiver_kp.public_key();
        let receiver_pubkey_bytes = *receiver_pubkey.as_bytes();
        let (mut inbox_a, sender_a) = Inbox::new_classified(test_support::classification_context(
            TrustStore::new(),
            false,
        ));
        let (mut inbox_b, sender_b) = Inbox::new_classified(test_support::classification_context(
            TrustStore::new(),
            false,
        ));
        registry.register_with_meta_in_namespace(
            &namespace_a,
            "receiver-a",
            receiver_pubkey,
            sender_a,
            PeerMeta::default(),
        );
        registry.register_with_meta_in_namespace(
            &namespace_b,
            "receiver-b",
            receiver_pubkey,
            sender_b,
            PeerMeta::default(),
        );

        let (_inbox, inbox_sender) = Inbox::new();
        let router = Router::new(
            Keypair::generate(),
            CommsConfig::default(),
            inbox_sender,
            false,
        )
        .with_inproc_namespace(Some(namespace_a.clone()));

        let peer_id = PeerId::from_ed25519_pubkey(&receiver_pubkey_bytes);
        router
            .add_trusted_peer_for_source(
                peer("receiver", receiver_pubkey_bytes, "inproc://receiver"),
                GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection,
                false,
            )
            .expect("trust add");

        let result = router
            .send(
                peer_id,
                MessageKind::Message {
                    body: "deliver in-namespace".to_string(),
                    blocks: None,
                    handling_mode: None,
                },
            )
            .await;
        assert!(
            result.is_ok(),
            "namespace-scoped send must deliver via the in-namespace resolution \
             even when the pubkey is also live elsewhere, got {result:?}"
        );
        assert!(
            !inbox_a.try_drain_classified().is_empty(),
            "envelope must arrive on the in-namespace inbox"
        );
        assert!(
            inbox_b.try_drain_classified().is_empty(),
            "envelope must never arrive on the foreign-namespace inbox"
        );

        registry.unregister_in_namespace(&namespace_a, &receiver_pubkey);
        registry.unregister_in_namespace(&namespace_b, &receiver_pubkey);
    }

    #[test]
    fn trust_re_add_ignores_display_name_but_rejects_divergent_address() {
        // TRUST-1: generated trust identity is the routing material (pubkey +
        // addr); `name` is display-only and legitimately differs across
        // projection sources for one peer (an inproc address embeds the comms
        // name; a non-inproc address falls back to a synthetic backend label).
        // A re-add under the SAME source with the same pubkey+addr but a
        // different name must be an idempotent no-op (Ok(false)); only a
        // divergent pubkey/addr is a genuine conflicting re-add.
        let router = test_router();
        let pubkey = [7u8; 32];
        let source = GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection;

        assert!(
            router
                .add_trusted_peer_for_source(
                    peer("member-inproc-name", pubkey, "tcp://10.0.0.1:7001"),
                    source,
                    false,
                )
                .expect("initial add"),
            "first add of a new peer creates the trust row"
        );

        // Same identity, DIFFERENT display name -> idempotent no-op.
        assert!(
            !router
                .add_trusted_peer_for_source(
                    peer("synthetic-backend-label", pubkey, "tcp://10.0.0.1:7001"),
                    source,
                    false,
                )
                .expect("re-add under a different name must be accepted, not rejected"),
            "a name-only divergence is not a new trust row"
        );

        // Same pubkey, DIVERGENT addr -> genuine conflict.
        let conflict = router.add_trusted_peer_for_source(
            peer("member-inproc-name", pubkey, "tcp://10.9.9.9:7001"),
            source,
            false,
        );
        assert!(
            matches!(
                conflict,
                Err(TrustError::ConflictingGeneratedTrustSource { .. })
            ),
            "a divergent routing address must conflict, got {conflict:?}"
        );
    }

    /// ROW #82 gate: directory visibility is a property of the
    /// source-attributed trust authority, not a separate flat side-map.
    /// Rebuilding a fresh Router by replaying the same generated trust-authority
    /// adds reproduces identical `comms.peers` visibility (private peers stay
    /// filtered) with no separate `private_peer_ids` population step.
    #[test]
    fn private_visibility_derives_from_source_authority_and_survives_rebuild() {
        let public_pubkey = [11u8; 32];
        let private_pubkey = [22u8; 32];
        let public_id = PeerId::from_ed25519_pubkey(&public_pubkey);
        let private_id = PeerId::from_ed25519_pubkey(&private_pubkey);
        let source = GeneratedCommsTrustAuthoritySourceKind::MeerkatMachineSupervisorPublish;

        // Replay the generated trust authority into a fresh router: one public
        // edge, one private (control-plane) edge.
        let install = |router: &Router| {
            router
                .add_trusted_peer_for_source(
                    peer("public-peer", public_pubkey, "tcp://10.0.0.1:7001"),
                    source,
                    false,
                )
                .expect("public add");
            router
                .add_trusted_peer_for_source(
                    peer("private-bridge", private_pubkey, "tcp://10.0.0.2:7001"),
                    source,
                    true,
                )
                .expect("private add");
        };

        let original = test_router();
        install(&original);
        assert!(
            !original.is_private_peer_id(&public_id),
            "a public trust edge is not directory-private"
        );
        assert!(
            original.is_private_peer_id(&private_id),
            "a privately-authorized trust edge is directory-private"
        );
        assert!(original.is_private(&PubKey::new(private_pubkey)));
        assert_eq!(
            original.private_peer_ids(),
            std::collections::BTreeSet::from([private_id]),
            "the private snapshot is derived from the source-attributed authority"
        );

        // A fresh router replaying the SAME authority reproduces identical
        // visibility — there is no separate side-map that has to be repopulated.
        let rebuilt = test_router();
        install(&rebuilt);
        assert_eq!(rebuilt.private_peer_ids(), original.private_peer_ids());
        assert!(rebuilt.is_private_peer_id(&private_id));
        assert!(!rebuilt.is_private_peer_id(&public_id));
    }

    /// ROW #82 gate: privacy is multi-source. A peer authorized privately by
    /// two sources stays directory-private until the LAST private source is
    /// removed; removing one private source does not prematurely expose it.
    #[test]
    fn private_visibility_clears_only_when_last_private_source_removed() {
        let pubkey = [33u8; 32];
        let peer_id = PeerId::from_ed25519_pubkey(&pubkey);
        let source_a = GeneratedCommsTrustAuthoritySourceKind::MeerkatMachineSupervisorPublish;
        let source_b = GeneratedCommsTrustAuthoritySourceKind::MobMachineMemberTrustWiring;

        let router = test_router();
        router
            .add_trusted_peer_for_source(
                peer("dual-private", pubkey, "tcp://10.0.0.3:7001"),
                source_a,
                true,
            )
            .expect("private add A");
        router
            .add_trusted_peer_for_source(
                peer("dual-private", pubkey, "tcp://10.0.0.3:7001"),
                source_b,
                true,
            )
            .expect("private add B");
        assert!(router.is_private_peer_id(&peer_id));

        // Removing one private source must NOT expose the peer.
        assert!(router.remove_trusted_peer_for_source(&peer_id, source_a));
        assert!(
            router.is_private_peer_id(&peer_id),
            "peer stays private while another private source owns it"
        );

        // Removing the last private source clears visibility.
        assert!(router.remove_trusted_peer_for_source(&peer_id, source_b));
        assert!(
            !router.is_private_peer_id(&peer_id),
            "the last private source removal clears directory privacy"
        );
        assert!(router.private_peer_ids().is_empty());
    }
}
