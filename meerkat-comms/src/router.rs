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
use meerkat_core::comms::{
    GeneratedCommsTrustAuthoritySourceKind, PeerId, SendTaintOverride, SenderContentTaint,
};

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
#[cfg(not(target_arch = "wasm32"))]
const DECLARED_REPLY_OPERATION_TIMEOUT: Duration = Duration::from_secs(2);

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
/// Carries the strongest delivery fact the transport can truthfully prove:
/// a verified peer ACK on stream transports, direct receiver-inbox admission
/// on the wait-based inproc lane, or only a queued transport write.
#[derive(Debug, Clone, Copy)]
pub struct SendOutcome {
    pub envelope_id: Uuid,
    pub delivery: meerkat_core::comms::PeerDeliveryOutcome,
}

/// A one-shot callback endpoint for responses to peers outside the trust
/// store.
///
/// A supervisor-bridge responder may need to deliver a typed `Rejected` reply
/// to an authenticated sender it has never trusted. When that request carries
/// an admissible callback capability, the responder stages it bound to the
/// AUTHENTICATED ingress signer key:
///
/// * `pubkey` MUST be the verified envelope signer from the ingress fact,
///   never a payload-claimed identity — the reply is addressed to the key
///   that proved itself, so a spoofed payload identity cannot redirect the
///   response to a third party's key.
/// * On the correlated signed-request path, `address` is reconstructed from
///   the kernel-observed TCP source IP plus the signed declared port. The
///   sender-selected host is never retained. Legacy uncorrelated callers may
///   use only machine-authorized stored endpoint truth, never the current
///   untrusted request payload.
///
/// Entries are consumed on first use and ONLY for [`MessageKind::Response`]
/// envelopes — this is a reply channel, not a general send capability. The
/// legacy uncorrelated path is consulted only when trust resolution misses;
/// an exact correlated route may override a stale trusted address for its one
/// matching response.
#[derive(Debug, Clone)]
pub struct DeclaredReplyEndpoint {
    /// Authenticated signer key of the request being answered.
    pub pubkey: crate::identity::PubKey,
    /// Callback-only reply address, e.g. `tcp://observed-source:port`.
    pub address: String,
}

#[cfg(not(target_arch = "wasm32"))]
fn declared_reply_timeout_error(address: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        format!(
            "declared reply endpoint operation timed out after {}s: {address}",
            DECLARED_REPLY_OPERATION_TIMEOUT.as_secs()
        ),
    )
}

#[cfg(not(target_arch = "wasm32"))]
async fn run_declared_reply_with_timeout<T, F>(address: &str, operation: F) -> Result<T, SendError>
where
    F: std::future::Future<Output = Result<T, SendError>>,
{
    tokio::time::timeout(DECLARED_REPLY_OPERATION_TIMEOUT, operation)
        .await
        .map_err(|_| SendError::Io(declared_reply_timeout_error(address)))?
}

