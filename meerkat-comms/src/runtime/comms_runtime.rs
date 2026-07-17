//! CommsRuntime - Full lifecycle manager for agent-to-agent communication.

#[cfg(not(target_arch = "wasm32"))]
use super::comms_config::ResolvedCommsConfig;
#[cfg(not(target_arch = "wasm32"))]
use crate::InboxSender;
use crate::agent::types::CommsMessage;
#[cfg(not(target_arch = "wasm32"))]
use crate::handle_connection;
#[cfg(not(target_arch = "wasm32"))]
use crate::io_task::handle_tcp_connection;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use crate::trust::{TrustEntry, TrustStore};
use crate::{InprocRegistry, Keypair, PubKey, Router, TrustedPeersView};
use async_trait::async_trait;
#[cfg(not(target_arch = "wasm32"))]
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use futures::Stream;
use futures::task::{Context, Poll};
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::comms::{
    CommsCommand, CommsTrustMutation, CommsTrustMutationResult, EventStream,
    GeneratedCommsTrustAuthoritySourceKind, InputStreamMode, PeerCapabilitySet, PeerDirectoryEntry,
    PeerDirectorySource, PeerId, PeerName, PeerRoute, PeerSendability, SendAndStreamError,
    SendError, SendReceipt, StreamError, StreamScope, TrustedPeerDescriptor,
};
use meerkat_core::config::PlainEventSource;
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::handles::{SessionClaim, SessionClaimError, SessionClaimHandle};
use meerkat_core::hydrate_content_blocks;
use meerkat_core::time_compat::Instant;
use meerkat_core::{BlobStore, MissingBlobBehavior, SessionId};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use thiserror::Error;
#[cfg(not(target_arch = "wasm32"))]
use tokio::net::TcpListener;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;
#[cfg(not(target_arch = "wasm32"))]
use tokio::task::JoinHandle;
#[cfg(not(target_arch = "wasm32"))]
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
};
use uuid::Uuid;

#[cfg(all(test, not(target_arch = "wasm32")))]
struct ListenerStartFault {
    participant_name: String,
    observed_tcp_address: Option<std::net::SocketAddr>,
    armed: bool,
}

/// Deterministic test-only failure after the signed TCP listener has bound but
/// before any staged handle is committed to the runtime.
#[cfg(all(test, not(target_arch = "wasm32")))]
static LISTENER_START_FAULT_AFTER_SIGNED_TCP: std::sync::Mutex<Option<ListenerStartFault>> =
    std::sync::Mutex::new(None);

#[cfg(all(test, not(target_arch = "wasm32")))]
struct ListenerStartPause {
    participant_name: String,
    reached: Option<tokio::sync::oneshot::Sender<std::net::SocketAddr>>,
    release: Option<tokio::sync::oneshot::Receiver<()>>,
}

/// Deterministic test-only pause after the signed TCP listener has bound and
/// its handle has entered the cancellation-safe staging guard.
#[cfg(all(test, not(target_arch = "wasm32")))]
static LISTENER_START_PAUSE_AFTER_SIGNED_TCP: std::sync::Mutex<Option<ListenerStartPause>> =
    std::sync::Mutex::new(None);

/// Default reservation TTL (time from `Reserved` to `Expired` if not attached).
const RESERVATION_TTL: meerkat_core::time_compat::Duration =
    meerkat_core::time_compat::Duration::from_secs(30);
const ABANDONMENT_PROJECTION_CAPACITY: usize = 4096;

type InteractionStreamAbandonmentSignal =
    Arc<Mutex<Option<meerkat_core::InteractionStreamAbandonReason>>>;

#[derive(Clone, Debug)]
struct InteractionStreamAbandonmentProjection {
    reason: meerkat_core::InteractionStreamAbandonReason,
    recorded_at: Instant,
}

type InteractionStreamAbandonmentRegistry =
    Arc<Mutex<HashMap<Uuid, InteractionStreamAbandonmentProjection>>>;

/// Channels-only projection of an interaction stream.
///
/// Streams store `Machine` whenever a MeerkatMachine interaction-stream handle
/// is installed, keeping lifecycle truth in the generated authority. The
/// `LocalTransportOnly` boundary exists only for standalone comms runtimes
/// without semantic session authority.
#[derive(Debug)]
struct StreamRegistryEntry {
    _sender: mpsc::Sender<meerkat_core::AgentEvent>,
    receiver: Option<Receiver<meerkat_core::AgentEvent>>,
    created_at: Instant,
    lifecycle_authority: InteractionStreamLifecycleAuthority,
    abandonment_signal: InteractionStreamAbandonmentSignal,
}

impl StreamRegistryEntry {
    fn new(
        sender: mpsc::Sender<meerkat_core::AgentEvent>,
        receiver: Receiver<meerkat_core::AgentEvent>,
        lifecycle_authority: InteractionStreamLifecycleAuthority,
    ) -> Self {
        Self {
            _sender: sender,
            receiver: Some(receiver),
            created_at: Instant::now(),
            lifecycle_authority,
            abandonment_signal: Arc::new(Mutex::new(None)),
        }
    }
}

type InteractionStreamRegistry = Arc<Mutex<HashMap<Uuid, StreamRegistryEntry>>>;
type PeerRequestResponseAuthorityHandles = (
    Arc<dyn meerkat_core::handles::PeerInteractionHandle>,
    Arc<dyn meerkat_core::handles::InteractionStreamHandle>,
);

fn interaction_stream_abandon_reason_for_send_error(
    error: &SendError,
) -> meerkat_core::InteractionStreamAbandonReason {
    match error {
        SendError::AdmissionDropped { .. } => {
            meerkat_core::InteractionStreamAbandonReason::AdmissionRejected
        }
        _ => meerkat_core::InteractionStreamAbandonReason::SendFailed,
    }
}

#[derive(Clone)]
pub struct PeerRequestResponseAuthority {
    peer_interaction: Arc<dyn meerkat_core::handles::PeerInteractionHandle>,
    interaction_stream: Arc<dyn meerkat_core::handles::InteractionStreamHandle>,
}

impl PeerRequestResponseAuthority {
    pub fn new(
        peer_interaction: Arc<dyn meerkat_core::handles::PeerInteractionHandle>,
        interaction_stream: Arc<dyn meerkat_core::handles::InteractionStreamHandle>,
    ) -> Self {
        Self {
            peer_interaction,
            interaction_stream,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InteractionStreamLifecycleAuthority {
    Machine,
    LocalTransportOnly,
}

enum InteractionStreamReservationAuthority {
    Machine(Arc<dyn meerkat_core::handles::InteractionStreamHandle>),
    LocalTransportOnly,
}

struct InteractionStream {
    id: Uuid,
    receiver: Option<Receiver<meerkat_core::AgentEvent>>,
    /// Shared stream-lifecycle DSL handle. `None` is valid only for
    /// transport-only local input streams; semantic peer request/response
    /// reservations require machine authority before they can be created.
    stream_handle: Option<Arc<dyn meerkat_core::handles::InteractionStreamHandle>>,
    /// Channels-only projection registry. With a handle installed, the cleanup
    /// observer drops the entry under the same authority lock as the DSL
    /// transition. Without a handle, the stream is a local transport-only
    /// channel and owns direct cleanup.
    registry: InteractionStreamRegistry,
    abandonment_signal: InteractionStreamAbandonmentSignal,
    abandonment_registry: InteractionStreamAbandonmentRegistry,
    source: meerkat_core::EventSourceIdentity,
    seq: u64,
}

struct ResolvedPeer {
    name: PeerName,
    peer_id: meerkat_core::comms::PeerId,
    address: meerkat_core::comms::PeerAddress,
    source: PeerDirectorySource,
    meta: crate::PeerMeta,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) const PAIRING_VERSION: u32 = 1;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const PAIRING_CHALLENGE_KIND: &str = "meerkat_pairing_challenge";
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const PAIRING_COMPLETE_KIND: &str = "meerkat_pairing_complete";
#[cfg(not(target_arch = "wasm32"))]
const PAIRING_CLASSIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// Maximum newline-delimited pairing frame size, including the trailing
/// newline. Pairing is pre-authentication input, so the reader must enforce
/// this bound before growing a caller-visible buffer.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const MAX_PAIRING_MESSAGE_BYTES: usize = 16 * 1024;

/// Typed identity-kind for a pairing caller.
///
/// Deserialization fails closed on any kind other than the single supported
/// `ed25519_public_key`, replacing the previous free-string `kind` field +
/// `== "ed25519_public_key"` equality check.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
pub(crate) enum PairingIdentityKind {
    #[serde(rename = "ed25519_public_key")]
    Ed25519PublicKey,
}

/// Typed pairing public key.
///
/// Deserializes once from the wire `"ed25519:<base64>"` string and validates
/// it into a [`PubKey`], failing closed on a malformed key. The original
/// canonical string is retained because the password proof is computed over
/// the exact wire form supplied by the caller.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
pub(crate) struct PairingPublicKey {
    pub(crate) raw: String,
    pub(crate) key: PubKey,
}

#[cfg(not(target_arch = "wasm32"))]
impl<'de> serde::Deserialize<'de> for PairingPublicKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        let key = PubKey::from_pubkey_string(&raw).map_err(serde::de::Error::custom)?;
        Ok(Self { raw, key })
    }
}

/// Typed pairing peer address newtype (transparent over the wire string).
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(transparent)]
pub(crate) struct PairingAddress(pub(crate) String);

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, serde::Deserialize)]
pub(crate) struct PairingPeerIdentity {
    pub(crate) kind: PairingIdentityKind,
    pub(crate) public_key: PairingPublicKey,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, serde::Deserialize)]
pub(crate) struct PairingPeer {
    pub(crate) name: String,
    pub(crate) address: PairingAddress,
    pub(crate) identity: PairingPeerIdentity,
}

/// Typed pairing-client wire messages, parsed once at ingress.
///
/// Deserialization fails closed on an unknown `kind`, a missing `version`, a
/// missing `password_proof`, or a malformed `caller` — replacing the previous
/// field-by-field raw-JSON `pairing_field` reads.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "kind")]
pub(crate) enum PairingClientMessage {
    #[serde(rename = "meerkat_pairing_hello")]
    Hello { version: u32 },
    #[serde(rename = "meerkat_pairing_proof")]
    Proof {
        caller: PairingPeer,
        password_proof: String,
    },
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn comms_pairing_password_proof(
    password: &str,
    challenge: &str,
    caller_public_key: &str,
    caller_address: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"meerkat-comms-pairing-v1");
    hasher.update([0]);
    hasher.update(password.as_bytes());
    hasher.update([0]);
    hasher.update(challenge.as_bytes());
    hasher.update([0]);
    hasher.update(caller_public_key.as_bytes());
    hasher.update([0]);
    hasher.update(caller_address.as_bytes());
    BASE64_STANDARD.encode(hasher.finalize())
}

/// Length-independent constant-time string comparison for bearer secrets
/// (pairing passwords, one-time bootstrap tokens). Public so every secret
/// gate in the workspace shares ONE comparison discipline instead of
/// re-deriving it (a plain `==` short-circuits per byte — a timing oracle
/// on long-lived listeners).
#[cfg(not(target_arch = "wasm32"))]
pub fn constant_time_str_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let mut diff = left.len() ^ right.len();
    let max = left.len().max(right.len());
    for idx in 0..max {
        let left_byte = left.get(idx).copied().unwrap_or_default();
        let right_byte = right.get(idx).copied().unwrap_or_default();
        diff |= usize::from(left_byte ^ right_byte);
    }
    diff == 0
}

/// Validate the minimum entropy-bearing length required by every comms pairing
/// entry point. Callers should run this before any externally visible startup
/// effects; listeners retain the same check as defense in depth.
#[cfg(not(target_arch = "wasm32"))]
pub fn validate_pairing_secret(secret: &str) -> Result<(), std::io::Error> {
    if secret.len() < 32 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "comms pairing secret must be at least 32 bytes; use a random one-time bootstrap token",
        ));
    }
    Ok(())
}

/// Derive a stable [`meerkat_core::comms::PeerId`] from a signing pubkey.
///
/// Wave-B V5 dogma: `PeerId` is the runtime routing identity and distinct
/// from the cryptographic pubkey. Until a runtime-managed UUID directory is
/// wired (Wave-C), we derive the UUID deterministically from the pubkey
/// bytes via UUID v5 so every surface that resolves the same peer sees the
/// same `PeerId`, and callers can go round-trip pubkey → PeerId → pubkey by
/// looking up in the trust store.
///
/// Delegates to the router's [`crate::router::peer_id_from_pubkey`] so both
/// the router and the runtime agree on the UUIDv5 derivation — trust
/// lookups by `PeerId` stay stable round-trip.
fn peer_id_from_pubkey(pubkey: &crate::identity::PubKey) -> meerkat_core::comms::PeerId {
    crate::router::peer_id_from_pubkey(pubkey)
}

// Only the native TCP listener path and test helpers parse raw addresses;
// the wasm build has no listener surface.
#[cfg(any(not(target_arch = "wasm32"), test))]
pub(crate) fn parse_peer_address(raw: &str) -> Result<meerkat_core::comms::PeerAddress, String> {
    meerkat_core::comms::PeerAddress::parse(raw).map_err(|err| err.to_string())
}

/// Convert a core-seam [`TrustedPeerDescriptor`] into the comms-internal
/// [`TrustEntry`] (which carries a richer typed `PubKey`).
///
/// The descriptor carries the Ed25519 pubkey bytes; this helper consumes
/// them and validates that the derived [`PeerId`] matches the descriptor's
/// stated routing id (guards against hand-assembled descriptors where the
/// two identities disagree).
fn descriptor_to_trust_entry(descriptor: TrustedPeerDescriptor) -> Result<TrustEntry, SendError> {
    let pubkey = PubKey::new(descriptor.pubkey);
    if descriptor.has_zero_pubkey() {
        return Err(SendError::Validation(
            "TrustedPeerDescriptor.pubkey must be non-zero for trust registration".to_string(),
        ));
    }
    let derived = pubkey.to_peer_id();
    if derived != descriptor.peer_id {
        return Err(SendError::Validation(format!(
            "TrustedPeerDescriptor.peer_id {} does not match pubkey-derived id {}",
            descriptor.peer_id, derived
        )));
    }
    Ok(TrustEntry {
        peer_id: descriptor.peer_id,
        name: descriptor.name,
        pubkey,
        address: descriptor.address,
        meta: crate::PeerMeta::default(),
    })
}

impl InteractionStream {
    fn finish(&mut self) {
        if let Some(mut receiver) = self.receiver.take() {
            receiver.close();
        }
        match self.stream_handle.as_ref() {
            Some(handle) => {
                // DSL-owned path (U6): fire `closed_early`. The transition's
                // `is_attached` guard rejects when the DSL has already moved
                // past Attached (e.g., `Completed` won the race), which is
                // the correct CAS semantics; the cleanup observer drops the
                // shell-side channel entry under the same authority lock.
                let corr_id = meerkat_core::PeerCorrelationId::from_uuid(self.id);
                if let Err(err) = handle.closed_early(corr_id) {
                    tracing::trace!(
                        interaction_id = %self.id,
                        error = %err,
                        "InteractionStreamHandle::closed_early rejected (likely terminal won race)"
                    );
                }
            }
            None => {
                // Transport-only local stream: no semantic lifecycle exists,
                // so the channel projection owns direct cleanup.
                self.registry.lock().remove(&self.id);
            }
        }
        self.abandonment_registry.lock().remove(&self.id);
    }

    fn take_abandonment_event(&mut self) -> Option<meerkat_core::AgentEvent> {
        let reason = self.abandonment_signal.lock().take()?;
        self.abandonment_registry.lock().remove(&self.id);
        Some(meerkat_core::AgentEvent::InteractionFailed {
            interaction_id: meerkat_core::InteractionId(self.id),
            reason: meerkat_core::event::InteractionFailureReason::InteractionStreamAbandoned {
                reason,
            },
        })
    }
}

fn is_terminal_interaction_event(event: &meerkat_core::AgentEvent, interaction_id: Uuid) -> bool {
    match event {
        meerkat_core::AgentEvent::InteractionComplete {
            interaction_id: event_id,
            ..
        }
        | meerkat_core::AgentEvent::InteractionCallbackPending {
            interaction_id: event_id,
            ..
        }
        | meerkat_core::AgentEvent::InteractionFailed {
            interaction_id: event_id,
            ..
        } => event_id.0 == interaction_id,
        _ => false,
    }
}

fn map_peer_ingress_phase(
    phase: crate::peer_types::PeerIngressState,
) -> meerkat_core::PeerIngressAuthorityPhase {
    match phase {
        crate::peer_types::PeerIngressState::Absent => {
            meerkat_core::PeerIngressAuthorityPhase::Absent
        }
        crate::peer_types::PeerIngressState::Received => {
            meerkat_core::PeerIngressAuthorityPhase::Received
        }
        crate::peer_types::PeerIngressState::Dropped => {
            meerkat_core::PeerIngressAuthorityPhase::Dropped
        }
        crate::peer_types::PeerIngressState::Delivered => {
            meerkat_core::PeerIngressAuthorityPhase::Delivered
        }
    }
}

impl Stream for InteractionStream {
    type Item = meerkat_core::EventEnvelope<meerkat_core::AgentEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match this.receiver.as_mut() {
            Some(receiver) => match Pin::new(receiver).poll_recv(cx) {
                Poll::Ready(None) => {
                    if let Some(event) = this.take_abandonment_event() {
                        this.receiver = None;
                        this.seq = this.seq.saturating_add(1);
                        return Poll::Ready(Some(meerkat_core::EventEnvelope::new_with_source(
                            this.source.clone(),
                            this.seq,
                            None,
                            event,
                        )));
                    }
                    this.finish();
                    Poll::Ready(None)
                }
                Poll::Ready(Some(event)) => {
                    if is_terminal_interaction_event(&event, this.id) {
                        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(this.id);
                        match this.stream_handle.as_ref() {
                            Some(handle) => {
                                if let Err(err) = handle.completed(corr_id) {
                                    tracing::trace!(
                                        interaction_id = %this.id,
                                        error = %err,
                                        "InteractionStreamHandle::completed rejected while consuming terminal event"
                                    );
                                }
                            }
                            None => {
                                this.registry.lock().remove(&this.id);
                            }
                        }
                    }
                    this.seq = this.seq.saturating_add(1);
                    let envelope = meerkat_core::EventEnvelope::new_with_source(
                        this.source.clone(),
                        this.seq,
                        None,
                        event,
                    );
                    Poll::Ready(Some(envelope))
                }
                Poll::Pending => Poll::Pending,
            },
            None => Poll::Ready(None),
        }
    }
}

impl Drop for InteractionStream {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl CoreCommsRuntime for CommsRuntime {
    fn set_outbound_content_taint(
        &self,
        taint: Option<meerkat_core::comms::SenderContentTaint>,
    ) -> Result<(), meerkat_core::comms::SendError> {
        // Inherent method (host-set carrier config on the router); the dyn
        // trait surface is how mob/runtime layers holding
        // `Arc<dyn CoreCommsRuntime>` reach it per member.
        CommsRuntime::set_outbound_content_taint(self, taint);
        Ok(())
    }

    async fn drain_messages(&self) -> Vec<String> {
        // Delegate through classified drain so messages from the classified
        // inbox (the sole consumer since 0.4.10) are returned. The legacy
        // string projection cannot validate member residency, so it must fail
        // closed for incarnation-fenced messages. The structured drain below
        // remains their only runtime admission path.
        self.drain_inbox_interactions()
            .await
            .into_iter()
            .filter_map(|interaction| {
                if matches!(
                    &interaction.content,
                    meerkat_core::InteractionContent::IncarnationFencedMessage { .. }
                ) {
                    None
                } else {
                    Some(interaction.rendered_text)
                }
            })
            .collect()
    }
    fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
        self.inbox_notify.clone()
    }

    fn peer_id(&self) -> Option<PeerId> {
        Some(self.peer_id)
    }

    fn public_key(&self) -> Option<String> {
        Some(self.public_key.to_pubkey_string())
    }

    fn public_key_bytes(&self) -> Option<[u8; 32]> {
        Some(*self.public_key.as_bytes())
    }

    fn comms_name(&self) -> Option<String> {
        Some(self.participant_name().to_string())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn host_acceptor_registration_payload(&self) -> Option<Arc<dyn std::any::Any + Send + Sync>> {
        Some(Arc::new(
            crate::host_acceptor::HostAcceptorRegistrationMaterial {
                pubkey: self.public_key,
                keypair: self.router.keypair_arc(),
                inbox_sender: self.router.inbox_sender().clone(),
            },
        ))
    }

    fn advertised_address(&self) -> Option<String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Some(crate::runtime::comms_runtime::CommsRuntime::advertised_address(self))
        }
        #[cfg(target_arch = "wasm32")]
        {
            Some(format!("inproc://{}", self.participant_name()))
        }
    }

    fn bridge_bootstrap_token(&self) -> Option<String> {
        Some(self.bridge_bootstrap_token.clone())
    }

    async fn stage_declared_reply_endpoint(
        &self,
        dest: PeerId,
        signer_pubkey: [u8; 32],
        declared_address: String,
    ) -> Result<(), SendError> {
        // Skip-if-routable: trust-store entries always win at send time, so
        // staging for an already-routable peer would only leave a stale
        // one-shot entry behind. The trust store (public + private union) is
        // the same resolution authority `Router::send_with_id` consults.
        if self.trusted_peers.read().get(&dest).is_some() {
            return Ok(());
        }
        self.router.stage_reply_endpoint(
            dest,
            crate::router::DeclaredReplyEndpoint {
                pubkey: PubKey::new(signer_pubkey),
                address: declared_address,
            },
        );
        Ok(())
    }

    async fn stage_correlated_reply_endpoint(
        &self,
        dest: PeerId,
        in_reply_to: meerkat_core::interaction::InteractionId,
        signer_pubkey: [u8; 32],
        declared_endpoint: meerkat_core::comms::PeerAddress,
    ) -> Result<(), SendError> {
        let signer_pubkey = PubKey::new(signer_pubkey);
        let signer_peer_id = signer_pubkey.to_peer_id();
        if signer_peer_id != dest {
            return Err(SendError::Validation(format!(
                "correlated reply signer derives peer id {signer_peer_id}, not destination {dest}"
            )));
        }
        if crate::classify::declared_tcp_port(&declared_endpoint).is_none() {
            return Err(SendError::Validation(
                "correlated reply endpoint must use tcp:// with a valid nonzero port".to_string(),
            ));
        }
        self.router.stage_correlated_reply_endpoint(
            dest,
            in_reply_to.0,
            crate::router::DeclaredReplyEndpoint {
                pubkey: signer_pubkey,
                address: declared_endpoint.to_string(),
            },
        );
        Ok(())
    }

    async fn unstage_correlated_reply_endpoint(
        &self,
        dest: PeerId,
        in_reply_to: meerkat_core::interaction::InteractionId,
    ) -> Result<(), SendError> {
        self.router
            .unstage_correlated_reply_endpoint(dest, in_reply_to.0);
        Ok(())
    }

    fn take_bridge_reply_waiter(
        &self,
        in_reply_to: &meerkat_core::interaction::InteractionId,
    ) -> Option<tokio::sync::oneshot::Sender<meerkat_core::interaction::PeerInputCandidate>> {
        match self.bridge_reply_waiters.lock().remove(&in_reply_to.0) {
            Some(BridgeReplyWaiterEntry::Waiting(sender)) => Some(sender),
            // Tombstone consumed: the caller pairs this with
            // `has_bridge_reply_waiter` to discard the late reply.
            Some(BridgeReplyWaiterEntry::TimedOut(_)) | None => None,
        }
    }

    fn has_bridge_reply_waiter(
        &self,
        in_reply_to: &meerkat_core::interaction::InteractionId,
    ) -> bool {
        self.bridge_reply_waiters
            .lock()
            .contains_key(&in_reply_to.0)
    }

    async fn apply_trust_mutation(
        &self,
        mutation: CommsTrustMutation,
    ) -> Result<CommsTrustMutationResult, SendError> {
        match mutation {
            CommsTrustMutation::AddTrustedPeer { peer, authority } => {
                self.validate_generated_trust_authority_owner(&authority)?;
                authority
                    .validate_public_add(self.peer_id(), &peer)
                    .map_err(SendError::Validation)?;
                let created = self
                    .apply_trusted_peer_descriptor(peer, authority.trust_row_owner_kind())
                    .await?;
                Ok(CommsTrustMutationResult::Added { created })
            }
            CommsTrustMutation::RemoveTrustedPeer { peer_id, authority } => {
                self.validate_generated_trust_authority_owner(&authority)?;
                let parsed_peer_id = meerkat_core::comms::PeerId::parse(&peer_id)
                    .map_err(|err| SendError::Validation(err.to_string()))?;
                authority
                    .validate_public_remove(self.peer_id(), parsed_peer_id)
                    .map_err(SendError::Validation)?;
                let removed = self
                    .apply_trusted_peer_removal(&peer_id, authority.trust_row_owner_kind())
                    .await?;
                Ok(CommsTrustMutationResult::Removed { removed })
            }
            CommsTrustMutation::AddPrivateTrustedPeer { peer, authority } => {
                self.validate_generated_trust_authority_owner(&authority)?;
                authority
                    .validate_private_add(self.peer_id(), &peer)
                    .map_err(SendError::Validation)?;
                let created = self
                    .apply_private_trusted_peer_descriptor(peer, authority.trust_row_owner_kind())
                    .await?;
                Ok(CommsTrustMutationResult::Added { created })
            }
            CommsTrustMutation::RemovePrivateTrustedPeer { peer_id, authority } => {
                self.validate_generated_trust_authority_owner(&authority)?;
                let parsed_peer_id = meerkat_core::comms::PeerId::parse(&peer_id)
                    .map_err(|err| SendError::Validation(err.to_string()))?;
                authority
                    .validate_private_remove(self.peer_id(), parsed_peer_id)
                    .map_err(SendError::Validation)?;
                let removed = self
                    .apply_private_trusted_peer_removal(&peer_id, authority.trust_row_owner_kind())
                    .await?;
                Ok(CommsTrustMutationResult::Removed { removed })
            }
        }
    }

    async fn install_generated_mob_trust_owner(
        &self,
        owner: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), SendError> {
        let mut expected = self.mob_machine_trust_owner.write();
        if let Some(existing) = expected.as_ref() {
            if Arc::ptr_eq(existing, &owner) {
                return Ok(());
            }
            return Err(SendError::Validation(
                "target runtime is already bound to a different generated MobMachine trust owner"
                    .to_string(),
            ));
        }
        *expected = Some(owner);
        Ok(())
    }

    async fn validate_recovered_generated_mob_trust_owner(
        &self,
        owner: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), SendError> {
        let expected = self.mob_machine_trust_owner.read();
        if let Some(existing) = expected.as_ref()
            && !Arc::ptr_eq(existing, &owner)
        {
            return Err(SendError::Validation(
                "target runtime is already bound to a different generated MobMachine trust owner"
                    .to_string(),
            ));
        }
        Ok(())
    }

    async fn install_recovered_generated_mob_trust_owner(
        &self,
        owner: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), SendError> {
        let mut expected = self.mob_machine_trust_owner.write();
        if let Some(existing) = expected.as_ref() {
            if Arc::ptr_eq(existing, &owner) {
                return Ok(());
            }
            return Err(SendError::Validation(
                "target runtime is already bound to a different generated MobMachine trust owner"
                    .to_string(),
            ));
        }
        *expected = Some(owner);
        Ok(())
    }

    fn event_injector(&self) -> Option<Arc<dyn meerkat_core::EventInjector>> {
        Some(self.event_injector())
    }

    fn interaction_event_injector(
        &self,
    ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        Some(self.interaction_event_injector())
    }

    fn stream(&self, scope: StreamScope) -> Result<EventStream, StreamError> {
        match scope {
            StreamScope::Session(session_id) => {
                Err(StreamError::NotFound(format!("session {session_id}")))
            }
            StreamScope::Interaction(interaction_id) => {
                let id = interaction_id.0;
                let corr_id = meerkat_core::PeerCorrelationId::from_uuid(id);

                // TTL check is shell-owned mechanics: the DSL holds the
                // lifecycle state for machine-authoritative reservations, but
                // `created_at` belongs with the channel (DSL state has no
                // time). Inspect under the registry lock, then fire the typed
                // cleanup path for the reservation authority that created it.
                let reservation = {
                    let registry = self.interaction_stream_registry.lock();
                    registry.get(&id).map(|entry| {
                        (
                            entry.created_at.elapsed() > RESERVATION_TTL,
                            entry.lifecycle_authority,
                        )
                    })
                };
                let Some((expired, lifecycle_authority)) = reservation else {
                    return Err(self.interaction_stream_terminal_or_not_reserved(interaction_id));
                };

                let stream_handle = match lifecycle_authority {
                    InteractionStreamLifecycleAuthority::Machine => {
                        let handle = self.interaction_stream_handle().ok_or_else(|| {
                            StreamError::Internal(
                                "machine interaction stream authority missing for reserved stream"
                                    .to_string(),
                            )
                        })?;
                        match handle.state(corr_id) {
                            Some(meerkat_core::InteractionStreamState::Attached) => {
                                return Err(StreamError::AlreadyAttached(interaction_id));
                            }
                            Some(meerkat_core::InteractionStreamState::Reserved) if expired => {
                                if handle.expired(corr_id).is_ok() {
                                    return Err(StreamError::Timeout(format!(
                                        "reservation expired for interaction {}",
                                        interaction_id.0
                                    )));
                                }
                                return match handle.state(corr_id) {
                                    Some(meerkat_core::InteractionStreamState::Attached) => {
                                        Err(StreamError::AlreadyAttached(interaction_id))
                                    }
                                    _ => Err(self.interaction_stream_terminal_or_not_reserved(
                                        interaction_id,
                                    )),
                                };
                            }
                            Some(meerkat_core::InteractionStreamState::Reserved) => {}
                            _ => {
                                return Err(self
                                    .interaction_stream_terminal_or_not_reserved(interaction_id));
                            }
                        }
                        Some(handle)
                    }
                    InteractionStreamLifecycleAuthority::LocalTransportOnly => {
                        let mut registry = self.interaction_stream_registry.lock();
                        let entry = registry
                            .get_mut(&id)
                            .ok_or(StreamError::NotReserved(interaction_id))?;
                        if entry.receiver.is_none() {
                            return Err(StreamError::AlreadyAttached(interaction_id));
                        }
                        if expired {
                            registry.remove(&id);
                            drop(registry);
                            self.subscriber_registry.lock().remove(&id);
                            return Err(StreamError::Timeout(format!(
                                "reservation expired for interaction {}",
                                interaction_id.0
                            )));
                        }
                        None
                    }
                };

                // Transition to Attached via the DSL handle only for
                // machine-authoritative reservations; local input streams
                // consume the receiver as transport-only channels.
                if let Some(handle) = stream_handle.as_ref() {
                    handle.attached(corr_id).map_err(|err| {
                        // The DSL guard distinguishes "not Reserved": if
                        // state is already Attached it's a duplicate
                        // attach, otherwise the reservation has terminated.
                        match handle.state(corr_id) {
                            Some(meerkat_core::InteractionStreamState::Attached) => {
                                StreamError::AlreadyAttached(interaction_id)
                            }
                            None => {
                                self.interaction_stream_terminal_or_not_reserved(interaction_id)
                            }
                            _ => StreamError::Internal(format!(
                                "unexpected interaction stream state for {}: {err}",
                                interaction_id.0
                            )),
                        }
                    })?;
                }

                let mut registry = self.interaction_stream_registry.lock();
                let Some(entry) = registry.get_mut(&id) else {
                    drop(registry);
                    return Err(self.interaction_stream_terminal_or_not_reserved(interaction_id));
                };
                let abandonment_signal = Arc::clone(&entry.abandonment_signal);
                let receiver = entry.receiver.take().ok_or_else(|| {
                    // Transport-only local stream: no DSL guard ran, so
                    // double-attach surfaces here as a missing receiver.
                    if lifecycle_authority == InteractionStreamLifecycleAuthority::Machine {
                        StreamError::Internal("interaction stream receiver missing".to_string())
                    } else {
                        StreamError::AlreadyAttached(interaction_id)
                    }
                })?;
                drop(registry);

                Ok(Box::pin(InteractionStream {
                    id,
                    receiver: Some(receiver),
                    stream_handle,
                    registry: self.interaction_stream_registry.clone(),
                    abandonment_signal,
                    abandonment_registry: Arc::clone(&self.interaction_stream_abandonments),
                    source: meerkat_core::EventSourceIdentity::interaction(
                        meerkat_core::InteractionId(id),
                    ),
                    seq: 0,
                }))
            }
        }
    }

    async fn send(&self, cmd: CommsCommand) -> Result<SendReceipt, SendError> {
        match cmd {
            CommsCommand::Input {
                body,
                source,
                stream,
                allow_self_session,
                session_id: _,
                blocks,
                handling_mode,
            } => {
                // Self-input guard: when allow_self_session is false, this runtime's
                // inbox is the target, which is a self-loop. Reject unless explicitly
                // opted in. MCP never exposes this flag.
                if !allow_self_session {
                    return Err(SendError::Validation(
                        "self-session input rejected: set allow_self_session=true to override"
                            .into(),
                    ));
                }
                match stream {
                    InputStreamMode::None => {
                        // Generate the interaction id *first* and stamp it onto the
                        // injected event so the returned public handle correlates
                        // with the event — not a fresh unrelated UUID (#269).
                        let interaction_id = Uuid::new_v4();
                        let injector = self.event_injector_concrete();
                        let content = match blocks {
                            Some(blocks) => meerkat_core::types::ContentInput::Blocks(blocks),
                            None => meerkat_core::types::ContentInput::Text(body),
                        };
                        injector
                            .inject_with_interaction_id(
                                interaction_id,
                                content,
                                PlainEventSource::from(source),
                                handling_mode,
                                None,
                            )
                            .map_err(map_event_injector_error)?;
                        Ok(SendReceipt::InputAccepted {
                            interaction_id: meerkat_core::InteractionId(interaction_id),
                            stream_reserved: false,
                        })
                    }
                    InputStreamMode::ReserveInteraction => {
                        let interaction_id = Uuid::new_v4();
                        self.register_interaction_stream(
                            interaction_id,
                            body,
                            blocks,
                            source,
                            handling_mode,
                        )?;
                        Ok(SendReceipt::InputAccepted {
                            interaction_id: meerkat_core::InteractionId(interaction_id),
                            stream_reserved: true,
                        })
                    }
                }
            }
            CommsCommand::PeerMessage {
                to,
                body,
                blocks,
                content_taint,
                handling_mode,
                objective_id,
            } => self
                .send_peer_command(
                    &to,
                    crate::types::MessageKind::Message {
                        body,
                        blocks,
                        content_taint: None,
                        handling_mode: Some(handling_mode),
                        objective_id,
                    },
                    content_taint,
                )
                .await
                .map(|outcome| SendReceipt::PeerMessageSent {
                    envelope_id: outcome.envelope_id,
                    delivery: outcome.delivery,
                }),
            CommsCommand::IncarnationFencedPeerMessage {
                to,
                body,
                blocks,
                content_taint,
                handling_mode,
                objective_id,
                expected_recipient,
            } => self
                .send_peer_command(
                    &to,
                    crate::types::MessageKind::IncarnationFencedMessage {
                        body,
                        blocks,
                        content_taint: None,
                        handling_mode: Some(handling_mode),
                        objective_id,
                        expected_recipient,
                    },
                    content_taint,
                )
                .await
                .map(|outcome| SendReceipt::PeerMessageSent {
                    envelope_id: outcome.envelope_id,
                    delivery: outcome.delivery,
                }),
            CommsCommand::PeerLifecycle { to, kind, params } => self
                .send_peer_command(
                    &to,
                    crate::types::MessageKind::Lifecycle { kind, params },
                    None,
                )
                .await
                .map(|outcome| SendReceipt::PeerLifecycleSent {
                    envelope_id: outcome.envelope_id,
                }),
            CommsCommand::PeerRequest {
                to,
                intent,
                params,
                blocks,
                content_taint,
                handling_mode,
                stream,
                objective_id,
            } => {
                self.send_peer_request_with_id_lifecycle(
                    Uuid::new_v4(),
                    &to,
                    intent,
                    params,
                    blocks,
                    content_taint,
                    handling_mode,
                    stream == InputStreamMode::ReserveInteraction,
                    objective_id,
                    None,
                )
                .await
            }
            CommsCommand::PeerResponse {
                to,
                in_reply_to,
                status,
                result,
                blocks,
                content_taint,
                handling_mode,
                objective_id,
            } => {
                let (peer_handle, _stream_authority) =
                    self.require_peer_request_response_authority("PeerResponse")?;
                let corr_id = meerkat_core::PeerCorrelationId::from_uuid(in_reply_to.0);
                let core_status = status;
                let status = crate::Status::from(status);
                if peer_handle.inbound_state(corr_id).is_none() {
                    return Err(SendError::Validation(format!(
                        "PeerResponse requires machine inbound peer request state for corr_id {corr_id}"
                    )));
                }
                let reply_terminality =
                    peer_handle
                        .classify_response_reply(core_status)
                        .map_err(|err| {
                            SendError::Validation(format!(
                                "DSL rejected PeerResponse reply classification for corr_id {corr_id}: {err}"
                            ))
                        })?;
                let is_terminal_reply = matches!(
                    reply_terminality,
                    meerkat_core::TerminalityClass::Terminal { .. }
                );
                if !is_terminal_reply && handling_mode.is_some() {
                    return Err(SendError::Validation(
                        "handling_mode is forbidden on progress peer responses".to_string(),
                    ));
                }
                let effective_handling_mode = if is_terminal_reply {
                    // Preflight the inbound lifecycle STATE before the
                    // irreversible transport send. The terminal `response_replied`
                    // transition applied after the send commits a
                    // `PeerResponseReplied` guarded on the `inbound_peer_requests`
                    // state being `Received` — a DIFFERENT substate than the
                    // handling-mode lane (`inbound_peer_request_lanes`) checked
                    // below. Without this state check, a recorded lane could pass
                    // the lane preflight while the request is no longer repliable,
                    // so the post-send `response_replied` would reject AFTER the
                    // remote has already seen the terminal response — a split
                    // where the remote considers the request answered but local
                    // authority reports rejection. Validating repliability here
                    // keeps the send and the authority transition consistent
                    // (the post-send transition cannot reject under the
                    // single-owner session task).
                    if peer_handle.inbound_state(corr_id)
                        != Some(meerkat_core::InboundPeerRequestState::Received)
                    {
                        return Err(SendError::Validation(format!(
                            "PeerResponse terminal reply for corr_id {corr_id} is not in a repliable inbound state; response not sent"
                        )));
                    }
                    let resolved =
                        handling_mode.or_else(|| peer_handle.inbound_handling_mode(corr_id));
                    if resolved.is_none() {
                        return Err(SendError::Validation(format!(
                            "PeerResponse requires machine inbound peer request handling mode for corr_id {corr_id}"
                        )));
                    }
                    resolved
                } else {
                    None
                };
                let envelope_id = self
                    .send_peer_command(
                        &to,
                        crate::types::MessageKind::Response {
                            in_reply_to: in_reply_to.0,
                            status,
                            result,
                            blocks,
                            content_taint: None,
                            handling_mode: effective_handling_mode,
                            objective_id,
                        },
                        content_taint,
                    )
                    .await?
                    .envelope_id;

                // The terminal inbound transition is coupled to the actual
                // response send outcome: route/transport failure leaves the
                // request in `Received` so the responder can retry.
                if is_terminal_reply && let Err(err) = peer_handle.response_replied(corr_id) {
                    tracing::warn!(
                        error = %err,
                        corr_id = %corr_id,
                        "PeerInteractionHandle::response_replied rejected"
                    );
                    return Err(SendError::Validation(format!(
                        "DSL rejected PeerResponseReplied for corr_id {corr_id}: {err}"
                    )));
                }
                Ok(SendReceipt::PeerResponseSent {
                    envelope_id,
                    in_reply_to,
                })
            }
        }
    }

    async fn send_and_stream(
        &self,
        cmd: CommsCommand,
    ) -> Result<(SendReceipt, EventStream), SendAndStreamError> {
        match cmd {
            CommsCommand::Input {
                body,
                source,
                stream: InputStreamMode::ReserveInteraction,
                allow_self_session,
                session_id: _,
                blocks,
                handling_mode,
            } => {
                if !allow_self_session {
                    return Err(SendAndStreamError::Send(SendError::Validation(
                        "self-session input rejected: set allow_self_session=true to override"
                            .into(),
                    )));
                }
                let interaction_id = Uuid::new_v4();
                self.register_interaction_stream(
                    interaction_id,
                    body,
                    blocks,
                    source,
                    handling_mode,
                )?;
                let receipt = SendReceipt::InputAccepted {
                    interaction_id: meerkat_core::InteractionId(interaction_id),
                    stream_reserved: true,
                };
                let stream = self
                    .stream(StreamScope::Interaction(meerkat_core::InteractionId(
                        interaction_id,
                    )))
                    .map_err(|error| SendAndStreamError::StreamAttach {
                        receipt: receipt.clone(),
                        error,
                    })?;
                Ok((receipt, stream))
            }
            CommsCommand::PeerRequest {
                to,
                intent,
                params,
                blocks,
                content_taint,
                handling_mode,
                stream: InputStreamMode::ReserveInteraction,
                objective_id,
            } => {
                let interaction_id = Uuid::new_v4();
                let receipt = self
                    .send_peer_request_with_id_lifecycle(
                        interaction_id,
                        &to,
                        intent,
                        params,
                        blocks,
                        content_taint,
                        handling_mode,
                        true,
                        objective_id,
                        None,
                    )
                    .await?;
                let event_stream = self
                    .stream(StreamScope::Interaction(meerkat_core::InteractionId(
                        interaction_id,
                    )))
                    .map_err(|error| SendAndStreamError::StreamAttach {
                        receipt: receipt.clone(),
                        error,
                    })?;
                Ok((receipt, event_stream))
            }
            other => {
                let receipt = self.send(other).await?;
                Err(SendAndStreamError::StreamAttach {
                    receipt,
                    error: StreamError::NotFound("command is not streamable".to_string()),
                })
            }
        }
    }

    async fn peers(&self) -> Vec<PeerDirectoryEntry> {
        self.resolve_peer_directory().await
    }

    async fn peer_count(&self) -> usize {
        self.resolve_peer_count().await
    }

    async fn drain_inbox_interactions(&self) -> Vec<meerkat_core::InboxInteraction> {
        // Delegate to classified drain and strip classification metadata.
        // This ensures a single drain path: classified queue is the sole consumer.
        self.drain_classified_inbox_interactions()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|ci| ci.interaction)
            .collect()
    }

    fn interaction_subscriber(
        &self,
        id: &meerkat_core::InteractionId,
    ) -> Option<tokio::sync::mpsc::Sender<meerkat_core::AgentEvent>> {
        // One-shot sender extraction is transport mechanics only. A response
        // arriving before the caller attaches does not prove TTL expiry and
        // must leave the independent stream reservation claimable.
        self.subscriber_registry.lock().remove(&id.0)
    }

    fn mark_interaction_complete(&self, id: &meerkat_core::InteractionId) {
        self.mark_interaction_complete(id.0);
    }

    fn abandon_interaction_stream(
        &self,
        id: &meerkat_core::InteractionId,
        reason: meerkat_core::InteractionStreamAbandonReason,
    ) {
        self.abandon_interaction_stream(id.0, reason);
    }

    fn peer_interaction_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::handles::PeerInteractionHandle>> {
        self.peer_interaction_handle()
    }

    fn peer_request_response_authority_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::handles::PeerInteractionHandle>> {
        let peer_handle = self.peer_interaction_handle();
        if peer_handle.is_some() && self.interaction_stream_handle().is_some() {
            peer_handle
        } else {
            None
        }
    }

    async fn drain_classified_inbox_interactions(
        &self,
    ) -> Result<Vec<meerkat_core::ClassifiedInboxInteraction>, meerkat_core::CommsCapabilityError>
    {
        use crate::agent::types::MessageIntent;
        use crate::types::MessageKind;

        let mut inbox = self.inbox.lock().await;
        let classified_entries = inbox.try_drain_classified();

        if classified_entries.is_empty() {
            return Ok(Vec::new());
        }

        // Use stored classification metadata — NO trust re-check.
        // Snapshot semantics: classification was fixed at enqueue time.
        Ok(classified_entries
            .into_iter()
            .filter_map(|entry| {
                let ingress = entry.ingress_fact;
                let lifecycle_peer = entry.lifecycle_peer;
                let from_peer = entry.from_peer.unwrap_or_else(|| "unknown".to_string());
                let from_peer_id = ingress.route.as_ref().map(|route| route.peer_id);
                let rendered_text = entry.text_projection.clone();

                match entry.item {
                    crate::types::InboxItem::External { envelope } => {
                        // Runtime lifecycle dismissal does NOT flow through a
                        // peer message body. A peer-controlled string can never
                        // stop the local executor; dismissal must arrive as a
                        // typed lifecycle signal owned by the runtime authority.
                        // Extract handling_mode before consuming the kind.
                        // `send_response` may omit this field, so the sender
                        // side defaults it from the original request mode.
                        let envelope_handling_mode = match &envelope.kind {
                            MessageKind::Message {
                                handling_mode: Some(mode),
                                ..
                            }
                            | MessageKind::IncarnationFencedMessage {
                                handling_mode: Some(mode),
                                ..
                            }
                            | MessageKind::Request {
                                handling_mode: Some(mode),
                                ..
                            }
                            | MessageKind::Response {
                                handling_mode: Some(mode),
                                ..
                            } => *mode,
                            MessageKind::Response {
                                handling_mode: None,
                                ..
                            }
                            | MessageKind::Message {
                                handling_mode: None,
                                ..
                            }
                            | MessageKind::IncarnationFencedMessage {
                                handling_mode: None,
                                ..
                            }
                            | MessageKind::Request {
                                handling_mode: None,
                                ..
                            }
                            | MessageKind::Lifecycle { .. }
                            | MessageKind::Ack { .. } => meerkat_core::types::HandlingMode::Queue,
                        };
                        // Sender-declared content taint travels with the
                        // content facts (signed-when-present on the envelope);
                        // extract it before the kind is consumed below.
                        let sender_taint = envelope.kind.content_taint();
                        let objective_id = envelope.kind.objective_id();
                        let content = match envelope.kind {
                            MessageKind::Message {
                                body,
                                blocks,
                                content_taint: _,
                                handling_mode: _,
                                objective_id: _,
                            } => meerkat_core::InteractionContent::Message { body, blocks },
                            MessageKind::IncarnationFencedMessage {
                                body,
                                blocks,
                                content_taint: _,
                                handling_mode: _,
                                objective_id: _,
                                expected_recipient,
                            } => meerkat_core::InteractionContent::IncarnationFencedMessage {
                                body,
                                blocks,
                                expected_recipient,
                            },
                            MessageKind::Request {
                                intent,
                                params,
                                blocks,
                                reply_endpoint: _,
                                content_taint: _,
                                handling_mode: _,
                                objective_id: _,
                            } => {
                                let typed_intent = MessageIntent::from(intent.as_str());
                                meerkat_core::InteractionContent::Request {
                                    intent: typed_intent.to_string(),
                                    params,
                                    blocks,
                                }
                            }
                            MessageKind::Lifecycle { kind, params } => {
                                meerkat_core::InteractionContent::Request {
                                    intent: kind.to_string(),
                                    params,
                                    blocks: None,
                                }
                            }
                            MessageKind::Response {
                                in_reply_to,
                                status,
                                result,
                                blocks,
                                content_taint: _,
                                handling_mode: _,
                                objective_id: _,
                            } => {
                                let core_status = match status {
                                    crate::types::Status::Accepted => {
                                        meerkat_core::ResponseStatus::Accepted
                                    }
                                    crate::types::Status::Completed => {
                                        meerkat_core::ResponseStatus::Completed
                                    }
                                    crate::types::Status::Failed => {
                                        meerkat_core::ResponseStatus::Failed
                                    }
                                };
                                meerkat_core::InteractionContent::Response {
                                    in_reply_to: meerkat_core::InteractionId(in_reply_to),
                                    status: core_status,
                                    result,
                                    blocks,
                                }
                            }
                            MessageKind::Ack { .. } => {
                                // Acks should never reach classified drain
                                return None;
                            }
                        };

                        Some(meerkat_core::ClassifiedInboxInteraction {
                            interaction: meerkat_core::InboxInteraction {
                                id: meerkat_core::InteractionId(envelope.id),
                                from_route: from_peer_id,
                                from: from_peer,
                                content,
                                rendered_text,
                                handling_mode: envelope_handling_mode,
                                render_metadata: None,
                                sender_taint,
                                objective_id,
                            },
                            ingress,
                            lifecycle_peer,
                            response_terminality: entry.response_terminality,
                        })
                    }
                    crate::types::InboxItem::PlainEvent {
                        body,
                        source,
                        handling_mode,
                        objective_id,
                        interaction_id,
                        blocks,
                        render_metadata,
                        ..
                    } => Some(meerkat_core::ClassifiedInboxInteraction {
                        interaction: meerkat_core::InboxInteraction {
                            id: meerkat_core::InteractionId(
                                interaction_id.unwrap_or_else(uuid::Uuid::new_v4),
                            ),
                            from_route: None,
                            from: format!("event:{source}"),
                            content: meerkat_core::InteractionContent::Message { body, blocks },
                            rendered_text,
                            handling_mode,
                            render_metadata,
                            // Plain events are host-injected, not peer
                            // envelopes: no sender declaration exists.
                            sender_taint: None,
                            objective_id,
                        },
                        ingress,
                        lifecycle_peer,
                        response_terminality: entry.response_terminality,
                    }),
                }
            })
            .collect())
    }

    async fn peer_ingress_queue_snapshot(
        &self,
    ) -> Result<meerkat_core::PeerIngressQueueSnapshot, meerkat_core::CommsCapabilityError> {
        let inbox = self.inbox.lock().await;
        inbox.classified_snapshot().ok_or_else(|| {
            meerkat_core::CommsCapabilityError::Unsupported(
                "peer_ingress_queue_snapshot: classified inbox not initialized".to_string(),
            )
        })
    }

    async fn peer_ingress_runtime_snapshot(
        &self,
    ) -> Result<meerkat_core::PeerIngressRuntimeSnapshot, meerkat_core::CommsCapabilityError> {
        let (queue, authority_phase, submission_queue_len) = {
            let inbox = self.inbox.lock().await;
            inbox.peer_runtime_snapshot().ok_or_else(|| {
                meerkat_core::CommsCapabilityError::Unsupported(
                    "peer_ingress_runtime_snapshot: classified inbox not initialized".to_string(),
                )
            })?
        };
        let trusted_peers = self.trusted_peer_descriptors(true);

        Ok(meerkat_core::PeerIngressRuntimeSnapshot {
            self_peer_id: peer_id_from_pubkey(&self.public_key),
            auth_required: self.require_peer_auth,
            authority_phase: map_peer_ingress_phase(authority_phase),
            trusted_peers,
            submission_queue_len,
            queue,
        })
    }

    async fn public_trusted_peer_projection_snapshot(
        &self,
    ) -> Result<Vec<TrustedPeerDescriptor>, meerkat_core::CommsCapabilityError> {
        Ok(self.trusted_peer_descriptors(false))
    }

    async fn trusted_peer_projection_snapshot_for_source(
        &self,
        source_kind: GeneratedCommsTrustAuthoritySourceKind,
    ) -> Result<Vec<TrustedPeerDescriptor>, meerkat_core::CommsCapabilityError> {
        Ok(self.trusted_peer_descriptors_for_source(false, source_kind))
    }

    fn actionable_input_notify(
        &self,
    ) -> Result<Arc<tokio::sync::Notify>, meerkat_core::CommsCapabilityError> {
        self.actionable_notify.clone().ok_or_else(|| {
            meerkat_core::CommsCapabilityError::Unsupported(
                "actionable_input_notify: classified inbox not initialized".to_string(),
            )
        })
    }
}

fn map_event_injector_error(error: meerkat_core::event_injector::EventInjectorError) -> SendError {
    match error {
        meerkat_core::event_injector::EventInjectorError::Full => {
            SendError::Validation("input queue full".into())
        }
        meerkat_core::event_injector::EventInjectorError::Closed => SendError::InputClosed,
    }
}

#[derive(Debug, Error)]
pub enum CommsRuntimeError {
    #[error("Identity error: {0}")]
    IdentityError(String),
    #[error("Session identity already active: {0}")]
    SessionIdentityInUse(SessionId),
    #[error("Trust load error: {0}")]
    TrustLoadError(String),
    #[error("Listener error: {0}")]
    ListenerError(#[from] std::io::Error),
    #[error("Listeners already started")]
    AlreadyStarted,
    #[error("Unsafe binding: {0}")]
    UnsafeBinding(String),
    #[error("Inproc registration rejected: {0:?}")]
    InprocRegistrationRejected(crate::RegistrationRejection),
    #[error("Inproc publication failed: {0}")]
    InprocPublication(#[from] crate::InprocPublicationError),
}

/// Observe a constructor-time inproc registration outcome and fail closed on
/// rejection.
///
/// A freshly constructed runtime binds its own (non-zero) keypair under its
/// participant name. A clean [`RegistrationOutcome::Registered`] is the
/// expected result. A name/pubkey displacement is surfaced as a typed warning
/// (legitimate during a same-name restart), and a hard rejection fails the
/// constructor closed rather than returning a runtime whose route was never
/// installed.
fn observe_inproc_registration(
    namespace: &str,
    name: &str,
    outcome: crate::RegistrationOutcome,
) -> Result<(), CommsRuntimeError> {
    use crate::RegistrationOutcome;
    match outcome {
        RegistrationOutcome::Registered => Ok(()),
        RegistrationOutcome::ReplacedPubkey { evicted_name } => {
            tracing::warn!(
                inproc_namespace = %namespace,
                peer_name = %name,
                %evicted_name,
                "inproc registration replaced an existing route under this pubkey"
            );
            Ok(())
        }
        RegistrationOutcome::EvictedName { evicted_pubkey } => {
            tracing::warn!(
                inproc_namespace = %namespace,
                peer_name = %name,
                evicted_pubkey = %evicted_pubkey.to_pubkey_string(),
                "inproc registration evicted a stale pubkey bound to this name"
            );
            Ok(())
        }
        RegistrationOutcome::ReplacedPubkeyAndEvictedName {
            evicted_name,
            evicted_pubkey,
        } => {
            tracing::warn!(
                inproc_namespace = %namespace,
                peer_name = %name,
                %evicted_name,
                evicted_pubkey = %evicted_pubkey.to_pubkey_string(),
                "inproc registration displaced both a name and a pubkey route"
            );
            Ok(())
        }
        RegistrationOutcome::Rejected { reason } => {
            Err(CommsRuntimeError::InprocRegistrationRejected(reason))
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<SessionClaimError> for CommsRuntimeError {
    fn from(err: SessionClaimError) -> Self {
        match err {
            SessionClaimError::SessionIdentityInUse(sid) => Self::SessionIdentityInUse(sid),
        }
    }
}

/// Material needed by the facade's comms tool composition helper.
///
/// Bundles the concrete router/trust/key access so the facade doesn't
/// reach into CommsRuntime internals directly. Construct via
/// [`CommsRuntime::tool_material()`].
pub struct CommsToolMaterial {
    router: Arc<Router>,
    trusted_peers: TrustedPeersView,
    self_pubkey: crate::identity::PubKey,
    runtime: Arc<dyn meerkat_core::agent::CommsRuntime>,
}

impl CommsToolMaterial {
    /// Router for tool dispatch.
    pub fn router(&self) -> &Arc<Router> {
        &self.router
    }
    /// Live trust state for peer availability gating.
    pub fn trusted_peers(&self) -> &TrustedPeersView {
        &self.trusted_peers
    }
    /// Owned clone of the read-only trust view (for availability closures).
    pub fn trusted_peers_shared(&self) -> TrustedPeersView {
        self.trusted_peers.clone()
    }
    /// This runtime's public key.
    pub fn self_pubkey(&self) -> crate::identity::PubKey {
        self.self_pubkey
    }
    /// Trait-erased runtime for the comms tool surface.
    pub fn into_runtime(self) -> Arc<dyn meerkat_core::agent::CommsRuntime> {
        self.runtime
    }
}

/// Bounded size of the bridge-reply waiter registry. Registration beyond
/// this cap is a typed error — an unbounded registry would let a stalled
/// remote responder pin arbitrary memory.
#[cfg(not(target_arch = "wasm32"))]
const BRIDGE_REPLY_WAITER_CAPACITY: usize = 256;

/// Lazy eviction horizon for timed-out waiter tombstones. A tombstone is
/// removed on take (the drain consumed the late reply) or evicted at the
/// next registry write once this horizon has passed.
#[cfg(not(target_arch = "wasm32"))]
const BRIDGE_REPLY_TOMBSTONE_TTL: meerkat_core::time_compat::Duration =
    meerkat_core::time_compat::Duration::from_secs(300);

/// One-fact-one-owner disposition for an outstanding bridge-reply envelope
/// id: the comms drain always finds a typed disposition — deliver
/// (`Waiting`), discard (`TimedOut` tombstone), or (never registered) the
/// ordinary session path.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum BridgeReplyWaiterEntry {
    /// A live waiter: the drain hands the terminal Response candidate to
    /// this one-shot sender instead of injecting it into the session.
    Waiting(tokio::sync::oneshot::Sender<meerkat_core::interaction::PeerInputCandidate>),
    /// The requester timed out; a late reply is consumed and discarded so
    /// bridge JSON never leaks into the member's transcript.
    TimedOut(Instant),
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub struct CommsRuntime {
    public_key: PubKey,
    peer_id: PeerId,
    router: Arc<Router>,
    trusted_peers: Arc<parking_lot::RwLock<TrustStore>>,
    inbox: Arc<AsyncMutex<crate::Inbox>>,
    /// Exact inbox generation published by this runtime. A prepared runtime
    /// carries `None`; publication stores one sender clone for compare-replace
    /// and compare-remove operations in the process-global registry.
    inproc_sender: Option<crate::InboxSender>,
    inbox_notify: Arc<tokio::sync::Notify>,
    #[cfg(not(target_arch = "wasm32"))]
    config: ResolvedCommsConfig,
    /// Participant name (wasm32-only; on native this comes from `config.name`).
    #[cfg(target_arch = "wasm32")]
    name: String,
    /// Inproc namespace (wasm32-only; on native this comes from `config.inproc_namespace`).
    #[cfg(target_arch = "wasm32")]
    inproc_namespace: Option<String>,
    #[cfg(not(target_arch = "wasm32"))]
    listener_handles: Mutex<Vec<ListenerHandle>>,
    #[cfg(not(target_arch = "wasm32"))]
    listeners_started: bool,
    #[cfg(not(target_arch = "wasm32"))]
    _session_identity_claim: Option<SessionClaim>,
    keypair: Arc<Keypair>,
    bridge_bootstrap_token: String,
    require_peer_auth: bool,
    subscriber_registry: crate::event_injector::SubscriberRegistry,
    interaction_stream_registry: InteractionStreamRegistry,
    /// Bounded, short-lived projection of machine-owned abandoned terminals.
    /// It lets a two-step caller observe a typed reason when abandonment wins
    /// before attach; attached streams carry the same reason on their shared
    /// signal and emit a typed terminal event at EOF.
    interaction_stream_abandonments: InteractionStreamAbandonmentRegistry,
    peer_comms_handle: crate::classify::PeerCommsHandleSlot,
    meerkat_machine_trust_owner:
        parking_lot::RwLock<Option<meerkat_core::comms::GeneratedPeerCommsOwnerToken>>,
    mob_machine_trust_owner: parking_lot::RwLock<Option<Arc<dyn std::any::Any + Send + Sync>>>,
    require_peer_comms_machine_authority: Arc<AtomicBool>,
    /// Narrow notify that fires only for actionable peer input (messages/requests).
    /// Set during construction when classified inbox is used.
    actionable_notify: Option<Arc<tokio::sync::Notify>>,
    blob_store: Option<Arc<dyn BlobStore>>,
    /// Session-scoped peer-interaction DSL handle (W1-A). When this is absent,
    /// the runtime is explicitly transport-only for semantic peer
    /// request/response: it may carry one-way peer messages, but it must not
    /// emit `PeerRequestSent` or `PeerResponseSent` receipts.
    peer_interaction_handle:
        parking_lot::RwLock<Option<Arc<dyn meerkat_core::handles::PeerInteractionHandle>>>,
    /// Session-scoped interaction-stream lifecycle DSL handle (U6 / dogma #5).
    ///
    /// When `Some`, the shell's `interaction_stream_registry` is a pure
    /// projection of the DSL `interaction_streams` map: stream(),
    /// mark_interaction_complete, reap_expired_reservations, and the
    /// `InteractionStream::finish` close path all fire DSL inputs; the
    /// installed cleanup observer drops registry entries on terminal
    /// transitions. When `None`, only local transport-only input streams may
    /// use the registry directly; peer request/response stream reservations
    /// require this handle.
    interaction_stream_handle:
        parking_lot::RwLock<Option<Arc<dyn meerkat_core::handles::InteractionStreamHandle>>>,
    /// One-shot bridge-reply waiters keyed by outbound envelope id (member
    /// upcall lane). Shell-owned mechanics: the peer-interaction DSL keeps
    /// the semantic request/response lifecycle; this registry only decides
    /// whether a terminal Response candidate is delivered to an
    /// agent-blocking waiter, discarded (tombstone), or takes the ordinary
    /// session path.
    bridge_reply_waiters: Mutex<HashMap<Uuid, BridgeReplyWaiterEntry>>,
}

/// Fully constructed comms runtime whose inproc route is still dormant.
///
/// The wrapper is deliberately non-`Clone`. Callers may configure listeners
/// and install generated authorities through `runtime_mut`/`runtime`, then
/// consume the wrapper with `publish` or `publish_replacing`. Dropping it has
/// no registry effect.
#[must_use = "a prepared comms runtime must be published or deliberately dropped"]
pub struct PreparedCommsRuntime {
    runtime: CommsRuntime,
}

impl PreparedCommsRuntime {
    pub fn runtime(&self) -> &CommsRuntime {
        &self.runtime
    }

    pub fn runtime_mut(&mut self) -> &mut CommsRuntime {
        &mut self.runtime
    }

    /// Publish this runtime using ordinary inproc registration semantics.
    pub fn publish(mut self) -> Result<CommsRuntime, CommsRuntimeError> {
        self.runtime.publish_prepared_inproc(None)?;
        Ok(self.runtime)
    }

    /// Atomically replace the exact inproc generation owned by `current`.
    ///
    /// Namespace and participant name must match. Any stale-current or
    /// replacement-key conflict is reported before registry mutation.
    pub fn publish_replacing(
        self,
        current: &CommsRuntime,
    ) -> Result<CommsRuntime, CommsRuntimeError> {
        self.publish_replacing_recoverable(current)
            .map_err(|(_prepared, error)| error)
    }

    /// Recoverable form of [`Self::publish_replacing`].
    ///
    /// A failed exact publication leaves the registry unchanged and returns
    /// ownership of the still-dormant candidate so callers can await listener
    /// teardown before rebinding a fixed endpoint.
    pub fn publish_replacing_recoverable(
        mut self,
        current: &CommsRuntime,
    ) -> Result<CommsRuntime, (Box<Self>, CommsRuntimeError)> {
        match self.runtime.publish_prepared_inproc(Some(current)) {
            Ok(()) => Ok(self.runtime),
            Err(error) => Err((Box::new(self), error)),
        }
    }
}

impl CommsRuntime {
    fn publish_prepared_inproc(
        &mut self,
        current: Option<&CommsRuntime>,
    ) -> Result<(), CommsRuntimeError> {
        debug_assert!(self.inproc_sender.is_none());
        let namespace = self.inproc_namespace().unwrap_or("").to_string();
        let name = self.participant_name().to_string();
        let sender = self.router.inbox_sender().clone();
        if let Some(current) = current
            && (current.inproc_namespace().unwrap_or("") != namespace
                || current.participant_name() != name)
        {
            return Err(crate::InprocPublicationError::IdentityMismatch.into());
        }
        let outcome = if let Some(current) = current {
            let current_sender = current.inproc_sender.as_ref().ok_or({
                CommsRuntimeError::InprocPublication(
                    crate::InprocPublicationError::CurrentRuntimeUnpublished,
                )
            })?;
            InprocRegistry::global()
                .replace_sender_in_namespace(
                    &namespace,
                    &name,
                    (&current.public_key, current_sender),
                    self.public_key,
                    sender.clone(),
                )
                .map_err(CommsRuntimeError::InprocPublication)?;
            crate::RegistrationOutcome::Registered
        } else {
            InprocRegistry::global().register_with_meta_in_namespace(
                &namespace,
                &name,
                self.public_key,
                sender.clone(),
                crate::PeerMeta::default(),
            )
        };
        observe_inproc_registration(&namespace, &name, outcome)?;
        self.inproc_sender = Some(sender);
        Ok(())
    }

    fn derive_bridge_bootstrap_token(keypair: &Keypair) -> String {
        let mut digest = Sha256::new();
        digest.update(b"meerkat.supervisor-bridge.bootstrap-token.v1");
        digest.update(keypair.secret_bytes());
        base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            digest.finalize(),
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn reject_persisted_trusted_peers_seed(
        path: &std::path::Path,
    ) -> Result<(), CommsRuntimeError> {
        let peers = TrustStore::load_or_default(path)
            .map_err(|err| CommsRuntimeError::TrustLoadError(err.to_string()))?;
        if peers.has_peers() {
            return Err(CommsRuntimeError::TrustLoadError(format!(
                "persisted trusted peer seed at {} is not authoritative; generated comms trust mutation authority required",
                path.display()
            )));
        }
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub async fn new(config: ResolvedCommsConfig) -> Result<Self, CommsRuntimeError> {
        Self::new_with_silent_intents(config, Arc::new(HashSet::new())).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub async fn new_with_silent_intents(
        config: ResolvedCommsConfig,
        silent_intents: Arc<HashSet<String>>,
    ) -> Result<Self, CommsRuntimeError> {
        Self::new_with_silent_intents_and_machine_authority_requirement(
            config,
            silent_intents,
            false,
        )
        .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub async fn new_machine_authority_required_with_silent_intents(
        config: ResolvedCommsConfig,
        silent_intents: Arc<HashSet<String>>,
    ) -> Result<Self, CommsRuntimeError> {
        Self::new_with_silent_intents_and_machine_authority_requirement(
            config,
            silent_intents,
            true,
        )
        .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn new_with_silent_intents_and_machine_authority_requirement(
        config: ResolvedCommsConfig,
        _silent_intents: Arc<HashSet<String>>,
        require_machine_authority: bool,
    ) -> Result<Self, CommsRuntimeError> {
        // Always load keypair regardless of auth mode. Trust rows are not
        // seeded from config; live trust changes must arrive through generated
        // machine/composition authority.
        let keypair = Keypair::load_or_generate(&config.identity_dir)
            .await
            .map_err(|e| CommsRuntimeError::IdentityError(e.to_string()))?;
        Self::reject_persisted_trusted_peers_seed(&config.trusted_peers_path)?;
        let trusted_peers = TrustStore::new();
        let public_key = keypair.public_key();
        // Single source of truth for trust state — shared internally by the
        // Router and IngressClassificationContext.
        let trusted_peers = Arc::new(parking_lot::RwLock::new(trusted_peers));

        // Build classified inbox using the same trusted_peers Arc
        let peer_comms_handle = Arc::new(parking_lot::RwLock::new(None));
        let require_peer_comms_machine_authority =
            Arc::new(AtomicBool::new(require_machine_authority));
        let classification_context = Arc::new(crate::classify::IngressClassificationContext {
            require_peer_auth: config.require_peer_auth,
            trusted_peers: trusted_peers.clone(),
            peer_comms_handle: peer_comms_handle.clone(),
            inproc_namespace: config.inproc_namespace.clone(),
        });
        let (inbox, inbox_sender) = crate::Inbox::new_classified(classification_context);
        let inbox_notify = inbox.notify();
        let actionable_notify = inbox.classified_actionable_notify();

        let router = Router::with_shared_peers(
            keypair.clone(),
            trusted_peers.clone(),
            config.comms_config.clone(),
            inbox_sender,
            config.require_peer_auth,
        )
        .with_inproc_namespace(config.inproc_namespace.clone());

        let runtime = Self {
            public_key,
            peer_id: public_key.to_peer_id(),
            router: Arc::new(router),
            trusted_peers,
            inbox: Arc::new(AsyncMutex::new(inbox)),
            inproc_sender: None,
            inbox_notify,
            config: config.clone(),
            listener_handles: Mutex::new(Vec::new()),
            listeners_started: false,
            _session_identity_claim: None,
            bridge_bootstrap_token: Self::derive_bridge_bootstrap_token(&keypair),
            keypair: Arc::new(keypair),
            require_peer_auth: config.require_peer_auth,
            subscriber_registry: crate::event_injector::new_subscriber_registry(),
            interaction_stream_registry: Arc::new(Mutex::new(HashMap::new())),
            interaction_stream_abandonments: Arc::new(Mutex::new(HashMap::new())),
            peer_comms_handle,
            meerkat_machine_trust_owner: parking_lot::RwLock::new(None),
            mob_machine_trust_owner: parking_lot::RwLock::new(None),
            require_peer_comms_machine_authority,
            actionable_notify,
            blob_store: None,
            peer_interaction_handle: parking_lot::RwLock::new(None),
            interaction_stream_handle: parking_lot::RwLock::new(None),
            bridge_reply_waiters: Mutex::new(HashMap::new()),
        };
        let mut runtime = runtime;
        runtime.publish_prepared_inproc(None)?;
        Ok(runtime)
    }

    pub fn inproc_only(name: &str) -> Result<Self, CommsRuntimeError> {
        Self::inproc_only_with_silent_intents(name, None, Arc::new(HashSet::new()))
    }

    pub fn inproc_only_scoped(
        name: &str,
        namespace: Option<String>,
    ) -> Result<Self, CommsRuntimeError> {
        Self::inproc_only_with_silent_intents(name, namespace, Arc::new(HashSet::new()))
    }

    pub fn inproc_only_with_silent_intents(
        name: &str,
        namespace: Option<String>,
        silent_intents: Arc<HashSet<String>>,
    ) -> Result<Self, CommsRuntimeError> {
        let keypair = Keypair::generate();
        Self::inproc_only_with_keypair_and_silent_intents(name, namespace, keypair, silent_intents)
    }

    pub fn inproc_only_with_keypair_and_silent_intents(
        name: &str,
        namespace: Option<String>,
        keypair: Keypair,
        silent_intents: Arc<HashSet<String>>,
    ) -> Result<Self, CommsRuntimeError> {
        Self::prepare_inproc_only_with_keypair_and_silent_intents(
            name,
            namespace,
            keypair,
            silent_intents,
        )?
        .publish()
    }

    /// Construct a complete inproc runtime without publishing its global
    /// route. This is the two-phase seam for cancellation-safe supervisor
    /// rotation and other exact runtime replacement.
    pub fn prepare_inproc_only_with_keypair_and_silent_intents(
        name: &str,
        namespace: Option<String>,
        keypair: Keypair,
        _silent_intents: Arc<HashSet<String>>,
    ) -> Result<PreparedCommsRuntime, CommsRuntimeError> {
        let public_key = keypair.public_key();
        // Single source of truth — same Arc shared by Router, classification, and callers.
        let trusted_peers = Arc::new(parking_lot::RwLock::new(TrustStore::new()));

        let peer_comms_handle = Arc::new(parking_lot::RwLock::new(None));
        let require_peer_comms_machine_authority = Arc::new(AtomicBool::new(false));
        let classification_context = Arc::new(crate::classify::IngressClassificationContext {
            require_peer_auth: true,
            trusted_peers: trusted_peers.clone(),
            peer_comms_handle: peer_comms_handle.clone(),
            inproc_namespace: namespace.clone(),
        });
        let (inbox, inbox_sender) = crate::Inbox::new_classified(classification_context);
        let inbox_notify = inbox.notify();
        let actionable_notify = inbox.classified_actionable_notify();
        let comms_config = crate::CommsConfig::default();
        #[cfg(not(target_arch = "wasm32"))]
        let config = ResolvedCommsConfig {
            enabled: true,
            name: name.to_string(),
            inproc_namespace: namespace.clone(),
            identity_dir: std::path::PathBuf::new(),
            trusted_peers_path: std::path::PathBuf::new(),
            listen_uds: None,
            listen_tcp: None,
            advertise_address: None,
            event_listen_tcp: None,
            #[cfg(unix)]
            event_listen_uds: None,
            comms_config: comms_config.clone(),
            auth: meerkat_core::CommsAuthMode::Open,
            require_peer_auth: true,
            allow_external_unauthenticated: false,
            pairing_password: None,
        };
        #[cfg(target_arch = "wasm32")]
        let runtime_namespace = namespace.clone();
        let router = Router::with_shared_peers(
            keypair.clone(),
            trusted_peers.clone(),
            comms_config,
            inbox_sender,
            true,
        )
        .with_inproc_namespace(namespace);
        let runtime = Self {
            public_key,
            peer_id: public_key.to_peer_id(),
            router: Arc::new(router),
            trusted_peers,
            inbox: Arc::new(AsyncMutex::new(inbox)),
            inproc_sender: None,
            inbox_notify,
            #[cfg(not(target_arch = "wasm32"))]
            config,
            #[cfg(target_arch = "wasm32")]
            name: name.to_string(),
            #[cfg(target_arch = "wasm32")]
            inproc_namespace: runtime_namespace,
            #[cfg(not(target_arch = "wasm32"))]
            listener_handles: Mutex::new(Vec::new()),
            #[cfg(not(target_arch = "wasm32"))]
            listeners_started: false,
            #[cfg(not(target_arch = "wasm32"))]
            _session_identity_claim: None,
            bridge_bootstrap_token: Self::derive_bridge_bootstrap_token(&keypair),
            keypair: Arc::new(keypair),
            require_peer_auth: true,
            subscriber_registry: crate::event_injector::new_subscriber_registry(),
            interaction_stream_registry: Arc::new(Mutex::new(HashMap::new())),
            interaction_stream_abandonments: Arc::new(Mutex::new(HashMap::new())),
            peer_comms_handle,
            meerkat_machine_trust_owner: parking_lot::RwLock::new(None),
            mob_machine_trust_owner: parking_lot::RwLock::new(None),
            require_peer_comms_machine_authority,
            actionable_notify,
            blob_store: None,
            peer_interaction_handle: parking_lot::RwLock::new(None),
            interaction_stream_handle: parking_lot::RwLock::new(None),
            bridge_reply_waiters: Mutex::new(HashMap::new()),
        };
        Ok(PreparedCommsRuntime { runtime })
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_listen_tcp_for_unstarted_runtime(
        &mut self,
        listen_tcp: std::net::SocketAddr,
    ) -> Result<(), CommsRuntimeError> {
        if self.listeners_started {
            return Err(CommsRuntimeError::AlreadyStarted);
        }
        self.config.listen_tcp = Some(listen_tcp);
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_advertise_address_for_unstarted_runtime(
        &mut self,
        advertise_address: String,
    ) -> Result<(), CommsRuntimeError> {
        if self.listeners_started {
            return Err(CommsRuntimeError::AlreadyStarted);
        }
        self.config.advertise_address = Some(advertise_address);
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub async fn inproc_only_session_scoped_with_silent_intents(
        name: &str,
        namespace: Option<String>,
        identity_root: std::path::PathBuf,
        session_id: &meerkat_core::SessionId,
        _silent_intents: Arc<HashSet<String>>,
        session_claim_handle: Arc<dyn SessionClaimHandle>,
    ) -> Result<Self, CommsRuntimeError> {
        let claim = session_claim_handle
            .try_acquire(session_id)
            .map_err(CommsRuntimeError::from)?;
        let identity_dir = identity_root.join(session_id.to_string());
        let keypair = Keypair::load_or_generate(&identity_dir)
            .await
            .map_err(|e| CommsRuntimeError::IdentityError(e.to_string()))?;
        let public_key = keypair.public_key();
        let trusted_peers = Arc::new(parking_lot::RwLock::new(TrustStore::new()));

        let peer_comms_handle = Arc::new(parking_lot::RwLock::new(None));
        let require_peer_comms_machine_authority = Arc::new(AtomicBool::new(true));
        let classification_context = Arc::new(crate::classify::IngressClassificationContext {
            require_peer_auth: true,
            trusted_peers: trusted_peers.clone(),
            peer_comms_handle: peer_comms_handle.clone(),
            inproc_namespace: namespace.clone(),
        });
        let (inbox, inbox_sender) = crate::Inbox::new_classified(classification_context);
        let inbox_notify = inbox.notify();
        let actionable_notify = inbox.classified_actionable_notify();
        let comms_config = crate::CommsConfig::default();
        let config = ResolvedCommsConfig {
            enabled: true,
            name: name.to_string(),
            inproc_namespace: namespace.clone(),
            identity_dir: identity_dir.clone(),
            trusted_peers_path: identity_dir.join("trusted_peers.json"),
            listen_uds: None,
            listen_tcp: None,
            advertise_address: None,
            event_listen_tcp: None,
            #[cfg(unix)]
            event_listen_uds: None,
            comms_config: comms_config.clone(),
            auth: meerkat_core::CommsAuthMode::Open,
            require_peer_auth: true,
            allow_external_unauthenticated: false,
            pairing_password: None,
        };
        let router = Router::with_shared_peers(
            keypair.clone(),
            trusted_peers.clone(),
            comms_config,
            inbox_sender,
            true,
        )
        .with_inproc_namespace(namespace);
        let runtime = Self {
            public_key,
            peer_id: public_key.to_peer_id(),
            router: Arc::new(router),
            trusted_peers,
            inbox: Arc::new(AsyncMutex::new(inbox)),
            inproc_sender: None,
            inbox_notify,
            config,
            listener_handles: Mutex::new(Vec::new()),
            listeners_started: false,
            _session_identity_claim: Some(claim),
            bridge_bootstrap_token: Self::derive_bridge_bootstrap_token(&keypair),
            keypair: Arc::new(keypair),
            require_peer_auth: true,
            subscriber_registry: crate::event_injector::new_subscriber_registry(),
            interaction_stream_registry: Arc::new(Mutex::new(HashMap::new())),
            interaction_stream_abandonments: Arc::new(Mutex::new(HashMap::new())),
            peer_comms_handle,
            meerkat_machine_trust_owner: parking_lot::RwLock::new(None),
            mob_machine_trust_owner: parking_lot::RwLock::new(None),
            require_peer_comms_machine_authority,
            actionable_notify,
            blob_store: None,
            peer_interaction_handle: parking_lot::RwLock::new(None),
            interaction_stream_handle: parking_lot::RwLock::new(None),
            bridge_reply_waiters: Mutex::new(HashMap::new()),
        };
        let mut runtime = runtime;
        runtime.publish_prepared_inproc(None)?;
        Ok(runtime)
    }

    pub fn participant_name(&self) -> &str {
        #[cfg(not(target_arch = "wasm32"))]
        {
            &self.config.name
        }
        #[cfg(target_arch = "wasm32")]
        {
            &self.name
        }
    }

    /// Set the blob store used to resolve blob-backed image blocks before
    /// transport send. Comms stays byte-oriented; refs never cross the peer
    /// boundary.
    pub fn set_blob_store(&mut self, blob_store: Arc<dyn BlobStore>) {
        self.blob_store = Some(blob_store);
    }

    #[cfg(test)]
    fn install_peer_comms_handle(&self, handle: Arc<dyn meerkat_core::handles::PeerCommsHandle>) {
        *self.meerkat_machine_trust_owner.write() = None;
        *self.peer_comms_handle.write() = Some(handle);
    }

    fn validate_generated_trust_authority_owner(
        &self,
        authority: &meerkat_core::comms::CommsTrustMutationAuthority,
    ) -> Result<(), SendError> {
        let expected_meerkat = self.meerkat_machine_trust_owner.read();
        let expected_mob = self.mob_machine_trust_owner.read();
        authority
            .validate_target_source_owner_token(expected_meerkat.as_ref(), expected_mob.as_ref())
            .map_err(SendError::Validation)
    }

    /// Mark this runtime as requiring peer-comms machine authority.
    /// Missing `PeerCommsHandle` already fails closed for classified ingress;
    /// this flag remains as an observable session-mode marker for callers.
    pub fn require_peer_comms_machine_authority(&self) {
        self.require_peer_comms_machine_authority
            .store(true, Ordering::SeqCst);
    }

    pub fn peer_comms_machine_authority_required(&self) -> bool {
        self.require_peer_comms_machine_authority
            .load(Ordering::SeqCst)
    }

    pub fn peer_comms_handle(&self) -> Option<Arc<dyn meerkat_core::handles::PeerCommsHandle>> {
        self.peer_comms_handle.read().clone()
    }

    /// Install both machine handles required for semantic peer request/response.
    ///
    /// This is the typed boundary between transport-only comms and semantic
    /// request/response. Without this authority, `PeerRequest` and
    /// `PeerResponse` sends fail closed before transport send or receipt
    /// emission.
    pub fn install_peer_request_response_authority(
        self: &Arc<Self>,
        authority: PeerRequestResponseAuthority,
    ) {
        self.install_peer_interaction_handle(authority.peer_interaction);
        self.install_interaction_stream_handle(authority.interaction_stream);
    }

    /// Install the session's peer-interaction DSL handle (W1-A).
    ///
    /// Called by the surface after the session's `SessionRuntimeBindings`
    /// land. Two things happen here:
    ///
    /// 1. The runtime stores the handle so outbound `PeerRequest` sends
    ///    fire `PeerInteractionHandle::request_sent` (DSL authority).
    /// 2. The runtime registers *itself* as the handle's cleanup observer,
    ///    so every DSL `PeerInteractionCleanup` effect drops the matching
    ///    entries in `subscriber_registry` / `interaction_stream_registry`.
    ///    That closes the "terminal transition → effect → cleanup" causal
    ///    chain — the shell registries are a projection of DSL truth, not
    ///    shadow state updated by lexical adjacency.
    pub fn install_peer_interaction_handle(
        self: &Arc<Self>,
        handle: Arc<dyn meerkat_core::handles::PeerInteractionHandle>,
    ) {
        let observer: Arc<dyn meerkat_core::handles::PeerInteractionCleanupObserver> =
            Arc::clone(self) as Arc<dyn meerkat_core::handles::PeerInteractionCleanupObserver>;
        handle.install_cleanup_observer(observer);
        *self.peer_interaction_handle.write() = Some(handle);
    }

    /// Read-side accessor for the installed peer-interaction handle.
    pub fn peer_interaction_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::handles::PeerInteractionHandle>> {
        self.peer_interaction_handle.read().clone()
    }

    /// Install the session's interaction-stream lifecycle DSL handle (U6).
    ///
    /// Two things happen here:
    ///
    /// 1. The runtime stores the handle so machine-authoritative reservation,
    ///    attach, completion, expiry, and consumer-drop transitions flow
    ///    through the DSL.
    /// 2. The runtime registers itself as the handle's cleanup observer, so
    ///    every `InteractionStreamCleanup` effect drops the matching entry
    ///    in `interaction_stream_registry` (and the paired
    ///    `subscriber_registry` entry under the same lock).
    ///
    /// For machine-authoritative entries, the registry holds channels only and
    /// lifecycle truth (Reserved/Attached/Completed/Expired/ClosedEarly) lives
    /// in the DSL. Local input streams stay transport-only.
    pub fn install_interaction_stream_handle(
        self: &Arc<Self>,
        handle: Arc<dyn meerkat_core::handles::InteractionStreamHandle>,
    ) {
        let observer: Arc<dyn meerkat_core::handles::InteractionStreamCleanupObserver> =
            Arc::clone(self) as Arc<dyn meerkat_core::handles::InteractionStreamCleanupObserver>;
        handle.install_cleanup_observer(observer);
        *self.interaction_stream_handle.write() = Some(handle);
    }

    /// Read-side accessor for the installed interaction-stream handle.
    pub fn interaction_stream_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::handles::InteractionStreamHandle>> {
        self.interaction_stream_handle.read().clone()
    }

    fn require_peer_interaction_authority(
        &self,
        command: &'static str,
    ) -> Result<Arc<dyn meerkat_core::handles::PeerInteractionHandle>, SendError> {
        self.peer_interaction_handle().ok_or_else(|| {
            SendError::Validation(format!(
                "{command} requires machine peer interaction authority; this CommsRuntime is transport-only for semantic peer request/response"
            ))
        })
    }

    fn require_interaction_stream_authority(
        &self,
        command: &'static str,
    ) -> Result<Arc<dyn meerkat_core::handles::InteractionStreamHandle>, SendError> {
        self.interaction_stream_handle().ok_or_else(|| {
            SendError::Validation(format!(
                "{command} requires machine interaction stream authority; this CommsRuntime is transport-only for semantic peer request/response streams"
            ))
        })
    }

    fn require_peer_request_response_authority(
        &self,
        command: &'static str,
    ) -> Result<PeerRequestResponseAuthorityHandles, SendError> {
        let peer_handle = self.require_peer_interaction_authority(command)?;
        let stream_handle = self.require_interaction_stream_authority(command)?;
        Ok((peer_handle, stream_handle))
    }

    fn peer_request_response_sendable_kinds(&self) -> Vec<PeerSendability> {
        let mut kinds = vec![PeerSendability::PeerMessage];
        if self.peer_interaction_handle().is_some() && self.interaction_stream_handle().is_some() {
            kinds.push(PeerSendability::PeerRequest);
            kinds.push(PeerSendability::PeerResponse);
        }
        kinds
    }

    async fn hydrate_message_kind_for_transport(
        &self,
        kind: crate::types::MessageKind,
    ) -> Result<crate::types::MessageKind, SendError> {
        match kind {
            crate::types::MessageKind::Message {
                body,
                blocks: Some(mut blocks),
                content_taint,
                handling_mode,
                objective_id,
            } => {
                self.hydrate_blocks_for_transport(&mut blocks, "comms message")
                    .await?;
                Ok(crate::types::MessageKind::Message {
                    body,
                    blocks: Some(blocks),
                    content_taint,
                    handling_mode,
                    objective_id,
                })
            }
            crate::types::MessageKind::IncarnationFencedMessage {
                body,
                blocks: Some(mut blocks),
                content_taint,
                handling_mode,
                objective_id,
                expected_recipient,
            } => {
                self.hydrate_blocks_for_transport(&mut blocks, "incarnation-fenced comms message")
                    .await?;
                Ok(crate::types::MessageKind::IncarnationFencedMessage {
                    body,
                    blocks: Some(blocks),
                    content_taint,
                    handling_mode,
                    objective_id,
                    expected_recipient,
                })
            }
            crate::types::MessageKind::Request {
                intent,
                params,
                blocks: Some(mut blocks),
                reply_endpoint,
                content_taint,
                handling_mode,
                objective_id,
            } => {
                self.hydrate_blocks_for_transport(&mut blocks, "comms request")
                    .await?;
                Ok(crate::types::MessageKind::Request {
                    intent,
                    params,
                    blocks: Some(blocks),
                    reply_endpoint,
                    content_taint,
                    handling_mode,
                    objective_id,
                })
            }
            crate::types::MessageKind::Response {
                in_reply_to,
                status,
                result,
                blocks: Some(mut blocks),
                content_taint,
                handling_mode,
                objective_id,
            } => {
                self.hydrate_blocks_for_transport(&mut blocks, "comms response")
                    .await?;
                Ok(crate::types::MessageKind::Response {
                    in_reply_to,
                    status,
                    result,
                    blocks: Some(blocks),
                    content_taint,
                    handling_mode,
                    objective_id,
                })
            }
            other => Ok(other),
        }
    }

    async fn hydrate_blocks_for_transport(
        &self,
        blocks: &mut [meerkat_core::types::ContentBlock],
        label: &str,
    ) -> Result<(), SendError> {
        if !blocks.iter().any(|block| {
            matches!(
                block,
                meerkat_core::types::ContentBlock::Image {
                    data: meerkat_core::types::ImageData::Blob { .. },
                    ..
                }
            )
        }) {
            return Ok(());
        }
        let blob_store = self.blob_store.as_ref().ok_or_else(|| {
            SendError::Internal(format!("blob-backed {label} requires blob store"))
        })?;
        hydrate_content_blocks(blob_store.as_ref(), blocks, MissingBlobBehavior::Error)
            .await
            .map_err(|err| SendError::Internal(err.to_string()))
    }

    pub fn inproc_namespace(&self) -> Option<&str> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.config.inproc_namespace.as_deref()
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.inproc_namespace.as_deref()
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn advertised_address(&self) -> String {
        if let Some(ref advertised) = self.config.advertise_address {
            return advertised.clone();
        }
        if let Some(ref uds) = self.config.listen_uds {
            return format!("uds://{}", uds.display());
        }
        if let Some(ref tcp) = self.config.listen_tcp {
            return format!("tcp://{tcp}");
        }
        format!("inproc://{}", self.config.name)
    }

    /// Actual TCP socket address bound by the signed peer listener.
    ///
    /// This is intentionally distinct from [`Self::advertised_address`]: an
    /// explicit advertised address may name a public/NAT route while this
    /// accessor reports the local kernel-selected socket (including the real
    /// ephemeral port after binding `:0`). Returns `None` before listener
    /// start, when no signed TCP listener is configured, or after its handle
    /// has been removed.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn bound_tcp_listener_address(&self) -> Option<std::net::SocketAddr> {
        self.listener_handles
            .lock()
            .iter()
            .find_map(|handle| handle.local_addr)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn validate_listener_start_config(&self) -> Result<(), CommsRuntimeError> {
        if let Some(listen_tcp) = self.config.listen_tcp {
            if let Some(secret) = self.config.pairing_password.as_deref() {
                validate_pairing_secret(secret)?;
            }
            if let Some(advertise_address) = self.config.advertise_address.as_deref() {
                parse_peer_address(advertise_address).map_err(|error| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("invalid comms advertised address '{advertise_address}': {error}"),
                    )
                })?;
            } else if listen_tcp.ip().is_unspecified() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "signed comms listener bound to a wildcard address requires an explicit advertised address",
                )
                .into());
            }
        }

        if self.config.auth == meerkat_core::CommsAuthMode::Open
            && let Some(addr) = &self.config.event_listen_tcp
            && !addr.ip().is_loopback()
            && !self.config.allow_external_unauthenticated
        {
            return Err(CommsRuntimeError::UnsafeBinding(
                "Plain event listener on non-loopback address is a prompt injection \
                 vector; set allow_external_unauthenticated=true to override"
                    .to_string(),
            ));
        }
        Ok(())
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    fn inject_listener_start_failure_after_signed_tcp(
        &self,
        observed_tcp_address: Option<std::net::SocketAddr>,
    ) -> bool {
        let mut slot = LISTENER_START_FAULT_AFTER_SIGNED_TCP
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(fault) = slot.as_mut() else {
            return false;
        };
        if !fault.armed || fault.participant_name != self.config.name {
            return false;
        }
        fault.armed = false;
        fault.observed_tcp_address = observed_tcp_address;
        true
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    async fn pause_listener_start_after_signed_tcp(
        &self,
        observed_tcp_address: std::net::SocketAddr,
    ) {
        let pause = {
            let mut slot = LISTENER_START_PAUSE_AFTER_SIGNED_TCP
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if slot
                .as_ref()
                .is_some_and(|pause| pause.participant_name == self.config.name)
            {
                slot.take()
            } else {
                None
            }
        };

        if let Some(mut pause) = pause {
            if let Some(reached) = pause.reached.take() {
                let _ = reached.send(observed_tcp_address);
            }
            if let Some(release) = pause.release.take() {
                let _ = release.await;
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub async fn start_listeners(&mut self) -> Result<(), CommsRuntimeError> {
        if self.listeners_started {
            return Err(CommsRuntimeError::AlreadyStarted);
        }
        self.validate_listener_start_config()?;

        let inbox_sender = self.router.inbox_sender().clone();
        let router = self.router.clone();
        let max_line_length = self.config.comms_config.max_message_bytes as usize;
        let mut staged = StagedListenerHandles::new();
        let mut resolved_listen_tcp = self.config.listen_tcp;

        // === Signed (Ed25519) listeners — ALWAYS run for agent-to-agent comms ===
        #[cfg(unix)]
        if let Some(ref path) = self.config.listen_uds {
            let handle = match spawn_uds_listener(
                path,
                self.keypair.clone(),
                self.require_peer_auth,
                inbox_sender.clone(),
            )
            .await
            {
                Ok(handle) => handle,
                Err(error) => {
                    staged.abort_and_wait().await;
                    return Err(error.into());
                }
            };
            staged.push(handle);
        }

        if let Some(ref addr) = self.config.listen_tcp {
            let handle = match spawn_tcp_listener(TcpListenerConfig {
                addr: addr.to_string(),
                keypair: self.keypair.clone(),
                router,
                trusted: self.trusted_peers.clone(),
                require_peer_auth: self.require_peer_auth,
                inbox_sender: inbox_sender.clone(),
                pairing_password: self.config.pairing_password.clone(),
                advertise_address: self.config.advertise_address.clone(),
                trusted_peers_path: self.config.trusted_peers_path.clone(),
                participant_name: self.config.name.clone(),
                bridge_bootstrap_token: self.bridge_bootstrap_token.clone(),
            })
            .await
            {
                Ok(handle) => handle,
                Err(error) => {
                    staged.abort_and_wait().await;
                    return Err(error.into());
                }
            };
            if let Some(local_addr) = handle.local_addr {
                resolved_listen_tcp = Some(local_addr);
            }
            staged.push(handle);

            #[cfg(test)]
            if let Some(resolved_listen_tcp) = resolved_listen_tcp {
                self.pause_listener_start_after_signed_tcp(resolved_listen_tcp)
                    .await;
            }

            #[cfg(test)]
            if self.inject_listener_start_failure_after_signed_tcp(resolved_listen_tcp) {
                staged.abort_and_wait().await;
                return Err(std::io::Error::other(
                    "fault-injected listener startup failure after signed TCP bind",
                )
                .into());
            }
        }

        // === Plain event listeners — run ADDITIONALLY when auth=Open ===
        if self.config.auth == meerkat_core::CommsAuthMode::Open {
            if let Some(ref addr) = self.config.event_listen_tcp {
                let handle = match spawn_plain_tcp_listener(
                    &addr.to_string(),
                    inbox_sender.clone(),
                    max_line_length,
                )
                .await
                {
                    Ok(handle) => handle,
                    Err(error) => {
                        staged.abort_and_wait().await;
                        return Err(error.into());
                    }
                };
                staged.push(handle);
            }

            #[cfg(unix)]
            if let Some(ref path) = self.config.event_listen_uds {
                let handle =
                    match spawn_plain_uds_listener(path, inbox_sender.clone(), max_line_length)
                        .await
                    {
                        Ok(handle) => handle,
                        Err(error) => {
                            staged.abort_and_wait().await;
                            return Err(error.into());
                        }
                    };
                staged.push(handle);
            }
        }

        self.config.listen_tcp = resolved_listen_tcp;
        self.listener_handles.lock().extend(staged.commit());
        self.listeners_started = true;
        Ok(())
    }

    pub fn public_key(&self) -> PubKey {
        self.public_key
    }

    pub fn bridge_bootstrap_token(&self) -> &str {
        &self.bridge_bootstrap_token
    }

    pub fn router(&self) -> &Router {
        &self.router
    }

    /// Install (or clear) the host-owned outbound content-taint declaration.
    ///
    /// Host-set config, not machine state: the declaration is stamped onto
    /// every content-bearing envelope at the router's single outbound choke
    /// point unless a per-send [`meerkat_core::comms::SendTaintOverride`]
    /// says otherwise. `None` clears the declaration (subsequent sends carry
    /// no taint field).
    pub fn set_outbound_content_taint(
        &self,
        taint: Option<meerkat_core::comms::SenderContentTaint>,
    ) {
        self.router.set_outbound_content_taint(taint);
    }

    /// The currently-installed host-owned outbound content-taint declaration.
    pub fn outbound_content_taint(&self) -> Option<meerkat_core::comms::SenderContentTaint> {
        self.router.outbound_content_taint()
    }

    fn trusted_peer_descriptors(&self, include_private: bool) -> Vec<TrustedPeerDescriptor> {
        self.trusted_peer_descriptors_filtered(include_private, None)
    }

    fn trusted_peer_descriptors_for_source(
        &self,
        include_private: bool,
        source_kind: GeneratedCommsTrustAuthoritySourceKind,
    ) -> Vec<TrustedPeerDescriptor> {
        self.trusted_peer_descriptors_filtered(include_private, Some(source_kind))
    }

    fn trusted_peer_descriptors_filtered(
        &self,
        include_private: bool,
        source_kind: Option<GeneratedCommsTrustAuthoritySourceKind>,
    ) -> Vec<TrustedPeerDescriptor> {
        let peer_rows: Vec<(meerkat_core::comms::PeerId, TrustEntry)> =
            if let Some(source_kind) = source_kind {
                self.router.trusted_peers_for_source(source_kind)
            } else {
                self.trusted_peers
                    .read()
                    .entries()
                    .map(|entry| (entry.peer_id, entry.clone()))
                    .collect()
            };
        // Trust entries are typed, validated material: zero pubkeys, invalid
        // names, and invalid addresses are structurally impossible here.
        let mut trusted_peers: Vec<TrustedPeerDescriptor> = peer_rows
            .into_iter()
            .filter_map(|(peer_id, entry)| {
                if !include_private && self.router.is_private_peer_id(&peer_id) {
                    return None;
                }
                Some(TrustedPeerDescriptor {
                    peer_id,
                    name: entry.name,
                    address: entry.address,
                    pubkey: *entry.pubkey.as_bytes(),
                })
            })
            .collect();
        trusted_peers.sort_by(|left, right| {
            left.name
                .as_str()
                .cmp(right.name.as_str())
                .then_with(|| left.peer_id.cmp(&right.peer_id))
                .then_with(|| left.address.to_string().cmp(&right.address.to_string()))
        });
        trusted_peers
    }

    async fn apply_trusted_peer_descriptor(
        &self,
        descriptor: TrustedPeerDescriptor,
        source_kind: GeneratedCommsTrustAuthoritySourceKind,
    ) -> Result<bool, SendError> {
        let entry = descriptor_to_trust_entry(descriptor)?;
        let created = self
            .router
            .add_trusted_peer_for_source(entry, source_kind, false)
            .map_err(|err| SendError::Validation(err.to_string()))?;
        Ok(created)
    }

    async fn apply_trusted_peer_removal(
        &self,
        peer_id: &str,
        source_kind: GeneratedCommsTrustAuthoritySourceKind,
    ) -> Result<bool, SendError> {
        let peer_id = meerkat_core::comms::PeerId::parse(peer_id)
            .map_err(|err| SendError::Validation(err.to_string()))?;
        if self.router.is_private_peer_id(&peer_id)
            && !self.router.has_trust_source(&peer_id, source_kind)
        {
            return Err(SendError::Validation(format!(
                "public trust authority cannot remove private trusted peer {peer_id}"
            )));
        }
        Ok(self
            .router
            .remove_trusted_peer_for_source(&peer_id, source_kind))
    }

    async fn apply_private_trusted_peer_descriptor(
        &self,
        descriptor: TrustedPeerDescriptor,
        source_kind: GeneratedCommsTrustAuthoritySourceKind,
    ) -> Result<bool, SendError> {
        let entry = descriptor_to_trust_entry(descriptor)?;
        let created = self
            .router
            .add_trusted_peer_for_source(entry, source_kind, true)
            .map_err(|err| SendError::Validation(err.to_string()))?;
        Ok(created)
    }

    async fn apply_private_trusted_peer_removal(
        &self,
        peer_id: &str,
        source_kind: GeneratedCommsTrustAuthoritySourceKind,
    ) -> Result<bool, SendError> {
        let peer_id = meerkat_core::comms::PeerId::parse(peer_id)
            .map_err(|err| SendError::Validation(err.to_string()))?;
        Ok(self
            .router
            .remove_trusted_peer_for_source(&peer_id, source_kind))
    }

    /// Update the inproc registry entry with friendly metadata.
    ///
    /// This is a published-runtime operation. The exact publication lease
    /// updates metadata in place; it never re-registers the route or changes
    /// the inbox generation owned by Drop.
    pub fn set_peer_meta(&self, meta: crate::PeerMeta) {
        let namespace = self.inproc_namespace().unwrap_or("");
        let name = self.participant_name();
        let Some(sender) = self.inproc_sender.as_ref() else {
            return;
        };
        if !InprocRegistry::global().update_meta_for_sender_in_namespace(
            namespace,
            name,
            &self.public_key,
            sender,
            meta,
        ) {
            tracing::warn!(
                inproc_namespace = %namespace,
                peer_name = %name,
                "inproc metadata refresh lost its exact inbox generation"
            );
        }
    }

    /// Canonical peer resolver.
    ///
    /// Discovery and sendability are derived from trusted peers, with in-proc peers
    /// included when auth is disabled.
    async fn resolve_peer_directory(&self) -> Vec<PeerDirectoryEntry> {
        let resolved = self.resolved_peers_snapshot().await;
        let sendable_kinds = self.peer_request_response_sendable_kinds();
        let mut peers = Vec::new();
        for peer in resolved {
            peers.push(PeerDirectoryEntry {
                name: peer.name,
                peer_id: peer.peer_id,
                address: peer.address,
                source: peer.source,
                sendable_kinds: sendable_kinds.clone(),
                capabilities: PeerCapabilitySet::default(),
                meta: peer.meta,
            });
        }
        peers
    }

    /// Resolve peer count using the same filtering/dedup rules as
    /// `resolve_peer_directory`, without materializing directory entries.
    async fn resolve_peer_count(&self) -> usize {
        self.resolved_peers_snapshot().await.len()
    }

    async fn resolved_peers_snapshot(&self) -> Vec<ResolvedPeer> {
        let mut peers = Vec::new();
        self.for_each_resolved_peer(|peer| peers.push(peer)).await;
        peers
    }

    async fn for_each_resolved_peer<F>(&self, mut on_peer: F) -> usize
    where
        F: FnMut(ResolvedPeer),
    {
        let participant_name = self.participant_name().to_string();
        let private_peer_ids = self.router.private_peer_ids();
        let mut emitted = 0usize;

        {
            // Trust entries are typed, validated material keyed by canonical
            // PeerId: duplicate identities, zero pubkeys, and invalid names
            // or addresses are structurally impossible here.
            let trusted = self.trusted_peers.read();
            for entry in trusted.entries() {
                if entry.name.as_str() == participant_name || entry.pubkey == self.public_key {
                    continue;
                }
                if private_peer_ids.contains(&entry.peer_id) {
                    continue;
                }
                on_peer(ResolvedPeer {
                    name: entry.name.clone(),
                    peer_id: entry.peer_id,
                    address: entry.address.clone(),
                    source: PeerDirectorySource::Trusted,
                    meta: entry.meta.clone(),
                });
                emitted += 1;
            }
        }

        emitted
    }

    /// Canonical send path: destination is a typed [`PeerRoute`] whose
    /// [`PeerId`] is already resolved at the surface boundary.
    async fn send_peer_command(
        &self,
        route: &PeerRoute,
        kind: crate::types::MessageKind,
        content_taint: Option<meerkat_core::comms::SendTaintOverride>,
    ) -> Result<crate::router::SendOutcome, SendError> {
        self.send_peer_command_with_id(route, Uuid::new_v4(), kind, content_taint)
            .await
    }

    /// Send a peer command and return the typed delivery outcome.
    ///
    /// The returned [`crate::router::SendOutcome`] carries the strongest typed
    /// delivery fact proved by the selected transport, so public receipts do
    /// not collapse ACK, direct handoff, and queued-write authority.
    ///
    /// `content_taint` is the per-send tri-state taint override, resolved
    /// against the runtime-level declaration at the router's single outbound
    /// choke point ([`crate::router::Router::send_with_id`]).
    async fn send_peer_command_with_id(
        &self,
        route: &PeerRoute,
        envelope_id: Uuid,
        kind: crate::types::MessageKind,
        content_taint: Option<meerkat_core::comms::SendTaintOverride>,
    ) -> Result<crate::router::SendOutcome, SendError> {
        let dest_peer_id = route.peer_id;
        let kind = self.hydrate_message_kind_for_transport(kind).await?;
        let result = self
            .router
            .send_with_id(dest_peer_id, envelope_id, kind, content_taint)
            .await;
        match result {
            Ok(outcome) => Ok(outcome),
            Err(crate::router::SendError::PeerNotFound(peer_id)) => {
                Err(SendError::PeerNotFound(peer_id.to_string()))
            }
            Err(crate::router::SendError::PeerOffline) => Err(SendError::PeerOffline),
            Err(crate::router::SendError::AdmissionDropped { reason }) => {
                // Transport worked; the receiver's ingress admission policy
                // rejected us. Surface the typed reason all the way to the
                // public API error payload so clients see e.g.
                // `untrusted_sender` rather than the old `PeerOffline` lie.
                Err(SendError::AdmissionDropped {
                    reason: reason.into(),
                })
            }
            Err(
                error @ (crate::router::SendError::Transport(_) | crate::router::SendError::Io(_)),
            ) => Err(SendError::Transport(error.to_string())),
        }
    }

    /// Register a `Waiting` bridge-reply waiter for `envelope_id`.
    ///
    /// Evicts expired tombstones first (lazy eviction at registry writes),
    /// then enforces the bounded capacity with a typed error.
    #[cfg(not(target_arch = "wasm32"))]
    fn insert_bridge_reply_waiter(
        &self,
        envelope_id: Uuid,
        sender: tokio::sync::oneshot::Sender<meerkat_core::interaction::PeerInputCandidate>,
    ) -> Result<(), SendError> {
        let mut waiters = self.bridge_reply_waiters.lock();
        let now = Instant::now();
        waiters.retain(|_, entry| match entry {
            BridgeReplyWaiterEntry::Waiting(_) => true,
            BridgeReplyWaiterEntry::TimedOut(at) => {
                now.duration_since(*at) < BRIDGE_REPLY_TOMBSTONE_TTL
            }
        });
        if waiters.len() >= BRIDGE_REPLY_WAITER_CAPACITY {
            return Err(SendError::Validation(format!(
                "bridge reply waiter registry at capacity ({BRIDGE_REPLY_WAITER_CAPACITY})"
            )));
        }
        waiters.insert(envelope_id, BridgeReplyWaiterEntry::Waiting(sender));
        Ok(())
    }

    /// Remove a waiter entry outright (send failure: the envelope never left,
    /// so no late reply can arrive and no tombstone is needed).
    #[cfg(not(target_arch = "wasm32"))]
    fn remove_bridge_reply_waiter(&self, envelope_id: Uuid) {
        self.bridge_reply_waiters.lock().remove(&envelope_id);
    }

    /// Replace a `Waiting` waiter with a `TimedOut` tombstone after the
    /// requester's attempt budget expired. The tombstone keeps a typed
    /// disposition for the late reply — the drain consumes and discards it
    /// instead of injecting bridge JSON into the session. Tombstones are
    /// removed on take or lazily evicted after
    /// [`BRIDGE_REPLY_TOMBSTONE_TTL`] at the next registry write.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn tombstone_bridge_reply_waiter(&self, envelope_id: Uuid) {
        if let Some(entry) = self.bridge_reply_waiters.lock().get_mut(&envelope_id) {
            *entry = BridgeReplyWaiterEntry::TimedOut(Instant::now());
        }
    }

    /// Canonical peer-Request lifecycle shared by the public command path,
    /// send-and-stream, and the explicit reply-endpoint helpers.
    ///
    /// The caller supplies the interaction id so it can reserve a waiter
    /// before dispatch. This owner performs the machine `request_sent`
    /// transition, optional stream reservation, signed envelope construction,
    /// transport send, and failure cleanup exactly once for every peer Request.
    #[allow(clippy::too_many_arguments)]
    async fn send_peer_request_with_id_lifecycle(
        &self,
        interaction_id: Uuid,
        to: &PeerRoute,
        intent: String,
        params: serde_json::Value,
        blocks: Option<Vec<meerkat_core::types::ContentBlock>>,
        content_taint: Option<meerkat_core::comms::SendTaintOverride>,
        handling_mode: meerkat_core::types::HandlingMode,
        stream_reserved: bool,
        objective_id: Option<meerkat_core::interaction::ObjectiveId>,
        reply_endpoint: Option<meerkat_core::comms::PeerAddress>,
    ) -> Result<SendReceipt, SendError> {
        if reply_endpoint
            .as_ref()
            .is_some_and(|endpoint| crate::classify::declared_tcp_port(endpoint).is_none())
        {
            return Err(SendError::Validation(
                "peer Request reply endpoint must use tcp:// with a valid nonzero port".to_string(),
            ));
        }

        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(interaction_id);
        let (peer_handle, stream_authority) =
            self.require_peer_request_response_authority("PeerRequest")?;
        let stream_handle = stream_reserved.then_some(stream_authority);

        if let Err(err) = peer_handle.request_sent(corr_id) {
            tracing::warn!(
                error = %err,
                corr_id = %corr_id,
                "PeerInteractionHandle::request_sent rejected — refusing to send"
            );
            return Err(SendError::Validation(format!(
                "DSL rejected PeerRequestSent for corr_id {corr_id}: {err}"
            )));
        }

        if let Some(stream_handle) = stream_handle
            && let Err(err) = self.reserve_interaction_stream_channels(
                corr_id,
                interaction_id,
                InteractionStreamReservationAuthority::Machine(stream_handle),
            )
        {
            let _ = peer_handle.request_send_failed(corr_id);
            return Err(err);
        }

        let envelope_id = match self
            .send_peer_command_with_id(
                to,
                interaction_id,
                crate::types::MessageKind::Request {
                    intent,
                    params,
                    blocks,
                    reply_endpoint,
                    content_taint: None,
                    handling_mode: Some(handling_mode),
                    objective_id,
                },
                content_taint,
            )
            .await
        {
            Ok(outcome) => outcome.envelope_id,
            Err(error) => {
                if stream_reserved {
                    self.abandon_interaction_stream(
                        interaction_id,
                        interaction_stream_abandon_reason_for_send_error(&error),
                    );
                }
                // A failed transport send is not a timeout. Preserve the typed
                // error and drive the canonical machine send-failure edge.
                let _ = peer_handle.request_send_failed(corr_id);
                return Err(error);
            }
        };

        Ok(SendReceipt::PeerRequestSent {
            envelope_id,
            interaction_id: meerkat_core::InteractionId(interaction_id),
            stream_reserved,
        })
    }

    /// Send a standard peer Request whose exact Response should return to the
    /// supplied signed socket endpoint. No session-drain waiter is registered;
    /// callers that own their own drain/wait loop use the returned correlation
    /// id from [`SendReceipt::PeerRequestSent`].
    pub async fn send_peer_request_at_endpoint(
        &self,
        to: PeerRoute,
        intent: &str,
        params: serde_json::Value,
        reply_endpoint: meerkat_core::comms::PeerAddress,
    ) -> Result<SendReceipt, SendError> {
        self.send_peer_request_with_id_lifecycle(
            Uuid::new_v4(),
            &to,
            intent.to_string(),
            params,
            None,
            None,
            meerkat_core::types::HandlingMode::Queue,
            false,
            None,
            Some(reply_endpoint),
        )
        .await
    }

    /// Send a peer Request and register a one-shot reply waiter for its
    /// terminal Response BEFORE dispatch (member upcall lane).
    ///
    /// The waiter is registered before the envelope goes out so a response
    /// racing the send return still finds it; the comms drain delivers the
    /// terminal Response candidate to the returned receiver instead of
    /// injecting it into the session. The peer-interaction DSL lifecycle is
    /// identical to the `CommsCommand::PeerRequest` arm (`request_sent`
    /// before dispatch, `request_send_failed` + waiter removal on a failed
    /// send). Returns the receiver and the envelope id (also the
    /// `in_reply_to` correlation key and the tombstone key for
    /// [`Self::tombstone_bridge_reply_waiter`]).
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn send_peer_request_with_reply_waiter(
        &self,
        to: PeerRoute,
        intent: &str,
        params: serde_json::Value,
    ) -> Result<
        (
            tokio::sync::oneshot::Receiver<meerkat_core::interaction::PeerInputCandidate>,
            Uuid,
        ),
        SendError,
    > {
        self.send_peer_request_with_reply_waiter_inner(to, intent, params, None)
            .await
    }

    /// Waiter-registering variant of [`Self::send_peer_request_at_endpoint`].
    /// The signed endpoint is carried on the Request while the terminal
    /// Response is intercepted by the runtime's bridge waiter registry.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn send_peer_request_with_reply_waiter_at_endpoint(
        &self,
        to: PeerRoute,
        intent: &str,
        params: serde_json::Value,
        reply_endpoint: meerkat_core::comms::PeerAddress,
    ) -> Result<
        (
            tokio::sync::oneshot::Receiver<meerkat_core::interaction::PeerInputCandidate>,
            Uuid,
        ),
        SendError,
    > {
        self.send_peer_request_with_reply_waiter_inner(to, intent, params, Some(reply_endpoint))
            .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn send_peer_request_with_reply_waiter_inner(
        &self,
        to: PeerRoute,
        intent: &str,
        params: serde_json::Value,
        reply_endpoint: Option<meerkat_core::comms::PeerAddress>,
    ) -> Result<
        (
            tokio::sync::oneshot::Receiver<meerkat_core::interaction::PeerInputCandidate>,
            Uuid,
        ),
        SendError,
    > {
        let interaction_id = Uuid::new_v4();

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.insert_bridge_reply_waiter(interaction_id, reply_tx)?;

        match self
            .send_peer_request_with_id_lifecycle(
                interaction_id,
                &to,
                intent.to_string(),
                params,
                None,
                None,
                meerkat_core::types::HandlingMode::Queue,
                false,
                None,
                reply_endpoint,
            )
            .await
        {
            Ok(SendReceipt::PeerRequestSent { envelope_id, .. }) => Ok((reply_rx, envelope_id)),
            Ok(other) => {
                self.remove_bridge_reply_waiter(interaction_id);
                Err(SendError::Internal(format!(
                    "peer Request lifecycle returned unexpected receipt {other:?}"
                )))
            }
            Err(error) => {
                // A send failure is NOT a timeout: remove (not tombstone) the
                // waiter — the envelope never left, so no late reply exists —
                // and return the typed error verbatim.
                self.remove_bridge_reply_waiter(interaction_id);
                Err(error)
            }
        }
    }

    /// Drop the peer-response subscriber and correlated callback projections
    /// for a correlation id.
    ///
    /// Invoked from the `PeerInteractionCleanup` DSL effect observer
    /// (W1-A): the DSL has already decided the peer-interaction lifecycle
    /// is terminal. The independently-owned interaction-stream registry is
    /// deliberately untouched: it terminates only through an
    /// `InteractionStream*` transition and its cleanup effect.
    fn drop_peer_interaction_projection(&self, interaction_id: Uuid) {
        let removed_callbacks = self
            .router
            .unstage_correlated_reply_endpoints_for_interaction(interaction_id);
        let removed_sender = self.subscriber_registry.lock().remove(&interaction_id);
        if removed_sender.is_some() || removed_callbacks != 0 {
            tracing::debug!(
                interaction_id = %interaction_id,
                removed_callbacks,
                "peer interaction projection dropped via DSL cleanup effect"
            );
        }
    }

    /// Mark an interaction stream as completed (terminal event received).
    ///
    /// Machine-authoritative streams complete through the DSL `is_attached`
    /// guard: if the consumer already closed the stream, the transition is
    /// rejected and this call is a no-op. Local transport-only streams have no
    /// semantic lifecycle twin, so completion drops their channel projection.
    pub fn mark_interaction_complete(&self, interaction_id: Uuid) {
        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(interaction_id);
        let lifecycle_authority = self
            .interaction_stream_registry
            .lock()
            .get(&interaction_id)
            .map(|entry| entry.lifecycle_authority);
        match lifecycle_authority {
            Some(InteractionStreamLifecycleAuthority::Machine) => {
                if let Some(handle) = self.interaction_stream_handle()
                    && let Err(err) = handle.completed(corr_id)
                {
                    tracing::trace!(
                        interaction_id = %interaction_id,
                        error = %err,
                        "InteractionStreamHandle::completed rejected (likely closed-early won race)"
                    );
                }
            }
            Some(InteractionStreamLifecycleAuthority::LocalTransportOnly) | None => {
                // Transport-only local stream: drop the channel and
                // subscriber entries directly. Without a DSL twin there's no
                // CAS, so double-completion is a benign no-op against the maps.
                self.subscriber_registry.lock().remove(&interaction_id);
                self.interaction_stream_registry
                    .lock()
                    .remove(&interaction_id);
            }
        }
    }

    /// Abandon an active interaction stream for an observed typed failure.
    ///
    /// This is distinct from attach-TTL expiry and peer request timeout. The
    /// machine-owned path drives the generated terminal transition; the
    /// explicit transport-only boundary removes its local channel projection.
    pub fn abandon_interaction_stream(
        &self,
        interaction_id: Uuid,
        reason: meerkat_core::InteractionStreamAbandonReason,
    ) {
        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(interaction_id);
        let lifecycle_authority = self
            .interaction_stream_registry
            .lock()
            .get(&interaction_id)
            .map(|entry| entry.lifecycle_authority);
        match lifecycle_authority {
            Some(InteractionStreamLifecycleAuthority::Machine) => {
                if let Some(handle) = self.interaction_stream_handle()
                    && let Err(err) = handle.abandoned(corr_id, reason)
                {
                    tracing::trace!(
                        %interaction_id,
                        ?reason,
                        error = %err,
                        "InteractionStreamHandle::abandoned rejected (another terminal won race)"
                    );
                }
            }
            Some(InteractionStreamLifecycleAuthority::LocalTransportOnly) => {
                self.interaction_stream_registry
                    .lock()
                    .remove(&interaction_id);
                self.subscriber_registry.lock().remove(&interaction_id);
            }
            None => {}
        }
    }

    fn take_interaction_stream_abandonment(
        &self,
        interaction_id: Uuid,
    ) -> Option<meerkat_core::InteractionStreamAbandonReason> {
        let mut abandonments = self.interaction_stream_abandonments.lock();
        let projection = abandonments.remove(&interaction_id)?;
        (projection.recorded_at.elapsed() <= RESERVATION_TTL).then_some(projection.reason)
    }

    fn interaction_stream_terminal_or_not_reserved(
        &self,
        interaction_id: meerkat_core::InteractionId,
    ) -> StreamError {
        match self.take_interaction_stream_abandonment(interaction_id.0) {
            Some(reason) => StreamError::Abandoned {
                interaction_id,
                reason,
            },
            None => StreamError::NotReserved(interaction_id),
        }
    }

    fn record_interaction_stream_abandonment(
        &self,
        interaction_id: Uuid,
        reason: meerkat_core::InteractionStreamAbandonReason,
    ) {
        if let Some(entry) = self.interaction_stream_registry.lock().get(&interaction_id) {
            *entry.abandonment_signal.lock() = Some(reason);
        }

        let mut abandonments = self.interaction_stream_abandonments.lock();
        abandonments.retain(|_, projection| projection.recorded_at.elapsed() <= RESERVATION_TTL);
        if abandonments.len() >= ABANDONMENT_PROJECTION_CAPACITY
            && let Some(oldest) = abandonments
                .iter()
                .min_by_key(|(_, projection)| projection.recorded_at)
                .map(|(id, _)| *id)
        {
            abandonments.remove(&oldest);
        }
        abandonments.insert(
            interaction_id,
            InteractionStreamAbandonmentProjection {
                reason,
                recorded_at: Instant::now(),
            },
        );
    }

    /// Reap expired reservations that were never attached within the TTL.
    ///
    /// TTL is shell-owned mechanics (`created_at` lives with the channel
    /// projection, not the DSL). Machine-authoritative reservations fire
    /// `InteractionStreamHandle::expired`; local transport-only reservations
    /// remove the unclaimed channel projection directly. Peer request response
    /// deadlines are a separate lifecycle and are not inferred from this
    /// attach-only TTL.
    pub fn reap_expired_reservations(&self) {
        self.interaction_stream_abandonments
            .lock()
            .retain(|_, projection| projection.recorded_at.elapsed() <= RESERVATION_TTL);
        let candidates: Vec<(Uuid, InteractionStreamLifecycleAuthority)> = {
            let registry = self.interaction_stream_registry.lock();
            registry
                .iter()
                .filter_map(|(id, entry)| {
                    (entry.created_at.elapsed() > RESERVATION_TTL)
                        .then_some((*id, entry.lifecycle_authority))
                })
                .collect()
        };
        if candidates.is_empty() {
            return;
        }
        let stream_handle = self.interaction_stream_handle();
        for (id, lifecycle_authority) in candidates {
            let corr_id = meerkat_core::PeerCorrelationId::from_uuid(id);
            // Stream lifecycle: only `Reserved` reservations expire. The
            // DSL guard rejects `Attached` (the stream is live and will
            // complete or close-early on its own).
            match lifecycle_authority {
                InteractionStreamLifecycleAuthority::Machine => {
                    let Some(handle) = stream_handle.as_ref() else {
                        continue;
                    };
                    if handle.state(corr_id) == Some(meerkat_core::InteractionStreamState::Reserved)
                    {
                        tracing::debug!(interaction_id = %id, "reservation expired (TTL)");
                        let _ = handle.expired(corr_id);
                    }
                }
                InteractionStreamLifecycleAuthority::LocalTransportOnly => {
                    // Transport-only local stream: drop the entry if the
                    // receiver is still present (consumer never attached).
                    let mut registry = self.interaction_stream_registry.lock();
                    if let Some(entry) = registry.get(&id)
                        && entry.receiver.is_some()
                    {
                        tracing::debug!(interaction_id = %id, "reservation expired (TTL)");
                        registry.remove(&id);
                        drop(registry);
                        self.subscriber_registry.lock().remove(&id);
                    }
                }
            }
        }
    }

    pub fn router_arc(&self) -> Arc<Router> {
        self.router.clone()
    }
    pub fn trusted_peers_shared(&self) -> TrustedPeersView {
        TrustedPeersView::new(self.trusted_peers.clone())
    }
    pub fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
        self.inbox_notify.clone()
    }

    /// Extract the material needed for comms tool composition.
    ///
    /// The facade uses this to compose comms tools without reaching into
    /// router/trust/key internals directly.
    pub fn tool_material(self: &Arc<Self>) -> CommsToolMaterial {
        CommsToolMaterial {
            router: self.router_arc(),
            trusted_peers: self.trusted_peers_shared(),
            self_pubkey: self.public_key,
            runtime: Arc::clone(self) as Arc<dyn meerkat_core::agent::CommsRuntime>,
        }
    }

    /// Get an event injector for this runtime's inbox.
    pub fn event_injector(&self) -> Arc<dyn meerkat_core::EventInjector> {
        Arc::new(self.event_injector_concrete())
    }

    /// Get the concrete event injector for this runtime's inbox.
    ///
    /// Returns the in-crate [`crate::CommsEventInjector`] so callers can reach
    /// inherent methods (e.g. `inject_with_interaction_id`) that are not part of
    /// the cross-crate `EventInjector` trait surface.
    fn event_injector_concrete(&self) -> crate::CommsEventInjector {
        crate::CommsEventInjector::new(
            self.router.inbox_sender().clone(),
            self.subscriber_registry.clone(),
        )
    }

    #[doc(hidden)]
    pub fn interaction_event_injector(
        &self,
    ) -> Arc<dyn meerkat_core::event_injector::SubscribableInjector> {
        Arc::new(crate::CommsEventInjector::new(
            self.router.inbox_sender().clone(),
            self.subscriber_registry.clone(),
        ))
    }

    fn register_interaction_stream(
        &self,
        interaction_id: Uuid,
        body: String,
        blocks: Option<Vec<meerkat_core::types::ContentBlock>>,
        source: meerkat_core::InputSource,
        handling_mode: meerkat_core::types::HandlingMode,
    ) -> Result<(), SendError> {
        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(interaction_id);
        let authority = self
            .interaction_stream_handle()
            .map(InteractionStreamReservationAuthority::Machine)
            .unwrap_or(InteractionStreamReservationAuthority::LocalTransportOnly);
        self.reserve_interaction_stream_channels(corr_id, interaction_id, authority)?;

        if let crate::inbox::AdmissionOutcome::Dropped { reason } = self
            .router
            .inbox_sender()
            .send_classified(crate::types::InboxItem::PlainEvent {
                body,
                source: PlainEventSource::from(source),
                handling_mode,
                interaction_id: Some(interaction_id),
                blocks,
                render_metadata: None,
                objective_id: None,
            })
        {
            self.abandon_interaction_stream(
                interaction_id,
                meerkat_core::InteractionStreamAbandonReason::AdmissionRejected,
            );
            return Err(match reason {
                crate::inbox::DropReason::InboxFull => {
                    SendError::Validation("input queue full".into())
                }
                crate::inbox::DropReason::SessionClosed => SendError::InputClosed,
                crate::inbox::DropReason::UntrustedSender
                | crate::inbox::DropReason::ClassificationRejected => {
                    SendError::Validation(format!("input rejected at ingress: {reason:?}"))
                }
            });
        }

        Ok(())
    }

    /// Reserve channel slots for an interaction stream.
    ///
    /// Semantic peer request/response reservations pass `Machine` so the
    /// DSL records lifecycle truth before the shell projection is inserted.
    /// Local input streams pass `LocalTransportOnly`, making the direct
    /// channel cleanup boundary explicit instead of acting as a semantic
    /// lifecycle owner.
    fn reserve_interaction_stream_channels(
        &self,
        corr_id: meerkat_core::PeerCorrelationId,
        interaction_id: Uuid,
        authority: InteractionStreamReservationAuthority,
    ) -> Result<(), SendError> {
        let lifecycle_authority = match &authority {
            InteractionStreamReservationAuthority::Machine(_) => {
                InteractionStreamLifecycleAuthority::Machine
            }
            InteractionStreamReservationAuthority::LocalTransportOnly => {
                InteractionStreamLifecycleAuthority::LocalTransportOnly
            }
        };
        match authority {
            InteractionStreamReservationAuthority::Machine(handle) => {
                if let Err(err) = handle.reserved(corr_id) {
                    return Err(SendError::Validation(format!(
                        "DSL rejected InteractionStreamReserved for corr_id {corr_id}: {err}"
                    )));
                }
            }
            InteractionStreamReservationAuthority::LocalTransportOnly => {}
        }
        let (sender, receiver) = mpsc::channel::<meerkat_core::AgentEvent>(4096);
        self.subscriber_registry
            .lock()
            .insert(interaction_id, sender.clone());
        self.interaction_stream_registry.lock().insert(
            interaction_id,
            StreamRegistryEntry::new(sender, receiver, lifecycle_authority),
        );
        Ok(())
    }

    pub async fn drain_messages(&self) -> Vec<CommsMessage> {
        // Drain from the classified queue (sole consumer since 0.4.10).
        // Convert classified entries back to CommsMessage using the
        // ingress-stored metadata (from_peer, class) rather than
        // re-resolving against live trust state. This preserves the
        // ingress-snapshot guarantee: a message accepted at ingress
        // cannot disappear or change from_peer if the peer is
        // removed/renamed before drain.
        let mut inbox = self.inbox.lock().await;
        let entries = inbox.try_drain_classified();
        entries
            .into_iter()
            .filter_map(|entry| CommsMessage::from_classified_entry(&entry))
            .collect()
    }

    pub async fn recv_message(&self) -> Option<CommsMessage> {
        loop {
            // Register the waiter BEFORE draining so a message that arrives
            // between the drain returning empty and the await cannot be lost.
            // inbox_notify uses notify_waiters() which only wakes already-
            // registered listeners, and a `Notified` future is not registered
            // until it is pinned and `enable()`d (or first polled) — so do that
            // here, before the drain, not at the trailing `.await`.
            let mut notified = std::pin::pin!(self.inbox_notify.notified());
            notified.as_mut().enable();
            {
                let mut inbox = self.inbox.lock().await;
                // Consume one entry at a time so back-to-back messages are
                // preserved. The previous implementation drained the entire
                // classified queue and discarded everything after the first
                // decodable message.
                while let Some(entry) = inbox.try_recv_one_classified() {
                    if let Some(msg) = CommsMessage::from_classified_entry(&entry) {
                        return Some(msg);
                    }
                }
            }
            notified.as_mut().await;
        }
    }

    pub fn shutdown(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            for handle in self.listener_handles.lock().drain(..) {
                handle.abort();
            }
            self.listeners_started = false;
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn abort_listeners_for_rebind(&self) {
        for handle in self.listener_handles.lock().drain(..) {
            handle.abort();
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub async fn stop_listeners_for_rebind(&self) {
        let handles = self.listener_handles.lock().drain(..).collect::<Vec<_>>();
        for handle in handles {
            handle.abort_and_wait().await;
        }
    }
}

impl meerkat_core::handles::PeerCommsInstallTarget for CommsRuntime {
    fn install_generated_peer_comms_handle(
        &self,
        install: meerkat_core::handles::GeneratedPeerCommsInstall,
    ) -> Result<(), String> {
        let target_peer_id = self.generated_peer_comms_target_endpoint()?.peer_id;
        if install.target_peer_id() != target_peer_id {
            return Err(format!(
                "generated peer-comms install targets peer_id {} but runtime peer_id is {}",
                install.target_peer_id(),
                target_peer_id
            ));
        }
        let owner_token = install.owner_token();
        let mut expected = self.meerkat_machine_trust_owner.write();
        if let Some(existing) = expected.as_ref()
            && !existing.same_owner(&owner_token)
        {
            return Err(
                "target runtime is already bound to a different generated MeerkatMachine trust owner"
                    .to_string(),
            );
        }
        *expected = Some(owner_token);
        *self.peer_comms_handle.write() = Some(Arc::clone(install.peer_comms_handle()));
        Ok(())
    }
}

impl meerkat_core::handles::PeerInteractionCleanupObserver for CommsRuntime {
    fn on_peer_interaction_cleanup(&self, corr_id: meerkat_core::PeerCorrelationId) {
        // The DSL decided the peer-interaction lifecycle is terminal; the
        // shell registries are a projection — drop the entries keyed on
        // the same correlation id.
        self.drop_peer_interaction_projection(corr_id.as_uuid());
    }
}

impl meerkat_core::handles::InteractionStreamCleanupObserver for CommsRuntime {
    fn on_interaction_stream_cleanup(
        &self,
        corr_id: meerkat_core::PeerCorrelationId,
        abandon_reason: Option<meerkat_core::InteractionStreamAbandonReason>,
    ) {
        // The DSL decided the stream lifecycle is terminal; drop both the
        // channel entry and any paired subscriber sender. When the generated
        // terminal carries an abandonment reason, publish that typed fact to
        // the attached stream signal / bounded late-attach projection before
        // closing the channel. Only this machine effect owns the projection.
        let id = corr_id.as_uuid();
        if let Some(reason) = abandon_reason {
            self.record_interaction_stream_abandonment(id, reason);
        }
        self.subscriber_registry.lock().remove(&id);
        self.interaction_stream_registry.lock().remove(&id);
    }
}

impl Drop for CommsRuntime {
    fn drop(&mut self) {
        self.shutdown();
        if let Some(sender) = self.inproc_sender.as_ref() {
            InprocRegistry::global().unregister_sender_in_namespace(
                self.inproc_namespace().unwrap_or(""),
                &self.public_key,
                sender,
            );
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub struct ListenerHandle {
    handle: JoinHandle<()>,
    local_addr: Option<std::net::SocketAddr>,
}

/// Owns listener tasks until startup commits them to the runtime.
///
/// Dropping a Tokio [`JoinHandle`] detaches its task, so ordinary collection
/// drop is not sufficient when `start_listeners` itself is cancelled. This
/// guard aborts every still-staged task on all non-commit exits. Explicit
/// startup errors additionally await termination through [`Self::abort_and_wait`].
#[cfg(not(target_arch = "wasm32"))]
struct StagedListenerHandles {
    handles: Vec<ListenerHandle>,
}

#[cfg(not(target_arch = "wasm32"))]
impl StagedListenerHandles {
    fn new() -> Self {
        Self {
            handles: Vec::new(),
        }
    }

    fn push(&mut self, handle: ListenerHandle) {
        self.handles.push(handle);
    }

    async fn abort_and_wait(&mut self) {
        while let Some(handle) = self.handles.pop() {
            handle.abort_and_wait().await;
        }
    }

    fn commit(mut self) -> Vec<ListenerHandle> {
        std::mem::take(&mut self.handles)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for StagedListenerHandles {
    fn drop(&mut self) {
        for handle in self.handles.drain(..) {
            handle.abort();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ListenerHandle {
    pub fn abort(&self) {
        self.handle.abort();
    }

    async fn abort_and_wait(self) {
        self.handle.abort();
        let _ = self.handle.await;
    }
}

#[cfg(unix)]
async fn spawn_uds_listener(
    path: &Path,
    keypair: Arc<Keypair>,
    require_peer_auth: bool,
    inbox_sender: InboxSender,
) -> Result<ListenerHandle, std::io::Error> {
    use tokio::net::UnixListener;
    let path = path.to_path_buf();
    if let Err(err) = tokio::fs::remove_file(&path).await
        && err.kind() != std::io::ErrorKind::NotFound
    {
        return Err(err);
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent).await?;
    }
    let listener = UnixListener::bind(&path)?;
    let handle = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (keypair, inbox_sender) = (keypair.clone(), inbox_sender.clone());
            tokio::spawn(async move {
                let _ = handle_connection(stream, require_peer_auth, &keypair, &inbox_sender).await;
            });
        }
    });
    Ok(ListenerHandle {
        handle,
        local_addr: None,
    })
}

#[cfg(not(target_arch = "wasm32"))]
struct TcpListenerConfig {
    addr: String,
    keypair: Arc<Keypair>,
    router: Arc<Router>,
    trusted: Arc<parking_lot::RwLock<TrustStore>>,
    require_peer_auth: bool,
    inbox_sender: InboxSender,
    pairing_password: Option<String>,
    advertise_address: Option<String>,
    trusted_peers_path: std::path::PathBuf,
    participant_name: String,
    bridge_bootstrap_token: String,
}

#[cfg(not(target_arch = "wasm32"))]
async fn spawn_tcp_listener(config: TcpListenerConfig) -> Result<ListenerHandle, std::io::Error> {
    let TcpListenerConfig {
        addr,
        keypair,
        router,
        trusted,
        require_peer_auth,
        inbox_sender,
        pairing_password,
        advertise_address,
        trusted_peers_path,
        participant_name,
        bridge_bootstrap_token,
    } = config;
    if let Some(secret) = pairing_password.as_deref() {
        validate_pairing_secret(secret)?;
    }
    let listener = TcpListener::bind(&addr).await?;
    let local_addr = listener.local_addr()?;
    let target_address = if let Some(advertise_address) = advertise_address {
        parse_peer_address(&advertise_address).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid comms advertised address '{advertise_address}': {error}"),
            )
        })?;
        advertise_address
    } else {
        if local_addr.ip().is_unspecified() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "signed comms listener bound to a wildcard address requires an explicit advertised address",
            ));
        }
        format!("tcp://{local_addr}")
    };
    let handle = tokio::spawn(async move {
        while let Ok((stream, source)) = listener.accept().await {
            let (keypair, router, trusted, inbox_sender) = (
                keypair.clone(),
                router.clone(),
                trusted.clone(),
                inbox_sender.clone(),
            );
            let pairing_password = pairing_password.clone();
            let trusted_peers_path = trusted_peers_path.clone();
            let participant_name = participant_name.clone();
            let bridge_bootstrap_token = bridge_bootstrap_token.clone();
            let target_address = target_address.clone();
            tokio::spawn(async move {
                // Share the router-owned trust handle directly — no
                // snapshot clone — so the transport trust gate reads the
                // same live state the inbox admission gate consults.
                let _ = handle_tcp_connection_or_pairing(
                    stream,
                    source,
                    require_peer_auth,
                    &keypair,
                    &router,
                    &trusted,
                    &inbox_sender,
                    pairing_password.as_deref(),
                    &trusted_peers_path,
                    &participant_name,
                    &bridge_bootstrap_token,
                    &target_address,
                )
                .await;
            });
        }
    });
    Ok(ListenerHandle {
        handle,
        local_addr: Some(local_addr),
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
async fn handle_tcp_connection_or_pairing(
    stream: TcpStream,
    observed_source: std::net::SocketAddr,
    require_peer_auth: bool,
    keypair: &Keypair,
    router: &Router,
    trusted: &Arc<parking_lot::RwLock<TrustStore>>,
    inbox_sender: &InboxSender,
    pairing_password: Option<&str>,
    trusted_peers_path: &Path,
    participant_name: &str,
    bridge_bootstrap_token: &str,
    target_address: &str,
) -> Result<(), std::io::Error> {
    let mut first = [0_u8; 1];
    let is_pairing = pairing_password.is_some()
        && matches!(
            tokio::time::timeout(PAIRING_CLASSIFY_TIMEOUT, stream.peek(&mut first)).await,
            Ok(Ok(1))
        )
        && first[0] == b'{';
    if is_pairing {
        let Some(pairing_password) = pairing_password else {
            return handle_tcp_connection(
                stream,
                observed_source,
                require_peer_auth,
                keypair,
                inbox_sender,
            )
            .await
            .map_err(|err| std::io::Error::other(err.to_string()));
        };
        return handle_pairing_connection(
            stream,
            keypair,
            router,
            trusted,
            pairing_password,
            trusted_peers_path,
            participant_name,
            bridge_bootstrap_token,
            target_address,
        )
        .await;
    }

    handle_tcp_connection(
        stream,
        observed_source,
        require_peer_auth,
        keypair,
        inbox_sender,
    )
    .await
    .map_err(|err| std::io::Error::other(err.to_string()))
}

#[cfg(not(target_arch = "wasm32"))]
async fn read_bounded_pairing_line<R>(
    reader: &mut R,
    buf: &mut String,
) -> Result<usize, std::io::Error>
where
    R: AsyncBufRead + Unpin,
{
    buf.clear();
    let mut bytes = Vec::new();
    loop {
        let (consumed, terminated) = {
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                let (kind, message) = if bytes.is_empty() {
                    (
                        std::io::ErrorKind::UnexpectedEof,
                        "pairing peer closed connection",
                    )
                } else {
                    (
                        std::io::ErrorKind::InvalidData,
                        "pairing message is missing its newline delimiter",
                    )
                };
                return Err(std::io::Error::new(kind, message));
            }

            let newline = available.iter().position(|byte| *byte == b'\n');
            let consumed = newline.map_or(available.len(), |index| index + 1);
            let next_len = bytes.len().checked_add(consumed).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "pairing message exceeds the maximum frame size",
                )
            })?;
            if next_len > MAX_PAIRING_MESSAGE_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "pairing message exceeds the maximum frame size",
                ));
            }
            bytes.extend_from_slice(&available[..consumed]);
            (consumed, newline.is_some())
        };
        reader.consume(consumed);
        if terminated {
            break;
        }
    }

    *buf = String::from_utf8(bytes).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "pairing message is not valid UTF-8",
        )
    })?;
    Ok(buf.len())
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn read_pairing_message(
    reader: &mut BufReader<TcpStream>,
    buf: &mut String,
) -> Result<PairingClientMessage, std::io::Error> {
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        read_bounded_pairing_line(reader, buf),
    )
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "pairing read timed out"))??;
    serde_json::from_str(buf).map_err(|err| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid pairing message: {err}"),
        )
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn write_pairing_json(
    writer: &mut TcpStream,
    value: serde_json::Value,
) -> Result<(), std::io::Error> {
    let bytes = serde_json::to_vec(&value).map_err(|err| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("pairing response encode failed: {err}"),
        )
    })?;
    writer.write_all(&bytes).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
async fn handle_pairing_connection(
    stream: TcpStream,
    keypair: &Keypair,
    router: &Router,
    trusted: &Arc<parking_lot::RwLock<TrustStore>>,
    pairing_password: &str,
    trusted_peers_path: &Path,
    participant_name: &str,
    bridge_bootstrap_token: &str,
    target_address: &str,
) -> Result<(), std::io::Error> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let PairingClientMessage::Hello { version } =
        read_pairing_message(&mut reader, &mut line).await?
    else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "expected meerkat pairing hello",
        ));
    };
    if version != PAIRING_VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unsupported meerkat pairing version",
        ));
    }

    let challenge = Uuid::new_v4().to_string();
    write_pairing_json(
        reader.get_mut(),
        serde_json::json!({
            "kind": PAIRING_CHALLENGE_KIND,
            "version": PAIRING_VERSION,
            "challenge": challenge,
            "target": {
                "name": participant_name,
                "address": target_address,
                "identity": {
                    "kind": "ed25519_public_key",
                    "public_key": keypair.public_key().to_pubkey_string(),
                }
            }
        }),
    )
    .await?;

    // Typed pairing-proof deserialization fails closed: a malformed public
    // key, an identity kind other than `ed25519_public_key`, or a missing
    // `password_proof` is rejected here, before any trust row is derived. No
    // raw-JSON field reads and no late, second public-key parse downstream.
    let PairingClientMessage::Proof {
        caller,
        password_proof,
    } = read_pairing_message(&mut reader, &mut line).await?
    else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "expected meerkat pairing proof",
        ));
    };
    let PairingIdentityKind::Ed25519PublicKey = caller.identity.kind;
    let expected = comms_pairing_password_proof(
        pairing_password,
        &challenge,
        &caller.identity.public_key.raw,
        &caller.address.0,
    );
    if !constant_time_str_eq(&password_proof, &expected) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "invalid pairing password proof",
        ));
    }

    // The trusted peer is built from the typed identity validated at
    // deserialization — not from re-parsed free strings. An invalid caller
    // name or address is a typed rejection before any trust row is derived.
    let pubkey = caller.identity.public_key.key;
    let caller_name = PeerName::new(caller.name.clone())
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    let caller_address = meerkat_core::comms::PeerAddress::parse(&caller.address.0)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))?;
    // Paired external-peer trust is a MobMachine-owned external-peer edge: the
    // remote target paired into this mob's control plane. Attribute the trust
    // row to the generated MobMachineExternalPeerTrustWiring authority source
    // rather than a legacy/unattributed mutation (Dogma Invariant 1).
    let paired_peer_id = pubkey.to_peer_id();
    router
        .add_trusted_peer_for_source(
            TrustEntry {
                peer_id: paired_peer_id,
                name: caller_name,
                pubkey,
                address: caller_address,
                meta: crate::PeerMeta::default(),
            },
            meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MobMachineExternalPeerTrustWiring,
            false,
        )
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))?;
    let peers_snapshot = trusted.read().clone();
    peers_snapshot
        .save(trusted_peers_path)
        .await
        .map_err(|err| std::io::Error::other(format!("persist paired trust: {err}")))?;

    write_pairing_json(
        reader.get_mut(),
        serde_json::json!({
            "kind": PAIRING_COMPLETE_KIND,
            "version": PAIRING_VERSION,
            "binding": {
                "kind": "external",
                "address": target_address,
                "bootstrap_token": bridge_bootstrap_token,
                "identity": {
                    "kind": "ed25519_public_key",
                    "public_key": keypair.public_key().to_pubkey_string(),
                }
            },
            "comms_name": participant_name,
        }),
    )
    .await
}

/// Max concurrent connections for the plain listener (DoS protection).
#[cfg(not(target_arch = "wasm32"))]
const PLAIN_LISTENER_MAX_CONCURRENT: usize = 64;

#[cfg(not(target_arch = "wasm32"))]
async fn spawn_plain_tcp_listener(
    addr: &str,
    inbox_sender: InboxSender,
    max_line_length: usize,
) -> Result<ListenerHandle, std::io::Error> {
    let listener = TcpListener::bind(addr).await?;
    let semaphore = Arc::new(tokio::sync::Semaphore::new(PLAIN_LISTENER_MAX_CONCURRENT));
    // Shared typed ingress-fault counters across all connection tasks for this
    // listener (codec overflow / inbox-full backpressure).
    let faults = Arc::new(crate::plain_listener::PlainIngressFaults::default());
    let handle = tokio::spawn(async move {
        while let Ok((stream, _peer)) = listener.accept().await {
            let sender = inbox_sender.clone();
            let sem = semaphore.clone();
            let faults = faults.clone();
            tokio::spawn(async move {
                let _permit = match sem.acquire().await {
                    Ok(p) => p,
                    Err(_) => return, // Semaphore closed
                };
                crate::plain_listener::handle_plain_connection(
                    stream,
                    sender,
                    max_line_length,
                    meerkat_core::PlainEventSource::Tcp,
                    &faults,
                )
                .await;
            });
        }
    });
    Ok(ListenerHandle {
        handle,
        local_addr: None,
    })
}

#[cfg(unix)]
async fn spawn_plain_uds_listener(
    path: &std::path::Path,
    inbox_sender: InboxSender,
    max_line_length: usize,
) -> Result<ListenerHandle, std::io::Error> {
    use tokio::net::UnixListener;
    let path = path.to_path_buf();
    if let Err(err) = tokio::fs::remove_file(&path).await
        && err.kind() != std::io::ErrorKind::NotFound
    {
        return Err(err);
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent).await?;
    }
    let listener = UnixListener::bind(&path)?;
    let semaphore = Arc::new(tokio::sync::Semaphore::new(PLAIN_LISTENER_MAX_CONCURRENT));
    // Shared typed ingress-fault counters across all connection tasks for this
    // listener (codec overflow / inbox-full backpressure).
    let faults = Arc::new(crate::plain_listener::PlainIngressFaults::default());
    let handle = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let sender = inbox_sender.clone();
            let sem = semaphore.clone();
            let faults = faults.clone();
            tokio::spawn(async move {
                let _permit = match sem.acquire().await {
                    Ok(p) => p,
                    Err(_) => return,
                };
                crate::plain_listener::handle_plain_connection(
                    stream,
                    sender,
                    max_line_length,
                    meerkat_core::PlainEventSource::Uds,
                    &faults,
                )
                .await;
            });
        }
    });
    Ok(ListenerHandle {
        handle,
        local_addr: None,
    })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::classify::test_support;

    #[tokio::test]
    async fn pairing_frame_reader_accepts_the_exact_size_boundary() {
        let mut wire = vec![b' '; MAX_PAIRING_MESSAGE_BYTES];
        wire[MAX_PAIRING_MESSAGE_BYTES - 1] = b'\n';
        let (mut writer, stream) = tokio::io::duplex(127);
        let write = tokio::spawn(async move {
            writer.write_all(&wire).await?;
            writer.shutdown().await
        });
        let mut reader = BufReader::with_capacity(113, stream);
        let mut line = "stale caller data".to_string();

        let bytes = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            read_bounded_pairing_line(&mut reader, &mut line),
        )
        .await
        .expect("bounded reader must complete")
        .expect("an exact-boundary frame must be accepted");

        assert_eq!(bytes, MAX_PAIRING_MESSAGE_BYTES);
        assert_eq!(line.len(), MAX_PAIRING_MESSAGE_BYTES);
        assert!(line.ends_with('\n'));
        write
            .await
            .expect("boundary writer task must join")
            .expect("boundary writer must finish");
    }

    #[tokio::test]
    async fn pairing_frame_reader_rejects_streaming_input_over_the_limit() {
        let wire = vec![b'{'; MAX_PAIRING_MESSAGE_BYTES + 1];
        let (mut writer, stream) = tokio::io::duplex(61);
        let write = tokio::spawn(async move {
            writer.write_all(&wire).await?;
            writer.shutdown().await
        });
        let mut reader = BufReader::with_capacity(47, stream);
        let mut line = String::new();

        let error = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            read_bounded_pairing_line(&mut reader, &mut line),
        )
        .await
        .expect("oversize streaming input must be rejected promptly")
        .expect_err("an over-limit frame must fail closed");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("maximum frame size"));
        assert!(
            line.is_empty(),
            "partial pre-auth input must not be published"
        );
        write.abort();
        let _ = write.await;
    }

    #[tokio::test]
    async fn pairing_frame_reader_rejects_an_unterminated_frame() {
        let (mut writer, stream) = tokio::io::duplex(64);
        let write = tokio::spawn(async move {
            writer
                .write_all(br#"{"kind":"meerkat_pairing_hello","version":1}"#)
                .await?;
            writer.shutdown().await
        });
        let mut reader = BufReader::new(stream);
        let mut line = String::new();

        let error = read_bounded_pairing_line(&mut reader, &mut line)
            .await
            .expect_err("pairing frames must be newline-delimited");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("newline delimiter"));
        assert!(line.is_empty(), "unterminated input must not be published");
        write
            .await
            .expect("unterminated writer task must join")
            .expect("unterminated writer must finish");
    }

    /// The dyn core-trait surface is how mob/runtime layers holding
    /// `Arc<dyn CoreCommsRuntime>` install the host's declaration per member;
    /// it must delegate to the concrete router config, not the typed default.
    #[tokio::test]
    async fn dyn_trait_taint_setter_delegates_to_router_declaration() {
        let runtime =
            Arc::new(CommsRuntime::inproc_only("dyn-taint-setter").expect("inproc runtime"));
        let dyn_runtime: Arc<dyn CoreCommsRuntime> = runtime.clone();

        dyn_runtime
            .set_outbound_content_taint(Some(meerkat_core::comms::SenderContentTaint::Tainted))
            .expect("concrete runtime must accept the declaration");
        assert_eq!(
            runtime.outbound_content_taint(),
            Some(meerkat_core::comms::SenderContentTaint::Tainted)
        );

        dyn_runtime
            .set_outbound_content_taint(None)
            .expect("clearing the declaration must succeed");
        assert_eq!(runtime.outbound_content_taint(), None);
    }
    use crate::event_injector::CommsEventInjector;
    use crate::identity::Signature;
    use crate::types::{Envelope, InboxItem, MessageKind, Status};
    use async_trait::async_trait;
    use futures::StreamExt;
    use meerkat_core::event_injector::SubscribableInjector;
    use meerkat_core::{
        BlobId, BlobPayload, BlobRef, BlobStore, BlobStoreError, SendError,
        comms::{
            CommsTrustMutation, CommsTrustMutationResult, InputSource, InputStreamMode,
            PeerDirectorySource, PeerId, PeerName, PeerRoute, PeerSendability, StreamError,
            StreamScope, TrustedPeerDescriptor,
        },
        interaction::InteractionId,
        types::{ContentBlock, ImageData, SessionId},
    };

    /// ROW #344 gate: the pairing caller is built from a typed identity that
    /// deserializes once and fails closed.
    ///
    /// - A malformed public key (not `ed25519:<base64>`) is rejected at typed
    ///   deserialization, before any trust row could be derived.
    /// - An identity `kind` other than `ed25519_public_key` is rejected by the
    ///   typed enum — no String equality check downstream.
    /// - A well-formed caller yields a typed [`PubKey`] (the trust row is built
    ///   from `identity.public_key.key`, not a re-parsed free string).
    #[test]
    fn pairing_caller_identity_is_typed_and_fails_closed() {
        // Malformed public key → typed deserialization rejects it.
        let bad_key = serde_json::json!({
            "name": "peer",
            "address": "tcp://127.0.0.1:4200",
            "identity": {
                "kind": "ed25519_public_key",
                "public_key": "not-a-valid-pubkey",
            }
        });
        assert!(
            serde_json::from_value::<PairingPeer>(bad_key).is_err(),
            "a malformed pairing public key must be rejected at deserialization"
        );

        // Wrong identity kind → typed enum rejects it (no String == check).
        let valid_pubkey = Keypair::generate().public_key();
        let wrong_kind = serde_json::json!({
            "name": "peer",
            "address": "tcp://127.0.0.1:4200",
            "identity": {
                "kind": "rsa_public_key",
                "public_key": valid_pubkey.to_pubkey_string(),
            }
        });
        assert!(
            serde_json::from_value::<PairingPeer>(wrong_kind).is_err(),
            "a non-ed25519 identity kind must be rejected at deserialization"
        );

        // Well-formed caller → typed PubKey is recovered, raw string retained
        // for the proof hash.
        let good = serde_json::json!({
            "name": "peer",
            "address": "tcp://127.0.0.1:4200",
            "identity": {
                "kind": "ed25519_public_key",
                "public_key": valid_pubkey.to_pubkey_string(),
            }
        });
        let caller: PairingPeer =
            serde_json::from_value(good).expect("well-formed pairing caller deserializes");
        assert_eq!(caller.identity.public_key.key, valid_pubkey);
        assert_eq!(
            caller.identity.public_key.raw,
            valid_pubkey.to_pubkey_string()
        );
        assert_eq!(caller.address.0, "tcp://127.0.0.1:4200");
        assert_eq!(caller.identity.kind, PairingIdentityKind::Ed25519PublicKey);
    }

    /// Pairing ingress gate: the full client wire message parses once into the
    /// typed [`PairingClientMessage`] envelope and fails closed — no raw-JSON
    /// field-by-field reads remain on the trust-enrollment path.
    #[test]
    fn pairing_client_messages_parse_typed_and_fail_closed() {
        // Unknown kind → rejected at typed deserialization.
        assert!(
            serde_json::from_value::<PairingClientMessage>(serde_json::json!({
                "kind": "meerkat_pairing_evil",
                "version": 1,
            }))
            .is_err(),
            "an unknown pairing message kind must be rejected"
        );

        // Hello missing version → rejected (no default-zero laundering).
        assert!(
            serde_json::from_value::<PairingClientMessage>(serde_json::json!({
                "kind": "meerkat_pairing_hello",
            }))
            .is_err(),
            "a pairing hello without a version must be rejected"
        );

        // Proof missing password_proof → rejected before any trust derivation.
        let pubkey = Keypair::generate().public_key();
        assert!(
            serde_json::from_value::<PairingClientMessage>(serde_json::json!({
                "kind": "meerkat_pairing_proof",
                "caller": {
                    "name": "peer",
                    "address": "tcp://127.0.0.1:4200",
                    "identity": {
                        "kind": "ed25519_public_key",
                        "public_key": pubkey.to_pubkey_string(),
                    }
                }
            }))
            .is_err(),
            "a pairing proof without a password proof must be rejected"
        );

        // Well-formed messages parse into the typed variants.
        let hello: PairingClientMessage = serde_json::from_value(serde_json::json!({
            "kind": "meerkat_pairing_hello",
            "version": 1,
        }))
        .expect("well-formed hello parses");
        assert!(matches!(hello, PairingClientMessage::Hello { version: 1 }));
        let proof: PairingClientMessage = serde_json::from_value(serde_json::json!({
            "kind": "meerkat_pairing_proof",
            "password_proof": "proof",
            "caller": {
                "name": "peer",
                "address": "tcp://127.0.0.1:4200",
                "identity": {
                    "kind": "ed25519_public_key",
                    "public_key": pubkey.to_pubkey_string(),
                }
            }
        }))
        .expect("well-formed proof parses");
        match proof {
            PairingClientMessage::Proof {
                caller,
                password_proof,
            } => {
                assert_eq!(password_proof, "proof");
                assert_eq!(caller.identity.public_key.key, pubkey);
            }
            PairingClientMessage::Hello { .. } => panic!("expected proof variant"),
        }
    }

    /// Test helper: build a [`TrustedPeerDescriptor`] from raw pubkey +
    /// display name + address string. Panics on invalid input — tests are
    /// expected to supply valid values.
    fn trusted_descriptor(name: &str, pubkey: PubKey, address: &str) -> TrustedPeerDescriptor {
        TrustedPeerDescriptor {
            peer_id: crate::router::peer_id_from_pubkey(&pubkey),
            name: PeerName::new(name.to_string()).expect("valid peer name"),
            address: parse_peer_address(address).expect("valid peer address"),
            pubkey: *pubkey.as_bytes(),
        }
    }

    fn local_descriptor_for_runtime(runtime: &CommsRuntime) -> TrustedPeerDescriptor {
        trusted_descriptor(
            runtime.participant_name(),
            runtime.public_key(),
            &runtime.advertised_address(),
        )
    }

    /// Test helper: build a typed [`TrustEntry`]. Panics on invalid input —
    /// tests are expected to supply valid values.
    fn test_trust_entry(name: &str, pubkey: PubKey, address: &str) -> TrustEntry {
        TrustEntry {
            peer_id: crate::router::peer_id_from_pubkey(&pubkey),
            name: PeerName::new(name.to_string()).expect("valid peer name"),
            pubkey,
            address: parse_peer_address(address).expect("valid peer address"),
            meta: crate::PeerMeta::default(),
        }
    }

    struct TestProjectionTrustAuthority {
        dsl: Arc<meerkat_runtime::HandleDslAuthority>,
        authority: meerkat_core::comms::CommsTrustMutationAuthority,
    }

    impl TestProjectionTrustAuthority {
        fn install_owner(&self, runtime: &CommsRuntime) {
            meerkat_runtime::RuntimePeerCommsHandle::install_generated_on(
                Arc::clone(&self.dsl),
                runtime,
            )
            .expect("install generated peer-comms handle");
        }
    }

    fn try_test_projection_add_authority(
        local: &TrustedPeerDescriptor,
        peer: &TrustedPeerDescriptor,
        epoch: u64,
    ) -> Result<TestProjectionTrustAuthority, String> {
        let local_endpoint = meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from(local);
        let endpoint = meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from(peer);
        let target_epoch = epoch.max(1);
        let dsl = Arc::new(meerkat_runtime::HandleDslAuthority::ephemeral());
        dsl.apply_signal(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineSignal::Initialize,
            "test_projection_add_authority::initialize",
        )
        .map_err(|err| err.to_string())?;
        dsl.apply_input(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
                session_id: meerkat_runtime::meerkat_machine::dsl::SessionId::from(
                    "meerkat-comms-runtime-test-projection",
                ),
            },
            "test_projection_add_authority::register_session",
        )
        .map_err(|err| err.to_string())?;
        dsl.apply_input(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::PrepareBindings {
                agent_runtime_id: meerkat_runtime::meerkat_machine::dsl::AgentRuntimeId::from(
                    "meerkat-comms-runtime-test-projection-runtime",
                ),
                fence_token: meerkat_runtime::meerkat_machine::dsl::FenceToken::from(0),
                generation: Some(meerkat_runtime::meerkat_machine::dsl::Generation::from(0)),
                runtime_epoch_id: None,
                session_id: meerkat_runtime::meerkat_machine::dsl::SessionId::from(
                    "meerkat-comms-runtime-test-projection",
                ),
            },
            "test_projection_add_authority::prepare_bindings",
        )
        .map_err(|err| err.to_string())?;
        dsl.apply_input(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::PublishLocalEndpoint {
                endpoint: local_endpoint,
            },
            "test_projection_add_authority::publish_local_endpoint",
        )
        .map_err(|err| err.to_string())?;
        for filler_index in 1..target_epoch {
            let filler_key = Keypair::generate().public_key();
            let filler = trusted_descriptor(
                &format!("filler-add-{filler_index}"),
                filler_key,
                &format!("inproc://filler-add-{filler_index}"),
            );
            let filler_endpoint =
                meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from(&filler);
            dsl.apply_input(
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::AddDirectPeerEndpoint {
                    endpoint: filler_endpoint,
                },
                "test_projection_add_authority::add_filler_endpoint",
            )
            .map_err(|err| err.to_string())?;
        }
        let transition = dsl
            .apply_input_with_transition(
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::AddDirectPeerEndpoint {
                    endpoint: endpoint.clone(),
                },
                "test_projection_add_authority::add_endpoint",
            )
            .map_err(|err| err.to_string())?;
        let mut obligations =
            meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
                &transition,
                dsl.peer_projection_freshness_authority(),
            );
        let obligation = obligations
            .pop()
            .ok_or_else(|| "generated reconcile obligation missing".to_string())?;
        let authority = meerkat_runtime::protocol_comms_trust_reconcile::authority_for_endpoint(
            &obligation,
            &endpoint,
        )?;
        Ok(TestProjectionTrustAuthority { dsl, authority })
    }

    fn test_projection_add_authority(
        local: &TrustedPeerDescriptor,
        peer: &TrustedPeerDescriptor,
        epoch: u64,
    ) -> TestProjectionTrustAuthority {
        try_test_projection_add_authority(local, peer, epoch)
            .expect("generated peer projection add authority")
    }

    fn test_projection_add_authority_with_dsl(
        dsl: Arc<meerkat_runtime::HandleDslAuthority>,
        peer: &TrustedPeerDescriptor,
    ) -> TestProjectionTrustAuthority {
        let endpoint = meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from(peer);
        let transition = dsl
            .apply_input_with_transition(
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::AddDirectPeerEndpoint {
                    endpoint: endpoint.clone(),
                },
                "test_projection_add_authority_with_dsl::add_endpoint",
            )
            .expect("AddDirectPeerEndpoint input");
        let mut obligations =
            meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
                &transition,
                dsl.peer_projection_freshness_authority(),
            );
        let obligation = obligations.pop().expect("generated reconcile obligation");
        let authority = meerkat_runtime::protocol_comms_trust_reconcile::authority_for_endpoint(
            &obligation,
            &endpoint,
        )
        .expect("generated peer projection add authority");
        TestProjectionTrustAuthority { dsl, authority }
    }

    fn test_projection_remove_authority(
        local: &TrustedPeerDescriptor,
        peer: &TrustedPeerDescriptor,
        epoch: u64,
    ) -> TestProjectionTrustAuthority {
        let local_endpoint = meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from(local);
        let endpoint = meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from(peer);
        let target_epoch = epoch.max(2);
        let dsl = Arc::new(meerkat_runtime::HandleDslAuthority::ephemeral());
        dsl.apply_signal(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineSignal::Initialize,
            "test_projection_remove_authority::initialize",
        )
        .expect("Initialize signal");
        dsl.apply_input(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
                session_id: meerkat_runtime::meerkat_machine::dsl::SessionId::from(
                    "meerkat-comms-runtime-test-projection",
                ),
            },
            "test_projection_remove_authority::register_session",
        )
        .expect("RegisterSession input");
        dsl.apply_input(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::PrepareBindings {
                agent_runtime_id: meerkat_runtime::meerkat_machine::dsl::AgentRuntimeId::from(
                    "meerkat-comms-runtime-test-projection-runtime",
                ),
                fence_token: meerkat_runtime::meerkat_machine::dsl::FenceToken::from(0),
                generation: Some(meerkat_runtime::meerkat_machine::dsl::Generation::from(0)),
                runtime_epoch_id: None,
                session_id: meerkat_runtime::meerkat_machine::dsl::SessionId::from(
                    "meerkat-comms-runtime-test-projection",
                ),
            },
            "test_projection_remove_authority::prepare_bindings",
        )
        .expect("PrepareBindings input");
        dsl.apply_input(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::PublishLocalEndpoint {
                endpoint: local_endpoint,
            },
            "test_projection_remove_authority::publish_local_endpoint",
        )
        .expect("PublishLocalEndpoint input");
        dsl.apply_input(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::AddDirectPeerEndpoint {
                endpoint: endpoint.clone(),
            },
            "test_projection_remove_authority::add_endpoint",
        )
        .expect("AddDirectPeerEndpoint input");
        for filler_index in 2..target_epoch {
            let filler_key = Keypair::generate().public_key();
            let filler = trusted_descriptor(
                &format!("filler-remove-{filler_index}"),
                filler_key,
                &format!("inproc://filler-remove-{filler_index}"),
            );
            let filler_endpoint =
                meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from(&filler);
            dsl.apply_input(
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::AddDirectPeerEndpoint {
                    endpoint: filler_endpoint,
                },
                "test_projection_remove_authority::add_filler_endpoint",
            )
            .expect("AddDirectPeerEndpoint filler input");
        }
        let transition = dsl
            .apply_input_with_transition(
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::RemoveDirectPeerEndpoint {
                    endpoint: endpoint.clone(),
                },
                "test_projection_remove_authority::remove_endpoint",
            )
            .expect("RemoveDirectPeerEndpoint input");
        let mut obligations =
            meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
                &transition,
                dsl.peer_projection_freshness_authority(),
            );
        let obligation = obligations.pop().expect("generated reconcile obligation");
        let authority =
            meerkat_runtime::protocol_comms_trust_reconcile::removal_authority_for_peer_id(
                &obligation,
                &endpoint.peer_id.0,
            )
            .expect("generated peer projection remove authority");
        TestProjectionTrustAuthority { dsl, authority }
    }

    fn test_projection_remove_authority_with_dsl(
        dsl: Arc<meerkat_runtime::HandleDslAuthority>,
        peer: &TrustedPeerDescriptor,
    ) -> TestProjectionTrustAuthority {
        let endpoint = meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from(peer);
        let transition = dsl
            .apply_input_with_transition(
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::RemoveDirectPeerEndpoint {
                    endpoint: endpoint.clone(),
                },
                "test_projection_remove_authority_with_dsl::remove_endpoint",
            )
            .expect("RemoveDirectPeerEndpoint input");
        let mut obligations =
            meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
                &transition,
                dsl.peer_projection_freshness_authority(),
            );
        let obligation = obligations.pop().expect("generated reconcile obligation");
        let authority =
            meerkat_runtime::protocol_comms_trust_reconcile::removal_authority_for_peer_id(
                &obligation,
                &endpoint.peer_id.0,
            )
            .expect("generated peer projection remove authority");
        TestProjectionTrustAuthority { dsl, authority }
    }

    async fn try_add_trusted_peer_with_generated_authority(
        runtime: &CommsRuntime,
        peer: TrustedPeerDescriptor,
    ) -> Result<Arc<meerkat_runtime::HandleDslAuthority>, SendError> {
        let projection =
            try_test_projection_add_authority(&local_descriptor_for_runtime(runtime), &peer, 0)
                .map_err(SendError::Validation)?;
        let dsl = Arc::clone(&projection.dsl);
        projection.install_owner(runtime);
        CoreCommsRuntime::apply_trust_mutation(
            runtime,
            CommsTrustMutation::AddTrustedPeer {
                authority: projection.authority,
                peer,
            },
        )
        .await
        .map(|_| dsl)
    }

    async fn add_trusted_peer_with_generated_authority(
        runtime: &CommsRuntime,
        peer: TrustedPeerDescriptor,
    ) -> Arc<meerkat_runtime::HandleDslAuthority> {
        try_add_trusted_peer_with_generated_authority(runtime, peer)
            .await
            .expect("generated add trusted peer should apply")
    }

    async fn remove_trusted_peer_with_generated_authority(
        runtime: &CommsRuntime,
        peer: &TrustedPeerDescriptor,
    ) -> Result<bool, SendError> {
        let projection =
            test_projection_remove_authority(&local_descriptor_for_runtime(runtime), peer, 0);
        projection.install_owner(runtime);
        let result = CoreCommsRuntime::apply_trust_mutation(
            runtime,
            CommsTrustMutation::RemoveTrustedPeer {
                authority: projection.authority,
                peer_id: peer.peer_id.to_string(),
            },
        )
        .await?;
        match result {
            CommsTrustMutationResult::Removed { removed } => Ok(removed),
            other => Err(SendError::Validation(format!(
                "unexpected generated trust mutation result: {other:?}"
            ))),
        }
    }

    #[tokio::test]
    async fn generated_peer_projection_trust_requires_canonical_owner() {
        let runtime = CommsRuntime::inproc_only("trust-owner-mismatch").expect("runtime");
        let local = local_descriptor_for_runtime(&runtime);
        let peer_key = Keypair::generate().public_key();
        let descriptor = trusted_descriptor("peer", peer_key, "inproc://peer");

        let authority = test_projection_add_authority(&local, &descriptor, 3);
        let missing_owner = CoreCommsRuntime::apply_trust_mutation(
            &runtime,
            CommsTrustMutation::AddTrustedPeer {
                peer: descriptor.clone(),
                authority: authority.authority.clone(),
            },
        )
        .await
        .expect_err("generated authority without runtime owner must fail closed");
        assert!(
            matches!(missing_owner, SendError::Validation(ref message) if message.contains("requires the target runtime's generated owner token")),
            "unexpected missing-owner rejection: {missing_owner:?}"
        );

        let foreign_owner = test_projection_add_authority(&local, &descriptor, 4);
        foreign_owner.install_owner(&runtime);
        let mismatched_owner = CoreCommsRuntime::apply_trust_mutation(
            &runtime,
            CommsTrustMutation::AddTrustedPeer {
                peer: descriptor,
                authority: authority.authority,
            },
        )
        .await
        .expect_err("foreign generated owner must fail closed");
        assert!(
            matches!(mismatched_owner, SendError::Validation(ref message) if message.contains("different generated owner")),
            "unexpected owner-mismatch rejection: {mismatched_owner:?}"
        );
        assert!(
            runtime.peers().await.is_empty(),
            "owner mismatch must not mutate peer directory projection"
        );
    }

    #[tokio::test]
    async fn recovered_mob_trust_owner_cannot_replace_live_owner() {
        let runtime = CommsRuntime::inproc_only("mob-trust-owner-recovery-guard").expect("runtime");
        let live_owner: Arc<dyn std::any::Any + Send + Sync> = Arc::new("live-owner");
        let recovered_owner: Arc<dyn std::any::Any + Send + Sync> = Arc::new("recovered-owner");

        CoreCommsRuntime::validate_recovered_generated_mob_trust_owner(
            &runtime,
            Arc::clone(&recovered_owner),
        )
        .await
        .expect("validation without installed owner should pass without binding");
        CoreCommsRuntime::install_generated_mob_trust_owner(&runtime, Arc::clone(&live_owner))
            .await
            .expect("install live mob owner");
        let validation_err = CoreCommsRuntime::validate_recovered_generated_mob_trust_owner(
            &runtime,
            Arc::clone(&recovered_owner),
        )
        .await
        .expect_err("read-only validation must not have installed recovered owner");
        assert!(
            matches!(validation_err, SendError::Validation(ref message) if message.contains("different generated MobMachine trust owner")),
            "unexpected recovered owner validation rejection: {validation_err:?}"
        );
        CoreCommsRuntime::install_recovered_generated_mob_trust_owner(
            &runtime,
            Arc::clone(&live_owner),
        )
        .await
        .expect("same recovered owner is idempotent");
        let err = CoreCommsRuntime::install_recovered_generated_mob_trust_owner(
            &runtime,
            recovered_owner,
        )
        .await
        .expect_err("recovered owner must not replace a different live owner");
        assert!(
            matches!(err, SendError::Validation(ref message) if message.contains("different generated MobMachine trust owner")),
            "unexpected recovered owner rejection: {err:?}"
        );
    }

    #[tokio::test]
    async fn generated_trust_mutation_updates_projection() {
        let runtime = CommsRuntime::inproc_only("trust-mutator-generated").expect("runtime");
        let local = local_descriptor_for_runtime(&runtime);
        let peer_key = Keypair::generate().public_key();
        let descriptor = trusted_descriptor("peer", peer_key, "inproc://peer");
        let peer_id = descriptor.peer_id.to_string();

        let add_authority = test_projection_add_authority(&local, &descriptor, 7);
        add_authority.install_owner(&runtime);
        let add_authority_replay = add_authority.authority.clone();
        let add = CoreCommsRuntime::apply_trust_mutation(
            &runtime,
            CommsTrustMutation::AddTrustedPeer {
                peer: descriptor.clone(),
                authority: add_authority.authority,
            },
        )
        .await
        .expect("generated add should apply");
        assert_eq!(add, CommsTrustMutationResult::Added { created: true });
        assert_eq!(runtime.peers().await.len(), 1);
        let replay = CoreCommsRuntime::apply_trust_mutation(
            &runtime,
            CommsTrustMutation::AddTrustedPeer {
                peer: descriptor.clone(),
                authority: add_authority_replay,
            },
        )
        .await
        .expect_err("cloned generated add authority must be one-use");
        assert!(
            matches!(replay, SendError::Validation(ref message) if message.contains("already consumed")),
            "unexpected replay rejection: {replay:?}"
        );
        let wrong_peer_key = Keypair::generate().public_key();
        let wrong_peer = trusted_descriptor(
            "wrong-operation-peer",
            wrong_peer_key,
            "inproc://wrong-operation-peer",
        );
        let wrong_operation_authority =
            test_projection_add_authority_with_dsl(Arc::clone(&add_authority.dsl), &wrong_peer);
        let wrong_operation = CoreCommsRuntime::apply_trust_mutation(
            &runtime,
            CommsTrustMutation::RemoveTrustedPeer {
                peer_id: peer_id.clone(),
                authority: wrong_operation_authority.authority,
            },
        )
        .await
        .expect_err("generated add authority must not remove trust");
        assert!(
            matches!(wrong_operation, SendError::Validation(ref message) if message.contains("PublicAdd") && message.contains("remove a public trusted peer")),
            "unexpected wrong-operation rejection: {wrong_operation:?}"
        );
        assert_eq!(
            runtime.peers().await.len(),
            1,
            "wrong-operation authority must not mutate trust projection",
        );

        let remove_authority =
            test_projection_remove_authority_with_dsl(Arc::clone(&add_authority.dsl), &descriptor);
        let remove = CoreCommsRuntime::apply_trust_mutation(
            &runtime,
            CommsTrustMutation::RemoveTrustedPeer {
                peer_id: peer_id.clone(),
                authority: remove_authority.authority,
            },
        )
        .await
        .expect("generated remove should apply");
        assert_eq!(remove, CommsTrustMutationResult::Removed { removed: true });
        assert!(runtime.peers().await.is_empty());
    }

    #[tokio::test]
    async fn generated_trust_add_reports_existing_source_without_rewriting_descriptor() {
        let runtime = CommsRuntime::inproc_only("trust-mutator-existing-source").expect("runtime");
        let peer_key = Keypair::generate().public_key();
        let descriptor = trusted_descriptor("peer", peer_key, "inproc://peer");
        let peer = descriptor_to_trust_entry(descriptor.clone()).expect("valid descriptor");

        runtime
            .router
            .add_trusted_peer_for_source(
                peer,
                GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection,
                false,
            )
            .expect("test setup installs MeerkatMachine-owned trust");

        let same_peer = descriptor_to_trust_entry(descriptor.clone()).expect("valid descriptor");
        let same = runtime
            .router
            .add_trusted_peer_for_source(
                same_peer,
                GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection,
                false,
            )
            .expect("same generated source row should be idempotent");
        assert!(!same);

        let conflicting_descriptor =
            trusted_descriptor("peer-renamed", peer_key, "inproc://peer-renamed");
        let conflicting_peer =
            descriptor_to_trust_entry(conflicting_descriptor).expect("valid descriptor");
        let conflict = runtime
            .router
            .add_trusted_peer_for_source(
                conflicting_peer,
                GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection,
                false,
            )
            .expect_err("trust repair must not rewrite an existing generated source row");
        assert!(
            matches!(
                conflict,
                crate::trust::TrustError::ConflictingGeneratedTrustSource { .. }
            ),
            "unexpected conflicting-source rejection: {conflict:?}"
        );

        let public_snapshot = CoreCommsRuntime::public_trusted_peer_projection_snapshot(&runtime)
            .await
            .expect("public trust snapshot");
        assert_eq!(public_snapshot.len(), 1);
        assert_eq!(public_snapshot[0].name.as_str(), "peer");
        assert_eq!(public_snapshot[0].address.to_string(), "inproc://peer");
    }

    #[tokio::test]
    async fn generated_remove_does_not_remove_trust_owned_by_other_source() {
        let runtime =
            CommsRuntime::inproc_only("trust-mutator-source-scoped-remove").expect("runtime");
        let local = local_descriptor_for_runtime(&runtime);
        let peer_key = Keypair::generate().public_key();
        let descriptor = trusted_descriptor("mob-owned-peer", peer_key, "inproc://mob-owned-peer");
        let peer_id = descriptor.peer_id.to_string();
        let peer = descriptor_to_trust_entry(descriptor.clone()).expect("valid descriptor");

        runtime
            .router
            .add_trusted_peer_for_source(
                peer,
                GeneratedCommsTrustAuthoritySourceKind::MobMachineMemberTrustWiring,
                false,
            )
            .expect("test setup installs mob-owned public trust");

        let meerkat_projection = CoreCommsRuntime::trusted_peer_projection_snapshot_for_source(
            &runtime,
            GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection,
        )
        .await
        .expect("source-scoped snapshot");
        assert!(
            meerkat_projection.is_empty(),
            "MeerkatMachine peer projection must not see MobMachine-owned trust as its canonical rows",
        );

        let remove_authority = test_projection_remove_authority(&local, &descriptor, 8);
        remove_authority.install_owner(&runtime);
        let remove = CoreCommsRuntime::apply_trust_mutation(
            &runtime,
            CommsTrustMutation::RemoveTrustedPeer {
                peer_id,
                authority: remove_authority.authority,
            },
        )
        .await
        .expect("generated remove for an unowned source should apply as no-op");
        assert_eq!(remove, CommsTrustMutationResult::Removed { removed: false });

        let public_snapshot = CoreCommsRuntime::public_trusted_peer_projection_snapshot(&runtime)
            .await
            .expect("public trust snapshot");
        assert_eq!(
            public_snapshot.len(),
            1,
            "MobMachine-owned trust must remain after a MeerkatMachine removal no-op",
        );
        assert_eq!(public_snapshot[0].peer_id, descriptor.peer_id);
    }

    #[tokio::test]
    async fn source_scoped_snapshot_uses_source_owned_descriptor() {
        let runtime =
            CommsRuntime::inproc_only("trust-mutator-source-scoped-descriptor").expect("runtime");
        let peer_key = Keypair::generate().public_key();
        let projection_descriptor =
            trusted_descriptor("projection-owned", peer_key, "inproc://projection-owned");
        let mob_descriptor = trusted_descriptor("mob-owned", peer_key, "inproc://mob-owned");
        let peer_id = projection_descriptor.peer_id;
        let projection_peer =
            descriptor_to_trust_entry(projection_descriptor.clone()).expect("valid descriptor");
        let mob_peer = descriptor_to_trust_entry(mob_descriptor.clone()).expect("valid descriptor");

        runtime
            .router
            .add_trusted_peer_for_source(
                projection_peer,
                GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection,
                false,
            )
            .expect("test setup installs MeerkatMachine-owned trust");
        runtime
            .router
            .add_trusted_peer_for_source(
                mob_peer,
                GeneratedCommsTrustAuthoritySourceKind::MobMachineMemberTrustWiring,
                false,
            )
            .expect("test setup installs MobMachine-owned trust");

        let projection_snapshot = CoreCommsRuntime::trusted_peer_projection_snapshot_for_source(
            &runtime,
            GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection,
        )
        .await
        .expect("projection source snapshot");
        assert_eq!(projection_snapshot, vec![projection_descriptor.clone()]);
        let mob_snapshot = CoreCommsRuntime::trusted_peer_projection_snapshot_for_source(
            &runtime,
            GeneratedCommsTrustAuthoritySourceKind::MobMachineMemberTrustWiring,
        )
        .await
        .expect("mob source snapshot");
        assert_eq!(mob_snapshot, vec![mob_descriptor.clone()]);

        let remove = runtime.router.remove_trusted_peer_for_source(
            &peer_id,
            GeneratedCommsTrustAuthoritySourceKind::MobMachineMemberTrustWiring,
        );
        assert!(remove, "test setup removal should remove MobMachine source");
        let public_snapshot = CoreCommsRuntime::public_trusted_peer_projection_snapshot(&runtime)
            .await
            .expect("public trust snapshot");
        assert_eq!(
            public_snapshot,
            vec![projection_descriptor],
            "union projection should fall back to the remaining source-owned descriptor",
        );
    }

    #[tokio::test]
    async fn generated_add_authority_rejects_descriptor_drift() {
        let runtime = CommsRuntime::inproc_only("trust-mutator-descriptor-drift").expect("runtime");
        let local = local_descriptor_for_runtime(&runtime);
        let peer_key = Keypair::generate().public_key();
        let descriptor = trusted_descriptor("peer", peer_key, "inproc://peer");
        let drifted_descriptor =
            trusted_descriptor("peer", peer_key, "inproc://peer-drifted-address");

        let add_authority = test_projection_add_authority(&local, &descriptor, 11);
        add_authority.install_owner(&runtime);
        let err = CoreCommsRuntime::apply_trust_mutation(
            &runtime,
            CommsTrustMutation::AddTrustedPeer {
                peer: drifted_descriptor,
                authority: add_authority.authority,
            },
        )
        .await
        .expect_err("generated add authority must bind the full peer descriptor");
        assert!(
            matches!(err, SendError::Validation(ref message) if message.contains("descriptor")),
            "unexpected descriptor drift rejection: {err:?}"
        );
        assert!(
            runtime.peers().await.is_empty(),
            "descriptor drift must not mutate public trust"
        );
    }

    #[tokio::test]
    async fn public_trust_projection_excludes_and_cannot_remove_private_peer() {
        let runtime = CommsRuntime::inproc_only("trust-mutator-private-split").expect("runtime");
        let local = local_descriptor_for_runtime(&runtime);
        let peer_key = Keypair::generate().public_key();
        let descriptor = trusted_descriptor("private-peer", peer_key, "inproc://private-peer");
        let peer_id = descriptor.peer_id.to_string();

        runtime
            .apply_private_trusted_peer_descriptor(
                descriptor.clone(),
                GeneratedCommsTrustAuthoritySourceKind::MeerkatMachineSupervisorPublish,
            )
            .await
            .expect("test setup installs private trust");
        let public_snapshot = CoreCommsRuntime::public_trusted_peer_projection_snapshot(&runtime)
            .await
            .expect("public trust snapshot");
        assert!(
            public_snapshot.is_empty(),
            "private trust must not be exported as public reconciliation truth"
        );

        let remove_authority = test_projection_remove_authority(&local, &descriptor, 12);
        remove_authority.install_owner(&runtime);
        let err = CoreCommsRuntime::apply_trust_mutation(
            &runtime,
            CommsTrustMutation::RemoveTrustedPeer {
                peer_id,
                authority: remove_authority.authority,
            },
        )
        .await
        .expect_err("public remove authority must not remove private trust");
        assert!(
            matches!(err, SendError::Validation(ref message) if message.contains("public trust authority cannot remove private trusted peer")),
            "unexpected private removal rejection: {err:?}"
        );
        let runtime_snapshot = CoreCommsRuntime::peer_ingress_runtime_snapshot(&runtime)
            .await
            .expect("runtime snapshot");
        assert_eq!(
            runtime_snapshot.trusted_peers.len(),
            1,
            "private trust should remain installed for admission"
        );
    }

    fn install_test_peer_comms_handle(runtime: &CommsRuntime) {
        runtime.install_peer_comms_handle(test_support::runtime_peer_comms_handle());
    }

    fn inproc_only_with_test_peer_authority(name: &str) -> CommsRuntime {
        let runtime = CommsRuntime::inproc_only(name).unwrap();
        install_test_peer_comms_handle(&runtime);
        runtime
    }

    async fn new_with_test_peer_authority(config: ResolvedCommsConfig) -> CommsRuntime {
        let runtime = CommsRuntime::new(config).await.unwrap();
        install_test_peer_comms_handle(&runtime);
        runtime
    }

    fn peer_route(name: &str, pubkey: PubKey) -> PeerRoute {
        PeerRoute::with_display_name(
            crate::router::peer_id_from_pubkey(&pubkey),
            PeerName::new(name.to_string()).expect("valid peer name"),
        )
    }

    async fn trusted_inproc_pair(prefix: &str) -> (CommsRuntime, CommsRuntime, String) {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender_name = format!("{prefix}-sender-{suffix}");
        let receiver_name = format!("{prefix}-receiver-{suffix}");
        let sender = CommsRuntime::inproc_only(&sender_name).expect("sender runtime");
        let receiver = inproc_only_with_test_peer_authority(&receiver_name);

        add_trusted_peer_with_generated_authority(
            &sender,
            trusted_descriptor(
                &receiver_name,
                receiver.public_key(),
                &format!("inproc://{receiver_name}"),
            ),
        )
        .await;
        add_trusted_peer_with_generated_authority(
            &receiver,
            trusted_descriptor(
                &sender_name,
                sender.public_key(),
                &format!("inproc://{sender_name}"),
            ),
        )
        .await;

        (sender, receiver, receiver_name)
    }

    async fn enqueue_fenced_then_plain(
        sender: &CommsRuntime,
        receiver: &CommsRuntime,
        receiver_name: &str,
    ) {
        let to = peer_route(receiver_name, receiver.public_key());
        let fenced = CommsCommand::IncarnationFencedPeerMessage {
            to: to.clone(),
            body: "authority-bearing fenced message".to_string(),
            blocks: None,
            content_taint: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            objective_id: None,
            expected_recipient: meerkat_core::comms::PeerRecipientIncarnation {
                mob_id: "mob".to_string(),
                agent_identity: "worker".to_string(),
                host_id: "host".to_string(),
                binding_generation: 2,
                member_session_id: "session".to_string(),
                generation: 3,
                fence_token: 5,
            },
        };
        CoreCommsRuntime::send(sender, fenced)
            .await
            .expect("fenced send");
        CoreCommsRuntime::send(
            sender,
            CommsCommand::PeerMessage {
                to,
                body: "ordinary message".to_string(),
                blocks: None,
                content_taint: None,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                objective_id: None,
            },
        )
        .await
        .expect("ordinary send");
    }

    fn missing_peer_route(name: &str) -> PeerRoute {
        PeerRoute::with_display_name(
            meerkat_core::comms::PeerId::new(),
            PeerName::new(name.to_string()).expect("valid peer name"),
        )
    }
    use parking_lot::Mutex;
    use std::{collections::HashMap, sync::Arc};
    use tokio::time::{Duration, timeout};
    use uuid::Uuid;

    #[derive(Default)]
    struct TestBlobStore {
        blobs: Mutex<HashMap<BlobId, BlobPayload>>,
    }

    #[async_trait]
    impl BlobStore for TestBlobStore {
        async fn put_image(&self, media_type: &str, data: &str) -> Result<BlobRef, BlobStoreError> {
            let blob_id = BlobId::from(format!("sha256:test-{media_type}-{data}"));
            let payload = BlobPayload {
                blob_id: blob_id.clone(),
                media_type: media_type.to_string(),
                data: data.to_string(),
            };
            self.blobs.lock().insert(blob_id.clone(), payload);
            Ok(BlobRef {
                blob_id,
                media_type: media_type.to_string(),
            })
        }

        async fn get(&self, blob_id: &BlobId) -> Result<BlobPayload, BlobStoreError> {
            self.blobs
                .lock()
                .get(blob_id)
                .cloned()
                .ok_or_else(|| BlobStoreError::NotFound(blob_id.clone()))
        }

        async fn delete(&self, blob_id: &BlobId) -> Result<(), BlobStoreError> {
            self.blobs.lock().remove(blob_id);
            Ok(())
        }

        fn is_persistent(&self) -> bool {
            false
        }
    }

    #[derive(Default)]
    struct TestPeerInteractionHandle {
        outbound:
            Mutex<HashMap<meerkat_core::PeerCorrelationId, meerkat_core::OutboundPeerRequestState>>,
        inbound:
            Mutex<HashMap<meerkat_core::PeerCorrelationId, meerkat_core::InboundPeerRequestState>>,
        inbound_modes:
            Mutex<HashMap<meerkat_core::PeerCorrelationId, meerkat_core::types::HandlingMode>>,
        cleanup_observer:
            Mutex<Option<Arc<dyn meerkat_core::handles::PeerInteractionCleanupObserver>>>,
    }

    impl TestPeerInteractionHandle {
        fn notify_cleanup(&self, corr_id: meerkat_core::PeerCorrelationId) {
            if let Some(observer) = self.cleanup_observer.lock().clone() {
                observer.on_peer_interaction_cleanup(corr_id);
            }
        }

        fn guard_rejected(
            context: &'static str,
            corr_id: meerkat_core::PeerCorrelationId,
        ) -> meerkat_core::handles::DslTransitionError {
            meerkat_core::handles::DslTransitionError::guard_rejected(
                context,
                format!("test authority rejected corr_id {corr_id}"),
            )
        }
    }

    impl meerkat_core::handles::PeerInteractionHandle for TestPeerInteractionHandle {
        fn request_sent(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            let mut outbound = self.outbound.lock();
            if outbound.contains_key(&corr_id) {
                return Err(Self::guard_rejected(
                    "PeerInteractionHandle::request_sent",
                    corr_id,
                ));
            }
            outbound.insert(corr_id, meerkat_core::OutboundPeerRequestState::Sent);
            Ok(())
        }

        fn response_progress(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            let mut outbound = self.outbound.lock();
            if !outbound.contains_key(&corr_id) {
                return Err(Self::guard_rejected(
                    "PeerInteractionHandle::response_progress",
                    corr_id,
                ));
            }
            outbound.insert(
                corr_id,
                meerkat_core::OutboundPeerRequestState::AcceptedProgress,
            );
            Ok(())
        }

        fn response_terminal(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
            _disposition: meerkat_core::handles::PeerTerminalDisposition,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            if self.outbound.lock().remove(&corr_id).is_none() {
                return Err(Self::guard_rejected(
                    "PeerInteractionHandle::response_terminal",
                    corr_id,
                ));
            }
            self.notify_cleanup(corr_id);
            Ok(())
        }

        fn response_rejected(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            if self.outbound.lock().remove(&corr_id).is_none() {
                return Err(Self::guard_rejected(
                    "PeerInteractionHandle::response_rejected",
                    corr_id,
                ));
            }
            self.notify_cleanup(corr_id);
            Ok(())
        }

        fn request_timed_out(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            if self.outbound.lock().remove(&corr_id).is_none() {
                return Err(Self::guard_rejected(
                    "PeerInteractionHandle::request_timed_out",
                    corr_id,
                ));
            }
            self.notify_cleanup(corr_id);
            Ok(())
        }

        fn request_send_failed(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            if self.outbound.lock().remove(&corr_id).is_none() {
                return Err(Self::guard_rejected(
                    "PeerInteractionHandle::request_send_failed",
                    corr_id,
                ));
            }
            self.notify_cleanup(corr_id);
            Ok(())
        }

        fn request_received(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
            handling_mode: meerkat_core::types::HandlingMode,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            let mut inbound = self.inbound.lock();
            if inbound.contains_key(&corr_id) {
                return Err(Self::guard_rejected(
                    "PeerInteractionHandle::request_received",
                    corr_id,
                ));
            }
            inbound.insert(corr_id, meerkat_core::InboundPeerRequestState::Received);
            self.inbound_modes.lock().insert(corr_id, handling_mode);
            Ok(())
        }

        fn classify_response_reply(
            &self,
            status: meerkat_core::ResponseStatus,
        ) -> Result<meerkat_core::TerminalityClass, meerkat_core::handles::DslTransitionError>
        {
            let generated = meerkat_runtime::handles::RuntimePeerInteractionHandle::ephemeral();
            meerkat_core::handles::PeerInteractionHandle::classify_response_reply(
                &generated, status,
            )
        }

        fn response_replied(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            if self.inbound.lock().remove(&corr_id).is_none() {
                return Err(Self::guard_rejected(
                    "PeerInteractionHandle::response_replied",
                    corr_id,
                ));
            }
            self.inbound_modes.lock().remove(&corr_id);
            Ok(())
        }

        fn outbound_state(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
        ) -> Option<meerkat_core::OutboundPeerRequestState> {
            self.outbound.lock().get(&corr_id).copied()
        }

        fn inbound_state(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
        ) -> Option<meerkat_core::InboundPeerRequestState> {
            self.inbound.lock().get(&corr_id).copied()
        }

        fn inbound_handling_mode(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
        ) -> Option<meerkat_core::types::HandlingMode> {
            self.inbound_modes.lock().get(&corr_id).copied()
        }

        fn install_cleanup_observer(
            &self,
            observer: Arc<dyn meerkat_core::handles::PeerInteractionCleanupObserver>,
        ) {
            *self.cleanup_observer.lock() = Some(observer);
        }
    }

    #[derive(Default)]
    struct TestInteractionStreamHandle {
        states:
            Mutex<HashMap<meerkat_core::PeerCorrelationId, meerkat_core::InteractionStreamState>>,
        cleanup_observer:
            Mutex<Option<Arc<dyn meerkat_core::handles::InteractionStreamCleanupObserver>>>,
    }

    impl TestInteractionStreamHandle {
        fn notify_cleanup(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
            abandon_reason: Option<meerkat_core::InteractionStreamAbandonReason>,
        ) {
            if let Some(observer) = self.cleanup_observer.lock().clone() {
                observer.on_interaction_stream_cleanup(corr_id, abandon_reason);
            }
        }

        fn guard_rejected(
            context: &'static str,
            corr_id: meerkat_core::PeerCorrelationId,
        ) -> meerkat_core::handles::DslTransitionError {
            meerkat_core::handles::DslTransitionError::guard_rejected(
                context,
                format!("test stream authority rejected corr_id {corr_id}"),
            )
        }

        fn terminal(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
            context: &'static str,
            abandon_reason: Option<meerkat_core::InteractionStreamAbandonReason>,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            if self.states.lock().remove(&corr_id).is_none() {
                return Err(Self::guard_rejected(context, corr_id));
            }
            self.notify_cleanup(corr_id, abandon_reason);
            Ok(())
        }
    }

    impl meerkat_core::handles::InteractionStreamHandle for TestInteractionStreamHandle {
        fn reserved(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            let mut states = self.states.lock();
            if states.contains_key(&corr_id) {
                return Err(Self::guard_rejected(
                    "InteractionStreamHandle::reserved",
                    corr_id,
                ));
            }
            states.insert(corr_id, meerkat_core::InteractionStreamState::Reserved);
            Ok(())
        }

        fn attached(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            let mut states = self.states.lock();
            match states.get_mut(&corr_id) {
                Some(state) if *state == meerkat_core::InteractionStreamState::Reserved => {
                    *state = meerkat_core::InteractionStreamState::Attached;
                    Ok(())
                }
                _ => Err(Self::guard_rejected(
                    "InteractionStreamHandle::attached",
                    corr_id,
                )),
            }
        }

        fn completed(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            self.terminal(corr_id, "InteractionStreamHandle::completed", None)
        }

        fn expired(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            self.terminal(corr_id, "InteractionStreamHandle::expired", None)
        }

        fn closed_early(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            self.terminal(corr_id, "InteractionStreamHandle::closed_early", None)
        }

        fn abandoned(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
            reason: meerkat_core::InteractionStreamAbandonReason,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            self.terminal(corr_id, "InteractionStreamHandle::abandoned", Some(reason))
        }

        fn state(
            &self,
            corr_id: meerkat_core::PeerCorrelationId,
        ) -> Option<meerkat_core::InteractionStreamState> {
            self.states.lock().get(&corr_id).copied()
        }

        fn install_cleanup_observer(
            &self,
            observer: Arc<dyn meerkat_core::handles::InteractionStreamCleanupObserver>,
        ) {
            *self.cleanup_observer.lock() = Some(observer);
        }
    }

    fn install_test_peer_request_response_authority(runtime: &Arc<CommsRuntime>) {
        runtime.install_peer_request_response_authority(PeerRequestResponseAuthority::new(
            Arc::new(TestPeerInteractionHandle::default()),
            Arc::new(TestInteractionStreamHandle::default()),
        ));
    }

    fn test_runtime_config(name: &str, tmp: &tempfile::TempDir) -> ResolvedCommsConfig {
        ResolvedCommsConfig {
            enabled: true,
            name: name.to_string(),
            inproc_namespace: None,
            listen_uds: None,
            listen_tcp: None,
            advertise_address: None,
            event_listen_tcp: None,
            #[cfg(unix)]
            event_listen_uds: None,
            identity_dir: tmp.path().join("identity"),
            trusted_peers_path: tmp.path().join("trusted_peers.json"),
            comms_config: crate::CommsConfig::default(),
            auth: meerkat_core::CommsAuthMode::Open,
            require_peer_auth: true,
            allow_external_unauthenticated: false,
            pairing_password: None,
        }
    }

    #[tokio::test]
    async fn pairing_password_installs_trusted_peer_and_returns_binding() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut config = test_runtime_config("pairing-target", &tmp);
        config.listen_tcp = Some("127.0.0.1:0".parse().unwrap());
        config.pairing_password = Some("0123456789abcdef0123456789abcdef".to_string());
        let mut runtime = CommsRuntime::new_machine_authority_required_with_silent_intents(
            config,
            Arc::new(HashSet::new()),
        )
        .await
        .unwrap();
        runtime.start_listeners().await.unwrap();
        let addr = runtime.config.listen_tcp.unwrap();

        let caller_keypair = Keypair::generate();
        let caller_public_key = caller_keypair.public_key().to_pubkey_string();
        let caller_address = "tcp://127.0.0.1:65530";
        let stream = TcpStream::connect(addr).await.unwrap();
        let mut reader = BufReader::new(stream);
        reader
            .get_mut()
            .write_all(
                serde_json::json!({
                    "kind": "meerkat_pairing_hello",
                    "version": PAIRING_VERSION,
                })
                .to_string()
                .as_bytes(),
            )
            .await
            .unwrap();
        reader.get_mut().write_all(b"\n").await.unwrap();

        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let challenge: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(challenge["kind"], PAIRING_CHALLENGE_KIND);
        let challenge_token = challenge["challenge"].as_str().unwrap();
        let proof = comms_pairing_password_proof(
            "0123456789abcdef0123456789abcdef",
            challenge_token,
            &caller_public_key,
            caller_address,
        );
        line.clear();
        reader
            .get_mut()
            .write_all(
                serde_json::json!({
                    "kind": "meerkat_pairing_proof",
                    "version": PAIRING_VERSION,
                    "password_proof": proof,
                    "caller": {
                        "name": "hive",
                        "address": caller_address,
                        "identity": {
                            "kind": "ed25519_public_key",
                            "public_key": caller_public_key,
                        }
                    }
                })
                .to_string()
                .as_bytes(),
            )
            .await
            .unwrap();
        reader.get_mut().write_all(b"\n").await.unwrap();

        reader.read_line(&mut line).await.unwrap();
        let complete: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(complete["kind"], PAIRING_COMPLETE_KIND);
        assert_eq!(complete["binding"]["kind"], "external");
        assert_eq!(
            complete["binding"]["identity"]["public_key"],
            runtime.public_key().to_pubkey_string()
        );

        let persisted = TrustStore::load(&runtime.config.trusted_peers_path)
            .await
            .unwrap();
        assert!(persisted.entries().any(|entry| {
            entry.name.as_str() == "hive" && entry.pubkey == caller_keypair.public_key()
        }));
        let live_peers = CoreCommsRuntime::peers(&runtime).await;
        assert!(
            live_peers
                .iter()
                .any(|peer| peer.peer_id == caller_keypair.public_key().to_peer_id()),
            "pairing must update the live router peer-id index, not only trusted_peers.json"
        );
    }

    #[tokio::test]
    async fn pairing_classification_tolerates_delayed_hello() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut config = test_runtime_config("pairing-delayed-target", &tmp);
        config.listen_tcp = Some("127.0.0.1:0".parse().unwrap());
        config.pairing_password = Some("0123456789abcdef0123456789abcdef".to_string());
        let mut runtime = CommsRuntime::new_machine_authority_required_with_silent_intents(
            config,
            Arc::new(HashSet::new()),
        )
        .await
        .unwrap();
        runtime.start_listeners().await.unwrap();
        let addr = runtime.config.listen_tcp.unwrap();

        let stream = TcpStream::connect(addr).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let mut reader = BufReader::new(stream);
        reader
            .get_mut()
            .write_all(
                serde_json::json!({
                    "kind": "meerkat_pairing_hello",
                    "version": PAIRING_VERSION,
                })
                .to_string()
                .as_bytes(),
            )
            .await
            .unwrap();
        reader.get_mut().write_all(b"\n").await.unwrap();

        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let challenge: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(challenge["kind"], PAIRING_CHALLENGE_KIND);
    }

    #[tokio::test]
    async fn wildcard_signed_listener_requires_advertised_address() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut config = test_runtime_config("wildcard-target", &tmp);
        config.listen_tcp = Some("0.0.0.0:0".parse().unwrap());
        let mut runtime = CommsRuntime::new_machine_authority_required_with_silent_intents(
            config,
            Arc::new(HashSet::new()),
        )
        .await
        .unwrap();

        let error = runtime
            .start_listeners()
            .await
            .expect_err("wildcard listener without advertise address should fail");
        assert!(
            error
                .to_string()
                .contains("requires an explicit advertised address"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn explicit_advertised_address_overrides_bound_listener_address() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut config = test_runtime_config("advertised-target", &tmp);
        config.listen_tcp = Some("127.0.0.1:0".parse().unwrap());
        config.advertise_address = Some("tcp://203.0.113.10:4200".to_string());
        let mut runtime = CommsRuntime::new_machine_authority_required_with_silent_intents(
            config,
            Arc::new(HashSet::new()),
        )
        .await
        .unwrap();

        assert_eq!(
            runtime.bound_tcp_listener_address(),
            None,
            "bound address is unavailable before listener start"
        );
        runtime.start_listeners().await.unwrap();
        assert_eq!(runtime.advertised_address(), "tcp://203.0.113.10:4200");
        let bound = runtime
            .bound_tcp_listener_address()
            .expect("actual signed TCP listener address after start");
        assert_eq!(bound.ip(), "127.0.0.1".parse::<std::net::IpAddr>().unwrap());
        assert_ne!(bound.port(), 0, "kernel must resolve the ephemeral port");
    }

    #[tokio::test]
    async fn listener_start_failure_unwinds_staged_handles_and_retry_succeeds() {
        let tmp = tempfile::TempDir::new().unwrap();
        let participant_name = format!("listener-start-transaction-{}", Uuid::new_v4().simple());
        let requested_address: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut config = test_runtime_config(&participant_name, &tmp);
        config.listen_tcp = Some(requested_address);
        let mut runtime = CommsRuntime::new_machine_authority_required_with_silent_intents(
            config,
            Arc::new(HashSet::new()),
        )
        .await
        .unwrap();

        {
            let mut fault = LISTENER_START_FAULT_AFTER_SIGNED_TCP
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *fault = Some(ListenerStartFault {
                participant_name: participant_name.clone(),
                observed_tcp_address: None,
                armed: true,
            });
        }

        let error = runtime
            .start_listeners()
            .await
            .expect_err("fault after signed TCP bind should fail startup");
        assert!(
            error
                .to_string()
                .contains("fault-injected listener startup failure"),
            "unexpected startup error: {error}"
        );
        let failed_address = LISTENER_START_FAULT_AFTER_SIGNED_TCP
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .and_then(|fault| fault.observed_tcp_address)
            .expect("fault seam should record the staged signed TCP address");

        assert!(!runtime.listeners_started);
        assert!(runtime.listener_handles.lock().is_empty());
        assert_eq!(runtime.bound_tcp_listener_address(), None);
        assert_eq!(
            runtime.config.listen_tcp,
            Some(requested_address),
            "failed staging must not publish the kernel-selected port"
        );
        let rebound = TcpListener::bind(failed_address)
            .await
            .expect("failed startup must release the staged TCP listener before returning");
        drop(rebound);

        runtime
            .start_listeners()
            .await
            .expect("the same runtime should retry cleanly after transactional rollback");
        assert!(runtime.listeners_started);
        assert_eq!(runtime.listener_handles.lock().len(), 1);
        assert!(runtime.bound_tcp_listener_address().is_some());
        runtime.shutdown();
    }

    #[tokio::test]
    async fn listener_start_cancellation_aborts_staged_handles_and_retry_succeeds() {
        let tmp = tempfile::TempDir::new().unwrap();
        let participant_name = format!("listener-start-cancellation-{}", Uuid::new_v4().simple());
        let requested_address: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut config = test_runtime_config(&participant_name, &tmp);
        config.listen_tcp = Some(requested_address);
        let mut runtime = CommsRuntime::new_machine_authority_required_with_silent_intents(
            config,
            Arc::new(HashSet::new()),
        )
        .await
        .unwrap();

        let (reached, reached_rx) = tokio::sync::oneshot::channel();
        let (release, release_rx) = tokio::sync::oneshot::channel();
        {
            let mut pause = LISTENER_START_PAUSE_AFTER_SIGNED_TCP
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *pause = Some(ListenerStartPause {
                participant_name,
                reached: Some(reached),
                release: Some(release_rx),
            });
        }

        let mut startup = Box::pin(runtime.start_listeners());
        let staged_address = tokio::select! {
            observed = reached_rx => observed.expect("pause seam should report the staged TCP address"),
            result = &mut startup => panic!("listener startup completed before cancellation: {result:?}"),
            () = tokio::time::sleep(Duration::from_secs(2)) => {
                panic!("listener startup did not reach the post-bind cancellation seam")
            }
        };

        // Cancelling startup drops its staging guard. The guard must abort the
        // bound listener rather than detach it from the cancelled future.
        drop(startup);
        drop(release);

        let rebound = timeout(Duration::from_secs(2), async {
            loop {
                match TcpListener::bind(staged_address).await {
                    Ok(listener) => break listener,
                    Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                        tokio::task::yield_now().await;
                    }
                    Err(error) => panic!("unexpected staged-port rebind failure: {error}"),
                }
            }
        })
        .await
        .expect("cancelled listener task should release its staged TCP port");
        drop(rebound);

        assert!(!runtime.listeners_started);
        assert!(runtime.listener_handles.lock().is_empty());
        assert_eq!(runtime.bound_tcp_listener_address(), None);
        assert_eq!(
            runtime.config.listen_tcp,
            Some(requested_address),
            "cancelled staging must not publish the kernel-selected port"
        );

        runtime
            .start_listeners()
            .await
            .expect("the same runtime should retry cleanly after cancellation rollback");
        assert!(runtime.listeners_started);
        assert_eq!(runtime.listener_handles.lock().len(), 1);
        assert!(runtime.bound_tcp_listener_address().is_some());
        runtime.shutdown();
    }

    #[tokio::test]
    async fn machine_required_tcp_runtime_rejects_ingress_before_handle_or_listener_start() {
        let tmp = tempfile::TempDir::new().unwrap();
        let suffix = Uuid::new_v4().simple().to_string();
        let sender_name = format!("pre-authority-sender-{suffix}");
        let receiver_name = format!("pre-authority-receiver-{suffix}");
        let sender = CommsRuntime::inproc_only(&sender_name).unwrap();
        let mut config = test_runtime_config(&receiver_name, &tmp);
        config.listen_tcp = Some("127.0.0.1:0".parse().unwrap());
        config.require_peer_auth = false;
        let receiver = CommsRuntime::new_machine_authority_required_with_silent_intents(
            config,
            Arc::new(HashSet::new()),
        )
        .await
        .unwrap();

        assert!(receiver.peer_comms_machine_authority_required());
        assert!(receiver.peer_comms_handle().is_none());

        add_trusted_peer_with_generated_authority(
            &sender,
            trusted_descriptor(
                &receiver_name,
                receiver.public_key(),
                &format!("inproc://{receiver_name}"),
            ),
        )
        .await;

        let result = CoreCommsRuntime::send(
            &sender,
            CommsCommand::PeerMessage {
                objective_id: None,
                content_taint: None,
                blocks: None,
                to: peer_route(&receiver_name, receiver.public_key()),
                body: "must not pass local classifier".to_string(),
                handling_mode: meerkat_core::types::HandlingMode::Queue,
            },
        )
        .await;

        assert!(matches!(
            result,
            Err(SendError::AdmissionDropped {
                reason: meerkat_core::comms::AdmissionDropReason::ClassificationRejected
            })
        ));
        let interactions = CoreCommsRuntime::drain_inbox_interactions(&receiver).await;
        assert!(
            interactions.is_empty(),
            "runtime-required ingress without a machine handle must not reach the inbox"
        );
    }

    fn signed_envelope(from: &Keypair, to: PubKey, kind: MessageKind) -> Envelope {
        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: from.public_key(),
            to,
            kind,
            sig: Signature::new([0u8; 64]),
        };
        envelope.sign(from);
        envelope
    }

    #[test]
    fn bridge_bootstrap_token_is_stable_for_the_same_keypair() {
        let keypair = Keypair::generate();
        let first = CommsRuntime::derive_bridge_bootstrap_token(&keypair);
        let second = CommsRuntime::derive_bridge_bootstrap_token(&keypair);
        assert_eq!(
            first, second,
            "same keypair should derive same bootstrap token"
        );
    }

    #[test]
    fn bridge_bootstrap_token_changes_with_a_different_keypair() {
        let first = CommsRuntime::derive_bridge_bootstrap_token(&Keypair::generate());
        let second = CommsRuntime::derive_bridge_bootstrap_token(&Keypair::generate());
        assert_ne!(
            first, second,
            "different keypairs should not derive the same bootstrap token"
        );
    }

    /// Regression: auth=Open must always load keypair and trusted peers from disk
    /// so that outbound routing still works.
    #[tokio::test]
    async fn test_auth_open_loads_keypair_and_peers() {
        let tmp = tempfile::TempDir::new().unwrap();

        let config = ResolvedCommsConfig {
            enabled: true,
            name: "test-agent".to_string(),
            inproc_namespace: None,
            listen_uds: None,
            listen_tcp: None,
            advertise_address: None,
            event_listen_tcp: None,
            #[cfg(unix)]
            event_listen_uds: None,
            identity_dir: tmp.path().join("identity"),
            trusted_peers_path: tmp.path().join("trusted_peers.json"),
            comms_config: crate::CommsConfig::default(),
            auth: meerkat_core::CommsAuthMode::Open,
            require_peer_auth: true,
            allow_external_unauthenticated: false,
            pairing_password: None,
        };

        let runtime = CommsRuntime::new(config).await.unwrap();
        assert_ne!(runtime.public_key().as_bytes(), &[0u8; 32]);
    }

    /// Regression: signed listener must ALWAYS start, regardless of auth mode.
    #[tokio::test]
    #[ignore] // integration-real: binds TCP port
    async fn test_signed_listener_starts_in_open_mode() {
        let tmp = tempfile::TempDir::new().unwrap();

        let config = ResolvedCommsConfig {
            enabled: true,
            name: "test-signed".to_string(),
            inproc_namespace: None,
            listen_uds: None,
            listen_tcp: Some("127.0.0.1:0".parse().unwrap()),
            advertise_address: None,
            event_listen_tcp: None,
            #[cfg(unix)]
            event_listen_uds: None,
            identity_dir: tmp.path().join("identity"),
            trusted_peers_path: tmp.path().join("trusted_peers.json"),
            comms_config: crate::CommsConfig::default(),
            auth: meerkat_core::CommsAuthMode::Open,
            require_peer_auth: true,
            allow_external_unauthenticated: false,
            pairing_password: None,
        };

        let mut runtime = CommsRuntime::new(config).await.unwrap();
        runtime.start_listeners().await.unwrap();
        assert!(runtime.listeners_started);
        runtime.shutdown();
    }

    /// Regression: non-loopback plain event listener must be rejected.
    #[tokio::test]
    async fn test_plain_listener_rejects_non_loopback() {
        let tmp = tempfile::TempDir::new().unwrap();

        let config = ResolvedCommsConfig {
            enabled: true,
            name: "test-reject".to_string(),
            inproc_namespace: None,
            listen_uds: None,
            listen_tcp: None,
            advertise_address: None,
            event_listen_tcp: Some("0.0.0.0:4201".parse().unwrap()),
            #[cfg(unix)]
            event_listen_uds: None,
            identity_dir: tmp.path().join("identity"),
            trusted_peers_path: tmp.path().join("trusted_peers.json"),
            comms_config: crate::CommsConfig::default(),
            auth: meerkat_core::CommsAuthMode::Open,
            require_peer_auth: true,
            allow_external_unauthenticated: false,
            pairing_password: None,
        };

        let mut runtime = CommsRuntime::new(config).await.unwrap();
        let result = runtime.start_listeners().await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("prompt injection"));
    }

    #[tokio::test]
    async fn test_drain_inbox_interactions_converts_all_authenticated_content_variants() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = test_runtime_config("variants", &tmp);
        let runtime = CommsRuntime::new(config).await.unwrap();
        install_test_peer_comms_handle(&runtime);

        let sender = Keypair::generate();
        add_trusted_peer_with_generated_authority(
            &runtime,
            trusted_descriptor("sender", sender.public_key(), "tcp://127.0.0.1:4200"),
        )
        .await;

        let request_id = Uuid::new_v4();
        let reply_to = Uuid::new_v4();

        let msg = signed_envelope(
            &sender,
            runtime.public_key(),
            MessageKind::Message {
                objective_id: None,
                content_taint: None,
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
            },
        );
        let req = signed_envelope(
            &sender,
            runtime.public_key(),
            MessageKind::Request {
                objective_id: None,
                intent: "review".to_string(),
                params: serde_json::json!({"pr": 19}),
                blocks: None,
                reply_endpoint: None,
                content_taint: None,
                handling_mode: None,
            },
        );
        let mut req = req;
        req.id = request_id;
        req.sign(&sender);
        let resp = signed_envelope(
            &sender,
            runtime.public_key(),
            MessageKind::Response {
                objective_id: None,
                in_reply_to: reply_to,
                status: Status::Completed,
                result: serde_json::json!({"ok": true}),
                blocks: None,
                content_taint: None,
                handling_mode: None,
            },
        );

        runtime
            .router
            .inbox_sender()
            .send_classified(InboxItem::External { envelope: msg })
            .into_result()
            .unwrap();
        runtime
            .router
            .inbox_sender()
            .send_classified(InboxItem::External { envelope: req })
            .into_result()
            .unwrap();
        runtime
            .router
            .inbox_sender()
            .send_classified(InboxItem::External { envelope: resp })
            .into_result()
            .unwrap();

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&runtime).await;
        assert_eq!(interactions.len(), 3);

        assert!(interactions.iter().any(|i| {
            matches!(
                &i.content,
                meerkat_core::InteractionContent::Message { body, .. } if body == "hello"
            )
        }));
        assert!(interactions.iter().any(|i| {
            matches!(
                &i.content,
                meerkat_core::InteractionContent::Request { intent, params, .. }
                    if intent == "review" && params["pr"] == 19
            )
        }));
        assert!(interactions.iter().any(|i| {
            matches!(
                &i.content,
                meerkat_core::InteractionContent::Response {
                    in_reply_to,
                    status,
                    result,
                    ..
                }
                    if in_reply_to.0 == reply_to
                        && *status == meerkat_core::ResponseStatus::Completed
                        && result["ok"] == true
            )
        }));
    }

    /// Ask 5 gate: the signed envelope's `content_taint` rides the classified
    /// drain into `InboxInteraction.sender_taint` as content-adjacent payload.
    /// `None` (no declaration) and `Some(Clean)` stay distinct typed states —
    /// the drain never coalesces an absent declaration into `Clean`.
    #[tokio::test]
    async fn drain_inbox_interactions_carries_sender_content_taint() {
        use meerkat_core::comms::SenderContentTaint;

        let tmp = tempfile::TempDir::new().unwrap();
        let config = test_runtime_config("taint-drain", &tmp);
        let runtime = CommsRuntime::new(config).await.unwrap();
        install_test_peer_comms_handle(&runtime);

        let sender = Keypair::generate();
        add_trusted_peer_with_generated_authority(
            &runtime,
            trusted_descriptor("sender", sender.public_key(), "tcp://127.0.0.1:4200"),
        )
        .await;

        let tainted = signed_envelope(
            &sender,
            runtime.public_key(),
            MessageKind::Message {
                objective_id: None,
                body: "tainted body".to_string(),
                blocks: None,
                content_taint: Some(SenderContentTaint::Tainted),
                handling_mode: None,
            },
        );
        let clean = signed_envelope(
            &sender,
            runtime.public_key(),
            MessageKind::Request {
                objective_id: None,
                intent: "review".to_string(),
                params: serde_json::json!({"pr": 7}),
                blocks: None,
                reply_endpoint: None,
                content_taint: Some(SenderContentTaint::Clean),
                handling_mode: None,
            },
        );
        let undeclared = signed_envelope(
            &sender,
            runtime.public_key(),
            MessageKind::Message {
                objective_id: None,
                body: "undeclared body".to_string(),
                blocks: None,
                content_taint: None,
                handling_mode: None,
            },
        );

        for envelope in [tainted, clean, undeclared] {
            runtime
                .router
                .inbox_sender()
                .send_classified(InboxItem::External { envelope })
                .into_result()
                .unwrap();
        }

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&runtime).await;
        assert_eq!(interactions.len(), 3);

        let taint_for = |predicate: &dyn Fn(&meerkat_core::InboxInteraction) -> bool| {
            interactions
                .iter()
                .find(|interaction| predicate(interaction))
                .expect("expected drained interaction")
                .sender_taint
        };
        assert_eq!(
            taint_for(&|i| matches!(
                &i.content,
                meerkat_core::InteractionContent::Message { body, .. } if body == "tainted body"
            )),
            Some(SenderContentTaint::Tainted)
        );
        assert_eq!(
            taint_for(&|i| matches!(
                &i.content,
                meerkat_core::InteractionContent::Request { intent, .. } if intent == "review"
            )),
            Some(SenderContentTaint::Clean)
        );
        assert_eq!(
            taint_for(&|i| matches!(
                &i.content,
                meerkat_core::InteractionContent::Message { body, .. } if body == "undeclared body"
            )),
            None,
            "no declaration must stay None, never coalesced into Clean"
        );
    }

    #[tokio::test]
    async fn lifecycle_envelopes_remain_non_responseable_request_projections() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = test_runtime_config("lifecycle-no-response-cache", &tmp);
        let runtime = CommsRuntime::new(config).await.unwrap();
        install_test_peer_comms_handle(&runtime);

        let sender = Keypair::generate();
        add_trusted_peer_with_generated_authority(
            &runtime,
            trusted_descriptor("sender", sender.public_key(), "tcp://127.0.0.1:4200"),
        )
        .await;

        let lifecycle_id = Uuid::new_v4();
        let mut envelope = signed_envelope(
            &sender,
            runtime.public_key(),
            MessageKind::Lifecycle {
                kind: meerkat_core::comms::PeerLifecycleKind::PeerAdded,
                params: serde_json::json!({"peer": "sender"}),
            },
        );
        envelope.id = lifecycle_id;
        envelope.sign(&sender);

        runtime
            .router
            .inbox_sender()
            .send_classified(InboxItem::External { envelope })
            .into_result()
            .unwrap();

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&runtime).await;
        assert_eq!(interactions.len(), 1);
        assert!(matches!(
            &interactions[0].content,
            meerkat_core::InteractionContent::Request { intent, .. }
                if intent == meerkat_core::comms::PeerLifecycleKind::PeerAdded.as_str()
        ));
        assert_eq!(
            interactions[0].handling_mode,
            meerkat_core::types::HandlingMode::Queue
        );
    }

    #[tokio::test]
    async fn test_drain_inbox_interactions_multimodal_message_keeps_body_as_projection() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = test_runtime_config("multimodal-body-projection", &tmp);
        let runtime = CommsRuntime::new(config).await.unwrap();
        install_test_peer_comms_handle(&runtime);

        let sender = Keypair::generate();
        add_trusted_peer_with_generated_authority(
            &runtime,
            trusted_descriptor("sender", sender.public_key(), "tcp://127.0.0.1:4200"),
        )
        .await;

        let blocks = vec![
            meerkat_core::types::ContentBlock::Text {
                text: "caption text".to_string(),
            },
            meerkat_core::types::ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "abc123".into(),
            },
        ];
        let msg = signed_envelope(
            &sender,
            runtime.public_key(),
            MessageKind::Message {
                objective_id: None,
                body: "please inspect this".to_string(),
                blocks: Some(blocks.clone()),
                content_taint: None,
                handling_mode: None,
            },
        );

        runtime
            .router
            .inbox_sender()
            .send_classified(InboxItem::External { envelope: msg })
            .into_result()
            .unwrap();

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&runtime).await;
        assert_eq!(interactions.len(), 1);
        let interaction = &interactions[0];
        assert_eq!(
            interaction.rendered_text,
            "Peer message from sender:\nplease inspect this"
        );
        match &interaction.content {
            meerkat_core::InteractionContent::Message {
                body,
                blocks: got_blocks,
            } => {
                assert_eq!(body, "please inspect this");
                assert_eq!(got_blocks.as_ref(), Some(&blocks));
            }
            other => panic!("expected message content, got {other:?}"),
        }
    }

    /// ROW #267 gate: a peer message whose body is "DISMISS" must NOT control
    /// runtime lifecycle. It is drained as an ordinary peer message, never
    /// swallowed into a dismiss signal. Lifecycle dismissal is a typed signal
    /// owned by the runtime authority, not a peer-controlled string body.
    #[tokio::test]
    async fn peer_message_body_dismiss_does_not_control_lifecycle() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = test_runtime_config("dismiss-body-is-not-lifecycle", &tmp);
        let runtime = CommsRuntime::new(config).await.unwrap();
        install_test_peer_comms_handle(&runtime);

        let sender = Keypair::generate();
        add_trusted_peer_with_generated_authority(
            &runtime,
            trusted_descriptor("sender", sender.public_key(), "tcp://127.0.0.1:4242"),
        )
        .await;

        let msg = signed_envelope(
            &sender,
            runtime.public_key(),
            MessageKind::Message {
                objective_id: None,
                body: "DISMISS".to_string(),
                blocks: None,
                content_taint: None,
                handling_mode: None,
            },
        );
        runtime
            .router
            .inbox_sender()
            .send_classified(InboxItem::External { envelope: msg })
            .into_result()
            .unwrap();

        // The body-string trigger is gone: the message survives the drain as a
        // normal peer message rather than being consumed as a dismiss signal.
        let interactions = CoreCommsRuntime::drain_inbox_interactions(&runtime).await;
        assert_eq!(
            interactions.len(),
            1,
            "a 'DISMISS' body must not be swallowed by a lifecycle trigger"
        );
        assert!(
            matches!(
                &interactions[0].content,
                meerkat_core::InteractionContent::Message { body, .. } if body == "DISMISS"
            ),
            "the 'DISMISS' body must drain as an ordinary peer message"
        );
        // And the runtime exposes no dismiss signal from a peer body: with the
        // override removed, the real runtime uses the trait default (`false`).
        assert!(
            !CoreCommsRuntime::dismiss_received(&runtime),
            "a peer message body must never raise a runtime dismiss signal"
        );
    }

    /// ROW #291 gate: a peer-request transport send failure must surface as a
    /// typed `SendError` to the caller, never be laundered into a timeout
    /// success/receipt. The connectivity failure (unreachable TCP peer) is a
    /// `Transport`/`PeerOffline` send error, not a `PeerRequestSent` receipt.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn peer_request_send_failure_is_typed_send_error_not_timeout() {
        let suffix = Uuid::new_v4().simple().to_string();
        let peer_name = format!("unreachable-peer-{suffix}");
        let runtime_name = format!("send-fail-runtime-{suffix}");

        let peer = CommsRuntime::inproc_only(&peer_name).unwrap();
        let runtime = Arc::new(CommsRuntime::inproc_only(&runtime_name).unwrap());
        install_test_peer_request_response_authority(&runtime);

        // Trust the peer at an unreachable TCP address so the transport send
        // genuinely fails (connection refused) rather than completing.
        add_trusted_peer_with_generated_authority(
            &runtime,
            trusted_descriptor(&peer_name, peer.public_key(), "tcp://127.0.0.1:1"),
        )
        .await;

        let result = CoreCommsRuntime::send(
            runtime.as_ref(),
            CommsCommand::PeerRequest {
                objective_id: None,
                content_taint: None,
                to: peer_route(&peer_name, peer.public_key()),
                intent: "checksum".to_string(),
                params: serde_json::json!({"path": "README.md"}),
                blocks: None,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                stream: InputStreamMode::ReserveInteraction,
            },
        )
        .await;

        // The failure must be a typed send error, never a successful receipt
        // (which would have masqueraded as a delivered-then-timed-out request).
        match result {
            Err(SendError::Transport(_) | SendError::PeerOffline | SendError::PeerNotFound(_)) => {}
            Ok(receipt) => {
                panic!("transport send failure must not produce a success receipt, got {receipt:?}")
            }
            Err(other) => panic!(
                "transport send failure must be a typed connectivity send error, got {other:?}"
            ),
        }
    }

    #[tokio::test]
    async fn test_subscription_correlation_e2e_one_shot() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = test_runtime_config("subscription", &tmp);
        let runtime = CommsRuntime::new(config).await.unwrap();
        install_test_peer_comms_handle(&runtime);

        let injector = CommsEventInjector::new(
            runtime.router.inbox_sender().clone(),
            runtime.subscriber_registry.clone(),
        );
        let sub = injector
            .inject_with_subscription(
                "tracked".to_string().into(),
                meerkat_core::PlainEventSource::Rpc,
                meerkat_core::types::HandlingMode::Queue,
                None,
            )
            .unwrap();
        let tracked_id = sub.id;

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&runtime).await;
        assert_eq!(interactions.len(), 1);
        assert_eq!(interactions[0].id, tracked_id);

        let first = CoreCommsRuntime::interaction_subscriber(&runtime, &tracked_id);
        assert!(first.is_some(), "subscriber should be found");
        let second = CoreCommsRuntime::interaction_subscriber(&runtime, &tracked_id);
        assert!(second.is_none(), "subscriber should be one-shot");
    }

    #[tokio::test]
    async fn test_peer_request_reserved_stream_correlates_via_request_envelope_id() {
        let suffix = Uuid::new_v4().simple().to_string();
        let peer_name = format!("corr-peer-{suffix}");
        let runtime_name = format!("corr-runtime-{suffix}");

        let peer = inproc_only_with_test_peer_authority(&peer_name);
        let runtime = Arc::new(CommsRuntime::inproc_only(&runtime_name).unwrap());
        install_test_peer_request_response_authority(&runtime);
        {
            let mut trusted = runtime.trusted_peers.write();
            trusted
                .upsert(test_trust_entry(
                    &peer_name,
                    peer.public_key(),
                    &format!("inproc://{peer_name}"),
                ))
                .expect("valid test peer should upsert");
        }
        // Peer must also trust the runtime: the receive-side admission gate
        // now drops untrusted envelopes with `UntrustedSender` instead of
        // silently reporting success.
        // Use the generated trust mutation path so the classified inbox's
        // canonical `trusted_peers` BTreeSet is synced too, not just the RwLock.
        add_trusted_peer_with_generated_authority(
            &peer,
            trusted_descriptor(
                &runtime_name,
                runtime.public_key(),
                &format!("inproc://{runtime_name}"),
            ),
        )
        .await;

        let receipt = CoreCommsRuntime::send(
            runtime.as_ref(),
            CommsCommand::PeerRequest {
                objective_id: None,
                content_taint: None,
                to: peer_route(&peer_name, peer.public_key()),
                intent: "checksum".to_string(),
                params: serde_json::json!({"path": "README.md"}),
                blocks: None,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                stream: InputStreamMode::ReserveInteraction,
            },
        )
        .await
        .expect("peer request send should succeed");

        let envelope_id = match receipt {
            SendReceipt::PeerRequestSent { envelope_id, .. } => envelope_id,
            other => panic!("expected PeerRequestSent, got {other:?}"),
        };

        let subscriber =
            CoreCommsRuntime::interaction_subscriber(runtime.as_ref(), &InteractionId(envelope_id));
        assert!(
            subscriber.is_some(),
            "reserved peer-request stream should correlate via the request envelope id carried in in_reply_to"
        );
    }

    #[tokio::test]
    async fn transport_only_runtime_rejects_peer_request_receipts_without_authority() {
        let suffix = Uuid::new_v4().simple().to_string();
        let peer_name = format!("transport-only-peer-{suffix}");
        let runtime_name = format!("transport-only-runtime-{suffix}");

        let peer = CommsRuntime::inproc_only(&peer_name).unwrap();
        let runtime = CommsRuntime::inproc_only(&runtime_name).unwrap();
        add_trusted_peer_with_generated_authority(
            &runtime,
            trusted_descriptor(
                &peer_name,
                peer.public_key(),
                &format!("inproc://{peer_name}"),
            ),
        )
        .await;
        add_trusted_peer_with_generated_authority(
            &peer,
            trusted_descriptor(
                &runtime_name,
                runtime.public_key(),
                &format!("inproc://{runtime_name}"),
            ),
        )
        .await;

        let result = CoreCommsRuntime::send(
            &runtime,
            CommsCommand::PeerRequest {
                objective_id: None,
                content_taint: None,
                to: peer_route(&peer_name, peer.public_key()),
                intent: "must-have-authority".to_string(),
                params: serde_json::json!({}),
                blocks: None,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                stream: InputStreamMode::None,
            },
        )
        .await;

        assert!(
            matches!(result, Err(SendError::Validation(ref message)) if message.contains("machine peer interaction authority")),
            "transport-only runtime must fail before emitting PeerRequestSent, got {result:?}"
        );
    }

    #[tokio::test]
    async fn transport_only_runtime_rejects_peer_request_send_and_stream_without_authority() {
        let suffix = Uuid::new_v4().simple().to_string();
        let peer_name = format!("transport-only-stream-peer-{suffix}");
        let runtime_name = format!("transport-only-stream-runtime-{suffix}");

        let peer = CommsRuntime::inproc_only(&peer_name).unwrap();
        let runtime = CommsRuntime::inproc_only(&runtime_name).unwrap();
        add_trusted_peer_with_generated_authority(
            &runtime,
            trusted_descriptor(
                &peer_name,
                peer.public_key(),
                &format!("inproc://{peer_name}"),
            ),
        )
        .await;
        add_trusted_peer_with_generated_authority(
            &peer,
            trusted_descriptor(
                &runtime_name,
                runtime.public_key(),
                &format!("inproc://{runtime_name}"),
            ),
        )
        .await;

        let result = CoreCommsRuntime::send_and_stream(
            &runtime,
            CommsCommand::PeerRequest {
                objective_id: None,
                content_taint: None,
                to: peer_route(&peer_name, peer.public_key()),
                intent: "must-have-stream-authority".to_string(),
                params: serde_json::json!({}),
                blocks: None,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                stream: InputStreamMode::ReserveInteraction,
            },
        )
        .await;

        assert!(
            matches!(
                result,
                Err(SendAndStreamError::Send(SendError::Validation(ref message)))
                    if message.contains("machine peer interaction authority")
            ),
            "transport-only runtime must fail before emitting streamed PeerRequestSent"
        );
    }

    #[tokio::test]
    async fn transport_only_runtime_rejects_peer_response_receipts_without_authority() {
        let suffix = Uuid::new_v4().simple().to_string();
        let peer_name = format!("transport-only-response-peer-{suffix}");
        let runtime_name = format!("transport-only-response-runtime-{suffix}");

        let peer = CommsRuntime::inproc_only(&peer_name).unwrap();
        let runtime = CommsRuntime::inproc_only(&runtime_name).unwrap();
        add_trusted_peer_with_generated_authority(
            &runtime,
            trusted_descriptor(
                &peer_name,
                peer.public_key(),
                &format!("inproc://{peer_name}"),
            ),
        )
        .await;
        add_trusted_peer_with_generated_authority(
            &peer,
            trusted_descriptor(
                &runtime_name,
                runtime.public_key(),
                &format!("inproc://{runtime_name}"),
            ),
        )
        .await;

        let result = CoreCommsRuntime::send(
            &runtime,
            CommsCommand::PeerResponse {
                objective_id: None,
                content_taint: None,
                to: peer_route(&peer_name, peer.public_key()),
                in_reply_to: meerkat_core::InteractionId(Uuid::new_v4()),
                status: meerkat_core::ResponseStatus::Completed,
                result: serde_json::json!({"ok": true}),
                blocks: None,
                handling_mode: Some(meerkat_core::types::HandlingMode::Queue),
            },
        )
        .await;

        assert!(
            matches!(result, Err(SendError::Validation(ref message)) if message.contains("machine peer interaction authority")),
            "transport-only runtime must fail before emitting PeerResponseSent, got {result:?}"
        );
    }

    #[tokio::test]
    async fn terminal_peer_response_route_failure_keeps_inbound_request_retryable() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = Arc::new(
            CommsRuntime::inproc_only(&format!("response-route-failure-{suffix}")).unwrap(),
        );
        let peer_handle = Arc::new(TestPeerInteractionHandle::default());
        runtime.install_peer_request_response_authority(PeerRequestResponseAuthority::new(
            peer_handle.clone(),
            Arc::new(TestInteractionStreamHandle::default()),
        ));

        let interaction_id = Uuid::new_v4();
        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(interaction_id);
        meerkat_core::handles::PeerInteractionHandle::request_received(
            peer_handle.as_ref(),
            corr_id,
            meerkat_core::types::HandlingMode::Queue,
        )
        .expect("test authority should seed inbound request state");

        let result = CoreCommsRuntime::send(
            runtime.as_ref(),
            CommsCommand::PeerResponse {
                objective_id: None,
                content_taint: None,
                to: missing_peer_route(&format!("missing-response-peer-{suffix}")),
                in_reply_to: InteractionId(interaction_id),
                status: meerkat_core::ResponseStatus::Completed,
                result: serde_json::json!({"ok": true}),
                blocks: None,
                handling_mode: Some(meerkat_core::types::HandlingMode::Queue),
            },
        )
        .await;

        assert!(
            matches!(result, Err(SendError::PeerNotFound(_))),
            "test must fail at transport routing, got {result:?}"
        );
        assert_eq!(
            meerkat_core::handles::PeerInteractionHandle::inbound_state(
                peer_handle.as_ref(),
                corr_id
            ),
            Some(meerkat_core::InboundPeerRequestState::Received),
            "failed terminal PeerResponse send must leave inbound request state retryable"
        );
    }

    #[tokio::test]
    async fn terminal_peer_response_default_handling_mode_projects_from_machine_authority() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender_name = format!("response-default-sender-{suffix}");
        let receiver_name = format!("response-default-receiver-{suffix}");
        let sender = Arc::new(CommsRuntime::inproc_only(&sender_name).unwrap());
        let receiver = Arc::new(CommsRuntime::inproc_only(&receiver_name).unwrap());
        let peer_handle = Arc::new(TestPeerInteractionHandle::default());
        sender.install_peer_request_response_authority(PeerRequestResponseAuthority::new(
            peer_handle.clone(),
            Arc::new(TestInteractionStreamHandle::default()),
        ));
        install_test_peer_comms_handle(receiver.as_ref());
        let receiver_peer_handle = Arc::new(TestPeerInteractionHandle::default());
        receiver.install_peer_request_response_authority(PeerRequestResponseAuthority::new(
            receiver_peer_handle.clone(),
            Arc::new(TestInteractionStreamHandle::default()),
        ));

        add_trusted_peer_with_generated_authority(
            sender.as_ref(),
            trusted_descriptor(
                &receiver_name,
                receiver.public_key(),
                &format!("inproc://{receiver_name}"),
            ),
        )
        .await;
        add_trusted_peer_with_generated_authority(
            receiver.as_ref(),
            trusted_descriptor(
                &sender_name,
                sender.public_key(),
                &format!("inproc://{sender_name}"),
            ),
        )
        .await;

        let interaction_id = Uuid::new_v4();
        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(interaction_id);
        meerkat_core::handles::PeerInteractionHandle::request_received(
            peer_handle.as_ref(),
            corr_id,
            meerkat_core::types::HandlingMode::Queue,
        )
        .expect("machine authority should record inbound request lane");
        assert_eq!(
            meerkat_core::handles::PeerInteractionHandle::inbound_handling_mode(
                peer_handle.as_ref(),
                corr_id
            ),
            Some(meerkat_core::types::HandlingMode::Queue)
        );
        meerkat_core::handles::PeerInteractionHandle::request_sent(
            receiver_peer_handle.as_ref(),
            corr_id,
        )
        .expect("receiver machine authority should expect the response");

        let receipt = CoreCommsRuntime::send(
            sender.as_ref(),
            CommsCommand::PeerResponse {
                objective_id: None,
                content_taint: None,
                to: peer_route(&receiver_name, receiver.public_key()),
                in_reply_to: InteractionId(interaction_id),
                status: meerkat_core::ResponseStatus::Completed,
                result: serde_json::json!({"ok": true}),
                blocks: None,
                handling_mode: None,
            },
        )
        .await
        .expect("terminal response send should succeed");
        assert!(matches!(receipt, SendReceipt::PeerResponseSent { .. }));

        let interactions = CoreCommsRuntime::drain_inbox_interactions(receiver.as_ref()).await;
        assert_eq!(interactions.len(), 1);
        assert_eq!(
            interactions[0].handling_mode,
            meerkat_core::types::HandlingMode::Queue
        );
        assert!(
            meerkat_core::handles::PeerInteractionHandle::inbound_handling_mode(
                peer_handle.as_ref(),
                corr_id
            )
            .is_none()
        );
        assert!(matches!(
            &interactions[0].content,
            meerkat_core::InteractionContent::Response { in_reply_to, .. }
                if in_reply_to.0 == interaction_id
        ));
    }

    #[tokio::test]
    async fn transport_only_peer_directory_does_not_advertise_semantic_request_response() {
        let suffix = Uuid::new_v4().simple().to_string();
        let peer_name = format!("directory-transport-peer-{suffix}");
        let runtime_name = format!("directory-transport-runtime-{suffix}");

        let peer = CommsRuntime::inproc_only(&peer_name).unwrap();
        let runtime = CommsRuntime::inproc_only(&runtime_name).unwrap();
        add_trusted_peer_with_generated_authority(
            &runtime,
            trusted_descriptor(
                &peer_name,
                peer.public_key(),
                &format!("inproc://{peer_name}"),
            ),
        )
        .await;

        let peers = CoreCommsRuntime::peers(&runtime).await;
        let entry = peers
            .iter()
            .find(|entry| entry.name.as_str() == peer_name)
            .expect("trusted peer should be listed");

        assert_eq!(entry.sendable_kinds, vec![PeerSendability::PeerMessage]);
    }

    #[tokio::test]
    async fn partial_peer_authority_does_not_advertise_semantic_request_response() {
        let suffix = Uuid::new_v4().simple().to_string();
        let peer_name = format!("directory-partial-peer-{suffix}");
        let runtime_name = format!("directory-partial-runtime-{suffix}");

        let peer = CommsRuntime::inproc_only(&peer_name).unwrap();
        let runtime = Arc::new(CommsRuntime::inproc_only(&runtime_name).unwrap());
        runtime.install_peer_interaction_handle(Arc::new(TestPeerInteractionHandle::default()));
        add_trusted_peer_with_generated_authority(
            runtime.as_ref(),
            trusted_descriptor(
                &peer_name,
                peer.public_key(),
                &format!("inproc://{peer_name}"),
            ),
        )
        .await;

        let peers = CoreCommsRuntime::peers(runtime.as_ref()).await;
        let entry = peers
            .iter()
            .find(|entry| entry.name.as_str() == peer_name)
            .expect("trusted peer should be listed");

        assert_eq!(entry.sendable_kinds, vec![PeerSendability::PeerMessage]);
    }

    #[tokio::test]
    async fn partial_peer_authority_rejects_peer_request_receipts_without_stream_authority() {
        let suffix = Uuid::new_v4().simple().to_string();
        let peer_name = format!("partial-authority-peer-{suffix}");
        let runtime_name = format!("partial-authority-runtime-{suffix}");

        let peer = CommsRuntime::inproc_only(&peer_name).unwrap();
        let runtime = Arc::new(CommsRuntime::inproc_only(&runtime_name).unwrap());
        runtime.install_peer_interaction_handle(Arc::new(TestPeerInteractionHandle::default()));
        add_trusted_peer_with_generated_authority(
            runtime.as_ref(),
            trusted_descriptor(
                &peer_name,
                peer.public_key(),
                &format!("inproc://{peer_name}"),
            ),
        )
        .await;
        add_trusted_peer_with_generated_authority(
            &peer,
            trusted_descriptor(
                &runtime_name,
                runtime.public_key(),
                &format!("inproc://{runtime_name}"),
            ),
        )
        .await;

        let result = CoreCommsRuntime::send(
            runtime.as_ref(),
            CommsCommand::PeerRequest {
                objective_id: None,
                content_taint: None,
                to: peer_route(&peer_name, peer.public_key()),
                intent: "must-have-complete-authority".to_string(),
                params: serde_json::json!({}),
                blocks: None,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                stream: InputStreamMode::None,
            },
        )
        .await;

        assert!(
            matches!(result, Err(SendError::Validation(ref message)) if message.contains("machine interaction stream authority")),
            "partial peer authority must fail before emitting PeerRequestSent, got {result:?}"
        );
    }

    #[tokio::test]
    async fn partial_peer_authority_rejects_peer_response_receipts_without_stream_authority() {
        let suffix = Uuid::new_v4().simple().to_string();
        let peer_name = format!("partial-response-peer-{suffix}");
        let runtime_name = format!("partial-response-runtime-{suffix}");

        let peer = CommsRuntime::inproc_only(&peer_name).unwrap();
        let runtime = Arc::new(CommsRuntime::inproc_only(&runtime_name).unwrap());
        let peer_handle = Arc::new(TestPeerInteractionHandle::default());
        runtime.install_peer_interaction_handle(peer_handle.clone());
        add_trusted_peer_with_generated_authority(
            runtime.as_ref(),
            trusted_descriptor(
                &peer_name,
                peer.public_key(),
                &format!("inproc://{peer_name}"),
            ),
        )
        .await;
        add_trusted_peer_with_generated_authority(
            &peer,
            trusted_descriptor(
                &runtime_name,
                runtime.public_key(),
                &format!("inproc://{runtime_name}"),
            ),
        )
        .await;

        let interaction_id = Uuid::new_v4();
        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(interaction_id);
        meerkat_core::handles::PeerInteractionHandle::request_received(
            peer_handle.as_ref(),
            corr_id,
            meerkat_core::types::HandlingMode::Queue,
        )
        .expect("seed inbound request state");

        let result = CoreCommsRuntime::send(
            runtime.as_ref(),
            CommsCommand::PeerResponse {
                objective_id: None,
                content_taint: None,
                to: peer_route(&peer_name, peer.public_key()),
                in_reply_to: InteractionId(interaction_id),
                status: meerkat_core::ResponseStatus::Completed,
                result: serde_json::json!({"ok": true}),
                blocks: None,
                handling_mode: Some(meerkat_core::types::HandlingMode::Queue),
            },
        )
        .await;

        assert!(
            matches!(result, Err(SendError::Validation(ref message)) if message.contains("machine interaction stream authority")),
            "partial peer authority must fail before emitting PeerResponseSent, got {result:?}"
        );
        assert_eq!(
            meerkat_core::handles::PeerInteractionHandle::inbound_state(
                peer_handle.as_ref(),
                corr_id
            ),
            Some(meerkat_core::InboundPeerRequestState::Received),
            "failed PeerResponse must leave inbound request state retryable"
        );
    }

    #[tokio::test]
    async fn peer_directory_advertises_semantic_request_response_with_authority() {
        let suffix = Uuid::new_v4().simple().to_string();
        let peer_name = format!("directory-semantic-peer-{suffix}");
        let runtime_name = format!("directory-semantic-runtime-{suffix}");

        let peer = CommsRuntime::inproc_only(&peer_name).unwrap();
        let runtime = Arc::new(CommsRuntime::inproc_only(&runtime_name).unwrap());
        install_test_peer_request_response_authority(&runtime);
        add_trusted_peer_with_generated_authority(
            runtime.as_ref(),
            trusted_descriptor(
                &peer_name,
                peer.public_key(),
                &format!("inproc://{peer_name}"),
            ),
        )
        .await;

        let peers = CoreCommsRuntime::peers(runtime.as_ref()).await;
        let entry = peers
            .iter()
            .find(|entry| entry.name.as_str() == peer_name)
            .expect("trusted peer should be listed");

        assert_eq!(
            entry.sendable_kinds,
            vec![
                PeerSendability::PeerMessage,
                PeerSendability::PeerRequest,
                PeerSendability::PeerResponse,
            ]
        );
    }

    #[tokio::test]
    async fn test_peer_lifecycle_send_stays_typed_and_non_rendered() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender_name = format!("life-sender-{suffix}");
        let receiver_name = format!("life-receiver-{suffix}");

        let sender = CommsRuntime::inproc_only(&sender_name).unwrap();
        let receiver = inproc_only_with_test_peer_authority(&receiver_name);

        add_trusted_peer_with_generated_authority(
            &sender,
            trusted_descriptor(
                &receiver_name,
                receiver.public_key(),
                &format!("inproc://{receiver_name}"),
            ),
        )
        .await;
        add_trusted_peer_with_generated_authority(
            &receiver,
            trusted_descriptor(
                &sender_name,
                sender.public_key(),
                &format!("inproc://{sender_name}"),
            ),
        )
        .await;

        let receipt = CoreCommsRuntime::send(
            &sender,
            CommsCommand::PeerLifecycle {
                to: peer_route(&receiver_name, receiver.public_key()),
                kind: meerkat_core::comms::PeerLifecycleKind::PeerAdded,
                params: serde_json::json!({
                    "peer": "worker-1",
                    "role": "worker",
                }),
            },
        )
        .await
        .expect("peer lifecycle send should succeed");

        match receipt {
            SendReceipt::PeerLifecycleSent { .. } => {}
            other => panic!("expected PeerLifecycleSent, got {other:?}"),
        }

        let interactions = receiver
            .drain_classified_inbox_interactions()
            .await
            .expect("classified drain should succeed");
        assert_eq!(interactions.len(), 1);
        assert_eq!(
            interactions[0].class(),
            meerkat_core::PeerInputClass::PeerLifecycleAdded
        );
        assert_eq!(interactions[0].lifecycle_peer.as_deref(), Some("worker-1"));
        assert!(
            interactions[0].interaction.rendered_text.is_empty(),
            "silent lifecycle notices must not synthesize prompt text"
        );
    }

    #[tokio::test]
    async fn test_subscription_correlation_preserves_inline_blocks() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = test_runtime_config("subscription-blocks", &tmp);
        let runtime = CommsRuntime::new(config).await.unwrap();
        install_test_peer_comms_handle(&runtime);

        let injector = CommsEventInjector::new(
            runtime.router.inbox_sender().clone(),
            runtime.subscriber_registry.clone(),
        );
        let sub = injector
            .inject_with_subscription(
                meerkat_core::types::ContentInput::Blocks(vec![
                    meerkat_core::types::ContentBlock::Text {
                        text: "tracked".to_string(),
                    },
                    meerkat_core::types::ContentBlock::Image {
                        media_type: "image/png".to_string(),
                        data: "aGVsbG8=".into(),
                    },
                ]),
                meerkat_core::PlainEventSource::Rpc,
                meerkat_core::types::HandlingMode::Queue,
                None,
            )
            .unwrap();
        let tracked_id = sub.id;

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&runtime).await;
        assert_eq!(interactions.len(), 1);
        assert_eq!(interactions[0].id, tracked_id);
        match &interactions[0].content {
            meerkat_core::InteractionContent::Message { blocks, .. } => {
                let blocks = blocks.as_ref().expect("blocks preserved");
                assert!(
                    blocks.iter().any(|block| matches!(
                        block,
                        meerkat_core::types::ContentBlock::Image { .. }
                    )),
                    "expected inline image block to survive into drained interaction"
                );
            }
            other => panic!("expected message interaction, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn runtime_bridge_red_ok_send_and_stream_reserves_one_interaction_channel() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = inproc_only_with_test_peer_authority(&format!("phase1-bridge-{suffix}"));

        let cmd = CommsCommand::Input {
            blocks: None,
            session_id: SessionId::new(),
            body: "phase 1 bridge input".to_string(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            source: InputSource::Rpc,
            stream: InputStreamMode::ReserveInteraction,
            allow_self_session: true,
        };

        let (receipt, _stream) = CoreCommsRuntime::send_and_stream(&runtime, cmd)
            .await
            .expect("send_and_stream should reserve interaction scope");

        let interaction_id = match receipt {
            SendReceipt::InputAccepted {
                interaction_id,
                stream_reserved,
            } => {
                assert!(
                    stream_reserved,
                    "bridge receipt should advertise reservation"
                );
                interaction_id
            }
            other => panic!("expected InputAccepted, got {other:?}"),
        };

        let duplicate =
            CoreCommsRuntime::stream(&runtime, StreamScope::Interaction(interaction_id));
        assert!(
            matches!(duplicate, Err(StreamError::AlreadyAttached(_))),
            "reserved interaction streams should reject a second attachment"
        );
    }

    #[tokio::test]
    async fn local_input_stream_uses_installed_machine_stream_authority() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = Arc::new(inproc_only_with_test_peer_authority(&format!(
            "local-transport-authority-{suffix}"
        )));
        let stream_handle = Arc::new(TestInteractionStreamHandle::default());
        runtime.install_interaction_stream_handle(stream_handle.clone());

        let cmd = CommsCommand::Input {
            blocks: None,
            session_id: SessionId::new(),
            body: "local input stream".to_string(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            source: InputSource::Rpc,
            stream: InputStreamMode::ReserveInteraction,
            allow_self_session: true,
        };

        let (receipt, _stream) = CoreCommsRuntime::send_and_stream(runtime.as_ref(), cmd)
            .await
            .expect("local input send_and_stream should attach without semantic stream authority");

        let interaction_id = match receipt {
            SendReceipt::InputAccepted {
                interaction_id,
                stream_reserved,
            } => {
                assert!(stream_reserved);
                interaction_id
            }
            other => panic!("expected InputAccepted, got {other:?}"),
        };
        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(interaction_id.0);
        assert_eq!(
            meerkat_core::handles::InteractionStreamHandle::state(stream_handle.as_ref(), corr_id),
            Some(meerkat_core::InteractionStreamState::Attached),
            "an installed machine handle must own local input stream lifecycle"
        );

        let duplicate =
            CoreCommsRuntime::stream(runtime.as_ref(), StreamScope::Interaction(interaction_id));
        assert!(
            matches!(duplicate, Err(StreamError::AlreadyAttached(_))),
            "machine-owned local input streams should reject duplicate attachment"
        );
    }

    #[tokio::test]
    async fn runtime_bridge_red_ok_completed_interaction_terminates_reserved_stream() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = inproc_only_with_test_peer_authority(&format!("phase1-complete-{suffix}"));

        let receipt = CoreCommsRuntime::send(
            &runtime,
            CommsCommand::Input {
                blocks: None,
                session_id: SessionId::new(),
                body: "complete reserved interaction".to_string(),
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                source: InputSource::Rpc,
                stream: InputStreamMode::ReserveInteraction,
                allow_self_session: true,
            },
        )
        .await
        .expect("send should succeed");

        let interaction_id = match receipt {
            SendReceipt::InputAccepted { interaction_id, .. } => interaction_id,
            other => panic!("expected InputAccepted, got {other:?}"),
        };

        let mut stream =
            CoreCommsRuntime::stream(&runtime, StreamScope::Interaction(interaction_id))
                .expect("reserved stream should attach");
        runtime.mark_interaction_complete(interaction_id.0);

        let terminal = timeout(Duration::from_millis(100), stream.next())
            .await
            .expect("stream should terminate promptly after completion");
        assert!(
            terminal.is_none(),
            "interaction completion should close the reserved stream for parent waiters"
        );
    }

    #[tokio::test]
    async fn test_plain_event_interaction_id_is_preserved_in_drain_inbox_interactions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = test_runtime_config("plain-id", &tmp);
        let runtime = CommsRuntime::new(config).await.unwrap();
        install_test_peer_comms_handle(&runtime);

        let interaction_id = Uuid::new_v4();
        runtime
            .router
            .inbox_sender()
            .send_classified(InboxItem::PlainEvent {
                objective_id: None,
                blocks: None,
                body: "evt".to_string(),
                source: meerkat_core::PlainEventSource::Tcp,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                interaction_id: Some(interaction_id),
                render_metadata: None,
            })
            .into_result()
            .unwrap();

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&runtime).await;
        assert_eq!(interactions.len(), 1);
        assert_eq!(interactions[0].id.0, interaction_id);
    }

    #[tokio::test]
    async fn test_plain_event_preserves_handling_mode_and_render_metadata_in_classified_drain() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = test_runtime_config("plain-hints", &tmp);
        let runtime = CommsRuntime::new(config).await.unwrap();
        install_test_peer_comms_handle(&runtime);
        let render_metadata = meerkat_core::types::RenderMetadata {
            class: meerkat_core::types::RenderClass::ExternalEvent,
            salience: meerkat_core::types::RenderSalience::Urgent,
        };

        runtime
            .router
            .inbox_sender()
            .send_classified(InboxItem::PlainEvent {
                objective_id: None,
                blocks: None,
                body: "evt".to_string(),
                source: meerkat_core::PlainEventSource::Tcp,
                handling_mode: meerkat_core::types::HandlingMode::Steer,
                interaction_id: None,
                render_metadata: Some(render_metadata.clone()),
            })
            .into_result()
            .unwrap();

        let interactions = runtime.drain_classified_inbox_interactions().await.unwrap();
        assert_eq!(interactions.len(), 1);
        let interaction = &interactions[0].interaction;
        assert_eq!(
            interaction.handling_mode,
            meerkat_core::types::HandlingMode::Steer
        );
        assert_eq!(interaction.render_metadata, Some(render_metadata));
    }

    #[tokio::test]
    async fn test_plain_event_without_interaction_id_gets_stable_id_between_snapshot_and_drain() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = test_runtime_config("plain-generated-id", &tmp);
        let runtime = CommsRuntime::new(config).await.unwrap();
        install_test_peer_comms_handle(&runtime);

        runtime
            .router
            .inbox_sender()
            .send_classified(InboxItem::PlainEvent {
                objective_id: None,
                blocks: None,
                body: "evt".to_string(),
                source: meerkat_core::PlainEventSource::Tcp,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                interaction_id: None,
                render_metadata: None,
            })
            .into_result()
            .unwrap();

        let snapshot = CoreCommsRuntime::peer_ingress_queue_snapshot(&runtime)
            .await
            .expect("classified queue snapshot should be available");
        let ingress_id = snapshot
            .queued_entries
            .first()
            .and_then(|entry| entry.interaction_id)
            .expect("plain event should have a stable generated interaction id at ingress")
            .0;
        assert_eq!(
            snapshot.queued_entries[0].raw_item_id,
            meerkat_core::InteractionId(ingress_id),
            "the queue snapshot should expose the same stable ingress id it uses as the raw item key"
        );

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&runtime).await;
        assert_eq!(interactions.len(), 1);
        assert_eq!(interactions[0].id.0, ingress_id);
    }

    #[tokio::test]
    async fn test_peer_ingress_queue_snapshot_reflects_classified_queue_without_draining() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut config = test_runtime_config("peer-snapshot", &tmp);
        config.require_peer_auth = false;
        let runtime = CommsRuntime::new(config).await.unwrap();
        install_test_peer_comms_handle(&runtime);

        let sender = Keypair::generate();
        let envelope = signed_envelope(
            &sender,
            runtime.public_key,
            crate::types::MessageKind::Request {
                objective_id: None,
                intent: "review".to_string(),
                params: serde_json::json!({"pr": 42}),
                blocks: None,
                reply_endpoint: None,
                content_taint: None,
                handling_mode: None,
            },
        );

        runtime
            .router
            .inbox_sender()
            .send_classified(InboxItem::External { envelope })
            .into_result()
            .unwrap();
        runtime
            .router
            .inbox_sender()
            .send_classified(InboxItem::PlainEvent {
                objective_id: None,
                blocks: None,
                body: "evt".to_string(),
                source: meerkat_core::PlainEventSource::Tcp,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                interaction_id: None,
                render_metadata: None,
            })
            .into_result()
            .unwrap();

        let snapshot = CoreCommsRuntime::peer_ingress_queue_snapshot(&runtime)
            .await
            .expect("classified queue snapshot should be available");
        assert_eq!(snapshot.total_count, 2);
        assert_eq!(snapshot.actionable_count, 2);
        assert_eq!(snapshot.plain_event_count, 1);
        assert_eq!(snapshot.response_count, 0);
        assert_eq!(snapshot.lifecycle_count, 0);
        assert_eq!(snapshot.queued_entries.len(), 2);
        assert_eq!(
            snapshot.queued_entries[0].kind,
            meerkat_core::PeerIngressKind::Request
        );
        assert_eq!(
            snapshot.queued_entries[1].kind,
            meerkat_core::PeerIngressKind::PlainEvent
        );

        let interactions = runtime.drain_classified_inbox_interactions().await.unwrap();
        assert_eq!(
            interactions.len(),
            2,
            "snapshot must not consume classified peer ingress"
        );
    }

    #[tokio::test]
    async fn test_peer_ingress_runtime_snapshot_reflects_trusted_peers_and_queue() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut config = test_runtime_config("peer-runtime-snapshot", &tmp);
        config.require_peer_auth = false;
        let runtime = CommsRuntime::new(config).await.unwrap();
        install_test_peer_comms_handle(&runtime);

        let peer_key = Keypair::generate();
        let trusted_peer = trusted_descriptor("ally", peer_key.public_key(), "inproc://ally");

        add_trusted_peer_with_generated_authority(&runtime, trusted_peer.clone()).await;

        runtime
            .event_injector()
            .inject(
                "evt".to_string().into(),
                meerkat_core::PlainEventSource::Tcp,
                meerkat_core::types::HandlingMode::Queue,
                None,
            )
            .expect("plain event injection should succeed");

        let snapshot = CoreCommsRuntime::peer_ingress_runtime_snapshot(&runtime)
            .await
            .expect("peer runtime snapshot should be available");
        assert_eq!(snapshot.self_peer_id, runtime.public_key().to_peer_id());
        assert!(!snapshot.auth_required);
        assert_eq!(
            snapshot.authority_phase,
            meerkat_core::PeerIngressAuthorityPhase::Absent
        );
        assert_eq!(snapshot.trusted_peers, vec![trusted_peer]);
        assert_eq!(snapshot.submission_queue_len, 1);
        assert_eq!(snapshot.queue.total_count, 1);
        assert_eq!(snapshot.queue.plain_event_count, 1);
        assert_eq!(snapshot.queue.queued_entries.len(), 1);
        assert_eq!(
            snapshot.queue.queued_entries[0].kind,
            meerkat_core::PeerIngressKind::PlainEvent
        );
        assert_eq!(snapshot.queue.queued_entries[0].admission_diagnostic, None);
        assert_eq!(
            snapshot.queue.queued_entries[0].raw_item_id,
            snapshot.queue.queued_entries[0]
                .interaction_id
                .expect("plain event should retain an ingress interaction id")
        );
    }

    #[tokio::test]
    async fn test_peer_ingress_runtime_snapshot_reflects_dropped_peer_authority_state() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = test_runtime_config("peer-runtime-dropped", &tmp);
        let runtime = CommsRuntime::new(config).await.unwrap();
        install_test_peer_comms_handle(&runtime);

        let sender = Keypair::generate();
        let envelope = signed_envelope(
            &sender,
            runtime.public_key(),
            MessageKind::Request {
                objective_id: None,
                intent: "review".to_string(),
                params: serde_json::json!({"scope": "peer"}),
                blocks: None,
                reply_endpoint: None,
                content_taint: None,
                handling_mode: None,
            },
        );

        // Untrusted sender, require_peer_auth on: the admission gate drops
        // the envelope with `UntrustedSender`. We assert the typed outcome
        // directly so the silent-Ok path can't creep back.
        let outcome = runtime
            .router
            .inbox_sender()
            .send_classified(InboxItem::External { envelope });
        assert_eq!(
            outcome,
            crate::inbox::AdmissionOutcome::Dropped {
                reason: crate::inbox::DropReason::UntrustedSender
            }
        );

        let snapshot = CoreCommsRuntime::peer_ingress_runtime_snapshot(&runtime)
            .await
            .expect("peer runtime snapshot should be available");
        assert_eq!(
            snapshot.authority_phase,
            meerkat_core::PeerIngressAuthorityPhase::Dropped
        );
        assert_eq!(snapshot.submission_queue_len, 0);
        assert_eq!(snapshot.queue.total_count, 0);
        assert!(snapshot.trusted_peers.is_empty());
    }

    #[tokio::test]
    async fn test_live_peer_authority_syncs_trust_receive_and_drain() {
        use crate::peer_types::PeerIngressState;

        let tmp = tempfile::TempDir::new().unwrap();
        let config = test_runtime_config("peer-authority-sync", &tmp);
        let runtime = CommsRuntime::new(config).await.unwrap();
        install_test_peer_comms_handle(&runtime);

        let sender = Keypair::generate();
        let pubkey_for_authority_check = sender.public_key();
        let envelope = signed_envelope(
            &sender,
            runtime.public_key(),
            MessageKind::Request {
                objective_id: None,
                intent: "review".to_string(),
                params: serde_json::json!({"scope": "peer"}),
                blocks: None,
                reply_endpoint: None,
                content_taint: None,
                handling_mode: None,
            },
        );

        // Untrusted sender under require_peer_auth: must be explicitly
        // dropped at admission with a typed reason — no more silent Ok(()).
        let outcome = runtime
            .router
            .inbox_sender()
            .send_classified(InboxItem::External {
                envelope: envelope.clone(),
            });
        assert_eq!(
            outcome,
            crate::inbox::AdmissionOutcome::Dropped {
                reason: crate::inbox::DropReason::UntrustedSender
            }
        );

        {
            let inbox = runtime.inbox.lock().await;
            assert_eq!(
                inbox
                    .classified_snapshot()
                    .expect("classified snapshot should exist")
                    .total_count,
                0
            );
            assert_eq!(
                inbox.peer_authority_test_snapshot(),
                Some((PeerIngressState::Dropped, 0))
            );
            assert_eq!(
                inbox.peer_authority_trusts_peer_for_test(&pubkey_for_authority_check),
                Some(false)
            );
            // Drop counter ticked exactly once — the bug repair is visible.
            assert_eq!(inbox.dropped_count(), Some(1));
        }

        let trusted_peer = trusted_descriptor("sender", sender.public_key(), "inproc://sender");
        let trust_dsl =
            add_trusted_peer_with_generated_authority(&runtime, trusted_peer.clone()).await;

        {
            let inbox = runtime.inbox.lock().await;
            assert_eq!(
                inbox.peer_authority_test_snapshot(),
                Some((PeerIngressState::Dropped, 0))
            );
            assert_eq!(
                inbox.peer_authority_trusts_peer_for_test(&pubkey_for_authority_check),
                Some(true)
            );
        }

        runtime
            .router
            .inbox_sender()
            .send_classified(InboxItem::External { envelope })
            .into_result()
            .unwrap();

        let snapshot = CoreCommsRuntime::peer_ingress_runtime_snapshot(&runtime)
            .await
            .expect("peer runtime snapshot should be available");
        assert_eq!(
            snapshot.authority_phase,
            meerkat_core::PeerIngressAuthorityPhase::Received
        );
        assert_eq!(snapshot.submission_queue_len, 1);
        assert_eq!(snapshot.queue.total_count, 1);
        assert_eq!(
            snapshot.queue.queued_entries[0].admission_diagnostic,
            Some(meerkat_core::PeerIngressAdmissionDiagnostic::TrustedAtAdmission)
        );

        {
            let inbox = runtime.inbox.lock().await;
            assert_eq!(
                inbox
                    .classified_snapshot()
                    .expect("classified snapshot should exist")
                    .total_count,
                1
            );
            assert_eq!(
                inbox.peer_authority_test_snapshot(),
                Some((PeerIngressState::Received, 1))
            );
        }

        let interactions = runtime.drain_classified_inbox_interactions().await.unwrap();
        assert_eq!(interactions.len(), 1);

        {
            let inbox = runtime.inbox.lock().await;
            assert_eq!(
                inbox.peer_authority_test_snapshot(),
                Some((PeerIngressState::Delivered, 0))
            );
        }

        let remove_authority =
            test_projection_remove_authority_with_dsl(Arc::clone(&trust_dsl), &trusted_peer);
        let removed = CoreCommsRuntime::apply_trust_mutation(
            &runtime,
            CommsTrustMutation::RemoveTrustedPeer {
                peer_id: trusted_peer.peer_id.to_string(),
                authority: remove_authority.authority,
            },
        )
        .await
        .expect("remove_trusted_peer should succeed");
        let CommsTrustMutationResult::Removed { removed } = removed else {
            panic!("unexpected generated trust mutation result: {removed:?}");
        };
        assert!(removed);

        {
            let inbox = runtime.inbox.lock().await;
            assert_eq!(
                inbox.peer_authority_test_snapshot(),
                Some((PeerIngressState::Delivered, 0))
            );
            assert_eq!(
                inbox.peer_authority_trusts_peer_for_test(&pubkey_for_authority_check),
                Some(false)
            );
        }
    }

    #[tokio::test]
    async fn test_live_peer_authority_accepts_unknown_peer_when_auth_open() {
        use crate::peer_types::PeerIngressState;

        let tmp = tempfile::TempDir::new().unwrap();
        let mut config = test_runtime_config("peer-authority-auth-open", &tmp);
        config.require_peer_auth = false;
        let runtime = CommsRuntime::new(config).await.unwrap();
        install_test_peer_comms_handle(&runtime);

        let sender = Keypair::generate();
        let envelope = signed_envelope(
            &sender,
            runtime.public_key(),
            MessageKind::Request {
                objective_id: None,
                intent: "status".to_string(),
                params: serde_json::json!({}),
                blocks: None,
                reply_endpoint: None,
                content_taint: None,
                handling_mode: None,
            },
        );

        runtime
            .router
            .inbox_sender()
            .send_classified(InboxItem::External { envelope })
            .into_result()
            .unwrap();

        {
            let inbox = runtime.inbox.lock().await;
            assert_eq!(
                inbox
                    .classified_snapshot()
                    .expect("classified snapshot should exist")
                    .total_count,
                1
            );
            assert_eq!(
                inbox.peer_authority_test_snapshot(),
                Some((PeerIngressState::Received, 1))
            );
        }

        let interactions = runtime.drain_classified_inbox_interactions().await.unwrap();
        assert_eq!(interactions.len(), 1);

        {
            let inbox = runtime.inbox.lock().await;
            assert_eq!(
                inbox.peer_authority_test_snapshot(),
                Some((PeerIngressState::Delivered, 0))
            );
        }
    }

    #[test]
    fn test_interaction_subscriber_correlation_miss_returns_none() {
        let runtime = CommsRuntime::inproc_only("corr-miss").unwrap();
        let random = meerkat_core::InteractionId(Uuid::new_v4());
        let sender = CoreCommsRuntime::interaction_subscriber(&runtime, &random);
        assert!(sender.is_none());
    }

    #[tokio::test]
    async fn test_core_send_input_no_reservation() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = inproc_only_with_test_peer_authority(&format!("input-nores-{suffix}"));

        let cmd = CommsCommand::Input {
            blocks: None,
            session_id: SessionId::new(),
            body: "standalone test input".to_string(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            source: InputSource::Rpc,
            stream: InputStreamMode::None,
            allow_self_session: true,
        };

        let receipt = CoreCommsRuntime::send(&runtime, cmd).await;
        assert!(receipt.is_ok(), "send should succeed: {receipt:?}");

        match receipt.unwrap() {
            SendReceipt::InputAccepted {
                interaction_id: _,
                stream_reserved,
            } => assert!(!stream_reserved),
            other => panic!("Expected InputAccepted, got: {other:?}"),
        }

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&runtime).await;
        assert_eq!(interactions.len(), 1);
        assert!(matches!(
            &interactions[0].content,
            meerkat_core::InteractionContent::Message { body, .. } if body == "standalone test input"
        ));
    }

    /// ROW #269 gate: a non-stream comms `Input` returns an `InteractionId`
    /// that matches the `interaction_id` stamped on the injected event
    /// (correlatable), not a fresh unrelated UUID.
    #[tokio::test]
    async fn test_core_send_input_no_reservation_returns_correlated_id() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = inproc_only_with_test_peer_authority(&format!("input-corr-{suffix}"));

        let cmd = CommsCommand::Input {
            blocks: None,
            session_id: SessionId::new(),
            body: "correlated input".to_string(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            source: InputSource::Rpc,
            stream: InputStreamMode::None,
            allow_self_session: true,
        };

        let returned_id = match CoreCommsRuntime::send(&runtime, cmd).await.unwrap() {
            SendReceipt::InputAccepted {
                interaction_id,
                stream_reserved,
            } => {
                assert!(!stream_reserved);
                interaction_id
            }
            other => panic!("Expected InputAccepted, got: {other:?}"),
        };

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&runtime).await;
        assert_eq!(interactions.len(), 1);
        // The injected event's interaction id is the SAME as the returned
        // handle — the handle correlates with the event, not a fresh UUID.
        assert_eq!(
            interactions[0].id, returned_id,
            "returned InteractionId must match the injected event's interaction id"
        );
    }

    #[tokio::test]
    async fn test_core_send_input_reserves_stream() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = inproc_only_with_test_peer_authority(&format!("input-res-{suffix}"));

        let cmd = CommsCommand::Input {
            blocks: None,
            session_id: SessionId::new(),
            body: "streaming input".to_string(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            source: InputSource::Rpc,
            stream: InputStreamMode::ReserveInteraction,
            allow_self_session: true,
        };

        let receipt = CoreCommsRuntime::send(&runtime, cmd).await.unwrap();
        let (interaction_id, reserved) = match receipt {
            SendReceipt::InputAccepted {
                interaction_id,
                stream_reserved,
            } => (interaction_id, stream_reserved),
            other => panic!("Expected InputAccepted, got: {other:?}"),
        };

        assert!(reserved);

        let sender = runtime
            .take_interaction_stream_sender(&interaction_id)
            .expect("reserved interaction should be registered");
        drop(sender);

        assert!(
            runtime
                .take_interaction_stream_sender(&interaction_id)
                .is_none(),
            "interaction registration must be one-shot"
        );
    }

    #[tokio::test]
    async fn test_core_stream_attachment_duplicate_attach_fails() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = inproc_only_with_test_peer_authority(&format!("dup-attach-{suffix}"));

        let cmd = CommsCommand::Input {
            blocks: None,
            session_id: SessionId::new(),
            body: "duplicate stream test".to_string(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            source: InputSource::Rpc,
            stream: InputStreamMode::ReserveInteraction,
            allow_self_session: true,
        };

        let interaction_id = match CoreCommsRuntime::send(&runtime, cmd).await.unwrap() {
            SendReceipt::InputAccepted {
                interaction_id,
                stream_reserved,
            } => {
                assert!(stream_reserved);
                interaction_id
            }
            other => panic!("Expected InputAccepted, got: {other:?}"),
        };

        let stream = CoreCommsRuntime::stream(&runtime, StreamScope::Interaction(interaction_id))
            .expect("first attach should succeed");

        let dup = CoreCommsRuntime::stream(&runtime, StreamScope::Interaction(interaction_id));
        assert!(matches!(dup, Err(StreamError::AlreadyAttached(_))));

        drop(stream);

        let after_drop =
            CoreCommsRuntime::stream(&runtime, StreamScope::Interaction(interaction_id));
        assert!(matches!(after_drop, Err(StreamError::NotReserved(_))));
    }

    #[tokio::test]
    async fn test_core_stream_not_reserved_before_send() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = inproc_only_with_test_peer_authority(&format!("pre-send-{suffix}"));
        let random = InteractionId(Uuid::new_v4());

        let missing = CoreCommsRuntime::stream(&runtime, StreamScope::Interaction(random));
        assert!(matches!(missing, Err(StreamError::NotReserved(_))));

        let cmd = CommsCommand::Input {
            blocks: None,
            session_id: SessionId::new(),
            body: "send before stream attach".to_string(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            source: InputSource::Rpc,
            stream: InputStreamMode::ReserveInteraction,
            allow_self_session: true,
        };
        let interaction_id = match CoreCommsRuntime::send(&runtime, cmd).await.unwrap() {
            SendReceipt::InputAccepted {
                interaction_id,
                stream_reserved,
            } => {
                assert!(stream_reserved);
                interaction_id
            }
            other => panic!("Expected InputAccepted, got: {other:?}"),
        };

        let _stream = CoreCommsRuntime::stream(&runtime, StreamScope::Interaction(interaction_id))
            .expect("should attach after send");
    }

    #[tokio::test]
    async fn test_core_send_and_stream_input_returns_stream_and_receipt() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = inproc_only_with_test_peer_authority(&format!("sas-{suffix}"));

        let cmd = CommsCommand::Input {
            blocks: None,
            session_id: SessionId::new(),
            body: "stream-first".to_string(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            source: InputSource::Rpc,
            stream: InputStreamMode::ReserveInteraction,
            allow_self_session: true,
        };

        let (receipt, _stream) = CoreCommsRuntime::send_and_stream(&runtime, cmd)
            .await
            .unwrap();

        let interaction_id = match receipt {
            SendReceipt::InputAccepted {
                interaction_id,
                stream_reserved,
            } => {
                assert!(stream_reserved);
                interaction_id
            }
            other => panic!("Expected InputAccepted, got: {other:?}"),
        };

        let dup = CoreCommsRuntime::stream(&runtime, StreamScope::Interaction(interaction_id));
        assert!(matches!(dup, Err(StreamError::AlreadyAttached(_))));

        assert!(
            CoreCommsRuntime::stream(&runtime, StreamScope::Interaction(interaction_id)).is_err()
        );
    }

    #[tokio::test]
    async fn test_core_send_unknown_peer_fails() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = CommsRuntime::inproc_only(&format!("sender-{suffix}")).unwrap();

        let cmd = CommsCommand::PeerMessage {
            objective_id: None,
            content_taint: None,
            blocks: None,
            to: missing_peer_route("missing-peer"),
            body: "hello".to_string(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
        };

        let result = CoreCommsRuntime::send(&runtime, cmd).await;
        assert!(matches!(result, Err(SendError::PeerNotFound(_))));
    }

    #[tokio::test]
    async fn test_core_send_success_and_drain() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender_name = format!("sender-{suffix}");
        let receiver_name = format!("receiver-{suffix}");
        let sender = CommsRuntime::inproc_only(&sender_name).unwrap();
        let receiver = inproc_only_with_test_peer_authority(&receiver_name);

        add_trusted_peer_with_generated_authority(
            &sender,
            trusted_descriptor(
                &receiver_name,
                receiver.public_key(),
                &format!("inproc://{receiver_name}"),
            ),
        )
        .await;

        add_trusted_peer_with_generated_authority(
            &receiver,
            trusted_descriptor(
                &sender_name,
                sender.public_key(),
                &format!("inproc://{sender_name}"),
            ),
        )
        .await;

        let cmd = CommsCommand::PeerMessage {
            objective_id: None,
            content_taint: None,
            blocks: None,
            to: peer_route(&receiver_name, receiver.public_key()),
            body: "greeting".to_string(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
        };

        let receipt = CoreCommsRuntime::send(&sender, cmd).await;
        match receipt {
            Ok(SendReceipt::PeerMessageSent {
                envelope_id: _,
                delivery,
            }) => assert_eq!(
                delivery,
                meerkat_core::comms::PeerDeliveryOutcome::HandedOff
            ),
            other => panic!("Expected peer message receipt, got: {other:?}"),
        }

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&receiver).await;
        assert_eq!(interactions.len(), 1);
        assert!(matches!(
            &interactions[0].content,
            meerkat_core::InteractionContent::Message { body, .. } if body == "greeting"
        ));
    }

    #[tokio::test]
    async fn test_core_send_hydrates_blob_refs_before_transport() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender_name = format!("sender-blob-{suffix}");
        let receiver_name = format!("receiver-blob-{suffix}");
        let mut sender = CommsRuntime::inproc_only(&sender_name).unwrap();
        let receiver = inproc_only_with_test_peer_authority(&receiver_name);
        let blob_store: Arc<dyn BlobStore> = Arc::new(TestBlobStore::default());
        let blob_ref = blob_store
            .put_image("image/png", "aGVsbG8=")
            .await
            .expect("blob stored");
        sender.set_blob_store(blob_store);

        add_trusted_peer_with_generated_authority(
            &sender,
            trusted_descriptor(
                &receiver_name,
                receiver.public_key(),
                &format!("inproc://{receiver_name}"),
            ),
        )
        .await;

        add_trusted_peer_with_generated_authority(
            &receiver,
            trusted_descriptor(
                &sender_name,
                sender.public_key(),
                &format!("inproc://{sender_name}"),
            ),
        )
        .await;

        let cmd = CommsCommand::PeerMessage {
            objective_id: None,
            content_taint: None,
            blocks: Some(vec![ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: ImageData::Blob {
                    blob_id: blob_ref.blob_id,
                },
            }]),
            to: peer_route(&receiver_name, receiver.public_key()),
            body: "blob-backed image".to_string(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
        };

        let receipt = CoreCommsRuntime::send(&sender, cmd).await;
        assert!(matches!(receipt, Ok(SendReceipt::PeerMessageSent { .. })));

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&receiver).await;
        assert_eq!(interactions.len(), 1);
        match &interactions[0].content {
            meerkat_core::InteractionContent::Message { blocks, .. } => {
                let blocks = blocks.as_ref().expect("received blocks");
                assert!(matches!(
                    &blocks[0],
                    ContentBlock::Image {
                        data: ImageData::Inline { data },
                        ..
                    } if data == "aGVsbG8="
                ));
            }
            other => panic!("expected message interaction, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_incarnation_fenced_send_hydrates_blob_and_preserves_objective() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender_name = format!("sender-fenced-blob-{suffix}");
        let receiver_name = format!("receiver-fenced-blob-{suffix}");
        let mut sender = CommsRuntime::inproc_only(&sender_name).unwrap();
        let receiver = inproc_only_with_test_peer_authority(&receiver_name);
        let blob_store: Arc<dyn BlobStore> = Arc::new(TestBlobStore::default());
        let blob_ref = blob_store
            .put_image("image/png", "aGVsbG8=")
            .await
            .expect("blob stored");
        sender.set_blob_store(blob_store);

        add_trusted_peer_with_generated_authority(
            &sender,
            trusted_descriptor(
                &receiver_name,
                receiver.public_key(),
                &format!("inproc://{receiver_name}"),
            ),
        )
        .await;
        add_trusted_peer_with_generated_authority(
            &receiver,
            trusted_descriptor(
                &sender_name,
                sender.public_key(),
                &format!("inproc://{sender_name}"),
            ),
        )
        .await;

        let objective_id = meerkat_core::interaction::ObjectiveId::new();
        let cmd = CommsCommand::IncarnationFencedPeerMessage {
            to: peer_route(&receiver_name, receiver.public_key()),
            body: "fenced blob-backed image".to_string(),
            blocks: Some(vec![ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: ImageData::Blob {
                    blob_id: blob_ref.blob_id,
                },
            }]),
            content_taint: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            objective_id: Some(objective_id),
            expected_recipient: meerkat_core::comms::PeerRecipientIncarnation {
                mob_id: "mob".to_string(),
                agent_identity: "worker".to_string(),
                host_id: "host".to_string(),
                binding_generation: 1,
                member_session_id: "session".to_string(),
                generation: 1,
                fence_token: 7,
            },
        };

        let receipt = CoreCommsRuntime::send(&sender, cmd).await;
        assert!(matches!(receipt, Ok(SendReceipt::PeerMessageSent { .. })));

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&receiver).await;
        assert_eq!(interactions.len(), 1);
        assert_eq!(interactions[0].objective_id, Some(objective_id));
        match &interactions[0].content {
            meerkat_core::InteractionContent::IncarnationFencedMessage { blocks, .. } => {
                let blocks = blocks.as_ref().expect("received blocks");
                assert!(matches!(
                    &blocks[0],
                    ContentBlock::Image {
                        data: ImageData::Inline { data },
                        ..
                    } if data == "aGVsbG8="
                ));
            }
            other => panic!("expected incarnation-fenced interaction, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn inherent_message_drain_excludes_fenced_but_keeps_ordinary_message() {
        let (sender, receiver, receiver_name) = trusted_inproc_pair("legacy-drain-fence").await;
        enqueue_fenced_then_plain(&sender, &receiver, &receiver_name).await;

        let messages = CommsRuntime::drain_messages(&receiver).await;
        assert_eq!(messages.len(), 1);
        assert!(matches!(
            &messages[0].content,
            crate::agent::CommsContent::Message { body, .. } if body == "ordinary message"
        ));
    }

    #[tokio::test]
    async fn inherent_message_recv_skips_fenced_queue_head() {
        let (sender, receiver, receiver_name) = trusted_inproc_pair("legacy-recv-fence").await;
        enqueue_fenced_then_plain(&sender, &receiver, &receiver_name).await;

        let message = CommsRuntime::recv_message(&receiver)
            .await
            .expect("ordinary message behind fenced queue head");
        assert!(matches!(
            message.content,
            crate::agent::CommsContent::Message { body, .. } if body == "ordinary message"
        ));
    }

    #[tokio::test]
    async fn core_string_drain_excludes_fenced_but_keeps_ordinary_message() {
        let (sender, receiver, receiver_name) = trusted_inproc_pair("core-string-fence").await;
        enqueue_fenced_then_plain(&sender, &receiver, &receiver_name).await;

        let messages = CoreCommsRuntime::drain_messages(&receiver).await;
        assert_eq!(messages.len(), 1);
        assert!(messages[0].contains("ordinary message"));
        assert!(!messages[0].contains("authority-bearing fenced message"));
    }

    #[tokio::test]
    async fn test_core_send_hydrates_blob_refs_on_peer_request_before_transport() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender_name = format!("sender-request-blob-{suffix}");
        let receiver_name = format!("receiver-request-blob-{suffix}");
        let mut sender_runtime = CommsRuntime::inproc_only(&sender_name).unwrap();
        let receiver = inproc_only_with_test_peer_authority(&receiver_name);
        let blob_store: Arc<dyn BlobStore> = Arc::new(TestBlobStore::default());
        let blob_ref = blob_store
            .put_image("image/png", "aGVsbG8=")
            .await
            .expect("blob stored");
        sender_runtime.set_blob_store(blob_store);
        let sender = Arc::new(sender_runtime);
        install_test_peer_request_response_authority(&sender);

        add_trusted_peer_with_generated_authority(
            sender.as_ref(),
            trusted_descriptor(
                &receiver_name,
                receiver.public_key(),
                &format!("inproc://{receiver_name}"),
            ),
        )
        .await;

        add_trusted_peer_with_generated_authority(
            &receiver,
            trusted_descriptor(
                &sender_name,
                sender.public_key(),
                &format!("inproc://{sender_name}"),
            ),
        )
        .await;

        let cmd = CommsCommand::PeerRequest {
            objective_id: None,
            content_taint: None,
            blocks: Some(vec![ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: ImageData::Blob {
                    blob_id: blob_ref.blob_id,
                },
            }]),
            to: peer_route(&receiver_name, receiver.public_key()),
            intent: "checksum".to_string(),
            params: serde_json::json!({"subject": "image"}),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            stream: InputStreamMode::ReserveInteraction,
        };

        let receipt = CoreCommsRuntime::send(sender.as_ref(), cmd).await;
        assert!(matches!(receipt, Ok(SendReceipt::PeerRequestSent { .. })));

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&receiver).await;
        assert_eq!(interactions.len(), 1);
        match &interactions[0].content {
            meerkat_core::InteractionContent::Request { blocks, .. } => {
                let blocks = blocks.as_ref().expect("received blocks");
                assert!(matches!(
                    &blocks[0],
                    ContentBlock::Image {
                        data: ImageData::Inline { data },
                        ..
                    } if data == "aGVsbG8="
                ));
            }
            other => panic!("expected request interaction, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_core_send_hydrates_blob_refs_on_peer_response_before_transport() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender_name = format!("sender-response-blob-{suffix}");
        let receiver_name = format!("receiver-response-blob-{suffix}");
        let mut sender_runtime = CommsRuntime::inproc_only(&sender_name).unwrap();
        let receiver = inproc_only_with_test_peer_authority(&receiver_name);
        let blob_store: Arc<dyn BlobStore> = Arc::new(TestBlobStore::default());
        let blob_ref = blob_store
            .put_image("image/png", "aGVsbG8=")
            .await
            .expect("blob stored");
        sender_runtime.set_blob_store(blob_store);
        let sender = Arc::new(sender_runtime);
        let peer_handle = Arc::new(TestPeerInteractionHandle::default());
        sender.install_peer_request_response_authority(PeerRequestResponseAuthority::new(
            peer_handle.clone(),
            Arc::new(TestInteractionStreamHandle::default()),
        ));

        add_trusted_peer_with_generated_authority(
            sender.as_ref(),
            trusted_descriptor(
                &receiver_name,
                receiver.public_key(),
                &format!("inproc://{receiver_name}"),
            ),
        )
        .await;

        add_trusted_peer_with_generated_authority(
            &receiver,
            trusted_descriptor(
                &sender_name,
                sender.public_key(),
                &format!("inproc://{sender_name}"),
            ),
        )
        .await;

        let interaction_id = Uuid::new_v4();
        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(interaction_id);
        meerkat_core::handles::PeerInteractionHandle::request_received(
            peer_handle.as_ref(),
            corr_id,
            meerkat_core::types::HandlingMode::Queue,
        )
        .expect("seed inbound request state");

        let cmd = CommsCommand::PeerResponse {
            objective_id: None,
            content_taint: None,
            blocks: Some(vec![ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: ImageData::Blob {
                    blob_id: blob_ref.blob_id,
                },
            }]),
            to: peer_route(&receiver_name, receiver.public_key()),
            in_reply_to: InteractionId(interaction_id),
            status: meerkat_core::ResponseStatus::Completed,
            result: serde_json::json!({"ok": true}),
            handling_mode: Some(meerkat_core::types::HandlingMode::Queue),
        };

        let receipt = CoreCommsRuntime::send(sender.as_ref(), cmd).await;
        assert!(matches!(receipt, Ok(SendReceipt::PeerResponseSent { .. })));

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&receiver).await;
        assert_eq!(interactions.len(), 1);
        match &interactions[0].content {
            meerkat_core::InteractionContent::Response { blocks, .. } => {
                let blocks = blocks.as_ref().expect("received blocks");
                assert!(matches!(
                    &blocks[0],
                    ContentBlock::Image {
                        data: ImageData::Inline { data },
                        ..
                    } if data == "aGVsbG8="
                ));
            }
            other => panic!("expected response interaction, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_core_send_fails_without_trusted_entry() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender_name = format!("sender-ipc-{suffix}");
        let receiver_name = format!("receiver-ipc-{suffix}");
        let sender = CommsRuntime::inproc_only(&sender_name).unwrap();
        let receiver = CommsRuntime::inproc_only(&receiver_name).unwrap();

        let cmd = CommsCommand::PeerMessage {
            objective_id: None,
            content_taint: None,
            blocks: None,
            to: peer_route(&receiver_name, receiver.public_key()),
            body: "inproc-only hello".to_string(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
        };

        let receipt = CoreCommsRuntime::send(&sender, cmd).await;
        match receipt {
            Err(SendError::PeerNotFound(_)) => {}
            other => panic!("Expected peer-not-found for missing trust, got: {other:?}"),
        }

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&receiver).await;
        assert_eq!(interactions.len(), 0);
    }

    #[tokio::test]
    async fn test_core_send_requires_generated_trust_when_auth_disabled() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender_name = format!("sender-ipc-no-auth-{suffix}");
        let receiver_name = format!("receiver-ipc-no-auth-{suffix}");
        let sender_tmp = tempfile::tempdir().unwrap();
        let receiver_tmp = tempfile::tempdir().unwrap();

        let sender_config = ResolvedCommsConfig {
            enabled: true,
            name: sender_name.clone(),
            inproc_namespace: None,
            identity_dir: sender_tmp.path().join("identity"),
            trusted_peers_path: sender_tmp.path().join("trusted_peers.json"),
            comms_config: crate::CommsConfig::default(),
            auth: meerkat_core::CommsAuthMode::Open,
            require_peer_auth: false,
            listen_uds: None,
            listen_tcp: None,
            advertise_address: None,
            event_listen_tcp: None,
            #[cfg(unix)]
            event_listen_uds: None,
            allow_external_unauthenticated: false,
            pairing_password: None,
        };

        let receiver_config = ResolvedCommsConfig {
            enabled: true,
            name: receiver_name.clone(),
            inproc_namespace: None,
            identity_dir: receiver_tmp.path().join("identity"),
            trusted_peers_path: receiver_tmp.path().join("trusted_peers.json"),
            comms_config: crate::CommsConfig::default(),
            auth: meerkat_core::CommsAuthMode::Open,
            require_peer_auth: false,
            listen_uds: None,
            listen_tcp: None,
            advertise_address: None,
            event_listen_tcp: None,
            #[cfg(unix)]
            event_listen_uds: None,
            allow_external_unauthenticated: false,
            pairing_password: None,
        };

        let sender = CommsRuntime::new(sender_config).await.unwrap();
        let receiver = new_with_test_peer_authority(receiver_config).await;
        let sender_pubkey = sender.public_key();
        let receiver_pubkey = receiver.public_key();

        let cmd = CommsCommand::PeerMessage {
            objective_id: None,
            content_taint: None,
            blocks: None,
            to: peer_route(&receiver_name, receiver.public_key()),
            body: "hello without trusted".to_string(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
        };

        let expected_peer_id = receiver.public_key().to_peer_id().to_string();
        let receipt = CoreCommsRuntime::send(&sender, cmd).await;
        match receipt {
            Err(SendError::PeerNotFound(peer_id)) if peer_id == expected_peer_id => {}
            other => panic!("Expected peer-not-found without generated trust, got: {other:?}"),
        }

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&receiver).await;
        assert_eq!(interactions.len(), 0);

        assert!(crate::InprocRegistry::global().unregister(&sender_pubkey));
        assert!(crate::InprocRegistry::global().unregister(&receiver_pubkey));
    }

    #[tokio::test]
    async fn test_core_peers_excludes_inproc_without_generated_trust_when_auth_disabled() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime_name = format!("directory-no-auth-runtime-{suffix}");
        let peer_name = format!("directory-no-auth-peer-{suffix}");
        let runtime_tmp = tempfile::tempdir().unwrap();
        let peer_tmp = tempfile::tempdir().unwrap();

        let runtime_config = ResolvedCommsConfig {
            enabled: true,
            name: runtime_name.clone(),
            inproc_namespace: None,
            identity_dir: runtime_tmp.path().join("identity"),
            trusted_peers_path: runtime_tmp.path().join("trusted_peers.json"),
            comms_config: crate::CommsConfig::default(),
            auth: meerkat_core::CommsAuthMode::Open,
            require_peer_auth: false,
            listen_uds: None,
            listen_tcp: None,
            event_listen_tcp: None,
            #[cfg(unix)]
            event_listen_uds: None,
            allow_external_unauthenticated: false,
            advertise_address: None,
            pairing_password: None,
        };
        let peer_config = ResolvedCommsConfig {
            enabled: true,
            name: peer_name.clone(),
            inproc_namespace: None,
            identity_dir: peer_tmp.path().join("identity"),
            trusted_peers_path: peer_tmp.path().join("trusted_peers.json"),
            comms_config: crate::CommsConfig::default(),
            auth: meerkat_core::CommsAuthMode::Open,
            require_peer_auth: false,
            listen_uds: None,
            listen_tcp: None,
            event_listen_tcp: None,
            #[cfg(unix)]
            event_listen_uds: None,
            allow_external_unauthenticated: false,
            advertise_address: None,
            pairing_password: None,
        };

        let runtime = CommsRuntime::new(runtime_config).await.unwrap();
        let peer = CommsRuntime::new(peer_config).await.unwrap();
        let runtime_pubkey = runtime.public_key();
        let peer_pubkey = peer.public_key();

        let peers = CoreCommsRuntime::peers(&runtime).await;
        assert!(
            peers.iter().all(|entry| entry.name.as_str() != peer_name),
            "inproc registry membership must not enter the peer directory without generated trust"
        );

        assert!(crate::InprocRegistry::global().unregister(&runtime_pubkey));
        assert!(crate::InprocRegistry::global().unregister(&peer_pubkey));
    }

    #[tokio::test]
    async fn test_add_trusted_peer_updates_peers_and_enables_send() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender_name = format!("trust-sender-{suffix}");
        let receiver_name = format!("trust-receiver-{suffix}");
        let sender = CommsRuntime::inproc_only(&sender_name).unwrap();
        let receiver = inproc_only_with_test_peer_authority(&receiver_name);

        let peer_spec = trusted_descriptor(
            &receiver_name,
            receiver.public_key(),
            &format!("inproc://{receiver_name}"),
        );

        add_trusted_peer_with_generated_authority(&sender, peer_spec).await;

        let reverse_spec = trusted_descriptor(
            &sender_name,
            sender.public_key(),
            &format!("inproc://{sender_name}"),
        );

        add_trusted_peer_with_generated_authority(&receiver, reverse_spec).await;

        let peers = CoreCommsRuntime::peers(&sender).await;
        let receiver_entries: Vec<_> = peers
            .iter()
            .filter(|entry| entry.name.as_str() == receiver_name)
            .collect();
        assert_eq!(receiver_entries.len(), 1, "peer should appear in peers()");

        let send_cmd = CommsCommand::PeerMessage {
            objective_id: None,
            content_taint: None,
            blocks: None,
            to: peer_route(&receiver_name, receiver.public_key()),
            body: "hello trusted peer".to_string(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
        };
        let receipt = CoreCommsRuntime::send(&sender, send_cmd).await;
        assert!(matches!(receipt, Ok(SendReceipt::PeerMessageSent { .. })));

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&receiver).await;
        assert_eq!(interactions.len(), 1);
        assert!(matches!(
            &interactions[0].content,
            meerkat_core::InteractionContent::Message { body, .. } if body == "hello trusted peer"
        ));
    }

    #[tokio::test]
    async fn test_add_trusted_peer_invalid_peer_id_is_rejected() {
        let sender = CommsRuntime::inproc_only("trust-invalid-sender").unwrap();
        // Construct a descriptor whose `peer_id` does not match the supplied
        // `pubkey`; generated peer-projection authority refuses to package a
        // trust mutation for a drifting descriptor.
        let bogus_peer_key = Keypair::generate();
        let real_peer_key = Keypair::generate();
        let invalid_peer_spec = meerkat_core::comms::TrustedPeerDescriptor {
            name: meerkat_core::comms::PeerName::new("invalid").expect("valid peer name"),
            peer_id: bogus_peer_key.public_key().to_peer_id(),
            address: meerkat_core::comms::PeerAddress::new(
                meerkat_core::comms::PeerTransport::Inproc,
                "invalid",
            ),
            pubkey: *real_peer_key.public_key().as_bytes(),
        };

        let result =
            try_add_trusted_peer_with_generated_authority(&sender, invalid_peer_spec).await;
        assert!(matches!(result, Err(SendError::Validation(_))));
    }

    #[tokio::test]
    async fn test_runtime_startup_rejects_persisted_zero_pubkey_trust() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = test_runtime_config("persisted-zero-pubkey", &tmp);
        tokio::fs::write(
            &config.trusted_peers_path,
            serde_json::json!({
                "peers": [{
                    "name": "persisted-zero-peer",
                    "pubkey": PubKey::new([0u8; 32]).to_pubkey_string(),
                    "addr": "inproc://persisted-zero-peer"
                }]
            })
            .to_string(),
        )
        .await
        .unwrap();

        let error = match CommsRuntime::new(config).await {
            Ok(_) => panic!("runtime startup must reject persisted zero-pubkey trust"),
            Err(error) => error,
        };

        match error {
            CommsRuntimeError::TrustLoadError(message) => {
                assert!(
                    message.contains("pubkey") && message.contains("non-zero"),
                    "unexpected trust-load error: {message}"
                );
            }
            other => panic!("unexpected runtime startup error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_runtime_startup_rejects_persisted_trusted_peer_seed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = test_runtime_config("persisted-trust-seed", &tmp);
        let peer_key = Keypair::generate().public_key();
        tokio::fs::write(
            &config.trusted_peers_path,
            serde_json::json!({
                "peers": [{
                    "name": "persisted-peer",
                    "pubkey": peer_key.to_pubkey_string(),
                    "addr": "inproc://persisted-peer"
                }]
            })
            .to_string(),
        )
        .await
        .unwrap();

        let error = match CommsRuntime::new(config).await {
            Ok(_) => panic!("runtime startup must reject persisted trust seeds"),
            Err(error) => error,
        };

        match error {
            CommsRuntimeError::TrustLoadError(message) => {
                assert!(
                    message.contains("generated comms trust mutation authority"),
                    "unexpected trust-load error: {message}"
                );
            }
            other => panic!("unexpected runtime startup error: {other:?}"),
        }
    }

    #[test]
    fn test_core_runtime_public_key_is_exposed_via_trait() {
        let runtime = CommsRuntime::inproc_only("pub-key-trait").unwrap();
        let public_key = <CommsRuntime as CoreCommsRuntime>::public_key(&runtime);
        assert!(
            public_key
                .as_ref()
                .is_some_and(|id| id.starts_with("ed25519:")),
            "public_key should be available and formatted as an ed25519 public key"
        );
        let public_key_bytes = <CommsRuntime as CoreCommsRuntime>::public_key_bytes(&runtime)
            .expect("typed public key bytes should be available");
        assert_ne!(public_key_bytes, [0u8; 32]);
        assert_eq!(
            <CommsRuntime as CoreCommsRuntime>::peer_id(&runtime),
            Some(PeerId::from_ed25519_pubkey(&public_key_bytes))
        );
    }

    #[test]
    fn prepared_inproc_runtime_is_dormant_and_drop_is_registry_inert() {
        let suffix = Uuid::new_v4().simple().to_string();
        let name = format!("prepared-dormant-{suffix}");
        let namespace = format!("prepared-dormant-ns-{suffix}");
        let current = CommsRuntime::inproc_only_with_keypair_and_silent_intents(
            &name,
            Some(namespace.clone()),
            Keypair::generate(),
            Arc::new(HashSet::new()),
        )
        .expect("current runtime should publish");
        let current_key = current.public_key();
        let current_sender = InprocRegistry::global()
            .get_by_pubkey_in_namespace(&namespace, &current_key)
            .expect("current route should be published");

        let prepared = CommsRuntime::prepare_inproc_only_with_keypair_and_silent_intents(
            &name,
            Some(namespace.clone()),
            Keypair::generate(),
            Arc::new(HashSet::new()),
        )
        .expect("replacement runtime should prepare");
        let prepared_key = prepared.runtime().public_key();

        assert!(
            InprocRegistry::global()
                .get_by_pubkey_in_namespace(&namespace, &prepared_key)
                .is_none(),
            "preparation must not publish the candidate route"
        );
        let during_prepare = InprocRegistry::global()
            .get_by_pubkey_in_namespace(&namespace, &current_key)
            .expect("preparation must retain the current route");
        assert!(current_sender.same_inbox(&during_prepare));

        drop(prepared);

        let after_drop = InprocRegistry::global()
            .get_by_pubkey_in_namespace(&namespace, &current_key)
            .expect("dropping a prepared runtime must retain the current route");
        assert!(current_sender.same_inbox(&after_drop));
        assert!(
            InprocRegistry::global()
                .get_by_pubkey_in_namespace(&namespace, &prepared_key)
                .is_none(),
            "dropping an unpublished candidate must have zero registry effect"
        );
    }

    #[test]
    fn prepared_inproc_runtime_atomically_replaces_exact_current_generation() {
        let suffix = Uuid::new_v4().simple().to_string();
        let name = format!("prepared-replace-{suffix}");
        let namespace = format!("prepared-replace-ns-{suffix}");
        let current = CommsRuntime::inproc_only_with_keypair_and_silent_intents(
            &name,
            Some(namespace.clone()),
            Keypair::generate(),
            Arc::new(HashSet::new()),
        )
        .expect("current runtime should publish");
        let current_key = current.public_key();
        let prepared = CommsRuntime::prepare_inproc_only_with_keypair_and_silent_intents(
            &name,
            Some(namespace.clone()),
            Keypair::generate(),
            Arc::new(HashSet::new()),
        )
        .expect("replacement runtime should prepare");
        let replacement_key = prepared.runtime().public_key();

        let replacement = prepared
            .publish_replacing(&current)
            .expect("exact current generation should replace atomically");
        assert!(
            InprocRegistry::global()
                .get_by_pubkey_in_namespace(&namespace, &current_key)
                .is_none(),
            "the old key must be unreachable after publication"
        );
        let replacement_sender = InprocRegistry::global()
            .get_by_pubkey_in_namespace(&namespace, &replacement_key)
            .expect("the replacement key must be reachable after publication");

        drop(current);

        let after_stale_drop = InprocRegistry::global()
            .get_by_pubkey_in_namespace(&namespace, &replacement_key)
            .expect("retired runtime Drop must not remove the replacement");
        assert!(replacement_sender.same_inbox(&after_stale_drop));
        drop(replacement);
        assert!(
            InprocRegistry::global()
                .get_by_pubkey_in_namespace(&namespace, &replacement_key)
                .is_none(),
            "replacement Drop must remove its own exact route"
        );
    }

    #[test]
    fn prepared_inproc_replacement_requires_exact_runtime_identity() {
        let suffix = Uuid::new_v4().simple().to_string();
        let name = format!("prepared-identity-{suffix}");
        let namespace = format!("prepared-identity-ns-{suffix}");
        let current = CommsRuntime::inproc_only_with_keypair_and_silent_intents(
            &name,
            Some(namespace.clone()),
            Keypair::generate(),
            Arc::new(HashSet::new()),
        )
        .expect("current runtime should publish");
        let current_key = current.public_key();
        let current_sender = InprocRegistry::global()
            .get_by_pubkey_in_namespace(&namespace, &current_key)
            .expect("current route should be published");

        for (candidate_name, candidate_namespace) in [
            (format!("{name}-other"), namespace.clone()),
            (name, format!("{namespace}-other")),
        ] {
            let prepared = CommsRuntime::prepare_inproc_only_with_keypair_and_silent_intents(
                &candidate_name,
                Some(candidate_namespace.clone()),
                Keypair::generate(),
                Arc::new(HashSet::new()),
            )
            .expect("mismatched replacement should prepare without publication");
            let candidate_key = prepared.runtime().public_key();
            let error = match prepared.publish_replacing(&current) {
                Ok(_) => panic!("mismatched runtime identity must fail closed"),
                Err(error) => error,
            };
            let CommsRuntimeError::InprocPublication(reason) = error else {
                panic!("unexpected identity mismatch error: {error}");
            };
            assert_eq!(reason, crate::InprocPublicationError::IdentityMismatch);
            assert!(
                InprocRegistry::global()
                    .get_by_pubkey_in_namespace(&candidate_namespace, &candidate_key)
                    .is_none(),
                "identity mismatch must not publish the candidate"
            );
            let after = InprocRegistry::global()
                .get_by_pubkey_in_namespace(&namespace, &current_key)
                .expect("identity mismatch must retain the current route");
            assert!(current_sender.same_inbox(&after));
        }
    }

    #[test]
    fn prepared_inproc_replacement_inherits_current_registry_metadata() {
        let suffix = Uuid::new_v4().simple().to_string();
        let name = format!("prepared-meta-{suffix}");
        let namespace = format!("prepared-meta-ns-{suffix}");
        let current = CommsRuntime::inproc_only_with_keypair_and_silent_intents(
            &name,
            Some(namespace.clone()),
            Keypair::generate(),
            Arc::new(HashSet::new()),
        )
        .expect("current runtime should publish");
        let prepared = CommsRuntime::prepare_inproc_only_with_keypair_and_silent_intents(
            &name,
            Some(namespace.clone()),
            Keypair::generate(),
            Arc::new(HashSet::new()),
        )
        .expect("replacement runtime should prepare");
        let inherited = crate::PeerMeta::default()
            .with_description("metadata inherited across replacement")
            .with_label("generation", "current");
        current.set_peer_meta(inherited.clone());

        let replacement = prepared
            .publish_replacing(&current)
            .expect("replacement should inherit current registry metadata");
        let observed = InprocRegistry::global()
            .peers_in_namespace(&namespace)
            .into_iter()
            .find(|peer| peer.pubkey == replacement.public_key())
            .expect("replacement should be published");
        assert_eq!(observed.meta, inherited);
    }

    #[test]
    fn stale_same_key_runtime_drop_cannot_remove_published_replacement() {
        let suffix = Uuid::new_v4().simple().to_string();
        let name = format!("prepared-same-key-{suffix}");
        let namespace = format!("prepared-same-key-ns-{suffix}");
        let secret = Keypair::generate().secret_bytes();
        let current = CommsRuntime::inproc_only_with_keypair_and_silent_intents(
            &name,
            Some(namespace.clone()),
            Keypair::from_secret(secret),
            Arc::new(HashSet::new()),
        )
        .expect("current runtime should publish");
        let key = current.public_key();
        let prepared = CommsRuntime::prepare_inproc_only_with_keypair_and_silent_intents(
            &name,
            Some(namespace.clone()),
            Keypair::from_secret(secret),
            Arc::new(HashSet::new()),
        )
        .expect("same-key replacement should prepare");
        let replacement = prepared
            .publish_replacing(&current)
            .expect("same-key replacement should publish by exact generation");
        let replacement_sender = InprocRegistry::global()
            .get_by_pubkey_in_namespace(&namespace, &key)
            .expect("same-key replacement route should be current");

        drop(current);

        let after_stale_drop = InprocRegistry::global()
            .get_by_pubkey_in_namespace(&namespace, &key)
            .expect("stale same-key Drop must retain the replacement route");
        assert!(replacement_sender.same_inbox(&after_stale_drop));
        drop(replacement);
        assert!(
            InprocRegistry::global()
                .get_by_pubkey_in_namespace(&namespace, &key)
                .is_none()
        );
    }

    #[test]
    fn exact_publish_conflict_leaves_competing_generation_unchanged() {
        let suffix = Uuid::new_v4().simple().to_string();
        let name = format!("prepared-conflict-{suffix}");
        let namespace = format!("prepared-conflict-ns-{suffix}");
        let current = CommsRuntime::inproc_only_with_keypair_and_silent_intents(
            &name,
            Some(namespace.clone()),
            Keypair::generate(),
            Arc::new(HashSet::new()),
        )
        .expect("current runtime should publish");
        let prepared = CommsRuntime::prepare_inproc_only_with_keypair_and_silent_intents(
            &name,
            Some(namespace.clone()),
            Keypair::generate(),
            Arc::new(HashSet::new()),
        )
        .expect("replacement runtime should prepare");
        let prepared_key = prepared.runtime().public_key();
        let winner = CommsRuntime::inproc_only_with_keypair_and_silent_intents(
            &name,
            Some(namespace.clone()),
            Keypair::generate(),
            Arc::new(HashSet::new()),
        )
        .expect("competing runtime should publish first");
        let winner_key = winner.public_key();
        let winner_sender = InprocRegistry::global()
            .get_by_pubkey_in_namespace(&namespace, &winner_key)
            .expect("competing route should be current");

        let (rejected, error) = match prepared.publish_replacing_recoverable(&current) {
            Ok(_) => panic!("stale exact lease must fail without mutation"),
            Err(rejected) => rejected,
        };
        let CommsRuntimeError::InprocPublication(reason) = error else {
            panic!("unexpected stale publication error: {error}");
        };
        assert_eq!(
            reason,
            crate::InprocPublicationError::ExpectedGenerationNotCurrent
        );
        assert!(
            InprocRegistry::global()
                .get_by_pubkey_in_namespace(&namespace, &prepared_key)
                .is_none(),
            "failed publication must not expose the prepared route"
        );
        let after_conflict = InprocRegistry::global()
            .get_by_pubkey_in_namespace(&namespace, &winner_key)
            .expect("failed publication must retain the competing route");
        assert!(winner_sender.same_inbox(&after_conflict));

        drop(rejected);

        drop(current);
        let after_stale_drop = InprocRegistry::global()
            .get_by_pubkey_in_namespace(&namespace, &winner_key)
            .expect("stale predecessor Drop must retain the competing route");
        assert!(winner_sender.same_inbox(&after_stale_drop));
    }

    #[test]
    fn peer_meta_update_retains_exact_inproc_generation() {
        let suffix = Uuid::new_v4().simple().to_string();
        let name = format!("lease-meta-{suffix}");
        let namespace = format!("lease-meta-ns-{suffix}");
        let runtime = CommsRuntime::inproc_only_with_keypair_and_silent_intents(
            &name,
            Some(namespace.clone()),
            Keypair::generate(),
            Arc::new(HashSet::new()),
        )
        .expect("runtime should publish");
        let key = runtime.public_key();
        let before = InprocRegistry::global()
            .get_by_pubkey_in_namespace(&namespace, &key)
            .expect("runtime route should be published");
        let meta = crate::PeerMeta::default()
            .with_description("lease-preserving metadata")
            .with_label("generation", "stable");

        runtime.set_peer_meta(meta.clone());

        let after = InprocRegistry::global()
            .get_by_pubkey_in_namespace(&namespace, &key)
            .expect("metadata refresh must retain the route");
        assert!(before.same_inbox(&after));
        let observed = InprocRegistry::global()
            .peers_in_namespace(&namespace)
            .into_iter()
            .find(|peer| peer.pubkey == key)
            .expect("metadata peer should remain listed");
        assert_eq!(observed.meta, meta);
    }

    /// Build a fresh per-test session-claim handle. Each test gets its own
    /// registry instance so suite-scoped identity claims do not leak across
    /// tests run in parallel.
    fn test_claim_handle() -> Arc<dyn SessionClaimHandle> {
        Arc::new(meerkat_core::handles::DefaultSessionClaimRegistry::new())
    }

    #[tokio::test]
    async fn test_session_scoped_inproc_identity_preserves_peer_id_for_same_session() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let session_id = SessionId::new();
        let claim_handle = test_claim_handle();
        let runtime = CommsRuntime::inproc_only_session_scoped_with_silent_intents(
            "session-scoped-peer",
            None,
            tmp.path().join("session-identities"),
            &session_id,
            Arc::new(HashSet::new()),
            Arc::clone(&claim_handle),
        )
        .await
        .expect("create first session-scoped runtime");
        let first_peer_id = runtime.public_key().to_peer_id();
        drop(runtime);

        let rebuilt = CommsRuntime::inproc_only_session_scoped_with_silent_intents(
            "session-scoped-peer",
            None,
            tmp.path().join("session-identities"),
            &session_id,
            Arc::new(HashSet::new()),
            claim_handle,
        )
        .await
        .expect("recreate session-scoped runtime");
        let rebuilt_peer_id = rebuilt.public_key().to_peer_id();

        assert_eq!(
            rebuilt_peer_id, first_peer_id,
            "same session id should preserve the inproc peer_id"
        );
    }

    #[tokio::test]
    async fn test_session_scoped_inproc_identity_rejects_second_live_activation() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let session_id = SessionId::new();
        let claim_handle = test_claim_handle();
        let _runtime = CommsRuntime::inproc_only_session_scoped_with_silent_intents(
            "session-scoped-lock",
            None,
            tmp.path().join("session-identities"),
            &session_id,
            Arc::new(HashSet::new()),
            Arc::clone(&claim_handle),
        )
        .await
        .expect("create first session-scoped runtime");

        let error = match CommsRuntime::inproc_only_session_scoped_with_silent_intents(
            "session-scoped-lock",
            None,
            tmp.path().join("session-identities"),
            &session_id,
            Arc::new(HashSet::new()),
            claim_handle,
        )
        .await
        {
            Ok(_) => panic!("second live activation should fail"),
            Err(error) => error,
        };

        assert!(
            matches!(error, CommsRuntimeError::SessionIdentityInUse(_)),
            "unexpected error: {error:?}"
        );
    }

    #[tokio::test]
    async fn test_session_scoped_inproc_identity_does_not_restore_trust_from_disk() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let session_id = SessionId::new();
        let claim_handle = test_claim_handle();
        let sender = CommsRuntime::inproc_only_session_scoped_with_silent_intents(
            "session-scoped-trust",
            None,
            tmp.path().join("session-identities"),
            &session_id,
            Arc::new(HashSet::new()),
            Arc::clone(&claim_handle),
        )
        .await
        .expect("create session-scoped runtime");
        let peer = CommsRuntime::inproc_only("session-scoped-trust-peer").unwrap();
        add_trusted_peer_with_generated_authority(
            &sender,
            trusted_descriptor(
                "session-scoped-trust-peer",
                peer.public_key(),
                &format!("inproc://{}", peer.participant_name()),
            ),
        )
        .await;
        assert_eq!(
            sender.peers().await.len(),
            1,
            "trust should be visible while live"
        );
        drop(sender);

        let rebuilt = CommsRuntime::inproc_only_session_scoped_with_silent_intents(
            "session-scoped-trust",
            None,
            tmp.path().join("session-identities"),
            &session_id,
            Arc::new(HashSet::new()),
            claim_handle,
        )
        .await
        .expect("recreate session-scoped runtime");

        assert!(
            rebuilt.peers().await.is_empty(),
            "session-scoped identity should preserve the keypair without replaying trusted peers"
        );
    }

    #[tokio::test]
    async fn test_core_peers_includes_trusted_without_self() {
        let suffix = Uuid::new_v4().simple().to_string();
        let peer_name = format!("trusted-only-{suffix}");
        let runtime_name = format!("runtime-mixed-{suffix}");

        let peer = CommsRuntime::inproc_only(&peer_name).unwrap();
        let runtime = CommsRuntime::inproc_only(&runtime_name).unwrap();

        {
            let mut trusted = runtime.trusted_peers.write();
            trusted
                .upsert(test_trust_entry(
                    &peer_name,
                    peer.public_key(),
                    &format!("inproc://{peer_name}"),
                ))
                .expect("valid test peer should upsert");
        }

        let peers = CoreCommsRuntime::peers(&runtime).await;
        let names: Vec<_> = peers.iter().map(|entry| entry.name.as_string()).collect();
        assert!(names.iter().any(|name| name == &peer_name));
        assert!(!names.iter().any(|name| name == &runtime_name));

        let matched: Vec<_> = peers
            .iter()
            .filter(|entry| entry.name.as_str() == peer_name)
            .collect();
        assert_eq!(matched.len(), 1);

        let peer = matched[0];
        assert_eq!(peer.source, PeerDirectorySource::Trusted);
        assert_eq!(peer.address.to_string(), format!("inproc://{peer_name}"));
        assert_eq!(peer.sendable_kinds, vec![PeerSendability::PeerMessage]);
    }

    #[tokio::test]
    async fn test_core_peers_send_success_keeps_peer_directory_projection() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender_name = format!("reach-sender-{suffix}");
        let receiver_name = format!("reach-receiver-{suffix}");
        let sender = CommsRuntime::inproc_only(&sender_name).unwrap();
        let receiver = inproc_only_with_test_peer_authority(&receiver_name);

        add_trusted_peer_with_generated_authority(
            &sender,
            trusted_descriptor(
                &receiver_name,
                receiver.public_key(),
                &format!("inproc://{receiver_name}"),
            ),
        )
        .await;
        // Receiver must trust the sender too: `require_peer_auth` is on by
        // default, and with typed drop-reasons the receiver no longer silently
        // accepts untrusted envelopes as `Ok(())` — the admission gate now
        // reports `Dropped { UntrustedSender }`, which surfaces to this caller
        // as `PeerOffline`. Mutual trust mirrors real deployments.
        add_trusted_peer_with_generated_authority(
            &receiver,
            trusted_descriptor(
                &sender_name,
                sender.public_key(),
                &format!("inproc://{sender_name}"),
            ),
        )
        .await;

        CoreCommsRuntime::send(
            &sender,
            CommsCommand::PeerMessage {
                objective_id: None,
                content_taint: None,
                blocks: None,
                to: peer_route(&receiver_name, receiver.public_key()),
                body: "hello".to_string(),
                handling_mode: meerkat_core::types::HandlingMode::Queue,
            },
        )
        .await
        .expect("send should succeed");

        let peers_after = CoreCommsRuntime::peers(&sender).await;
        let after = peers_after
            .iter()
            .find(|entry| entry.name.as_str() == receiver_name)
            .expect("receiver should still be listed");
        assert_eq!(after.peer_id, receiver.public_key().to_peer_id());
    }

    #[tokio::test]
    async fn test_core_peers_transport_failure_keeps_directory_projection() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender = CommsRuntime::inproc_only(&format!("transport-sender-{suffix}")).unwrap();
        let peer_name = format!("transport-peer-{suffix}");
        let peer_pubkey = Keypair::generate().public_key();

        add_trusted_peer_with_generated_authority(
            &sender,
            trusted_descriptor(&peer_name, peer_pubkey, "tcp://127.0.0.1:9"),
        )
        .await;

        let result = CoreCommsRuntime::send(
            &sender,
            CommsCommand::PeerMessage {
                objective_id: None,
                content_taint: None,
                blocks: None,
                to: peer_route(&peer_name, peer_pubkey),
                body: "hello".to_string(),
                handling_mode: meerkat_core::types::HandlingMode::Queue,
            },
        )
        .await;
        assert!(
            matches!(result, Err(SendError::Transport(_))),
            "transport failure should surface as a typed transport send error, got: {result:?}"
        );

        let peers = CoreCommsRuntime::peers(&sender).await;
        let entry = peers
            .iter()
            .find(|listed| listed.name.as_str() == peer_name)
            .expect("trusted peer should remain listed after transport failure");
        assert_eq!(entry.peer_id, peer_pubkey.to_peer_id());
    }

    #[tokio::test]
    async fn test_send_to_untrusted_peer_surfaces_admission_dropped_not_offline() {
        // Regression: before typed `AdmissionOutcome`, the router collapsed a
        // receiver-side ingress rejection into `PeerOffline`. That made a
        // policy failure indistinguishable from a transport failure in
        // REST/RPC/MCP payloads. Now it must come through as
        // `SendError::AdmissionDropped { UntrustedSender }`; peer directory
        // liveness is intentionally not updated from this send result.
        let suffix = Uuid::new_v4().simple().to_string();
        let sender_name = format!("admit-sender-{suffix}");
        let receiver_name = format!("admit-receiver-{suffix}");
        let sender = CommsRuntime::inproc_only(&sender_name).unwrap();
        let receiver = inproc_only_with_test_peer_authority(&receiver_name);

        // Sender trusts receiver; receiver does NOT trust sender. Receiver
        // has `require_peer_auth: true` by default, so our envelope lands
        // at its ingress gate and gets rejected with `UntrustedSender`.
        add_trusted_peer_with_generated_authority(
            &sender,
            trusted_descriptor(
                &receiver_name,
                receiver.public_key(),
                &format!("inproc://{receiver_name}"),
            ),
        )
        .await;

        let result = CoreCommsRuntime::send(
            &sender,
            CommsCommand::PeerMessage {
                objective_id: None,
                content_taint: None,
                blocks: None,
                to: peer_route(&receiver_name, receiver.public_key()),
                body: "policy-rejected".to_string(),
                handling_mode: meerkat_core::types::HandlingMode::Queue,
            },
        )
        .await;
        assert!(
            matches!(
                result,
                Err(SendError::AdmissionDropped {
                    reason: meerkat_core::comms::AdmissionDropReason::UntrustedSender
                })
            ),
            "untrusted-sender ingress drop must surface as typed AdmissionDropped, \
             got: {result:?}"
        );

        let peers = CoreCommsRuntime::peers(&sender).await;
        peers
            .iter()
            .find(|listed| listed.name.as_str() == receiver_name)
            .expect("trusted peer should remain listed after admission drop");
    }

    #[tokio::test]
    async fn test_unknown_send_target_does_not_create_directory_entry() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender = CommsRuntime::inproc_only(&format!("unknown-target-{suffix}")).unwrap();
        let missing_name = format!("missing-{suffix}");

        let result = CoreCommsRuntime::send(
            &sender,
            CommsCommand::PeerMessage {
                objective_id: None,
                content_taint: None,
                blocks: None,
                to: missing_peer_route(&missing_name),
                body: "hello".to_string(),
                handling_mode: meerkat_core::types::HandlingMode::Queue,
            },
        )
        .await;
        assert!(
            matches!(result, Err(SendError::PeerNotFound(_))),
            "missing peer should fail with PeerNotFound, got: {result:?}"
        );

        let peers = CoreCommsRuntime::peers(&sender).await;
        assert!(
            peers
                .iter()
                .all(|entry| entry.name.as_str() != missing_name),
            "unknown attempted target must not become a directory entry"
        );
    }

    #[tokio::test]
    async fn test_core_peers_resolved_peer_not_found_keeps_directory_projection() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender =
            CommsRuntime::inproc_only(&format!("resolved-missing-sender-{suffix}")).unwrap();
        let missing_name = format!("resolved-missing-peer-{suffix}");
        let missing_pubkey = Keypair::generate().public_key();
        let missing_peer_id = missing_pubkey.to_peer_id().as_str();

        add_trusted_peer_with_generated_authority(
            &sender,
            trusted_descriptor(
                &missing_name,
                missing_pubkey,
                &format!("inproc://{missing_name}"),
            ),
        )
        .await;

        let result = CoreCommsRuntime::send(
            &sender,
            CommsCommand::PeerMessage {
                objective_id: None,
                content_taint: None,
                blocks: None,
                to: peer_route(&missing_name, missing_pubkey),
                body: "hello".to_string(),
                handling_mode: meerkat_core::types::HandlingMode::Queue,
            },
        )
        .await;
        assert!(
            matches!(result, Err(SendError::PeerNotFound(ref peer)) if peer == &missing_peer_id),
            "resolved missing peer should preserve PeerNotFound, got: {result:?}"
        );

        let peers = CoreCommsRuntime::peers(&sender).await;
        let entry = peers
            .iter()
            .find(|listed| listed.name.as_str() == missing_name)
            .expect("trusted peer should remain listed after failed send");
        assert_eq!(entry.peer_id, missing_pubkey.to_peer_id());
    }

    /// Truthfulness invariant: every peer/kind in peers() must be sendable.
    #[tokio::test]
    async fn test_m3_truthfulness_invariant_all_advertised_peers_are_sendable() {
        let suffix = Uuid::new_v4().simple().to_string();
        let peer_name = format!("truth-peer-{suffix}");
        let runtime_name = format!("truth-runtime-{suffix}");

        let peer = CommsRuntime::inproc_only(&peer_name).unwrap();
        let runtime = CommsRuntime::inproc_only(&runtime_name).unwrap();
        {
            let mut trusted = runtime.trusted_peers.write();
            trusted
                .upsert(test_trust_entry(
                    &peer_name,
                    peer.public_key(),
                    &format!("inproc://{peer_name}"),
                ))
                .expect("valid test peer should upsert");
        }

        let all_peers = CoreCommsRuntime::peers(&runtime).await;
        let peers: Vec<_> = all_peers
            .iter()
            .filter(|e| e.name.as_str() == peer_name)
            .collect();
        assert_eq!(
            peers.len(),
            1,
            "peer should be visible when explicitly trusted"
        );

        for entry in peers {
            for kind in &entry.sendable_kinds {
                let cmd = match kind {
                    PeerSendability::PeerMessage => CommsCommand::PeerMessage {
                        objective_id: None,
                        content_taint: None,
                        blocks: None,
                        to: PeerRoute::with_display_name(entry.peer_id, entry.name.clone()),
                        body: "truthfulness test".to_string(),
                        handling_mode: meerkat_core::types::HandlingMode::Queue,
                    },
                    PeerSendability::PeerRequest => CommsCommand::PeerRequest {
                        objective_id: None,
                        content_taint: None,
                        to: PeerRoute::with_display_name(entry.peer_id, entry.name.clone()),
                        intent: "test".to_string(),
                        params: serde_json::json!({}),
                        blocks: None,
                        handling_mode: meerkat_core::types::HandlingMode::Queue,
                        stream: InputStreamMode::None,
                    },
                    PeerSendability::PeerResponse => CommsCommand::PeerResponse {
                        objective_id: None,
                        content_taint: None,
                        to: PeerRoute::with_display_name(entry.peer_id, entry.name.clone()),
                        in_reply_to: meerkat_core::InteractionId(Uuid::new_v4()),
                        status: meerkat_core::ResponseStatus::Completed,
                        result: serde_json::json!({}),
                        blocks: None,
                        handling_mode: None,
                    },
                };
                let result = CoreCommsRuntime::send(&runtime, cmd).await;
                assert!(
                    !matches!(result, Err(SendError::PeerNotFound(_))),
                    "peer '{}' advertised kind '{}' but send failed with PeerNotFound",
                    entry.name.as_str(),
                    kind
                );
            }
        }
    }

    // --- M4: Reservation FSM + concurrency tests ---

    #[tokio::test]
    async fn terminal_subscriber_race_keeps_stream_claimable_and_attach_ttl_does_not_timeout_peer()
    {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime =
            Arc::new(CommsRuntime::inproc_only(&format!("terminal-attach-race-{suffix}")).unwrap());
        let peer_handle = Arc::new(TestPeerInteractionHandle::default());
        let stream_handle = Arc::new(TestInteractionStreamHandle::default());
        runtime.install_peer_request_response_authority(PeerRequestResponseAuthority::new(
            peer_handle.clone(),
            stream_handle.clone(),
        ));

        let interaction_id = meerkat_core::InteractionId(Uuid::new_v4());
        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(interaction_id.0);
        meerkat_core::handles::PeerInteractionHandle::request_sent(peer_handle.as_ref(), corr_id)
            .unwrap();
        runtime
            .reserve_interaction_stream_channels(
                corr_id,
                interaction_id.0,
                InteractionStreamReservationAuthority::Machine(stream_handle.clone()),
            )
            .unwrap();

        // Model a terminal response winning the race with caller attachment:
        // taking the one-shot sender is mechanics, not stream expiry.
        assert!(
            CoreCommsRuntime::interaction_subscriber(runtime.as_ref(), &interaction_id).is_some()
        );
        assert_eq!(
            meerkat_core::handles::InteractionStreamHandle::state(stream_handle.as_ref(), corr_id),
            Some(meerkat_core::InteractionStreamState::Reserved)
        );

        let stream =
            CoreCommsRuntime::stream(runtime.as_ref(), StreamScope::Interaction(interaction_id))
                .expect("response-before-attach must leave the stream claimable");
        runtime
            .interaction_stream_registry
            .lock()
            .get_mut(&interaction_id.0)
            .unwrap()
            .created_at = Instant::now().checked_sub(Duration::from_secs(60)).unwrap();

        runtime.reap_expired_reservations();
        assert_eq!(
            meerkat_core::handles::InteractionStreamHandle::state(stream_handle.as_ref(), corr_id),
            Some(meerkat_core::InteractionStreamState::Attached),
            "attach TTL must not expire an attached stream"
        );
        assert_eq!(
            meerkat_core::handles::PeerInteractionHandle::outbound_state(
                peer_handle.as_ref(),
                corr_id
            ),
            Some(meerkat_core::OutboundPeerRequestState::Sent),
            "attach TTL is not a peer response deadline"
        );
        assert!(matches!(
            CoreCommsRuntime::stream(runtime.as_ref(), StreamScope::Interaction(interaction_id)),
            Err(StreamError::AlreadyAttached(_))
        ));
        drop(stream);
    }

    #[tokio::test]
    async fn terminal_event_buffered_before_attach_completes_when_consumer_observes_it() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = Arc::new(
            CommsRuntime::inproc_only(&format!("terminal-buffered-before-attach-{suffix}"))
                .unwrap(),
        );
        let peer_handle = Arc::new(TestPeerInteractionHandle::default());
        let stream_handle = Arc::new(TestInteractionStreamHandle::default());
        runtime.install_peer_request_response_authority(PeerRequestResponseAuthority::new(
            peer_handle.clone(),
            stream_handle.clone(),
        ));
        let interaction_id = meerkat_core::InteractionId(Uuid::new_v4());
        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(interaction_id.0);
        meerkat_core::handles::PeerInteractionHandle::request_sent(peer_handle.as_ref(), corr_id)
            .unwrap();
        runtime
            .reserve_interaction_stream_channels(
                corr_id,
                interaction_id.0,
                InteractionStreamReservationAuthority::Machine(stream_handle.clone()),
            )
            .unwrap();

        let sender = CoreCommsRuntime::interaction_subscriber(runtime.as_ref(), &interaction_id)
            .expect("terminal response takes the one-shot sender");
        meerkat_core::handles::PeerInteractionHandle::response_terminal(
            peer_handle.as_ref(),
            corr_id,
            meerkat_core::handles::PeerTerminalDisposition::Completed,
        )
        .unwrap();
        sender
            .send(meerkat_core::AgentEvent::InteractionComplete {
                interaction_id,
                result: "done".to_string(),
                structured_output: None,
            })
            .await
            .unwrap();

        let mut stream =
            CoreCommsRuntime::stream(runtime.as_ref(), StreamScope::Interaction(interaction_id))
                .expect("buffered terminal response must remain attachable");
        let terminal = stream.next().await.expect("buffered terminal event");
        assert!(matches!(
            terminal.payload,
            meerkat_core::AgentEvent::InteractionComplete { result, .. } if result == "done"
        ));
        assert_eq!(
            meerkat_core::handles::InteractionStreamHandle::state(stream_handle.as_ref(), corr_id),
            None,
            "terminal state commits when the attached consumer observes the event"
        );
        assert!(
            !runtime
                .interaction_stream_registry
                .lock()
                .contains_key(&interaction_id.0)
        );
    }

    #[tokio::test]
    async fn attached_stream_observes_typed_terminal_delivery_abandonment() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = Arc::new(
            CommsRuntime::inproc_only(&format!("stream-attached-abandon-{suffix}")).unwrap(),
        );
        let peer_handle = Arc::new(TestPeerInteractionHandle::default());
        let stream_handle = Arc::new(TestInteractionStreamHandle::default());
        runtime.install_peer_request_response_authority(PeerRequestResponseAuthority::new(
            peer_handle,
            stream_handle.clone(),
        ));
        let interaction_id = meerkat_core::InteractionId(Uuid::new_v4());
        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(interaction_id.0);
        runtime
            .reserve_interaction_stream_channels(
                corr_id,
                interaction_id.0,
                InteractionStreamReservationAuthority::Machine(stream_handle),
            )
            .unwrap();
        let mut stream =
            CoreCommsRuntime::stream(runtime.as_ref(), StreamScope::Interaction(interaction_id))
                .unwrap();

        runtime.abandon_interaction_stream(
            interaction_id.0,
            meerkat_core::InteractionStreamAbandonReason::TerminalDeliveryFailed,
        );

        let terminal = stream.next().await.expect("typed abandonment terminal");
        assert!(matches!(
            terminal.payload,
            meerkat_core::AgentEvent::InteractionFailed {
                reason: meerkat_core::event::InteractionFailureReason::InteractionStreamAbandoned {
                    reason: meerkat_core::InteractionStreamAbandonReason::TerminalDeliveryFailed,
                },
                ..
            }
        ));
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn expired_unattached_stream_does_not_fabricate_peer_timeout() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = Arc::new(
            CommsRuntime::inproc_only(&format!("stream-expiry-not-peer-timeout-{suffix}")).unwrap(),
        );
        let peer_handle = Arc::new(TestPeerInteractionHandle::default());
        let stream_handle = Arc::new(TestInteractionStreamHandle::default());
        runtime.install_peer_request_response_authority(PeerRequestResponseAuthority::new(
            peer_handle.clone(),
            stream_handle.clone(),
        ));

        let interaction_id = meerkat_core::InteractionId(Uuid::new_v4());
        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(interaction_id.0);
        meerkat_core::handles::PeerInteractionHandle::request_sent(peer_handle.as_ref(), corr_id)
            .unwrap();
        runtime
            .reserve_interaction_stream_channels(
                corr_id,
                interaction_id.0,
                InteractionStreamReservationAuthority::Machine(stream_handle.clone()),
            )
            .unwrap();
        runtime
            .interaction_stream_registry
            .lock()
            .get_mut(&interaction_id.0)
            .unwrap()
            .created_at = Instant::now().checked_sub(Duration::from_secs(60)).unwrap();

        runtime.reap_expired_reservations();

        assert_eq!(
            meerkat_core::handles::InteractionStreamHandle::state(stream_handle.as_ref(), corr_id),
            None,
            "unattached reservation should expire"
        );
        assert_eq!(
            meerkat_core::handles::PeerInteractionHandle::outbound_state(
                peer_handle.as_ref(),
                corr_id
            ),
            Some(meerkat_core::OutboundPeerRequestState::Sent),
            "stream expiry must not claim that the peer response deadline elapsed"
        );
    }

    #[tokio::test]
    async fn explicit_stream_abandonment_cleans_projection_without_expiry() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime =
            Arc::new(CommsRuntime::inproc_only(&format!("stream-abandon-{suffix}")).unwrap());
        let peer_handle = Arc::new(TestPeerInteractionHandle::default());
        let stream_handle = Arc::new(TestInteractionStreamHandle::default());
        runtime.install_peer_request_response_authority(PeerRequestResponseAuthority::new(
            peer_handle,
            stream_handle.clone(),
        ));
        let interaction_id = meerkat_core::InteractionId(Uuid::new_v4());
        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(interaction_id.0);
        runtime
            .reserve_interaction_stream_channels(
                corr_id,
                interaction_id.0,
                InteractionStreamReservationAuthority::Machine(stream_handle.clone()),
            )
            .unwrap();

        runtime.abandon_interaction_stream(
            interaction_id.0,
            meerkat_core::InteractionStreamAbandonReason::ResponseRejected,
        );

        assert_eq!(
            meerkat_core::handles::InteractionStreamHandle::state(stream_handle.as_ref(), corr_id),
            None
        );
        assert!(
            !runtime
                .interaction_stream_registry
                .lock()
                .contains_key(&interaction_id.0)
        );
        assert!(matches!(
            CoreCommsRuntime::stream(runtime.as_ref(), StreamScope::Interaction(interaction_id)),
            Err(StreamError::Abandoned {
                reason: meerkat_core::InteractionStreamAbandonReason::ResponseRejected,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn test_m4_duplicate_close_is_safe() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = inproc_only_with_test_peer_authority(&format!("dup-close-{suffix}"));
        let session_id = meerkat_core::SessionId::new();

        let cmd = CommsCommand::Input {
            blocks: None,
            session_id: session_id.clone(),
            body: "hello".to_string(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            source: meerkat_core::InputSource::Rpc,
            stream: InputStreamMode::ReserveInteraction,
            allow_self_session: true,
        };
        let receipt = CoreCommsRuntime::send(&runtime, cmd).await.unwrap();
        let iid = match receipt {
            SendReceipt::InputAccepted { interaction_id, .. } => interaction_id,
            _ => panic!("expected InputAccepted"),
        };

        let stream = CoreCommsRuntime::stream(&runtime, StreamScope::Interaction(iid)).unwrap();
        // Drop stream → ClosedEarly
        drop(stream);

        // Second attach after close → NotReserved (entry cleaned up)
        let result = CoreCommsRuntime::stream(&runtime, StreamScope::Interaction(iid));
        assert!(matches!(result, Err(StreamError::NotReserved(_))));
    }

    #[tokio::test]
    async fn test_m4_mark_interaction_complete_cleans_up() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = inproc_only_with_test_peer_authority(&format!("complete-{suffix}"));
        let session_id = meerkat_core::SessionId::new();

        let cmd = CommsCommand::Input {
            blocks: None,
            session_id: session_id.clone(),
            body: "hello".to_string(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            source: meerkat_core::InputSource::Rpc,
            stream: InputStreamMode::ReserveInteraction,
            allow_self_session: true,
        };
        let receipt = CoreCommsRuntime::send(&runtime, cmd).await.unwrap();
        let iid = match receipt {
            SendReceipt::InputAccepted { interaction_id, .. } => interaction_id,
            _ => panic!("expected InputAccepted"),
        };

        let mut stream = CoreCommsRuntime::stream(&runtime, StreamScope::Interaction(iid)).unwrap();

        // Simulate terminal event
        runtime.mark_interaction_complete(iid.0);

        let terminal = timeout(Duration::from_millis(100), stream.next())
            .await
            .expect("stream should terminate promptly after completion");
        assert!(terminal.is_none());

        // Registry entry should be cleaned up
        let registry = runtime.interaction_stream_registry.lock();
        assert!(
            !registry.contains_key(&iid.0),
            "completed entry should be cleaned from registry"
        );
    }

    #[tokio::test]
    async fn test_m4_reap_expired_reservations() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = CommsRuntime::inproc_only(&format!("reap-{suffix}")).unwrap();

        // Manually insert an entry with an old timestamp
        let id = Uuid::new_v4();
        let (sender, receiver) = mpsc::channel::<meerkat_core::AgentEvent>(16);
        {
            let mut entry = StreamRegistryEntry::new(
                sender,
                receiver,
                InteractionStreamLifecycleAuthority::LocalTransportOnly,
            );
            entry.created_at = Instant::now()
                .checked_sub(meerkat_core::time_compat::Duration::from_secs(60))
                .unwrap();
            runtime.interaction_stream_registry.lock().insert(id, entry);
        }

        runtime.reap_expired_reservations();

        let registry = runtime.interaction_stream_registry.lock();
        assert!(
            !registry.contains_key(&id),
            "expired reservation should have been reaped"
        );
    }

    // --- M5: send_and_stream partial-failure tests ---

    #[tokio::test]
    async fn test_m5_send_and_stream_non_streamable_returns_stream_attach() {
        let suffix = Uuid::new_v4().simple().to_string();
        let peer_name = format!("m5-peer-{suffix}");
        let runtime_name = format!("m5-runtime-{suffix}");

        let peer = inproc_only_with_test_peer_authority(&peer_name);
        let runtime = CommsRuntime::inproc_only(&runtime_name).unwrap();
        {
            let mut trusted = runtime.trusted_peers.write();
            trusted
                .upsert(test_trust_entry(
                    &peer_name,
                    peer.public_key(),
                    &format!("inproc://{peer_name}"),
                ))
                .expect("valid test peer should upsert");
        }
        // Receive side must also trust the sender after the silent-drop fix.
        // Use generated trust mutation so the classified queue's trust set syncs.
        add_trusted_peer_with_generated_authority(
            &peer,
            trusted_descriptor(
                &runtime_name,
                runtime.public_key(),
                &format!("inproc://{runtime_name}"),
            ),
        )
        .await;

        let cmd = CommsCommand::PeerMessage {
            objective_id: None,
            content_taint: None,
            blocks: None,
            to: peer_route(&peer_name, peer.public_key()),
            body: "not streamable".to_string(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
        };
        let result = CoreCommsRuntime::send_and_stream(&runtime, cmd).await;
        match result {
            Err(SendAndStreamError::StreamAttach { receipt, error }) => {
                assert!(
                    matches!(receipt, SendReceipt::PeerMessageSent { .. }),
                    "receipt should be PeerMessageSent"
                );
                assert!(
                    matches!(error, StreamError::NotFound(_)),
                    "error should be NotFound for non-streamable command"
                );
            }
            Err(e) => panic!("expected StreamAttach error, got send error: {e}"),
            Ok(_) => panic!("expected StreamAttach error, got Ok"),
        }
    }

    #[tokio::test]
    async fn test_m5_send_and_stream_input_none_is_not_streamable() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = inproc_only_with_test_peer_authority(&format!("m5-none-{suffix}"));
        let session_id = meerkat_core::SessionId::new();

        let cmd = CommsCommand::Input {
            blocks: None,
            session_id: session_id.clone(),
            body: "no stream".to_string(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            source: meerkat_core::InputSource::Rpc,
            stream: InputStreamMode::None,
            allow_self_session: true,
        };
        let result = CoreCommsRuntime::send_and_stream(&runtime, cmd).await;
        match result {
            Err(SendAndStreamError::StreamAttach { receipt, error }) => {
                assert!(
                    matches!(
                        receipt,
                        SendReceipt::InputAccepted {
                            stream_reserved: false,
                            ..
                        }
                    ),
                    "receipt should indicate no stream reserved"
                );
                assert!(matches!(error, StreamError::NotFound(_)));
            }
            Err(e) => panic!("expected StreamAttach error, got send error: {e}"),
            Ok(_) => panic!("expected StreamAttach error, got Ok"),
        }
    }

    #[tokio::test]
    async fn test_m4_attach_after_completed_returns_not_reserved() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = inproc_only_with_test_peer_authority(&format!("post-comp-{suffix}"));
        let session_id = meerkat_core::SessionId::new();

        let cmd = CommsCommand::Input {
            blocks: None,
            session_id: session_id.clone(),
            body: "hello".to_string(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            source: meerkat_core::InputSource::Rpc,
            stream: InputStreamMode::ReserveInteraction,
            allow_self_session: true,
        };
        let receipt = CoreCommsRuntime::send(&runtime, cmd).await.unwrap();
        let iid = match receipt {
            SendReceipt::InputAccepted { interaction_id, .. } => interaction_id,
            _ => panic!("expected InputAccepted"),
        };

        // Attach
        let _stream = CoreCommsRuntime::stream(&runtime, StreamScope::Interaction(iid)).unwrap();

        // Complete
        runtime.mark_interaction_complete(iid.0);

        // Try to attach again → NotReserved
        let result = CoreCommsRuntime::stream(&runtime, StreamScope::Interaction(iid));
        assert!(
            matches!(result, Err(StreamError::NotReserved(_))),
            "attach after completed should return NotReserved"
        );
    }

    #[tokio::test]
    async fn test_allow_self_session_guard_rejects_default() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = CommsRuntime::inproc_only(&format!("self-guard-{suffix}")).unwrap();
        let cmd = CommsCommand::Input {
            blocks: None,
            session_id: meerkat_core::SessionId::new(),
            body: "blocked".to_string(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            source: meerkat_core::InputSource::Rpc,
            stream: InputStreamMode::None,
            allow_self_session: false,
        };
        let result = CoreCommsRuntime::send(&runtime, cmd).await;
        assert!(
            matches!(result, Err(SendError::Validation(_))),
            "allow_self_session=false should reject self-input"
        );
    }

    #[tokio::test]
    async fn test_remove_trusted_peer_removes_from_peers_and_revokes_send() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender_name = format!("rm-sender-{suffix}");
        let receiver_name = format!("rm-receiver-{suffix}");
        let sender = CommsRuntime::inproc_only(&sender_name).unwrap();
        let receiver = CommsRuntime::inproc_only(&receiver_name).unwrap();

        // Add receiver as trusted peer of sender
        let peer_spec = trusted_descriptor(
            &receiver_name,
            receiver.public_key(),
            &format!("inproc://{receiver_name}"),
        );

        let trust_dsl = add_trusted_peer_with_generated_authority(&sender, peer_spec.clone()).await;

        // Verify receiver appears in sender's peers
        let peers = CoreCommsRuntime::peers(&sender).await;
        let receiver_entries: Vec<_> = peers
            .iter()
            .filter(|entry| entry.name.as_str() == receiver_name)
            .collect();
        assert_eq!(
            receiver_entries.len(),
            1,
            "receiver should appear in sender's peers after add"
        );

        // Remove the trusted peer by canonical PeerId.
        let remove_authority =
            test_projection_remove_authority_with_dsl(Arc::clone(&trust_dsl), &peer_spec);
        let removed = CoreCommsRuntime::apply_trust_mutation(
            &sender,
            CommsTrustMutation::RemoveTrustedPeer {
                peer_id: peer_spec.peer_id.to_string(),
                authority: remove_authority.authority,
            },
        )
        .await
        .expect("remove_trusted_peer should succeed");
        let CommsTrustMutationResult::Removed { removed } = removed else {
            panic!("unexpected generated trust mutation result: {removed:?}");
        };
        assert!(removed, "remove should return true for existing peer");

        // Verify receiver no longer appears in sender's peers
        let peers_after = CoreCommsRuntime::peers(&sender).await;
        let receiver_entries_after: Vec<_> = peers_after
            .iter()
            .filter(|entry| entry.name.as_str() == receiver_name)
            .collect();
        assert!(
            receiver_entries_after.is_empty(),
            "receiver should not appear in sender's peers after removal"
        );

        // Verify sending to the removed peer fails (PeerNotFound)
        let cmd = CommsCommand::PeerMessage {
            objective_id: None,
            content_taint: None,
            blocks: None,
            to: peer_route(&receiver_name, receiver.public_key()),
            body: "should fail".to_string(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
        };
        let result = CoreCommsRuntime::send(&sender, cmd).await;
        assert!(
            matches!(result, Err(SendError::PeerNotFound(_))),
            "send to removed peer should fail with PeerNotFound, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_add_private_trusted_peer_zero_pubkey_fails_closed_without_generated_authority() {
        let suffix = Uuid::new_v4().simple().to_string();
        let runtime = CommsRuntime::inproc_only(&format!("rm-private-zero-{suffix}")).unwrap();
        let peer_id = PeerId::new();
        let peer_name = format!("private-legacy-peer-{suffix}");
        let spec = TrustedPeerDescriptor::test_only_unsigned_typed(
            peer_name.clone(),
            peer_id,
            format!("inproc://{peer_name}"),
        )
        .expect("valid zero-pubkey descriptor");

        let result = CoreCommsRuntime::add_private_trusted_peer(&runtime, spec).await;

        assert!(
            matches!(result, Err(SendError::Unsupported(ref message)) if message.contains("generated comms private trust mutation authority")),
            "raw private zero-pubkey add must fail closed before mutation: {result:?}"
        );
        let removed = CoreCommsRuntime::remove_private_trusted_peer(&runtime, &peer_id.to_string())
            .await
            .expect_err("raw private removal must fail closed");
        assert!(
            matches!(removed, SendError::Unsupported(ref message) if message.contains("generated comms private trust mutation authority")),
            "raw private removal must fail closed: {removed:?}"
        );
    }

    #[tokio::test]
    async fn test_remove_trusted_peer_returns_false_for_absent_peer() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender = CommsRuntime::inproc_only(&format!("rm-absent-{suffix}")).unwrap();
        let other = CommsRuntime::inproc_only(&format!("rm-absent-other-{suffix}")).unwrap();

        let peer_id = other.public_key().to_peer_id().to_string();
        let peer = trusted_descriptor(
            &format!("rm-absent-other-{suffix}"),
            other.public_key(),
            &format!("inproc://rm-absent-other-{suffix}"),
        );
        let removed = remove_trusted_peer_with_generated_authority(&sender, &peer)
            .await
            .expect("remove_trusted_peer should succeed even for absent peer");
        assert_eq!(peer.peer_id.to_string(), peer_id);
        assert!(
            !removed,
            "remove should return false for peer that was never trusted"
        );
    }

    #[tokio::test]
    async fn test_remove_trusted_peer_rejects_pubkey_string_argument() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender = CommsRuntime::inproc_only(&format!("rm-pubkey-sender-{suffix}")).unwrap();
        let receiver = CommsRuntime::inproc_only(&format!("rm-pubkey-receiver-{suffix}")).unwrap();

        let peer_pubkey_string = receiver.public_key().to_pubkey_string();
        let peer = trusted_descriptor(
            &format!("rm-pubkey-receiver-{suffix}"),
            receiver.public_key(),
            &format!("inproc://rm-pubkey-receiver-{suffix}"),
        );
        let remove_authority =
            test_projection_remove_authority(&local_descriptor_for_runtime(&sender), &peer, 0);
        remove_authority.install_owner(&sender);
        let result = CoreCommsRuntime::apply_trust_mutation(
            &sender,
            CommsTrustMutation::RemoveTrustedPeer {
                authority: remove_authority.authority,
                peer_id: peer_pubkey_string,
            },
        )
        .await;

        assert!(
            matches!(result, Err(SendError::Validation(_))),
            "remove_trusted_peer should validate its argument as PeerId, got: {result:?}"
        );
    }

    /// T-13: `stage_declared_reply_endpoint` forwards to
    /// `Router::stage_reply_endpoint` — staged entry visible (routes a
    /// Response to an untrusted peer), one-shot (second send misses), and
    /// Response-only (a Message kind never consults the staged entry).
    #[tokio::test]
    async fn stage_declared_reply_endpoint_forwards_to_router_one_shot_response_only() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender_name = format!("declared-reply-sender-{suffix}");
        let sender = CommsRuntime::inproc_only(&sender_name).unwrap();
        // Declared reply endpoints are SOCKET addresses by construction
        // (inproc is typed-rejected at the router): the "receiver" is a raw
        // loopback listener draining frames — the sender does NOT trust it,
        // so only the staged endpoint can route the reply.
        let receiver_keypair = Keypair::generate();
        let receiver_pubkey = receiver_keypair.public_key();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind declared-reply listener");
        let receiver_address = format!("tcp://{}", listener.local_addr().expect("listener addr"));
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                use tokio::io::AsyncReadExt as _;
                let mut sink = Vec::new();
                let _ = stream.read_to_end(&mut sink).await;
            }
        });

        let dest = receiver_pubkey.to_peer_id();
        CoreCommsRuntime::stage_declared_reply_endpoint(
            &sender,
            dest,
            *receiver_pubkey.as_bytes(),
            receiver_address.clone(),
        )
        .await
        .expect("staging a declared reply endpoint must succeed");

        let response_kind = || MessageKind::Response {
            in_reply_to: Uuid::new_v4(),
            status: Status::Completed,
            result: serde_json::json!({"ok": true}),
            blocks: None,
            content_taint: None,
            handling_mode: None,
            objective_id: None,
        };
        sender
            .router()
            .send_with_id(dest, Uuid::new_v4(), response_kind(), None)
            .await
            .expect("staged endpoint must route the first Response");
        let second = sender
            .router()
            .send_with_id(dest, Uuid::new_v4(), response_kind(), None)
            .await;
        assert!(
            matches!(second, Err(crate::router::SendError::PeerNotFound(_))),
            "staged endpoint is one-shot; second Response must miss, got {second:?}"
        );

        // Response-only: a staged entry is never consulted for Message kinds.
        CoreCommsRuntime::stage_declared_reply_endpoint(
            &sender,
            dest,
            *receiver_pubkey.as_bytes(),
            receiver_address,
        )
        .await
        .expect("restaging must succeed");
        let message = sender
            .router()
            .send_with_id(
                dest,
                Uuid::new_v4(),
                MessageKind::Message {
                    body: "hi".to_string(),
                    blocks: None,
                    content_taint: None,
                    handling_mode: None,
                    objective_id: None,
                },
                None,
            )
            .await;
        assert!(
            matches!(message, Err(crate::router::SendError::PeerNotFound(_))),
            "staged endpoint must not grant Message sendability, got {message:?}"
        );
    }

    /// T-13: skip-if-routable — staging for a peer the trust store already
    /// resolves is a no-op (trust-store-wins), so no one-shot entry is left
    /// behind to go stale.
    #[tokio::test]
    async fn stage_declared_reply_endpoint_skips_already_routable_peer() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender_name = format!("declared-skip-sender-{suffix}");
        let receiver_name = format!("declared-skip-receiver-{suffix}");
        let sender = CommsRuntime::inproc_only(&sender_name).unwrap();
        let receiver = CommsRuntime::inproc_only(&receiver_name).unwrap();
        add_trusted_peer_with_generated_authority(
            &sender,
            trusted_descriptor(
                &receiver_name,
                receiver.public_key(),
                &format!("inproc://{receiver_name}"),
            ),
        )
        .await;
        add_trusted_peer_with_generated_authority(
            &receiver,
            trusted_descriptor(
                &sender_name,
                sender.public_key(),
                &format!("inproc://{sender_name}"),
            ),
        )
        .await;

        let dest = receiver.public_key().to_peer_id();
        // Deliberately stage a BROKEN address: if the impl staged despite the
        // trust row, nothing changes (trust wins at send time), and the
        // Response below must still deliver over the trusted route.
        CoreCommsRuntime::stage_declared_reply_endpoint(
            &sender,
            dest,
            *receiver.public_key().as_bytes(),
            "inproc://nonexistent-declared-target".to_string(),
        )
        .await
        .expect("staging for a routable peer must be an Ok no-op");

        sender
            .router()
            .send_with_id(
                dest,
                Uuid::new_v4(),
                MessageKind::Response {
                    in_reply_to: Uuid::new_v4(),
                    status: Status::Completed,
                    result: serde_json::json!({"ok": true}),
                    blocks: None,
                    content_taint: None,
                    handling_mode: None,
                    objective_id: None,
                },
                None,
            )
            .await
            .expect("trusted route must serve the Response");
    }

    #[tokio::test]
    async fn correlated_reply_endpoint_trait_staging_validates_identity_and_cleans_idempotently() {
        let runtime = CommsRuntime::inproc_only("correlated-stage-runtime").unwrap();
        let peer = Keypair::generate().public_key();
        let dest = peer.to_peer_id();
        let correlation = InteractionId(Uuid::new_v4());
        let endpoint = meerkat_core::comms::PeerAddress::parse("tcp://127.0.0.1:4311")
            .expect("valid endpoint");

        let mismatch = CoreCommsRuntime::stage_correlated_reply_endpoint(
            &runtime,
            PeerId::new(),
            correlation,
            *peer.as_bytes(),
            endpoint.clone(),
        )
        .await;
        assert!(matches!(mismatch, Err(SendError::Validation(_))));

        let uds = CoreCommsRuntime::stage_correlated_reply_endpoint(
            &runtime,
            dest,
            correlation,
            *peer.as_bytes(),
            meerkat_core::comms::PeerAddress::parse("uds:///tmp/reply.sock")
                .expect("scheme-level UDS endpoint parses"),
        )
        .await;
        assert!(
            matches!(uds, Err(SendError::Validation(_))),
            "correlated callback staging is TCP-only"
        );

        CoreCommsRuntime::stage_correlated_reply_endpoint(
            &runtime,
            dest,
            correlation,
            *peer.as_bytes(),
            endpoint,
        )
        .await
        .expect("authenticated correlated endpoint must stage");
        CoreCommsRuntime::unstage_correlated_reply_endpoint(&runtime, dest, correlation)
            .await
            .expect("first cleanup must succeed");
        CoreCommsRuntime::unstage_correlated_reply_endpoint(&runtime, dest, correlation)
            .await
            .expect("cleanup is idempotent");

        let result = runtime
            .router()
            .send_with_id(
                dest,
                Uuid::new_v4(),
                MessageKind::Response {
                    in_reply_to: correlation.0,
                    status: Status::Failed,
                    result: serde_json::json!({"error": "not sent"}),
                    blocks: None,
                    content_taint: None,
                    handling_mode: None,
                    objective_id: None,
                },
                None,
            )
            .await;
        assert!(matches!(
            result,
            Err(crate::router::SendError::PeerNotFound(_))
        ));
    }

    #[tokio::test]
    async fn peer_interaction_cleanup_removes_staged_correlated_reply_endpoint() {
        let runtime = CommsRuntime::inproc_only("correlated-cleanup-runtime").unwrap();
        let peer = Keypair::generate().public_key();
        let dest = peer.to_peer_id();
        let correlation = InteractionId(Uuid::new_v4());
        CoreCommsRuntime::stage_correlated_reply_endpoint(
            &runtime,
            dest,
            correlation,
            *peer.as_bytes(),
            meerkat_core::comms::PeerAddress::parse("tcp://127.0.0.1:4311")
                .expect("valid endpoint"),
        )
        .await
        .expect("correlated endpoint must stage");

        meerkat_core::handles::PeerInteractionCleanupObserver::on_peer_interaction_cleanup(
            &runtime,
            meerkat_core::PeerCorrelationId::from_uuid(correlation.0),
        );

        let result = runtime
            .router()
            .send_with_id(
                dest,
                Uuid::new_v4(),
                MessageKind::Response {
                    in_reply_to: correlation.0,
                    status: Status::Failed,
                    result: serde_json::json!({"error": "must not dial"}),
                    blocks: None,
                    content_taint: None,
                    handling_mode: None,
                    objective_id: None,
                },
                None,
            )
            .await;
        assert!(matches!(
            result,
            Err(crate::router::SendError::PeerNotFound(_))
        ));
    }

    #[tokio::test]
    async fn send_peer_request_at_endpoint_uses_standard_lifecycle_without_inproc_callback_route() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender_name = format!("endpoint-sender-{suffix}");
        let receiver_name = format!("endpoint-receiver-{suffix}");
        let sender = Arc::new(CommsRuntime::inproc_only(&sender_name).unwrap());
        let receiver = Arc::new(CommsRuntime::inproc_only(&receiver_name).unwrap());
        let peer_handle = Arc::new(TestPeerInteractionHandle::default());
        sender.install_peer_request_response_authority(PeerRequestResponseAuthority::new(
            peer_handle.clone(),
            Arc::new(TestInteractionStreamHandle::default()),
        ));
        install_test_peer_comms_handle(receiver.as_ref());
        add_trusted_peer_with_generated_authority(
            sender.as_ref(),
            trusted_descriptor(
                &receiver_name,
                receiver.public_key(),
                &format!("inproc://{receiver_name}"),
            ),
        )
        .await;
        add_trusted_peer_with_generated_authority(
            receiver.as_ref(),
            trusted_descriptor(
                &sender_name,
                sender.public_key(),
                &format!("inproc://{sender_name}"),
            ),
        )
        .await;
        let endpoint = meerkat_core::comms::PeerAddress::parse("tcp://127.0.0.1:4311")
            .expect("valid reply endpoint");

        let receipt = sender
            .send_peer_request_at_endpoint(
                peer_route(&receiver_name, receiver.public_key()),
                "test.probe",
                serde_json::json!({"seq": 1}),
                endpoint.clone(),
            )
            .await
            .expect("endpoint-bearing peer Request must dispatch");
        let SendReceipt::PeerRequestSent { interaction_id, .. } = receipt else {
            panic!("expected PeerRequestSent receipt");
        };
        assert_eq!(
            meerkat_core::handles::PeerInteractionHandle::outbound_state(
                peer_handle.as_ref(),
                meerkat_core::PeerCorrelationId::from_uuid(interaction_id.0),
            ),
            Some(meerkat_core::OutboundPeerRequestState::Sent)
        );
        assert!(
            !CoreCommsRuntime::has_bridge_reply_waiter(sender.as_ref(), &interaction_id),
            "no-waiter helper must not mutate the session-drain waiter registry"
        );

        let candidates = CoreCommsRuntime::drain_peer_input_candidates(receiver.as_ref()).await;
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].ingress.declared_reply_endpoint, None,
            "serialized endpoint data is not promoted without observed TCP source evidence"
        );
    }

    #[tokio::test]
    async fn send_peer_request_at_endpoint_rejects_non_tcp_endpoint() {
        let runtime = CommsRuntime::inproc_only("endpoint-transport-validation").unwrap();
        let result = runtime
            .send_peer_request_at_endpoint(
                peer_route("unused", Keypair::generate().public_key()),
                "test.probe",
                serde_json::json!({}),
                meerkat_core::comms::PeerAddress::parse("uds:///tmp/reply.sock")
                    .expect("scheme-level UDS endpoint parses"),
            )
            .await;
        assert!(matches!(result, Err(SendError::Validation(_))));
    }

    /// T-13: `send_peer_request_with_reply_waiter` registers the waiter
    /// BEFORE dispatch (a response racing the send return still finds it)
    /// and records the outbound DSL lifecycle exactly like the
    /// `CommsCommand::PeerRequest` arm.
    #[tokio::test]
    async fn send_peer_request_with_reply_waiter_registers_waiter_and_dsl_lifecycle() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender_name = format!("waiter-sender-{suffix}");
        let receiver_name = format!("waiter-receiver-{suffix}");
        let sender = Arc::new(CommsRuntime::inproc_only(&sender_name).unwrap());
        let receiver = Arc::new(CommsRuntime::inproc_only(&receiver_name).unwrap());
        let peer_handle = Arc::new(TestPeerInteractionHandle::default());
        sender.install_peer_request_response_authority(PeerRequestResponseAuthority::new(
            peer_handle.clone(),
            Arc::new(TestInteractionStreamHandle::default()),
        ));
        install_test_peer_comms_handle(receiver.as_ref());
        add_trusted_peer_with_generated_authority(
            sender.as_ref(),
            trusted_descriptor(
                &receiver_name,
                receiver.public_key(),
                &format!("inproc://{receiver_name}"),
            ),
        )
        .await;
        add_trusted_peer_with_generated_authority(
            receiver.as_ref(),
            trusted_descriptor(
                &sender_name,
                sender.public_key(),
                &format!("inproc://{sender_name}"),
            ),
        )
        .await;

        let (reply_rx, envelope_id) = sender
            .send_peer_request_with_reply_waiter(
                peer_route(&receiver_name, receiver.public_key()),
                "test.bridge.upcall",
                serde_json::json!({"op": "ping"}),
            )
            .await
            .expect("waiter-registered request must dispatch");
        drop(reply_rx);

        let interaction_id = InteractionId(envelope_id);
        assert!(
            CoreCommsRuntime::has_bridge_reply_waiter(sender.as_ref(), &interaction_id),
            "the waiter must be registered under the returned envelope id"
        );
        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(envelope_id);
        assert_eq!(
            meerkat_core::handles::PeerInteractionHandle::outbound_state(
                peer_handle.as_ref(),
                corr_id
            ),
            Some(meerkat_core::OutboundPeerRequestState::Sent),
            "request_sent must be recorded exactly like the PeerRequest arm"
        );

        let interactions = CoreCommsRuntime::drain_inbox_interactions(receiver.as_ref()).await;
        assert_eq!(interactions.len(), 1, "receiver must see the Request");
        assert!(matches!(
            &interactions[0].content,
            meerkat_core::InteractionContent::Request { intent, .. }
                if intent == "test.bridge.upcall"
        ));

        // take is one-shot and consumes the live waiter.
        assert!(
            CoreCommsRuntime::take_bridge_reply_waiter(sender.as_ref(), &interaction_id).is_some()
        );
        assert!(
            !CoreCommsRuntime::has_bridge_reply_waiter(sender.as_ref(), &interaction_id),
            "take must consume the waiter entry"
        );
    }

    /// T-13: a failed send removes the waiter (no stale registration) and
    /// fires `request_send_failed` — the typed error is returned verbatim.
    #[tokio::test]
    async fn send_peer_request_with_reply_waiter_removes_waiter_on_send_failure() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender = Arc::new(CommsRuntime::inproc_only(&format!("waiter-fail-{suffix}")).unwrap());
        let peer_handle = Arc::new(TestPeerInteractionHandle::default());
        sender.install_peer_request_response_authority(PeerRequestResponseAuthority::new(
            peer_handle.clone(),
            Arc::new(TestInteractionStreamHandle::default()),
        ));

        let result = sender
            .send_peer_request_with_reply_waiter(
                missing_peer_route(&format!("waiter-missing-{suffix}")),
                "test.bridge.upcall",
                serde_json::json!({"op": "ping"}),
            )
            .await;
        assert!(
            matches!(result, Err(SendError::PeerNotFound(_))),
            "route failure must surface the typed send error, got {result:?}"
        );
    }

    /// T-13/§5.3: tombstoning replaces a live waiter; the entry still
    /// answers `has_bridge_reply_waiter` (so the drain discards the late
    /// reply) but `take` yields no sender and consumes the tombstone.
    #[tokio::test]
    async fn tombstoned_bridge_reply_waiter_discards_instead_of_delivering() {
        let suffix = Uuid::new_v4().simple().to_string();
        let sender_name = format!("waiter-tomb-sender-{suffix}");
        let receiver_name = format!("waiter-tomb-receiver-{suffix}");
        let sender = Arc::new(CommsRuntime::inproc_only(&sender_name).unwrap());
        let receiver = Arc::new(CommsRuntime::inproc_only(&receiver_name).unwrap());
        let peer_handle = Arc::new(TestPeerInteractionHandle::default());
        sender.install_peer_request_response_authority(PeerRequestResponseAuthority::new(
            peer_handle.clone(),
            Arc::new(TestInteractionStreamHandle::default()),
        ));
        install_test_peer_comms_handle(receiver.as_ref());
        add_trusted_peer_with_generated_authority(
            sender.as_ref(),
            trusted_descriptor(
                &receiver_name,
                receiver.public_key(),
                &format!("inproc://{receiver_name}"),
            ),
        )
        .await;
        add_trusted_peer_with_generated_authority(
            receiver.as_ref(),
            trusted_descriptor(
                &sender_name,
                sender.public_key(),
                &format!("inproc://{sender_name}"),
            ),
        )
        .await;

        let (_reply_rx, envelope_id) = sender
            .send_peer_request_with_reply_waiter(
                peer_route(&receiver_name, receiver.public_key()),
                "test.bridge.upcall",
                serde_json::json!({"op": "ping"}),
            )
            .await
            .expect("waiter-registered request must dispatch");

        sender.tombstone_bridge_reply_waiter(envelope_id);
        let interaction_id = InteractionId(envelope_id);
        assert!(
            CoreCommsRuntime::has_bridge_reply_waiter(sender.as_ref(), &interaction_id),
            "tombstone must keep answering has_bridge_reply_waiter so late replies are discarded"
        );
        assert!(
            CoreCommsRuntime::take_bridge_reply_waiter(sender.as_ref(), &interaction_id).is_none(),
            "a tombstoned entry must not yield a live sender"
        );
        assert!(
            !CoreCommsRuntime::has_bridge_reply_waiter(sender.as_ref(), &interaction_id),
            "take must consume the tombstone"
        );
    }
}