/// Apply the callback-only deadline at the socket-routing choke point.
/// Durable trusted routes deliberately retain their transport's normal
/// lifetime; a sender-declared callback can never impose an unbounded connect,
/// write, or ACK wait on the responder.
#[cfg(not(target_arch = "wasm32"))]
async fn run_socket_route_operation<T, F>(
    address: &str,
    is_declared_reply: bool,
    operation: F,
) -> Result<T, SendError>
where
    F: std::future::Future<Output = Result<T, SendError>>,
{
    if is_declared_reply {
        run_declared_reply_with_timeout(address, operation).await
    } else {
        operation.await
    }
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
    /// Host-owned outbound content-taint declaration. Stamped onto every
    /// content-bearing envelope at the single outbound choke point
    /// ([`Router::send_with_id`]) unless a per-send override says otherwise.
    /// Host-set config, not machine state: installed via
    /// [`Router::set_outbound_content_taint`].
    outbound_content_taint: RwLock<Option<SenderContentTaint>>,
    /// Legacy uncorrelated one-shot reply endpoints for responses to peers the
    /// trust store does not know (see [`DeclaredReplyEndpoint`]). Only
    /// machine-authorized stored endpoint truth may enter this map. It grants
    /// no admission or general sendability and is consumed by the first
    /// [`MessageKind::Response`] send to that peer id.
    staged_reply_endpoints: RwLock<std::collections::BTreeMap<PeerId, DeclaredReplyEndpoint>>,
    /// Authenticated one-shot reply endpoints keyed by the exact destination
    /// identity and request correlation id. Unlike the legacy uncorrelated
    /// staging map above, an exact correlated endpoint overrides a durable
    /// trusted route for that one Response only. This lets a peer rotate its
    /// socket endpoint without granting a general routing capability or
    /// misrouting concurrent responses for the same identity.
    staged_correlated_reply_endpoints:
        RwLock<std::collections::BTreeMap<(PeerId, Uuid), DeclaredReplyEndpoint>>,
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
            outbound_content_taint: RwLock::new(None),
            staged_reply_endpoints: RwLock::new(std::collections::BTreeMap::new()),
            staged_correlated_reply_endpoints: RwLock::new(std::collections::BTreeMap::new()),
            config,
            require_peer_auth,
            inbox_sender,
            inproc_namespace: None,
        }
    }

    /// Install (or clear) the host-owned outbound content-taint declaration.
    ///
    /// `Some(taint)` stamps every subsequent content-bearing send with the
    /// declaration; `None` sends no declaration. Per-send overrides on the
    /// comms command surface take precedence via [`SendTaintOverride`].
    pub fn set_outbound_content_taint(&self, taint: Option<SenderContentTaint>) {
        *self.outbound_content_taint.write() = taint;
    }

    /// The currently-installed host-owned outbound content-taint declaration.
    pub fn outbound_content_taint(&self) -> Option<SenderContentTaint> {
        *self.outbound_content_taint.read()
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

    /// Stage a one-shot [`DeclaredReplyEndpoint`] for `dest`.
    ///
    /// Restaging the same peer id replaces the prior machine-authorized repair
    /// endpoint. Raw Request metadata and decoded payload/sender addresses
    /// must never call this legacy uncorrelated seam. Trust-store entries
    /// always win at send time; a staged entry for a peer that is (or later
    /// becomes) trusted is simply never consulted.
    pub fn stage_reply_endpoint(&self, dest: PeerId, endpoint: DeclaredReplyEndpoint) {
        self.staged_reply_endpoints.write().insert(dest, endpoint);
    }

    fn take_staged_reply_endpoint(&self, dest: &PeerId) -> Option<DeclaredReplyEndpoint> {
        self.staged_reply_endpoints.write().remove(dest)
    }

    /// Stage a one-shot endpoint for the exact `(dest, in_reply_to)` Response.
    ///
    /// Restaging the same correlation replaces only that entry. Other
    /// correlations for the same peer remain independent, which is required
    /// when requests overlap during a socket rotation.
    pub(crate) fn stage_correlated_reply_endpoint(
        &self,
        dest: PeerId,
        in_reply_to: Uuid,
        endpoint: DeclaredReplyEndpoint,
    ) {
        self.staged_correlated_reply_endpoints
            .write()
            .insert((dest, in_reply_to), endpoint);
    }

    fn take_staged_correlated_reply_endpoint(
        &self,
        dest: &PeerId,
        in_reply_to: Uuid,
    ) -> Option<DeclaredReplyEndpoint> {
        self.staged_correlated_reply_endpoints
            .write()
            .remove(&(*dest, in_reply_to))
    }

    /// Idempotently remove one exact correlated endpoint without sending.
    /// Returns whether an entry existed so focused callers/tests can observe
    /// cleanup while the cross-crate capability keeps idempotent `Ok(())`
    /// semantics.
    pub(crate) fn unstage_correlated_reply_endpoint(
        &self,
        dest: PeerId,
        in_reply_to: Uuid,
    ) -> bool {
        self.staged_correlated_reply_endpoints
            .write()
            .remove(&(dest, in_reply_to))
            .is_some()
    }

    /// Remove every staged callback for a terminal interaction correlation.
    ///
    /// The machine cleanup effect carries the correlation but not the remote
    /// destination, so cleanup must match the second key component across
    /// peers. The map is bounded by active inbound requests and this path runs
    /// only at interaction termination.
    pub(crate) fn unstage_correlated_reply_endpoints_for_interaction(
        &self,
        in_reply_to: Uuid,
    ) -> usize {
        let mut endpoints = self.staged_correlated_reply_endpoints.write();
        let before = endpoints.len();
        endpoints.retain(|(_, correlation), _| *correlation != in_reply_to);
        before - endpoints.len()
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
                        // path that may report `PeerDeliveryOutcome::Acked`.
                        Ok(SendOutcome {
                            envelope_id: sent_id,
                            delivery: meerkat_core::comms::PeerDeliveryOutcome::Acked,
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
                delivery: meerkat_core::comms::PeerDeliveryOutcome::Queued,
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
        // The wait-based inproc handoff returns only after the receiver's
        // trust-gated inbox admitted the envelope (every drop maps to a typed
        // error above). This proves direct handoff, not a cryptographic peer
        // ACK, so retain that distinction in the typed delivery outcome.
        Ok(SendOutcome {
            envelope_id,
            delivery: meerkat_core::comms::PeerDeliveryOutcome::HandedOff,
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
        self.send_with_id(dest, Uuid::new_v4(), kind, None).await
    }

    /// Canonical send with a caller-supplied envelope id.
    ///
    /// Used by the correlated request/response path, which reserves a
    /// stream key before the envelope goes out so replies can correlate
    /// via `in_reply_to` without an extra local id map.
    ///
    /// `content_taint` is the per-send tri-state taint override: `None`
    /// inherits the runtime-level declaration installed via
    /// [`Router::set_outbound_content_taint`]; `Undeclared` strips the
    /// declaration; `Declare(taint)` stamps exactly `taint`. The resolved
    /// declaration is stamped here — the single outbound choke point — onto
    /// content-bearing kinds, INSIDE the signed region, before signing.
    ///
    /// The returned [`SendOutcome`] carries the strongest typed delivery fact
    /// proved by the selected transport; only a verified stream ACK yields
    /// [`meerkat_core::comms::PeerDeliveryOutcome::Acked`].
    pub async fn send_with_id(
        &self,
        dest: PeerId,
        envelope_id: Uuid,
        kind: MessageKind,
        content_taint: Option<SendTaintOverride>,
    ) -> Result<SendOutcome, SendError> {
        // An exact correlated endpoint is the authority for precisely one
        // Response and therefore takes precedence over a potentially stale
        // durable route. For every other send, durable trust remains the
        // authority; the legacy uncorrelated endpoint is consulted only when
        // trust misses, preserving its existing trust-store-wins contract.
        enum ResolvedDest {
            Trusted(TrustEntry),
            Declared(DeclaredReplyEndpoint),
        }
        let correlated = match &kind {
            MessageKind::Response { in_reply_to, .. } => {
                self.take_staged_correlated_reply_endpoint(&dest, *in_reply_to)
            }
            _ => None,
        };
        let dest_entry = match correlated {
            Some(endpoint) => ResolvedDest::Declared(endpoint),
            None => match self.trusted_peer_by_peer_id(&dest) {
                Some(peer) => ResolvedDest::Trusted(peer),
                None => {
                    let staged = if matches!(kind, MessageKind::Response { .. }) {
                        self.take_staged_reply_endpoint(&dest)
                    } else {
                        None
                    };
                    match staged {
                        Some(endpoint) => ResolvedDest::Declared(endpoint),
                        None => return Err(SendError::PeerNotFound(dest)),
                    }
                }
            },
        };
        let (addr, to_pubkey) = match &dest_entry {
            ResolvedDest::Trusted(peer) => {
                (PeerAddr::parse(&peer.address.to_string())?, peer.pubkey)
            }
            ResolvedDest::Declared(endpoint) => {
                (PeerAddr::parse(&endpoint.address)?, endpoint.pubkey)
            }
        };
        let resolved_taint = match content_taint {
            // Absent override = inherit. A declaration the caller already
            // constructed INTO the kind wins over the runtime-level
            // declaration — the stamping choke point must never silently
            // degrade an explicit Some(..) to the unset runtime default.
            None => kind
                .content_taint()
                .or_else(|| self.outbound_content_taint()),
            Some(SendTaintOverride::Declare(taint)) => Some(taint),
            Some(SendTaintOverride::Undeclared) => None,
        };
        let kind = kind.with_content_taint(resolved_taint);
        let mut envelope = Envelope {
            id: envelope_id,
            from: self.keypair.public_key(),
            to: to_pubkey,
            kind,
            sig: crate::identity::Signature::new([0u8; 64]),
        };
        if self.require_peer_auth {
            envelope.sign(&self.keypair);
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let wait_for_ack = should_wait_for_ack(&envelope.kind);
            let is_declared_reply = matches!(&dest_entry, ResolvedDest::Declared(_));
            match addr {
                #[cfg(unix)]
                PeerAddr::Uds(path) => {
                    let display = path.display().to_string();
                    run_socket_route_operation(&display, is_declared_reply, async {
                        let mut stream = UnixStream::connect(&path).await?;
                        self.send_on_stream(&mut stream, envelope, wait_for_ack)
                            .await
                    })
                    .await
                }
                #[cfg(not(unix))]
                PeerAddr::Uds(_path) => Err(std::io::Error::new(
                    ErrorKind::Unsupported,
                    "unix domain sockets are not supported on this platform",
                )
                .into()),
                PeerAddr::Tcp(addr_str) => {
                    run_socket_route_operation(&addr_str, is_declared_reply, async {
                        let mut stream = TcpStream::connect(&addr_str).await?;
                        self.send_on_stream(&mut stream, envelope, wait_for_ack)
                            .await
                    })
                    .await
                }
                PeerAddr::Inproc(_) => match &dest_entry {
                    ResolvedDest::Trusted(peer) => {
                        self.send_inproc_in_namespace(peer, envelope, dest).await
                    }
                    ResolvedDest::Declared(_) => {
                        Err(SendError::Transport(TransportError::InvalidAddress(
                            "declared reply endpoints must be socket addresses (tcp:// or uds://)"
                                .to_string(),
                        )))
                    }
                },
            }
        }

        #[cfg(target_arch = "wasm32")]
        {
            match addr {
                PeerAddr::Tcp(_) => Err(SendError::Transport(TransportError::InvalidAddress(
                    "TCP transport is not available on wasm32".to_string(),
                ))),
                PeerAddr::Inproc(_) => match &dest_entry {
                    ResolvedDest::Trusted(peer) => {
                        self.send_inproc_in_namespace(peer, envelope, dest).await
                    }
                    ResolvedDest::Declared(_) => {
                        Err(SendError::Transport(TransportError::InvalidAddress(
                            "declared reply endpoints must be socket addresses (tcp:// or uds://)"
                                .to_string(),
                        )))
                    }
                },
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

    /// ROW #268 gate: the outcome reflects verified delivery-path authority.
    /// A stream send that receives and verifies a peer ACK reports
    /// [`PeerDeliveryOutcome::Acked`]; sends without receiver acknowledgement
    /// report the weaker typed `Queued` or `HandedOff` outcome.
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
                objective_id: None,
                body: "needs ack".to_string(),
                blocks: None,
                content_taint: None,
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
        assert_eq!(
            outcome.delivery,
            meerkat_core::comms::PeerDeliveryOutcome::Acked
        );

        // A kind that does not await an ACK (Response) proves only a write.
        let (mut client2, _server2) = tokio::io::duplex(4096);
        let mut response_env = Envelope {
            id: Uuid::new_v4(),
            from: router_pubkey,
            to: peer_pubkey,
            kind: MessageKind::Response {
                objective_id: None,
                in_reply_to: Uuid::new_v4(),
                status: crate::types::Status::Completed,
                result: serde_json::json!({"ok": true}),
                blocks: None,
                content_taint: None,
                handling_mode: None,
            },
            sig: crate::identity::Signature::new([0u8; 64]),
        };
        response_env.sign(&router.keypair);
        let no_ack = router
            .send_on_stream(&mut client2, response_env, false)
            .await
            .expect("non-ack-bearing send succeeds");
        assert_eq!(
            no_ack.delivery,
            meerkat_core::comms::PeerDeliveryOutcome::Queued
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
                    objective_id: None,
                    body: "cross-namespace must not deliver".to_string(),
                    blocks: None,
                    content_taint: None,
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
                    objective_id: None,
                    body: "deliver in-namespace".to_string(),
                    blocks: None,
                    content_taint: None,
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

    /// Ask 5 gate: the router is the single outbound taint-stamping choke
    /// point. The runtime-level declaration is inherited when the per-send
    /// override is absent; `Undeclared` strips it; `Declare(taint)` sets it.
    #[tokio::test]
    async fn send_with_id_stamps_outbound_content_taint_declaration() {
        use crate::classify::test_support;
        use crate::inbox::Inbox;
        use crate::types::{InboxItem, MessageKind};
        use meerkat_core::comms::SenderContentTaint;

        let registry = InprocRegistry::global();
        let suffix = Uuid::new_v4().simple().to_string();
        let namespace = format!("taint-{suffix}");

        let receiver_kp = Keypair::generate();
        let receiver_pubkey = receiver_kp.public_key();
        let receiver_pubkey_bytes = *receiver_pubkey.as_bytes();
        let (mut receiver_inbox, receiver_sender) = Inbox::new_classified(
            test_support::classification_context(TrustStore::new(), false),
        );
        registry.register_with_meta_in_namespace(
            &namespace,
            "taint-receiver",
            receiver_pubkey,
            receiver_sender,
            PeerMeta::default(),
        );

        let (_inbox, inbox_sender) = Inbox::new();
        let router = Router::new(
            Keypair::generate(),
            CommsConfig::default(),
            inbox_sender,
            false,
        )
        .with_inproc_namespace(Some(namespace.clone()));
        let peer_id = PeerId::from_ed25519_pubkey(&receiver_pubkey_bytes);
        router
            .add_trusted_peer_for_source(
                peer(
                    "taint-receiver",
                    receiver_pubkey_bytes,
                    "inproc://taint-receiver",
                ),
                GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection,
                false,
            )
            .expect("trust add");

        let message = |body: &str| MessageKind::Message {
            objective_id: None,
            body: body.to_string(),
            blocks: None,
            content_taint: None,
            handling_mode: None,
        };
        let mut received_taint = |expected_body: &str| {
            let entries = receiver_inbox.try_drain_classified();
            assert_eq!(entries.len(), 1, "expected exactly one delivered envelope");
            let InboxItem::External { envelope } = &entries[0].item else {
                panic!("expected external envelope");
            };
            let MessageKind::Message { body, .. } = &envelope.kind else {
                panic!("expected message kind");
            };
            assert_eq!(body, expected_body);
            envelope.kind.content_taint()
        };

        // No runtime declaration + absent override => no declaration.
        router
            .send_with_id(peer_id, Uuid::new_v4(), message("undeclared"), None)
            .await
            .expect("send");
        assert_eq!(received_taint("undeclared"), None);

        // Runtime declaration + absent override => inherit.
        router.set_outbound_content_taint(Some(SenderContentTaint::Tainted));
        router
            .send_with_id(peer_id, Uuid::new_v4(), message("inherited"), None)
            .await
            .expect("send");
        assert_eq!(
            received_taint("inherited"),
            Some(SenderContentTaint::Tainted)
        );

        // Runtime declaration + Undeclared override => strip.
        router
            .send_with_id(
                peer_id,
                Uuid::new_v4(),
                message("stripped"),
                Some(SendTaintOverride::Undeclared),
            )
            .await
            .expect("send");
        assert_eq!(received_taint("stripped"), None);

        // Declare override wins over the runtime declaration.
        router
            .send_with_id(
                peer_id,
                Uuid::new_v4(),
                message("declared"),
                Some(SendTaintOverride::Declare(SenderContentTaint::Clean)),
            )
            .await
            .expect("send");
        assert_eq!(received_taint("declared"), Some(SenderContentTaint::Clean));

        registry.unregister_in_namespace(&namespace, &receiver_pubkey);
    }

    /// One connection's framed envelope read off a raw loopback listener —
    /// the receiver side for declared-reply-endpoint delivery tests.
    #[cfg(not(target_arch = "wasm32"))]
    async fn accept_one_envelope(
        listener: tokio::net::TcpListener,
    ) -> tokio::task::JoinHandle<Envelope> {
        use crate::transport::codec::TransportCodec;
        use futures::StreamExt;
        use tokio_util::codec::Framed;
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept reply connection");
            let mut framed = Framed::new(stream, TransportCodec::new(DEFAULT_MAX_MESSAGE_BYTES));
            framed
                .next()
                .await
                .expect("reply frame arrives")
                .expect("reply frame decodes")
                .envelope
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn accept_envelopes(
        listener: tokio::net::TcpListener,
        count: usize,
    ) -> tokio::task::JoinHandle<Vec<Envelope>> {
        use crate::transport::codec::TransportCodec;
        use futures::StreamExt;
        tokio::spawn(async move {
            let mut envelopes = Vec::with_capacity(count);
            for _ in 0..count {
                let (stream, _) = listener.accept().await.expect("accept reply connection");
                let mut framed =
                    Framed::new(stream, TransportCodec::new(DEFAULT_MAX_MESSAGE_BYTES));
                envelopes.push(
                    framed
                        .next()
                        .await
                        .expect("reply frame arrives")
                        .expect("reply frame decodes")
                        .envelope,
                );
            }
            envelopes
        })
    }

    fn response_kind(marker: &str) -> MessageKind {
        response_kind_for(Uuid::new_v4(), marker)
    }

    fn response_kind_for(in_reply_to: Uuid, marker: &str) -> MessageKind {
        MessageKind::Response {
            in_reply_to,
            status: crate::types::Status::Failed,
            result: serde_json::json!({ "marker": marker }),
            blocks: None,
            content_taint: None,
            handling_mode: None,
            objective_id: None,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test(start_paused = true)]
    async fn declared_reply_operation_is_bounded() {
        let error = run_declared_reply_with_timeout(
            "tcp://192.0.2.1:4311",
            std::future::pending::<Result<(), SendError>>(),
        )
        .await
        .expect_err("pending declared-route operation must time out");
        assert!(matches!(
            error,
            SendError::Io(error) if error.kind() == std::io::ErrorKind::TimedOut
        ));
    }

    /// The production socket-routing wrapper applies its deadline only to a
    /// declared callback. Advancing beyond that budget must not complete a
    /// pending operation selected as durable trusted routing.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test(start_paused = true)]
    async fn trusted_socket_route_operation_has_no_declared_reply_deadline() {
        let mut operation = Box::pin(run_socket_route_operation(
            "tcp://trusted-peer:4311",
            false,
            std::future::pending::<Result<(), SendError>>(),
        ));
        assert!(
            futures::poll!(operation.as_mut()).is_pending(),
            "trusted operation starts pending"
        );

        tokio::time::advance(DECLARED_REPLY_OPERATION_TIMEOUT + Duration::from_millis(1)).await;
        assert!(
            futures::poll!(operation.as_mut()).is_pending(),
            "trusted routes must not inherit the untrusted callback deadline"
        );
    }

    /// The declared-route deadline covers the frame write too, not just the
    /// socket connect. Use a deliberately tiny in-memory stream so the test
    /// does not depend on platform TCP autotuning or kernel buffer sizes.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test(start_paused = true)]
    async fn declared_socket_reply_frame_write_is_bounded() {
        let router = test_router();
        let recipient = Keypair::generate().public_key();
        let correlation = Uuid::new_v4();
        let (mut sender, _nonreading_receiver) = tokio::io::duplex(1024);
        let marker = "x".repeat(900 * 1024);
        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: router.keypair.public_key(),
            to: recipient,
            kind: response_kind_for(correlation, &marker),
            sig: crate::identity::Signature::new([0u8; 64]),
        };
        envelope.sign(&router.keypair);

        let error = run_socket_route_operation("tcp://declared-callback", true, async {
            router.send_on_stream(&mut sender, envelope, false).await
        })
        .await
        .expect_err("non-reading declared callback must time out");
        assert!(matches!(
            error,
            SendError::Io(error) if error.kind() == std::io::ErrorKind::TimedOut
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn declared_uds_reply_frame_write_is_bounded() {
        let router = test_router();
        let recipient = Keypair::generate().public_key();
        let dest = recipient.to_peer_id();
        let correlation = Uuid::new_v4();
        // macOS sockaddr_un paths are limited to 103 bytes and its per-user
        // TMPDIR is already long. Keep this fixture path short enough that the
        // test reaches the write-deadline behavior it is meant to exercise.
        let socket_path = std::path::PathBuf::from("/tmp")
            .join(format!("rkat-dr-{}.sock", Uuid::new_v4().simple()));
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind UDS callback");
        router.stage_correlated_reply_endpoint(
            dest,
            correlation,
            DeclaredReplyEndpoint {
                pubkey: recipient,
                address: format!("uds://{}", socket_path.display()),
            },
        );
        let trap = tokio::spawn(async move {
            let (_nonreading_receiver, _) = listener.accept().await.expect("accept UDS callback");
            std::future::pending::<()>().await;
        });

        let marker = "x".repeat(900 * 1024);
        let error = router
            .send_with_id(
                dest,
                Uuid::new_v4(),
                response_kind_for(correlation, &marker),
                None,
            )
            .await
            .expect_err("non-reading correlated UDS callback must time out");
        trap.abort();
        let _ = std::fs::remove_file(&socket_path);
        assert!(matches!(
            error,
            SendError::Io(error) if error.kind() == std::io::ErrorKind::TimedOut
        ));
    }

    /// A staged [`DeclaredReplyEndpoint`] delivers a `Response` to a peer the
    /// trust store does not know, addressed to the staged (authenticated)
    /// key — and is consumed by that one send.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn staged_reply_endpoint_delivers_response_once_to_untrusted_peer() {
        let router = test_router();
        let sender_kp = Keypair::generate();
        let sender_pubkey = sender_kp.public_key();
        let dest = sender_pubkey.to_peer_id();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind reply listener");
        let address = format!("tcp://{}", listener.local_addr().expect("listener addr"));
        let received = accept_one_envelope(listener).await;

        router.stage_reply_endpoint(
            dest,
            DeclaredReplyEndpoint {
                pubkey: sender_pubkey,
                address,
            },
        );
        let outcome = router
            .send_with_id(dest, Uuid::new_v4(), response_kind("reject"), None)
            .await
            .expect("staged endpoint must deliver the response");
        assert_eq!(
            outcome.delivery,
            meerkat_core::comms::PeerDeliveryOutcome::Queued,
            "Response sends never wait for an ack"
        );

        let envelope = received.await.expect("receiver task");
        assert_eq!(
            envelope.to, sender_pubkey,
            "the reply is addressed to the staged authenticated key"
        );
        assert!(
            matches!(envelope.kind, MessageKind::Response { .. }),
            "staged delivery carries the response kind"
        );

        // One-shot: the same destination misses the trust store again AND the
        // staged entry is gone.
        let second = router
            .send_with_id(dest, Uuid::new_v4(), response_kind("replay"), None)
            .await;
        assert!(
            matches!(second, Err(SendError::PeerNotFound(_))),
            "a consumed staged endpoint must not serve a second send: {second:?}"
        );
    }

    /// Staged endpoints are a REPLY channel: non-`Response` kinds never
    /// consult them (and must not consume them).
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn staged_reply_endpoint_never_serves_non_response_kinds() {
        let router = test_router();
        let sender_kp = Keypair::generate();
        let sender_pubkey = sender_kp.public_key();
        let dest = sender_pubkey.to_peer_id();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind reply listener");
        let address = format!("tcp://{}", listener.local_addr().expect("listener addr"));
        let received = accept_one_envelope(listener).await;

        router.stage_reply_endpoint(
            dest,
            DeclaredReplyEndpoint {
                pubkey: sender_pubkey,
                address,
            },
        );
        let message = router
            .send_with_id(
                dest,
                Uuid::new_v4(),
                MessageKind::Message {
                    body: "not a reply".to_string(),
                    blocks: None,
                    content_taint: None,
                    handling_mode: None,
                    objective_id: None,
                },
                None,
            )
            .await;
        assert!(
            matches!(message, Err(SendError::PeerNotFound(_))),
            "a staged endpoint must not grant general sendability: {message:?}"
        );

        // The refused Message send did not consume the entry: the Response
        // still delivers.
        router
            .send_with_id(dest, Uuid::new_v4(), response_kind("still-staged"), None)
            .await
            .expect("the staged endpoint survives non-response sends");
        let envelope = received.await.expect("receiver task");
        assert_eq!(envelope.to, sender_pubkey);
    }

    /// Trust-store entries are the send authority: when the destination is
    /// trusted, the response dials the TRUSTED address and the staged entry
    /// is never consulted.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn trust_store_wins_over_staged_reply_endpoint() {
        let router = test_router();
        let sender_kp = Keypair::generate();
        let sender_pubkey = sender_kp.public_key();
        let dest = sender_pubkey.to_peer_id();

        let trusted_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind trusted listener");
        let trusted_address = format!("tcp://{}", trusted_listener.local_addr().expect("addr"));
        let received = accept_one_envelope(trusted_listener).await;

        let staged_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind staged listener");
        let staged_address = format!("tcp://{}", staged_listener.local_addr().expect("addr"));

        router
            .add_trusted_peer_for_source(
                peer("trusted-sender", sender_pubkey.0, &trusted_address),
                GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection,
                false,
            )
            .expect("install trusted entry");
        router.stage_reply_endpoint(
            dest,
            DeclaredReplyEndpoint {
                pubkey: sender_pubkey,
                address: staged_address,
            },
        );

        router
            .send_with_id(dest, Uuid::new_v4(), response_kind("via-trust"), None)
            .await
            .expect("trusted response send");
        let envelope = received.await.expect("trusted receiver task");
        assert_eq!(
            envelope.to, sender_pubkey,
            "the trusted route serves the response; the staged endpoint is not consulted"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn correlated_reply_endpoints_are_exact_concurrent_and_one_shot() {
        let router = test_router();
        let sender_kp = Keypair::generate();
        let sender_pubkey = sender_kp.public_key();
        let dest = sender_pubkey.to_peer_id();
        let correlation_a = Uuid::new_v4();
        let correlation_b = Uuid::new_v4();

        let listener_a = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind reply listener A");
        let address_a = format!(
            "tcp://{}",
            listener_a.local_addr().expect("listener A addr")
        );
        let received_a = accept_one_envelope(listener_a).await;
        let listener_b = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind reply listener B");
        let address_b = format!(
            "tcp://{}",
            listener_b.local_addr().expect("listener B addr")
        );
        let received_b = accept_one_envelope(listener_b).await;

        router.stage_correlated_reply_endpoint(
            dest,
            correlation_a,
            DeclaredReplyEndpoint {
                pubkey: sender_pubkey,
                address: address_a,
            },
        );
        router.stage_correlated_reply_endpoint(
            dest,
            correlation_b,
            DeclaredReplyEndpoint {
                pubkey: sender_pubkey,
                address: address_b,
            },
        );

        // A non-Response cannot consume either entry.
        let non_response = router
            .send_with_id(
                dest,
                Uuid::new_v4(),
                MessageKind::Message {
                    body: "not a response".to_string(),
                    blocks: None,
                    content_taint: None,
                    handling_mode: None,
                    objective_id: None,
                },
                None,
            )
            .await;
        assert!(matches!(non_response, Err(SendError::PeerNotFound(_))));

        // An unmatched correlation cannot consume either exact entry.
        let unmatched = router
            .send_with_id(
                dest,
                Uuid::new_v4(),
                response_kind_for(Uuid::new_v4(), "unmatched"),
                None,
            )
            .await;
        assert!(matches!(unmatched, Err(SendError::PeerNotFound(_))));

        let (sent_a, sent_b) = tokio::join!(
            router.send_with_id(
                dest,
                Uuid::new_v4(),
                response_kind_for(correlation_a, "a"),
                None,
            ),
            router.send_with_id(
                dest,
                Uuid::new_v4(),
                response_kind_for(correlation_b, "b"),
                None,
            )
        );
        sent_a.expect("correlation A must route independently");
        sent_b.expect("correlation B must route independently");
        let envelope_a = received_a.await.expect("receiver A task");
        let envelope_b = received_b.await.expect("receiver B task");
        assert!(matches!(
            envelope_a.kind,
            MessageKind::Response { in_reply_to, .. } if in_reply_to == correlation_a
        ));
        assert!(matches!(
            envelope_b.kind,
            MessageKind::Response { in_reply_to, .. } if in_reply_to == correlation_b
        ));

        for correlation in [correlation_a, correlation_b] {
            let replay = router
                .send_with_id(
                    dest,
                    Uuid::new_v4(),
                    response_kind_for(correlation, "replay"),
                    None,
                )
                .await;
            assert!(
                matches!(replay, Err(SendError::PeerNotFound(_))),
                "correlated endpoint must be one-shot: {replay:?}"
            );
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn exact_correlated_reply_overrides_trusted_route_only_for_its_response() {
        let router = test_router();
        let sender_kp = Keypair::generate();
        let sender_pubkey = sender_kp.public_key();
        let dest = sender_pubkey.to_peer_id();
        let correlation = Uuid::new_v4();
        let other_correlation = Uuid::new_v4();

        let trusted_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind stale trusted listener");
        let trusted_address = format!(
            "tcp://{}",
            trusted_listener
                .local_addr()
                .expect("trusted listener addr")
        );
        let trusted_received = accept_envelopes(trusted_listener, 2).await;
        router
            .add_trusted_peer_for_source(
                peer("rotating-peer", sender_pubkey.0, &trusted_address),
                GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection,
                false,
            )
            .expect("install durable route");

        let correlated_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind correlated listener");
        let correlated_address = format!(
            "tcp://{}",
            correlated_listener
                .local_addr()
                .expect("correlated listener addr")
        );
        let correlated_received = accept_one_envelope(correlated_listener).await;
        router.stage_correlated_reply_endpoint(
            dest,
            correlation,
            DeclaredReplyEndpoint {
                pubkey: sender_pubkey,
                address: correlated_address,
            },
        );

        router
            .send_with_id(
                dest,
                Uuid::new_v4(),
                response_kind_for(correlation, "exact"),
                None,
            )
            .await
            .expect("exact correlation must bypass stale durable route");
        let exact = correlated_received.await.expect("correlated receiver task");
        assert!(matches!(
            exact.kind,
            MessageKind::Response { in_reply_to, .. } if in_reply_to == correlation
        ));

        router
            .send_with_id(
                dest,
                Uuid::new_v4(),
                response_kind_for(other_correlation, "durable"),
                None,
            )
            .await
            .expect("other correlation must retain durable trust routing");
        router
            .send_with_id(
                dest,
                Uuid::new_v4(),
                response_kind_for(correlation, "exact-replay"),
                None,
            )
            .await
            .expect("consumed exact endpoint must fall back to durable trust");
        let durable = trusted_received.await.expect("trusted receiver task");
        assert_eq!(durable.len(), 2);
        assert!(matches!(
            &durable[0].kind,
            MessageKind::Response { in_reply_to, .. } if *in_reply_to == other_correlation
        ));
        assert!(matches!(
            &durable[1].kind,
            MessageKind::Response { in_reply_to, .. } if *in_reply_to == correlation
        ));
    }

    #[test]
    fn correlated_reply_endpoint_cleanup_is_idempotent_and_exact() {
        let router = test_router();
        let sender_pubkey = Keypair::generate().public_key();
        let dest = sender_pubkey.to_peer_id();
        let correlation = Uuid::new_v4();
        router.stage_correlated_reply_endpoint(
            dest,
            correlation,
            DeclaredReplyEndpoint {
                pubkey: sender_pubkey,
                address: "tcp://127.0.0.1:4311".to_string(),
            },
        );
        assert!(!router.unstage_correlated_reply_endpoint(dest, Uuid::new_v4()));
        assert!(router.unstage_correlated_reply_endpoint(dest, correlation));
        assert!(!router.unstage_correlated_reply_endpoint(dest, correlation));
    }
}
