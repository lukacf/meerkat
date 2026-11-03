//! In-process message transport for peer communication within one runtime.
//!
//! This module provides a process-global registry that allows agents within
//! the same process to communicate without network sockets. Messages are
//! delivered directly via in-memory channels.
//!
//! # Usage
//!
//! ```text
//! // Register an agent's inbox
//! let (inbox, sender) = Inbox::new_transport_only();
//! InprocRegistry::global().register("my-agent", pubkey, sender);
//!
//! // Delivery is pubkey-keyed: the Router resolves a trusted peer's
//! // signing key and delivers through the namespace-scoped
//! // send_to_pubkey_*_wait owners.
//!
//! // Unregister when done
//! InprocRegistry::global().unregister(&pubkey);
//! ```
//!
//! # Wake routes
//!
//! Besides live routes (an inbox that receives now), the registry holds a
//! second table of wake routes: one per seated member of a mob whose member is
//! not necessarily materialized in this process. A wake route is a declared
//! projection of its owner's membership, never an inbox: it is published and
//! retracted only from the owner's machine effects, it occupies no name, and
//! it delivers nothing. When a send finds no live route (or a closed one) for
//! a key that has a wake route, the registry asks the route's owner to wake
//! the member, holds the envelope in the sender's own send future meanwhile,
//! and hands it off once, through the ordinary receipt path, after the owner
//! answered that the member is ready. A key with neither route is not a
//! member: the send fails `PeerNotFound` exactly as before.

#[cfg(target_arch = "wasm32")]
use crate::tokio;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, Weak};

use meerkat_core::comms::PeerWakeRefusal;
use meerkat_core::time_compat::Instant;
use parking_lot::RwLock;
use uuid::Uuid;

use crate::identity::{Keypair, PubKey, Signature};
use crate::inbox::{
    AdmissionOutcome, BoundedIngressDelivery, DropReason, InboxSender, IngressDeliveryOutcome,
};
use crate::peer_meta::PeerMeta;
use crate::types::{Envelope, InboxItem, MessageKind};

const DEFAULT_NAMESPACE: &str = "";

#[derive(Debug)]
pub(crate) struct InprocDelivery {
    pub(crate) envelope_id: Uuid,
    pub(crate) outcome: InprocDeliveryOutcome,
}

/// What the in-process lane can truthfully say about one delivery attempt.
///
/// `Resolved` carries the receiver's own terminal ingress fact. `Unconfirmed`
/// says only that the envelope IS sitting on the receiver's queue and no drain
/// committed it inside the sender's bound: not a drop, not a success. Callers
/// must not launder it into either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InprocDeliveryOutcome {
    Resolved(IngressDeliveryOutcome),
    Unconfirmed { park_bound: std::time::Duration },
}

#[cfg(test)]
impl InprocDelivery {
    /// Test-only accessor asserting the receiver produced a terminal fact.
    ///
    /// Deliberately panics on `Unconfirmed` so a delivery test that silently
    /// starts timing out is a failure, not a pass on a different fact.
    #[allow(clippy::panic)]
    fn resolved_outcome(&self) -> IngressDeliveryOutcome {
        match self.outcome {
            InprocDeliveryOutcome::Resolved(outcome) => outcome,
            InprocDeliveryOutcome::Unconfirmed { park_bound } => panic!(
                "expected a receiver-resolved inproc delivery, got unconfirmed after {park_bound:?}"
            ),
        }
    }
}

/// Snapshot of an inproc peer returned by [`InprocRegistry::peers()`].
#[derive(Debug, Clone)]
pub struct InprocPeerInfo {
    pub name: String,
    pub pubkey: PubKey,
    pub meta: PeerMeta,
}

/// Why a registration was rejected without mutating the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RegistrationRejection {
    /// The supplied pubkey was the all-zero key, which can never identify a
    /// distinct peer and is refused fail-closed.
    ZeroPubkey,
    /// This participant name already has a live route, so it is not the
    /// registrant's to claim.
    ///
    /// `holder_pubkey` is the incumbent's key, and it is *evidence*, not a
    /// verdict about the registrant's identity. Whether it equals the
    /// registrant's own key changes nothing about the refusal: a live route
    /// under this name means the incumbent generation is still receiving, and
    /// registration cannot tell one host rebuilding itself apart from a second
    /// live host of the same identity. Both are refused, without mutation, and
    /// the incumbent keeps routing.
    ///
    /// Succession is proved, never asserted. Either the incumbent releases the
    /// name first (`CommsRuntime::retire_inproc_route`, or its `Drop`), leaving
    /// it unbound for the next claimant, or the successor hands in the exact
    /// predecessor generation it is replacing through
    /// `InprocRegistry::replace_sender_in_namespace`. A registrant that can do
    /// neither has not proved the incumbent is gone.
    NameOccupied { holder_pubkey: PubKey },
}

impl std::fmt::Display for RegistrationRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroPubkey => {
                f.write_str("the all-zero public key can never identify a distinct peer")
            }
            // Remedy first. The holder key is named last and explicitly framed
            // as evidence: in 0.8.23 a refusal that led with a public key read
            // as a crypto/trust failure when the real cause was an unreleased
            // predecessor, and the misreading cost an adopter fleet an hour.
            Self::NameOccupied { holder_pubkey } => write!(
                f,
                "the incumbent has not released this participant name; retire it first \
                 (release the live route via CommsRuntime::retire_inproc_route, or drop the \
                 runtime holding it) or publish through the replacing seam with the predecessor \
                 generation in hand. The live route is held by public key {} - that is evidence \
                 of who holds the name, not a key or trust failure; it may well be this \
                 registrant's own earlier generation",
                holder_pubkey.to_pubkey_string()
            ),
        }
    }
}

/// Typed result of registering an inproc peer.
///
/// Registration claims a participant name, and there is exactly one rule for
/// that claim: the name must be unbound. A registrant may move its own key onto
/// a free name ([`RegistrationOutcome::ReplacedPubkey`]); no registration path
/// unbinds a live route under the name being claimed, whichever key holds it
/// ([`RegistrationRejection::NameOccupied`]). Callers (runtime constructors,
/// metadata refresh) must observe these facts rather than assume a clean
/// success.
///
/// There is deliberately no "rebound my own name" outcome. Succeeding a live
/// generation is proved by evidence, not by identity: the incumbent releases the
/// name, or the successor hands its exact generation in through
/// `InprocRegistry::replace_sender_in_namespace`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationOutcome {
    /// The peer was inserted without touching any existing route.
    Registered,
    /// This pubkey was already registered under a different name; the old name
    /// mapping was removed and replaced with the new name.
    ///
    /// The *claimed* name was still unbound, so this is a rename inside one
    /// identity, not a claim over a live route.
    ReplacedPubkey { evicted_name: String },
    /// The registration was refused without mutating the registry.
    Rejected { reason: RegistrationRejection },
}

impl RegistrationOutcome {
    /// Whether the registration was rejected (no mutation occurred).
    pub fn is_rejected(&self) -> bool {
        matches!(self, Self::Rejected { .. })
    }
}

/// Why publishing an exact inproc runtime replacement failed before mutation.
#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InprocPublicationError {
    #[error("the prepared and current runtimes do not have the same namespace and name")]
    IdentityMismatch,
    #[error("the current runtime has no published inproc generation")]
    CurrentRuntimeUnpublished,
    #[error("the replacement runtime has an invalid zero public key")]
    ZeroPubkey,
    #[error("the expected current inproc generation is no longer published")]
    ExpectedGenerationNotCurrent,
    #[error("the replacement public key is already occupied")]
    ReplacementPubkeyOccupied,
}

/// Global inproc registry instance.
static GLOBAL_REGISTRY: OnceLock<InprocRegistry> = OnceLock::new();

/// Registry entry for an inproc peer.
#[derive(Clone)]
struct InprocPeer {
    name: String,
    pubkey: PubKey,
    sender: InboxSender,
    meta: PeerMeta,
}

/// What the owner of a wake route is asked when a send needs its member.
///
/// `observed_missing` is the raw routing observation behind the request: the
/// member had no live route, or its live route was closed. `sender_waking`
/// is the sender's own residency as its owner published it: the send comes
/// from a member that is itself being woken (from inside its own build). The
/// registry forwards both as observations; the owner's machine decides.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PeerWakeRequest {
    /// The owner's name for the member behind the route (as published).
    pub member: String,
    /// The recipient's signing key (the route's key).
    pub recipient: PubKey,
    /// The sender's signing key.
    pub sender: PubKey,
    /// The envelope the sender holds until the member is ready.
    pub envelope_id: Uuid,
    pub observed_missing: bool,
    pub input_class: PeerWakeInputClass,
    pub sender_waking: bool,
}

/// What kind of input a held envelope is, classified at the sender from its
/// [`MessageKind`]. A silent lifecycle notice never needs its recipient
/// woken: the recipient's topology at its wake carries the notice's content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PeerWakeInputClass {
    /// Model-facing or reply traffic: a message, request, response, or a
    /// kickoff status notice.
    Content,
    /// A silent control-plane lifecycle notice (peer added, retired,
    /// unwired, dismiss).
    Lifecycle,
}

impl PeerWakeInputClass {
    /// Classify a held envelope's kind.
    pub fn of(kind: &MessageKind) -> Self {
        match kind {
            MessageKind::Lifecycle { kind, .. } if !kind.is_kickoff() => Self::Lifecycle,
            _ => Self::Content,
        }
    }
}

/// What a wake route's owner answered when the member can receive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerWakeAnswer {
    /// The member is ready (woken, or already live): resolve its live route
    /// and hand the envelope off once.
    Ready,
    /// The envelope is a silent control notice (`PeerWakeInputClass::Lifecycle`)
    /// the owner consumed for its member without waking it: the member's
    /// topology at its next wake carries the notice's content, exactly as a
    /// live member's drain consumes such a notice without a turn. Nothing is
    /// handed to an inbox.
    ControlNoticeConsumed,
}

/// Why a wake route's owner did not make the member ready.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerWakeDenial {
    /// The key's member is no longer a member of the owner (the route is
    /// stale). The send is `PeerNotFound`, as for any non-member.
    NotAMember,
    /// The member is a member, and its wake was refused or did not finish.
    Refused(PeerWakeRefusal),
}

/// The future a wake answers with.
#[cfg(not(target_arch = "wasm32"))]
pub type PeerWakeFuture =
    futures::future::BoxFuture<'static, Result<PeerWakeAnswer, PeerWakeDenial>>;
/// The future a wake answers with (wasm32: single-threaded, not `Send`).
#[cfg(target_arch = "wasm32")]
pub type PeerWakeFuture =
    futures::future::LocalBoxFuture<'static, Result<PeerWakeAnswer, PeerWakeDenial>>;

/// The owner of wake routes: wakes the seated member behind a route.
///
/// Implemented by the member's mob. The owner answers from its machine:
/// whether the member is woken, already live, or refused (typed). It never
/// delivers the envelope itself; the registry hands it off once after the
/// answer.
pub trait InprocPeerWaker: Send + Sync {
    /// Wake the member behind `request.member`, and answer once it is ready
    /// to receive, or with the typed refusal.
    fn wake(&self, request: PeerWakeRequest) -> PeerWakeFuture;

    /// Whether the member behind `member` is being woken right now, as the
    /// owner last published its machine state. An observation only: it is
    /// forwarded to the recipient's owner as `sender_waking`.
    fn member_is_waking(&self, member: &str) -> bool;
}

/// A seated member's wake route.
#[derive(Clone)]
struct WakeRoute {
    member: String,
    waker: Weak<dyn InprocPeerWaker>,
}

/// A wake route whose owner is alive.
struct ResolvedWakeRoute {
    member: String,
    waker: Arc<dyn InprocPeerWaker>,
}

/// Internal namespace state protected by a single lock to prevent deadlocks.
#[derive(Default)]
struct NamespaceState {
    /// Map from pubkey to peer entry.
    peers: HashMap<PubKey, InprocPeer>,
    /// Map from name to pubkey for name-based lookup.
    names: HashMap<String, PubKey>,
    /// Wake routes of seated members, by key. A declared projection of their
    /// owners' membership (published and retracted only from the owners'
    /// machine effects); never an inbox and never a name claim.
    wake_routes: HashMap<PubKey, WakeRoute>,
    /// Live routes their runtime closed for peer ingress while it lives on
    /// (a stopped mob's live members). Never resolved for delivery and never a
    /// name claim; kept only so the exact generation can reopen with its
    /// metadata.
    closed_routes: HashMap<PubKey, InprocPeer>,
}

/// Where a send resolves its destination: inside one namespace (the
/// namespace-isolated routers), or in any namespace, failing closed on
/// ambiguity.
#[derive(Clone, Copy)]
enum RouteScope<'a> {
    Namespace(&'a str),
    AnyNamespace,
}

/// Internal registry state keyed by namespace.
struct RegistryState {
    namespaces: HashMap<String, NamespaceState>,
}

impl RegistryState {
    fn namespace_mut(&mut self, namespace: &str) -> &mut NamespaceState {
        self.namespaces.entry(namespace.to_string()).or_default()
    }

    fn namespace(&self, namespace: &str) -> Option<&NamespaceState> {
        self.namespaces.get(namespace)
    }

    fn namespace_len(&self, namespace: &str) -> usize {
        self.namespace(namespace).map_or(0, |ns| ns.peers.len())
    }

    fn namespace_is_empty(&self, namespace: &str) -> bool {
        self.namespace_len(namespace) == 0
    }
}

/// Process-global registry for in-process peer communication.
///
/// This registry maps agent pubkeys to their inbox senders, allowing
/// direct message delivery without network transport.
///
/// # Thread Safety
///
/// All operations are protected by a single RwLock to ensure consistent
/// state and prevent deadlocks.
pub struct InprocRegistry {
    state: RwLock<RegistryState>,
}

impl InprocRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            state: RwLock::new(RegistryState {
                namespaces: HashMap::new(),
            }),
        }
    }

    /// Get the global registry instance.
    ///
    /// This creates the registry on first access.
    pub fn global() -> &'static InprocRegistry {
        GLOBAL_REGISTRY.get_or_init(InprocRegistry::new)
    }

    /// Register an agent's inbox for inproc communication.
    ///
    /// Returns a typed [`RegistrationOutcome`] describing whether the insert was
    /// clean or displaced an existing route (see
    /// [`register_with_meta_in_namespace`](Self::register_with_meta_in_namespace)).
    pub fn register(
        &self,
        name: impl Into<String>,
        pubkey: PubKey,
        sender: InboxSender,
    ) -> RegistrationOutcome {
        self.register_with_meta_in_namespace(
            DEFAULT_NAMESPACE,
            name,
            pubkey,
            sender,
            PeerMeta::default(),
        )
    }

    /// Register an agent's inbox within an explicit namespace.
    ///
    /// ONE claim rule, for every registrant with no exceptions: a name may be
    /// published only while it is UNBOUND. Key identity does not fork it.
    ///
    /// - a name that already has a live route is refused before any mutation
    ///   ([`RegistrationRejection::NameOccupied`]) and the incumbent keeps
    ///   routing, *whichever key holds it*. Delivery is key-addressed and this
    ///   registry cannot see hosts, so a peer rebuilding itself and a second
    ///   live host of that same identity arrive here as the same event. Only
    ///   one of them is safe to admit, so neither is: honouring the newcomer
    ///   would silently orphan a route that is more likely to be live, not
    ///   less, precisely when the two keys match.
    /// - succession is admitted on EVIDENCE, in one of two forms. Either the
    ///   incumbent released the name first - `CommsRuntime::retire_inproc_route`
    ///   at a chosen point, or its `Drop` - which leaves the name unbound for
    ///   an ordinary claim; or the successor hands in the exact predecessor
    ///   generation through
    ///   `replace_sender_in_namespace`,
    ///   which authorizes replacement by naming what it replaces. Holding the
    ///   same private key is not evidence of either.
    /// - re-registering an existing key under a *free* name is that
    ///   registrant's own rename ([`RegistrationOutcome::ReplacedPubkey`]). The
    ///   claimed name was unbound, so the one rule is satisfied.
    /// - a zero pubkey is refused ([`RegistrationRejection::ZeroPubkey`]).
    ///
    /// Callers must observe the outcome rather than assume a clean success.
    pub fn register_with_meta_in_namespace(
        &self,
        namespace: &str,
        name: impl Into<String>,
        pubkey: PubKey,
        sender: InboxSender,
        meta: PeerMeta,
    ) -> RegistrationOutcome {
        let name = name.into();
        if pubkey.is_zero() {
            tracing::warn!(
                inproc_namespace = %namespace,
                peer_name = %name,
                "rejecting zero-pubkey inproc registration"
            );
            return RegistrationOutcome::Rejected {
                reason: RegistrationRejection::ZeroPubkey,
            };
        }
        let peer = InprocPeer {
            name: name.clone(),
            pubkey,
            sender,
            meta,
        };

        let mut state = self.state.write();
        let namespace_state = state.namespace_mut(namespace);

        // Fail closed before any mutation, on occupancy alone. A live route
        // under this name is a serving peer's only inbox, and from here one
        // participant rebuilding itself is indistinguishable from a second live
        // instance of it. There is deliberately no key-identity exemption: the
        // same-key case is the one where the displaced route is MOST likely to
        // still be live, so admitting it is silent displacement wearing an
        // identity argument. Succession is proved instead - the incumbent
        // releases the name, or the successor names the exact generation it
        // replaces via `replace_sender_in_namespace`.
        if let Some(holder_pubkey) = namespace_state.names.get(&name).copied() {
            tracing::warn!(
                inproc_namespace = %namespace,
                peer_name = %name,
                holder_pubkey = %holder_pubkey.to_pubkey_string(),
                registrant_pubkey = %pubkey.to_pubkey_string(),
                holder_is_registrant = holder_pubkey == pubkey,
                "refusing inproc registration that would unbind a live route under this name"
            );
            return RegistrationOutcome::Rejected {
                reason: RegistrationRejection::NameOccupied { holder_pubkey },
            };
        }

        // The claimed name is unbound. This pubkey may still hold a *different*
        // name, and that route belongs to this same registrant, so releasing the
        // stale mapping is its own rename rather than a displacement of anyone.
        let evicted_name = namespace_state
            .peers
            .get(&pubkey)
            .map(|old_peer| old_peer.name.clone());
        if let Some(old_name) = &evicted_name {
            namespace_state.names.remove(old_name);
        }

        namespace_state.peers.insert(pubkey, peer);
        namespace_state.names.insert(name, pubkey);

        match evicted_name {
            Some(evicted_name) => RegistrationOutcome::ReplacedPubkey { evicted_name },
            None => RegistrationOutcome::Registered,
        }
    }

    /// Atomically replace one exact inbox generation.
    ///
    /// All checks happen under the registry write lock. A stale predecessor or
    /// occupied replacement key therefore leaves the live route unchanged.
    pub(crate) fn replace_sender_in_namespace(
        &self,
        namespace: &str,
        name: &str,
        current: (&PubKey, &InboxSender),
        replacement_pubkey: PubKey,
        replacement_sender: InboxSender,
    ) -> Result<(), InprocPublicationError> {
        let (current_pubkey, current_sender) = current;
        if replacement_pubkey.is_zero() {
            return Err(InprocPublicationError::ZeroPubkey);
        }

        let mut state = self.state.write();
        let Some(namespace_state) = state.namespaces.get_mut(namespace) else {
            return Err(InprocPublicationError::ExpectedGenerationNotCurrent);
        };
        let current_meta = namespace_state
            .peers
            .get(current_pubkey)
            .filter(|peer| peer.name == name && peer.sender.same_inbox(current_sender))
            .map(|peer| peer.meta.clone());
        if namespace_state.names.get(name) != Some(current_pubkey) {
            return Err(InprocPublicationError::ExpectedGenerationNotCurrent);
        }
        let Some(current_meta) = current_meta else {
            return Err(InprocPublicationError::ExpectedGenerationNotCurrent);
        };
        if replacement_pubkey != *current_pubkey
            && namespace_state.peers.contains_key(&replacement_pubkey)
        {
            return Err(InprocPublicationError::ReplacementPubkeyOccupied);
        }

        namespace_state.peers.remove(current_pubkey);
        namespace_state.peers.insert(
            replacement_pubkey,
            InprocPeer {
                name: name.to_string(),
                pubkey: replacement_pubkey,
                sender: replacement_sender,
                meta: current_meta,
            },
        );
        namespace_state
            .names
            .insert(name.to_string(), replacement_pubkey);
        Ok(())
    }

    /// Unregister an agent by pubkey.
    ///
    /// Returns true if the agent was found and removed.
    pub fn unregister(&self, pubkey: &PubKey) -> bool {
        self.unregister_in_namespace(DEFAULT_NAMESPACE, pubkey)
    }

    /// Unregister an agent by pubkey from an explicit namespace.
    pub fn unregister_in_namespace(&self, namespace: &str, pubkey: &PubKey) -> bool {
        let mut state = self.state.write();
        if let Some(namespace_state) = state.namespaces.get_mut(namespace)
            && let Some(peer) = namespace_state.peers.remove(pubkey)
        {
            namespace_state.names.remove(&peer.name);
            return true;
        }
        false
    }

    /// Remove a route only when its exact inbox generation is still current.
    /// A closed route of the same generation is forgotten too.
    pub(crate) fn unregister_sender_in_namespace(
        &self,
        namespace: &str,
        pubkey: &PubKey,
        sender: &InboxSender,
    ) -> bool {
        let mut state = self.state.write();
        let Some(namespace_state) = state.namespaces.get_mut(namespace) else {
            return false;
        };
        if namespace_state
            .closed_routes
            .get(pubkey)
            .is_some_and(|peer| peer.sender.same_inbox(sender))
        {
            namespace_state.closed_routes.remove(pubkey);
        }
        if !namespace_state
            .peers
            .get(pubkey)
            .is_some_and(|peer| peer.sender.same_inbox(sender))
        {
            return false;
        }
        let Some(peer) = namespace_state.peers.remove(pubkey) else {
            return false;
        };
        if namespace_state.names.get(&peer.name) == Some(pubkey) {
            namespace_state.names.remove(&peer.name);
        }
        true
    }

    /// Close an exact live route for peer ingress: it stops resolving and
    /// frees its name, and is kept aside for [`Self::reopen_sender_in_namespace`].
    pub(crate) fn close_sender_in_namespace(
        &self,
        namespace: &str,
        pubkey: &PubKey,
        sender: &InboxSender,
    ) -> bool {
        let mut state = self.state.write();
        let Some(namespace_state) = state.namespaces.get_mut(namespace) else {
            return false;
        };
        if !namespace_state
            .peers
            .get(pubkey)
            .is_some_and(|peer| peer.sender.same_inbox(sender))
        {
            return false;
        }
        let Some(peer) = namespace_state.peers.remove(pubkey) else {
            return false;
        };
        if namespace_state.names.get(&peer.name) == Some(pubkey) {
            namespace_state.names.remove(&peer.name);
        }
        namespace_state.closed_routes.insert(*pubkey, peer);
        true
    }

    /// Reopen an exact route [`Self::close_sender_in_namespace`] closed, under
    /// the ordinary claim rule: its name and key must still be unbound.
    pub(crate) fn reopen_sender_in_namespace(
        &self,
        namespace: &str,
        pubkey: &PubKey,
        sender: &InboxSender,
    ) -> bool {
        let mut state = self.state.write();
        let Some(namespace_state) = state.namespaces.get_mut(namespace) else {
            return false;
        };
        let reopenable = namespace_state
            .closed_routes
            .get(pubkey)
            .is_some_and(|peer| {
                peer.sender.same_inbox(sender) && !namespace_state.names.contains_key(&peer.name)
            })
            && !namespace_state.peers.contains_key(pubkey);
        if !reopenable {
            return false;
        }
        let Some(peer) = namespace_state.closed_routes.remove(pubkey) else {
            return false;
        };
        namespace_state.names.insert(peer.name.clone(), *pubkey);
        namespace_state.peers.insert(*pubkey, peer);
        true
    }

    /// Update metadata only when the exact inbox generation is still current.
    pub(crate) fn update_meta_for_sender_in_namespace(
        &self,
        namespace: &str,
        name: &str,
        pubkey: &PubKey,
        sender: &InboxSender,
        meta: PeerMeta,
    ) -> bool {
        let mut state = self.state.write();
        let Some(namespace_state) = state.namespaces.get_mut(namespace) else {
            return false;
        };
        let Some(peer) = namespace_state.peers.get_mut(pubkey) else {
            return false;
        };
        if peer.name != name
            || !peer.sender.same_inbox(sender)
            || namespace_state.names.get(name) != Some(pubkey)
        {
            return false;
        }
        peer.meta = meta;
        true
    }

    /// Look up an inproc peer by pubkey.
    pub fn get_by_pubkey(&self, pubkey: &PubKey) -> Option<InboxSender> {
        self.get_by_pubkey_in_namespace(DEFAULT_NAMESPACE, pubkey)
    }

    /// Look up an inproc peer by pubkey in an explicit namespace.
    pub fn get_by_pubkey_in_namespace(
        &self,
        namespace: &str,
        pubkey: &PubKey,
    ) -> Option<InboxSender> {
        if pubkey.is_zero() {
            return None;
        }
        self.state
            .read()
            .namespace(namespace)?
            .peers
            .get(pubkey)
            .map(|p| p.sender.clone())
    }

    /// Look up an inproc peer by pubkey across all namespaces.
    ///
    /// Cross-namespace delivery has no typed target namespace. If the same
    /// canonical identity is live in more than one namespace, fail closed
    /// rather than choosing whichever namespace the map happens to yield first.
    pub(crate) fn get_by_pubkey_any_namespace(&self, pubkey: &PubKey) -> Option<InboxSender> {
        if pubkey.is_zero() {
            return None;
        }
        let state = self.state.read();
        let mut found = None;
        for namespace_state in state.namespaces.values() {
            if let Some(peer) = namespace_state.peers.get(pubkey) {
                if found.is_some() {
                    return None;
                }
                found = Some(peer.sender.clone());
            }
        }
        found
    }

    /// Look up an inproc peer name by public key.
    pub fn get_name_by_pubkey(&self, pubkey: &PubKey) -> Option<String> {
        self.get_name_by_pubkey_in_namespace(DEFAULT_NAMESPACE, pubkey)
    }

    /// Look up an inproc peer name by public key in an explicit namespace.
    pub fn get_name_by_pubkey_in_namespace(
        &self,
        namespace: &str,
        pubkey: &PubKey,
    ) -> Option<String> {
        if pubkey.is_zero() {
            return None;
        }
        self.state
            .read()
            .namespace(namespace)?
            .peers
            .get(pubkey)
            .map(|peer| peer.name.clone())
    }

    /// Check if a peer is registered.
    pub fn contains(&self, pubkey: &PubKey) -> bool {
        self.state
            .read()
            .namespace(DEFAULT_NAMESPACE)
            .is_some_and(|ns| ns.peers.contains_key(pubkey))
    }

    /// Check if a peer name is registered.
    pub fn contains_name(&self, name: &str) -> bool {
        self.state
            .read()
            .namespace(DEFAULT_NAMESPACE)
            .is_some_and(|ns| ns.names.contains_key(name))
    }

    /// Get the number of registered peers.
    pub fn len(&self) -> usize {
        self.state.read().namespace_len(DEFAULT_NAMESPACE)
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.state.read().namespace_is_empty(DEFAULT_NAMESPACE)
    }

    /// Clear all registrations (primarily for testing).
    pub fn clear(&self) {
        self.state.write().namespaces.clear();
    }

    /// Publish a seated member's wake route under `pubkey` in `namespace`.
    ///
    /// Called only from the owner's machine effects (a seating transition).
    /// The route occupies no name and never delivers: a live route under the
    /// same key always wins. Re-publishing replaces the key's route (the
    /// owner's own projection rebuild). A zero key is refused.
    pub fn publish_wake_route(
        &self,
        namespace: &str,
        pubkey: PubKey,
        member: impl Into<String>,
        waker: Weak<dyn InprocPeerWaker>,
    ) -> bool {
        if pubkey.is_zero() {
            return false;
        }
        let route = WakeRoute {
            member: member.into(),
            waker,
        };
        self.state
            .write()
            .namespace_mut(namespace)
            .wake_routes
            .insert(pubkey, route);
        true
    }

    /// Retract a wake route, only while it is still `waker`'s: an owner can
    /// never retract a route another owner published for the same key.
    /// Called only from the owner's machine effects (an unseating
    /// transition or an endpoint migration).
    pub fn retract_wake_route(
        &self,
        namespace: &str,
        pubkey: &PubKey,
        waker: &Weak<dyn InprocPeerWaker>,
    ) -> bool {
        let mut state = self.state.write();
        let Some(namespace_state) = state.namespaces.get_mut(namespace) else {
            return false;
        };
        if !namespace_state
            .wake_routes
            .get(pubkey)
            .is_some_and(|route| Weak::ptr_eq(&route.waker, waker))
        {
            return false;
        }
        namespace_state.wake_routes.remove(pubkey).is_some()
    }

    /// Whether a wake route is published for `pubkey` in `namespace`.
    pub fn has_wake_route_in_namespace(&self, namespace: &str, pubkey: &PubKey) -> bool {
        self.state
            .read()
            .namespace(namespace)
            .is_some_and(|ns| ns.wake_routes.contains_key(pubkey))
    }

    fn live_route(&self, scope: RouteScope<'_>, pubkey: &PubKey) -> Option<InboxSender> {
        match scope {
            RouteScope::Namespace(namespace) => self.get_by_pubkey_in_namespace(namespace, pubkey),
            RouteScope::AnyNamespace => self.get_by_pubkey_any_namespace(pubkey),
        }
    }

    /// The wake route for `pubkey`, as a route whose owner is alive.
    ///
    /// `Err(())` when the key has a route whose owner is gone (its mob actor
    /// exited without retracting it): the member exists, but nothing in this
    /// process can wake it. In any namespace, a key routed in more than one
    /// namespace fails closed as no route, exactly as live routes do.
    fn wake_route(
        &self,
        scope: RouteScope<'_>,
        pubkey: &PubKey,
    ) -> Result<Option<ResolvedWakeRoute>, ()> {
        if pubkey.is_zero() {
            return Ok(None);
        }
        let route = {
            let state = self.state.read();
            match scope {
                RouteScope::Namespace(namespace) => state
                    .namespace(namespace)
                    .and_then(|ns| ns.wake_routes.get(pubkey))
                    .cloned(),
                RouteScope::AnyNamespace => {
                    let mut found = None;
                    for namespace_state in state.namespaces.values() {
                        if let Some(route) = namespace_state.wake_routes.get(pubkey) {
                            if found.is_some() {
                                return Ok(None);
                            }
                            found = Some(route.clone());
                        }
                    }
                    found
                }
            }
        };
        let Some(route) = route else {
            return Ok(None);
        };
        match route.waker.upgrade() {
            Some(waker) => Ok(Some(ResolvedWakeRoute {
                member: route.member,
                waker,
            })),
            None => Err(()),
        }
    }

    /// Whether the sender is itself a seated member being woken, as its own
    /// wake route's owner published it. No route, or a gone owner, is "not
    /// waking": such a sender is in no build this registry can see.
    fn sender_is_waking(&self, scope: RouteScope<'_>, sender: &PubKey) -> bool {
        match self.wake_route(scope, sender) {
            Ok(Some(route)) => route.waker.member_is_waking(&route.member),
            Ok(None) | Err(()) => false,
        }
    }

    /// Backpressured pubkey-keyed delivery across all namespaces.
    ///
    /// Runtime-originated peer sends should await receiver capacity instead of
    /// turning a transient full inbox into semantic message loss. `budget`
    /// caps the whole send: how long the sender's own liveness may depend on
    /// the receiver's drain, and on a wake the receiver needs first; see
    /// [`Self::send_resolving_wake`].
    pub(crate) async fn send_to_pubkey_any_namespace_with_id_wait(
        &self,
        from_keypair: &Keypair,
        to_pubkey: &PubKey,
        envelope_id: Uuid,
        kind: MessageKind,
        sign_envelope: bool,
        budget: std::time::Duration,
    ) -> Result<InprocDelivery, InprocSendError> {
        self.send_resolving_wake(
            RouteScope::AnyNamespace,
            from_keypair,
            to_pubkey,
            envelope_id,
            kind,
            sign_envelope,
            budget,
        )
        .await
    }

    /// Namespace-scoped variant of
    /// [`Self::send_to_pubkey_any_namespace_with_id_wait`]: the destination is
    /// resolved *inside* `namespace` only, and that resolution is the delivery
    /// target.
    ///
    /// This is the single-resolution send for namespace-isolated routers. The
    /// namespace is the delivery authority, so the destination must not be
    /// re-derived from the global registry between an isolation check and the
    /// inbox handoff - a second any-namespace lookup would open a window where
    /// the peer re-registers elsewhere and delivery crosses the namespace
    /// boundary. A wake route is looked up in the same namespace, so a member
    /// is never woken from another namespace.
    // One argument over the lint's threshold, and the extra one is the
    // `namespace` delivery authority that distinguishes this send from its
    // any-namespace sibling. Bundling the envelope arguments into a params
    // struct would only move the same values behind a name that adds no
    // meaning, so the sibling signatures are kept identical instead.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn send_to_pubkey_in_namespace_with_id_wait(
        &self,
        namespace: &str,
        from_keypair: &Keypair,
        to_pubkey: &PubKey,
        envelope_id: Uuid,
        kind: MessageKind,
        sign_envelope: bool,
        budget: std::time::Duration,
    ) -> Result<InprocDelivery, InprocSendError> {
        self.send_resolving_wake(
            RouteScope::Namespace(namespace),
            from_keypair,
            to_pubkey,
            envelope_id,
            kind,
            sign_envelope,
            budget,
        )
        .await
    }

    /// Resolve the destination and hand the envelope off once.
    ///
    /// Resolution order:
    /// - a live route is the delivery target, exactly as before;
    /// - no live route, but a wake route: the envelope stays in this future
    ///   while the route's owner wakes the member; once it answers ready, the
    ///   live route is resolved again and the envelope handed off once;
    /// - neither: `PeerNotFound` (not a member, or another namespace).
    ///
    /// A live route that answers closed for a key with a wake route is a
    /// stale route of a seated member: it takes the wake path too. After a
    /// wake answered ready, a second miss or closed route wakes once more;
    /// after that the send is refused `materialization_lost`. Every refusal
    /// is "not delivered": nothing was handed off.
    ///
    /// `budget` bounds the whole send. Expiring while the wake is pending is
    /// `wake_timed_out` (not delivered, safe to retry); expiring after the
    /// hand-off is the ordinary unconfirmed delivery (ambiguous). The
    /// registry lock is never held across an await.
    ///
    /// A sender that is itself being woken (the send comes from inside its
    /// own build) never waits on another member's wake or on a waking
    /// member's early live route: the recipient's owner is asked first, with
    /// `sender_waking` set, and answers from its machine. Two builds that
    /// message each other therefore cannot wait on each other.
    // Same argument list as the two public owners above plus the scope.
    #[allow(clippy::too_many_arguments)]
    async fn send_resolving_wake(
        &self,
        scope: RouteScope<'_>,
        from_keypair: &Keypair,
        to_pubkey: &PubKey,
        envelope_id: Uuid,
        kind: MessageKind,
        sign_envelope: bool,
        budget: std::time::Duration,
    ) -> Result<InprocDelivery, InprocSendError> {
        let started = Instant::now();
        let remaining = || budget.saturating_sub(started.elapsed());
        let input_class = PeerWakeInputClass::of(&kind);
        let sender = from_keypair.public_key();
        let peer_not_found = || InprocSendError::PeerNotFound(to_pubkey.to_peer_id().to_string());
        let mut wakes = 0u8;
        loop {
            let live = self.live_route(scope, to_pubkey);
            let wake = match self.wake_route(scope, to_pubkey) {
                Ok(route) => route,
                // A seated member whose owner is gone: nothing in this
                // process can wake it. Only a live route can still serve.
                Err(()) => match live {
                    Some(inbox) => {
                        return Self::deliver_within_budget(
                            from_keypair,
                            *to_pubkey,
                            inbox,
                            envelope_id,
                            kind,
                            sign_envelope,
                            remaining(),
                            budget,
                        )
                        .await;
                    }
                    None => {
                        return Err(InprocSendError::WakeRefused(
                            PeerWakeRefusal::MobUnavailable,
                        ));
                    }
                },
            };
            let route = match (live, wake) {
                (None, None) => return Err(peer_not_found()),
                // Not a seated member of any owner: delivery as before.
                (Some(inbox), None) => {
                    return Self::deliver_within_budget(
                        from_keypair,
                        *to_pubkey,
                        inbox,
                        envelope_id,
                        kind,
                        sign_envelope,
                        remaining(),
                        budget,
                    )
                    .await;
                }
                (Some(inbox), Some(route)) => {
                    // A member whose wake is in flight already has its live
                    // route (its inbox is registered during its build) but
                    // is not ready yet: its trust and drain settle before its
                    // owner answers ready. The send joins that wake through
                    // the owner first, and hands off only after it answered,
                    // so it never meets a half-built inbox. A sender that is
                    // itself being woken is refused there instead of waiting.
                    if route.waker.member_is_waking(&route.member) {
                        let request = PeerWakeRequest {
                            member: route.member.clone(),
                            recipient: *to_pubkey,
                            sender,
                            envelope_id,
                            observed_missing: false,
                            input_class,
                            sender_waking: self.sender_is_waking(scope, &sender),
                        };
                        match Self::await_wake(&route, request, remaining()).await? {
                            PeerWakeAnswer::Ready => {}
                            PeerWakeAnswer::ControlNoticeConsumed => {
                                return Ok(Self::consumed_by_owner(envelope_id));
                            }
                        }
                    }
                    match Self::deliver_within_budget(
                        from_keypair,
                        *to_pubkey,
                        inbox,
                        envelope_id,
                        kind.clone(),
                        sign_envelope,
                        remaining(),
                        budget,
                    )
                    .await
                    {
                        // A closed live route of a seated member is stale.
                        // The closed inbox refused the envelope, or admitted
                        // it and lost the receipt with its queue; either way
                        // the wake path hands the same envelope, with the same
                        // id, to the member's next inbox once, and the
                        // runtime's peer-ingress idempotency (one input per
                        // peer and envelope id) admits it at most once.
                        Err(InprocSendError::InboxClosed) => route,
                        other => return other,
                    }
                }
                (None, Some(route)) => route,
            };
            // The wake path. The first wake answers the miss; one more is
            // allowed after a miss that follows a ready answer.
            if wakes >= 2 {
                return Err(InprocSendError::WakeRefused(
                    PeerWakeRefusal::MaterializationLost,
                ));
            }
            wakes += 1;
            let request = PeerWakeRequest {
                member: route.member.clone(),
                recipient: *to_pubkey,
                sender,
                envelope_id,
                observed_missing: true,
                input_class,
                sender_waking: self.sender_is_waking(scope, &sender),
            };
            if let PeerWakeAnswer::ControlNoticeConsumed =
                Self::await_wake(&route, request, remaining()).await?
            {
                return Ok(Self::consumed_by_owner(envelope_id));
            }
            if remaining().is_zero() {
                // Ready, but the sender's budget is spent: hand nothing off
                // rather than hand off with no time to confirm it.
                return Err(InprocSendError::WakeRefused(PeerWakeRefusal::WakeTimedOut));
            }
        }
    }

    /// Hand the envelope off with what is left of the sender's budget, and
    /// report an unconfirmed delivery against the whole-send budget: the
    /// caller asked for one bound, and that is the bound it learns about.
    #[allow(clippy::too_many_arguments)]
    async fn deliver_within_budget(
        from_keypair: &Keypair,
        to_pubkey: PubKey,
        sender: InboxSender,
        envelope_id: Uuid,
        kind: MessageKind,
        sign_envelope: bool,
        remaining: std::time::Duration,
        budget: std::time::Duration,
    ) -> Result<InprocDelivery, InprocSendError> {
        Self::deliver_to_sender_wait(
            from_keypair,
            to_pubkey,
            sender,
            envelope_id,
            kind,
            sign_envelope,
            remaining,
        )
        .await
        .map(|delivery| match delivery.outcome {
            InprocDeliveryOutcome::Unconfirmed { .. } => InprocDelivery {
                envelope_id: delivery.envelope_id,
                outcome: InprocDeliveryOutcome::Unconfirmed { park_bound: budget },
            },
            InprocDeliveryOutcome::Resolved(_) => delivery,
        })
    }

    /// Ask a wake route's owner and wait for its answer, bounded by what is
    /// left of the sender's budget. The wake itself is the owner's: it goes
    /// on when this future gives up or is dropped.
    async fn await_wake(
        route: &ResolvedWakeRoute,
        request: PeerWakeRequest,
        remaining: std::time::Duration,
    ) -> Result<PeerWakeAnswer, InprocSendError> {
        let recipient = request.recipient;
        match tokio::time::timeout(remaining, route.waker.wake(request)).await {
            Ok(Ok(answer)) => Ok(answer),
            // The route was stale: its owner no longer holds the key as a
            // member (the retraction has not been realized yet).
            Ok(Err(PeerWakeDenial::NotAMember)) => Err(InprocSendError::PeerNotFound(
                recipient.to_peer_id().to_string(),
            )),
            Ok(Err(PeerWakeDenial::Refused(refusal))) => Err(InprocSendError::WakeRefused(refusal)),
            Err(_) => Err(InprocSendError::WakeRefused(PeerWakeRefusal::WakeTimedOut)),
        }
    }

    /// The owner took a silent control notice for its member instead of
    /// waking it, exactly as a live member's drain consumes one without a
    /// turn: the receiver side's control consumer owns the envelope.
    fn consumed_by_owner(envelope_id: Uuid) -> InprocDelivery {
        InprocDelivery {
            envelope_id,
            outcome: InprocDeliveryOutcome::Resolved(IngressDeliveryOutcome::VolatileHandedOff),
        }
    }

    /// Hand one envelope to a resolved in-process receiver.
    ///
    /// Non-`Response` kinds wait for the receiver's ingress authority to commit
    /// the exact claim, which makes the caller's liveness depend on the
    /// RECEIVER's drain being alive. `park_bound` caps that dependency: on
    /// expiry the caller is told delivery is unconfirmed rather than being
    /// parked forever behind a dead or stalled drain.
    ///
    /// `MessageKind::Response` deliberately keeps capacity-only semantics: it
    /// returns as soon as the item is admitted to the queue and never waits on
    /// the drain, so it has no receipt park to bound.
    async fn deliver_to_sender_wait(
        from_keypair: &Keypair,
        to_pubkey: PubKey,
        sender: InboxSender,
        envelope_id: Uuid,
        kind: MessageKind,
        sign_envelope: bool,
        park_bound: std::time::Duration,
    ) -> Result<InprocDelivery, InprocSendError> {
        let response_uses_legacy_queue_semantics = matches!(&kind, MessageKind::Response { .. });
        let mut envelope = Envelope {
            id: envelope_id,
            from: from_keypair.public_key(),
            to: to_pubkey,
            kind,
            sig: Signature::new([0u8; 64]),
        };
        if sign_envelope {
            envelope.sign(from_keypair);
        }

        let envelope_id = envelope.id;
        if response_uses_legacy_queue_semantics {
            return match sender
                .send_wait_in_process(InboxItem::External { envelope })
                .await
            {
                AdmissionOutcome::Admitted => Ok(InprocDelivery {
                    envelope_id,
                    outcome: InprocDeliveryOutcome::Resolved(IngressDeliveryOutcome::Queued),
                }),
                AdmissionOutcome::Dropped {
                    reason: DropReason::SessionClosed,
                } => Err(InprocSendError::InboxClosed),
                AdmissionOutcome::Dropped {
                    reason: DropReason::InboxFull,
                } => Err(InprocSendError::InboxFull),
                AdmissionOutcome::Dropped { reason } => {
                    Err(InprocSendError::IngressDropped(reason))
                }
            };
        }
        match sender
            .send_wait_for_delivery(InboxItem::External { envelope }, park_bound)
            .await
        {
            BoundedIngressDelivery::Resolved(IngressDeliveryOutcome::Queued) => unreachable!(
                "only the immediate legacy Response path produces queued inproc delivery"
            ),
            BoundedIngressDelivery::Resolved(
                outcome @ (IngressDeliveryOutcome::DurablyResolved(_)
                | IngressDeliveryOutcome::VolatileHandedOff),
            ) => Ok(InprocDelivery {
                envelope_id,
                outcome: InprocDeliveryOutcome::Resolved(outcome),
            }),
            BoundedIngressDelivery::Resolved(IngressDeliveryOutcome::Dropped {
                reason: DropReason::SessionClosed,
            }) => Err(InprocSendError::InboxClosed),
            BoundedIngressDelivery::Resolved(IngressDeliveryOutcome::Dropped {
                reason: DropReason::InboxFull,
            }) => Err(InprocSendError::InboxFull),
            BoundedIngressDelivery::Resolved(IngressDeliveryOutcome::Dropped { reason }) => {
                Err(InprocSendError::IngressDropped(reason))
            }
            // The receiver's queue never opened inside the bound: nothing was
            // admitted, so this is an admission refusal for a typed capacity
            // reason - NOT an ambiguous delivery, and not "peer unreachable".
            // The peer was present and its queue answered; it was full.
            // `IngressDropped` is the arm that carries that typed reason
            // through to `peer_admission_dropped: inbox_full`; the bare
            // `InboxFull` arm above collapses into `PeerOffline` on the way
            // out, which would report an unreachable peer for a reachable one.
            BoundedIngressDelivery::AdmissionParkTimedOut { park_bound } => {
                tracing::warn!(
                    %envelope_id,
                    park_bound_secs = park_bound.as_secs_f64(),
                    "inproc send gave up waiting for receiver inbox capacity"
                );
                Err(InprocSendError::IngressDropped(DropReason::InboxFull))
            }
            BoundedIngressDelivery::AdmittedUnconfirmed { park_bound } => Ok(InprocDelivery {
                envelope_id,
                outcome: InprocDeliveryOutcome::Unconfirmed { park_bound },
            }),
        }
    }

    /// List all registered peer names in an explicit namespace.
    pub fn peer_names_in_namespace(&self, namespace: &str) -> Vec<String> {
        self.state
            .read()
            .namespace(namespace)
            .map_or_else(Vec::new, |ns| ns.names.keys().cloned().collect())
    }

    /// List all registered peers.
    pub fn peers(&self) -> Vec<InprocPeerInfo> {
        self.peers_in_namespace(DEFAULT_NAMESPACE)
    }

    /// List all registered peers in an explicit namespace.
    pub fn peers_in_namespace(&self, namespace: &str) -> Vec<InprocPeerInfo> {
        self.state
            .read()
            .namespace(namespace)
            .map_or_else(Vec::new, |ns| {
                ns.peers
                    .values()
                    .map(|peer| InprocPeerInfo {
                        name: peer.name.clone(),
                        pubkey: peer.pubkey,
                        meta: peer.meta.clone(),
                    })
                    .collect()
            })
    }
}

impl Default for InprocRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors that can occur during inproc send operations.
#[derive(Debug, thiserror::Error)]
pub enum InprocSendError {
    /// No live route and no wake route: not a member (or another namespace).
    #[error("Inproc peer not found: {0}")]
    PeerNotFound(String),
    #[error("Peer inbox has been closed")]
    InboxClosed,
    #[error("Peer inbox is full")]
    InboxFull,
    #[error("Peer inbox dropped ingress: {0:?}")]
    IngressDropped(crate::inbox::DropReason),
    /// The recipient is a seated member, and the wake the send needed was
    /// refused or did not finish. Nothing was handed off.
    #[error("Peer wake refused: {0}")]
    WakeRefused(PeerWakeRefusal),
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::classify::test_support;
    use crate::inbox::Inbox;
    use crate::trust::TrustStore;
    use parking_lot::RwLock;
    use std::sync::Arc;
    use std::time::Duration;

    /// Park bound for tests whose receiver DOES drain: generous enough that a
    /// healthy drain always wins the race, so these tests keep asserting
    /// delivery semantics rather than accidentally asserting the deadline.
    const TEST_PARK_BOUND: Duration = Duration::from_secs(30);

    #[cfg(feature = "workspace-tests")]
    fn classified_inbox() -> (Inbox, crate::InboxSender) {
        classified_inbox_with_auth(false)
    }

    #[cfg(feature = "workspace-tests")]
    fn classified_inbox_with_auth(require_peer_auth: bool) -> (Inbox, crate::InboxSender) {
        Inbox::new_classified(test_support::classification_context(
            TrustStore::new(),
            require_peer_auth,
        ))
    }

    #[cfg(feature = "workspace-tests")]
    fn classified_inbox_with_runtime() -> (
        Inbox,
        crate::InboxSender,
        Arc<meerkat_runtime::TestPeerIngressRuntimeFinalizer>,
    ) {
        let (peer_comms_handle, finalizer) =
            meerkat_runtime::test_peer_comms_handle_and_runtime_finalizer();
        let context = Arc::new(crate::classify::IngressClassificationContext {
            require_peer_auth: false,
            trusted_peers: Arc::new(RwLock::new(TrustStore::new())),
            peer_comms_handle: Arc::new(RwLock::new(Some(peer_comms_handle))),
            inproc_namespace: None,
            durable_runtime_consumer: true,
        });
        let (inbox, sender) = Inbox::new_classified(context);
        (inbox, sender, Arc::new(finalizer))
    }

    #[cfg(feature = "workspace-tests")]
    fn spawn_runtime_finalization(
        inbox: Arc<Inbox>,
        finalizer: Arc<meerkat_runtime::TestPeerIngressRuntimeFinalizer>,
    ) -> tokio::task::JoinHandle<meerkat_core::interaction::PeerInputCandidate> {
        tokio::spawn(async move {
            let claimed = loop {
                if let Some(claimed) = inbox.try_claim_one_classified() {
                    break claimed;
                }
                tokio::task::yield_now().await;
            };
            let claim = crate::runtime::comms_runtime::test_peer_ingress_queue_claim(claimed);
            finalizer
                .finalize(claim)
                .await
                .expect("real test MeerkatMachine must durably finalize the exact inproc claim")
        })
    }

    fn spawn_volatile_handoff(
        inbox: Arc<Inbox>,
    ) -> tokio::task::JoinHandle<crate::inbox::ClassifiedInboxEntry> {
        tokio::spawn(async move {
            let claimed = loop {
                if let Some(claimed) = inbox.try_claim_one_classified() {
                    break claimed;
                }
                tokio::task::yield_now().await;
            };
            let entry = claimed.entry.clone();
            let claim = meerkat_core::interaction::PeerIngressQueueClaim::from_comms_queue(
                claimed.claim_id,
                claimed.entry.raw_item_id,
                claimed.entry.class,
                claimed.entry.delivery_contract,
                None,
                claimed.lease,
            );
            claim
                .__handoff_volatile()
                .expect("volatile inproc handoff must remove the exact claimed head");
            entry
        })
    }

    fn make_keypair() -> Keypair {
        Keypair::generate()
    }

    #[test]
    fn test_registry_new() {
        let registry = InprocRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn test_registry_register_and_lookup() {
        let registry = InprocRegistry::new();
        let keypair = make_keypair();
        let pubkey = keypair.public_key();
        let (_, sender) = classified_inbox();

        registry.register("test-agent", pubkey, sender);

        assert!(!registry.is_empty());
        assert_eq!(registry.len(), 1);
        assert!(registry.contains(&pubkey));
        assert!(registry.contains_name("test-agent"));

        // Name is display metadata; routing lookups are pubkey-keyed.
        assert_eq!(
            registry.get_name_by_pubkey(&pubkey).as_deref(),
            Some("test-agent")
        );
        assert!(registry.get_by_pubkey(&pubkey).is_some());
    }

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn test_registry_rejects_zero_pubkey_registration() {
        let registry = InprocRegistry::new();
        let (_, sender) = classified_inbox();
        let zero_pubkey = PubKey::new([0u8; 32]);

        registry.register("zero-agent", zero_pubkey, sender);

        assert!(registry.is_empty());
        assert!(!registry.contains_name("zero-agent"));
        assert!(registry.get_by_pubkey(&zero_pubkey).is_none());
    }

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn test_registry_zero_pubkey_registration_does_not_shadow_valid_name() {
        let registry = InprocRegistry::new();
        let valid_keypair = make_keypair();
        let valid_pubkey = valid_keypair.public_key();
        let (_, valid_sender) = classified_inbox();
        let (_, zero_sender) = classified_inbox();
        let zero_pubkey = PubKey::new([0u8; 32]);

        registry.register("stable-agent", valid_pubkey, valid_sender);
        registry.register("stable-agent", zero_pubkey, zero_sender);

        assert_eq!(registry.len(), 1);
        assert!(registry.contains(&valid_pubkey));
        assert!(registry.contains_name("stable-agent"));
        assert!(registry.get_by_pubkey(&valid_pubkey).is_some());
        assert!(registry.get_by_pubkey(&zero_pubkey).is_none());

        assert_eq!(
            registry.get_name_by_pubkey(&valid_pubkey).as_deref(),
            Some("stable-agent"),
            "valid name mapping should remain"
        );
    }

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn test_registry_unregister() {
        let registry = InprocRegistry::new();
        let keypair = make_keypair();
        let pubkey = keypair.public_key();
        let (_, sender) = classified_inbox();

        registry.register("test-agent", pubkey, sender);
        assert!(registry.contains(&pubkey));

        let removed = registry.unregister(&pubkey);
        assert!(removed);
        assert!(!registry.contains(&pubkey));
        assert!(!registry.contains_name("test-agent"));
        assert!(registry.is_empty());

        // Unregister non-existent returns false
        let removed_again = registry.unregister(&pubkey);
        assert!(!removed_again);
    }

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn test_registry_replace_on_same_pubkey() {
        let registry = InprocRegistry::new();
        let keypair = make_keypair();
        let pubkey = keypair.public_key();
        let (_, sender1) = classified_inbox();
        let (_, sender2) = classified_inbox();

        // Register with first name
        registry.register("agent-v1", pubkey, sender1);
        assert!(registry.contains_name("agent-v1"));

        // Re-register same pubkey with different name
        registry.register("agent-v2", pubkey, sender2);

        // Old name should be removed, new name should exist
        assert!(!registry.contains_name("agent-v1"));
        assert!(registry.contains_name("agent-v2"));
        assert_eq!(registry.len(), 1);
    }

    /// There is no takeover opt-in left: a name held by another live key is
    /// refused and the incumbent's maps are untouched.
    #[cfg(feature = "workspace-tests")]
    #[test]
    fn test_registry_refuses_same_name_different_pubkey() {
        let registry = InprocRegistry::new();
        let keypair1 = make_keypair();
        let pubkey1 = keypair1.public_key();
        let keypair2 = make_keypair();
        let pubkey2 = keypair2.public_key();
        let (_, sender1) = classified_inbox();
        let (_, sender2) = classified_inbox();

        // Register first agent
        registry.register("my-agent", pubkey1, sender1);
        assert!(registry.contains(&pubkey1));
        assert!(registry.contains_name("my-agent"));
        assert_eq!(registry.len(), 1);

        // Re-register same name with different pubkey
        assert_eq!(
            registry.register("my-agent", pubkey2, sender2),
            RegistrationOutcome::Rejected {
                reason: RegistrationRejection::NameOccupied {
                    holder_pubkey: pubkey1
                }
            }
        );

        // The incumbent keeps the route; the newcomer was never installed.
        assert!(registry.contains(&pubkey1), "incumbent must keep its route");
        assert!(!registry.contains(&pubkey2));
        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry.get_name_by_pubkey(&pubkey1).as_deref(),
            Some("my-agent")
        );
    }

    fn probe_message(body: &str) -> MessageKind {
        MessageKind::Message {
            objective_id: None,
            content_taint: None,
            blocks: None,
            body: body.to_string(),
            handling_mode: None,
            in_reply_to: None,
        }
    }

    /// One participant name registered from two different identities in two
    /// different namespaces is two peers that cannot see each other, so both
    /// routes stay live and both deliver. Namespaces are the isolation seam;
    /// nothing about a same-name neighbour in another namespace may unbind a
    /// live route.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn same_name_in_distinct_namespaces_keeps_both_routes_and_both_deliver() {
        let registry = InprocRegistry::new();
        let name = "mob.shared/lead/lead-1";
        let first_keypair = make_keypair();
        let first_pubkey = first_keypair.public_key();
        let second_keypair = make_keypair();
        let second_pubkey = second_keypair.public_key();
        let (first_inbox, first_sender, first_finalizer) = classified_inbox_with_runtime();
        let (second_inbox, second_sender, second_finalizer) = classified_inbox_with_runtime();

        assert_eq!(
            registry.register_with_meta_in_namespace(
                "realm-one",
                name,
                first_pubkey,
                first_sender,
                PeerMeta::default(),
            ),
            RegistrationOutcome::Registered
        );
        assert_eq!(
            registry.register_with_meta_in_namespace(
                "realm-two",
                name,
                second_pubkey,
                second_sender,
                PeerMeta::default(),
            ),
            RegistrationOutcome::Registered,
            "a same-name peer in another namespace must not displace anything"
        );

        assert!(
            registry
                .get_by_pubkey_in_namespace("realm-one", &first_pubkey)
                .is_some()
        );
        assert!(
            registry
                .get_by_pubkey_in_namespace("realm-two", &second_pubkey)
                .is_some()
        );

        let first_finalize = spawn_runtime_finalization(Arc::new(first_inbox), first_finalizer);
        let second_finalize = spawn_runtime_finalization(Arc::new(second_inbox), second_finalizer);
        let peer_keypair = make_keypair();
        for (namespace, target, body) in [
            ("realm-one", first_pubkey, "to realm one"),
            ("realm-two", second_pubkey, "to realm two"),
        ] {
            let delivery = registry
                .send_to_pubkey_in_namespace_with_id_wait(
                    namespace,
                    &peer_keypair,
                    &target,
                    Uuid::new_v4(),
                    probe_message(body),
                    true,
                    TEST_PARK_BOUND,
                )
                .await
                .expect("both same-name peers must remain routable");
            assert!(matches!(
                delivery.resolved_outcome(),
                IngressDeliveryOutcome::DurablyResolved(
                    meerkat_core::PeerIngressTerminalOutcomeKind::Accepted
                )
            ));
        }

        let first_candidate = first_finalize.await.expect("first finalizer completes");
        let second_candidate = second_finalize.await.expect("second finalizer completes");
        for (candidate, expected) in [
            (first_candidate, "to realm one"),
            (second_candidate, "to realm two"),
        ] {
            match candidate.interaction.content {
                meerkat_core::InteractionContent::Message {
                    body,
                    blocks: None,
                    in_reply_to: _,
                } => {
                    assert_eq!(body, expected);
                }
                other => panic!("expected Message interaction, got {other:?}"),
            }
        }
    }

    /// The carried 0.8.22 defect: one namespace, one name, two identities. The
    /// newcomer used to silently unbind the incumbent's only route and the
    /// incumbent only found out by no longer receiving peer messages. The
    /// registration is now refused before any mutation, and there is no
    /// takeover opt-in for any registrant to reach for.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn same_name_in_one_namespace_refuses_takeover_and_incumbent_still_delivers() {
        let registry = InprocRegistry::new();
        let namespace = "mob.shared";
        let name = "mob.shared/lead/lead-1";
        let incumbent_keypair = make_keypair();
        let incumbent_pubkey = incumbent_keypair.public_key();
        let newcomer_keypair = make_keypair();
        let newcomer_pubkey = newcomer_keypair.public_key();
        let (incumbent_inbox, incumbent_sender, incumbent_finalizer) =
            classified_inbox_with_runtime();
        let (_newcomer_inbox, newcomer_sender) = classified_inbox();

        assert_eq!(
            registry.register_with_meta_in_namespace(
                namespace,
                name,
                incumbent_pubkey,
                incumbent_sender,
                PeerMeta::default(),
            ),
            RegistrationOutcome::Registered
        );

        assert_eq!(
            registry.register_with_meta_in_namespace(
                namespace,
                name,
                newcomer_pubkey,
                newcomer_sender,
                PeerMeta::default(),
            ),
            RegistrationOutcome::Rejected {
                reason: RegistrationRejection::NameOccupied {
                    holder_pubkey: incumbent_pubkey
                }
            },
            "a foreign identity must not silently take over a live name"
        );
        assert_eq!(registry.peers_in_namespace(namespace).len(), 1);
        assert!(
            registry
                .get_by_pubkey_in_namespace(namespace, &newcomer_pubkey)
                .is_none(),
            "a refused registration must not install the newcomer route"
        );
        assert_eq!(
            registry.get_name_by_pubkey_in_namespace(namespace, &incumbent_pubkey),
            Some(name.to_string()),
            "a refused registration must leave the incumbent name binding intact"
        );

        // The incumbent is still a live route, not just a surviving map entry.
        let finalize = spawn_runtime_finalization(Arc::new(incumbent_inbox), incumbent_finalizer);
        let peer_keypair = make_keypair();
        let delivery = registry
            .send_to_pubkey_in_namespace_with_id_wait(
                namespace,
                &peer_keypair,
                &incumbent_pubkey,
                Uuid::new_v4(),
                probe_message("incumbent keeps routing"),
                true,
                TEST_PARK_BOUND,
            )
            .await
            .expect("the refused registration must not have unbound the incumbent");
        assert!(matches!(
            delivery.resolved_outcome(),
            IngressDeliveryOutcome::DurablyResolved(
                meerkat_core::PeerIngressTerminalOutcomeKind::Accepted
            )
        ));
        let candidate = finalize.await.expect("incumbent finalizer completes");
        match candidate.interaction.content {
            meerkat_core::InteractionContent::Message {
                body,
                blocks: None,
                in_reply_to: _,
            } => {
                assert_eq!(body, "incumbent keeps routing");
            }
            other => panic!("expected Message interaction, got {other:?}"),
        }

        // The refusal is keyed on the NAME being live, not on identity. The
        // incumbent's own key gets the identical refusal, carrying its own key
        // back as the holder (see
        // `same_key_registration_over_a_live_route_is_refused_like_any_other`).
        let (_rebuild_inbox, rebuild_sender) = classified_inbox();
        assert_eq!(
            registry.register_with_meta_in_namespace(
                namespace,
                name,
                incumbent_pubkey,
                rebuild_sender,
                PeerMeta::default(),
            ),
            RegistrationOutcome::Rejected {
                reason: RegistrationRejection::NameOccupied {
                    holder_pubkey: incumbent_pubkey
                }
            },
            "the holder of a live name does not get a second claim rule for rebuilding itself"
        );
    }

    /// The behaviour the one-claim rule buys, and the exact inversion of what
    /// this test asserted through 0.8.23. A same-key second registration used to
    /// be admitted, which moved delivery to the NEWEST generation's inbox and
    /// silently orphaned the still-live predecessor. That was the worst case,
    /// not the safe one: two live hosts of one identity are cryptographically
    /// the same peer, so the registry cannot tell a rebuild from a displacement,
    /// and in-proc occupancy was in practice the only thing excluding two live
    /// hosts of one mob at all.
    ///
    /// Now the second claim is refused and the PREDECESSOR keeps receiving. The
    /// delivery leg is the point: this asserts a live route, not a surviving map
    /// entry.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn same_key_registration_over_a_live_route_leaves_the_predecessor_receiving() {
        let registry = InprocRegistry::new();
        let namespace = "realm-rebind-delivery";
        let name = "mob.rebind/lead/lead-1";
        let keypair = make_keypair();
        let pubkey = keypair.public_key();
        let (predecessor_inbox, predecessor_sender, predecessor_finalizer) =
            classified_inbox_with_runtime();
        let (successor_inbox, successor_sender, _successor_finalizer) =
            classified_inbox_with_runtime();

        assert_eq!(
            registry.register_with_meta_in_namespace(
                namespace,
                name,
                pubkey,
                predecessor_sender,
                PeerMeta::default(),
            ),
            RegistrationOutcome::Registered
        );
        assert_eq!(
            registry.register_with_meta_in_namespace(
                namespace,
                name,
                pubkey,
                successor_sender,
                PeerMeta::default(),
            ),
            RegistrationOutcome::Rejected {
                reason: RegistrationRejection::NameOccupied {
                    holder_pubkey: pubkey
                }
            },
            "one identity rebuilding itself over its own live route gets the same refusal as \
             any other claimant, with its own key carried back as the holder"
        );

        let successor_inbox = Arc::new(successor_inbox);
        let predecessor_finalize =
            spawn_runtime_finalization(Arc::new(predecessor_inbox), predecessor_finalizer);
        let peer_keypair = make_keypair();
        let delivery = registry
            .send_to_pubkey_in_namespace_with_id_wait(
                namespace,
                &peer_keypair,
                &pubkey,
                Uuid::new_v4(),
                probe_message("after the refused claim"),
                true,
                TEST_PARK_BOUND,
            )
            .await
            .expect("the refused claim must not have unbound the incumbent generation");
        assert!(matches!(
            delivery.resolved_outcome(),
            IngressDeliveryOutcome::DurablyResolved(
                meerkat_core::PeerIngressTerminalOutcomeKind::Accepted
            )
        ));

        let candidate = predecessor_finalize
            .await
            .expect("predecessor finalizer completes");
        match candidate.interaction.content {
            meerkat_core::InteractionContent::Message {
                body,
                blocks: None,
                in_reply_to: _,
            } => {
                assert_eq!(body, "after the refused claim");
            }
            other => panic!("expected Message interaction, got {other:?}"),
        }
        assert!(
            successor_inbox.try_claim_one_classified().is_none(),
            "a refused claim must not receive: the route was never handed to it"
        );
    }

    /// Same namespace, same name, same signing key, new inbox generation. There
    /// is exactly ONE door to that, and it is the replacing seam: the successor
    /// hands in the exact predecessor generation it replaces. Registration is
    /// not a second door, and this pins both halves in one test so neither can
    /// drift from the other:
    ///
    /// 1. registration is REFUSED and the refusal carries the holder key, which
    ///    here is the claimant's own;
    /// 2. `replace_sender_in_namespace` is admitted, the key never stops
    ///    resolving and now resolves to the newest generation, and the
    ///    superseded predecessor's later generation-checked unregistration (its
    ///    `Drop`) cannot unbind the successor.
    #[cfg(feature = "workspace-tests")]
    #[test]
    fn same_identity_generational_move_goes_through_the_replacing_seam_only() {
        let registry = InprocRegistry::new();
        let namespace = "realm-rebind";
        let keypair = make_keypair();
        let pubkey = keypair.public_key();
        let (_first_inbox, first_sender) = classified_inbox();
        let (_second_inbox, second_sender) = classified_inbox();

        assert_eq!(
            registry.register_with_meta_in_namespace(
                namespace,
                "agent",
                pubkey,
                first_sender.clone(),
                PeerMeta::default(),
            ),
            RegistrationOutcome::Registered
        );
        assert_eq!(
            registry.register_with_meta_in_namespace(
                namespace,
                "agent",
                pubkey,
                second_sender.clone(),
                PeerMeta::default(),
            ),
            RegistrationOutcome::Rejected {
                reason: RegistrationRejection::NameOccupied {
                    holder_pubkey: pubkey
                }
            },
            "registration is not a door to a generational move, not even for the key that \
             already holds the name"
        );
        let live = registry
            .get_by_pubkey_in_namespace(namespace, &pubkey)
            .expect("the refused claim must leave the incumbent generation live");
        assert!(
            live.same_inbox(&first_sender),
            "a refused claim must not move the route onto the newcomer's inbox"
        );

        // The one legal door: name the predecessor generation being replaced.
        registry
            .replace_sender_in_namespace(
                namespace,
                "agent",
                (&pubkey, &first_sender),
                pubkey,
                second_sender.clone(),
            )
            .expect("naming the exact predecessor generation must be admitted");
        let live = registry
            .get_by_pubkey_in_namespace(namespace, &pubkey)
            .expect("the identity must never stop being addressable");
        assert!(
            live.same_inbox(&second_sender),
            "the replacement must resolve to the newest inbox generation"
        );
        assert_eq!(registry.peers_in_namespace(namespace).len(), 1);

        // The predecessor generation is torn down afterwards. Its
        // generation-checked unregistration must not unbind the successor.
        assert!(
            !registry.unregister_sender_in_namespace(namespace, &pubkey, &first_sender),
            "a stale generation must not be able to remove the live route"
        );
        let live = registry
            .get_by_pubkey_in_namespace(namespace, &pubkey)
            .expect("the successor route must survive the predecessor teardown");
        assert!(live.same_inbox(&second_sender));
        assert_eq!(
            registry.get_name_by_pubkey_in_namespace(namespace, &pubkey),
            Some("agent".to_string())
        );
    }

    /// A name released by its holder is reusable by a *different* identity: the
    /// refusal is a liveness boundary on the incumbent route, not a permanent
    /// reservation of the name.
    #[cfg(feature = "workspace-tests")]
    #[test]
    fn a_released_name_is_claimable_by_a_different_identity() {
        let registry = InprocRegistry::new();
        let namespace = "realm-release";
        let first = make_keypair().public_key();
        let second = make_keypair().public_key();
        let (_first_inbox, first_sender) = classified_inbox();
        let (_second_inbox, second_sender) = classified_inbox();

        assert_eq!(
            registry.register_with_meta_in_namespace(
                namespace,
                "agent",
                first,
                first_sender.clone(),
                PeerMeta::default(),
            ),
            RegistrationOutcome::Registered
        );
        assert_eq!(
            registry.register_with_meta_in_namespace(
                namespace,
                "agent",
                second,
                second_sender.clone(),
                PeerMeta::default(),
            ),
            RegistrationOutcome::Rejected {
                reason: RegistrationRejection::NameOccupied {
                    holder_pubkey: first
                }
            }
        );

        assert!(registry.unregister_sender_in_namespace(namespace, &first, &first_sender));
        assert_eq!(
            registry.register_with_meta_in_namespace(
                namespace,
                "agent",
                second,
                second_sender.clone(),
                PeerMeta::default(),
            ),
            RegistrationOutcome::Registered,
            "a released name must be claimable by the next identity"
        );
        assert!(
            registry
                .get_by_pubkey_in_namespace(namespace, &second)
                .is_some_and(|live| live.same_inbox(&second_sender))
        );
        assert!(
            registry
                .get_by_pubkey_in_namespace(namespace, &first)
                .is_none()
        );
    }

    /// The exact-generation seam is the one reconstruction path that CAN take a
    /// live name over, because the caller has to present the predecessor
    /// generation it is replacing. A caller that presents a stale generation is
    /// refused and the live route is untouched.
    #[cfg(feature = "workspace-tests")]
    #[test]
    fn exact_generation_replacement_is_the_authorized_reconstruction_seam() {
        let registry = InprocRegistry::new();
        let namespace = "realm-exact";
        let name = "agent";
        let incumbent = make_keypair().public_key();
        let successor = make_keypair().public_key();
        let (_incumbent_inbox, incumbent_sender) = classified_inbox();
        let (_stale_inbox, stale_sender) = classified_inbox();
        let (_successor_inbox, successor_sender) = classified_inbox();

        assert_eq!(
            registry.register_with_meta_in_namespace(
                namespace,
                name,
                incumbent,
                incumbent_sender.clone(),
                PeerMeta::default(),
            ),
            RegistrationOutcome::Registered
        );

        // A witness that is not the live generation proves nothing.
        assert_eq!(
            registry.replace_sender_in_namespace(
                namespace,
                name,
                (&incumbent, &stale_sender),
                successor,
                successor_sender.clone(),
            ),
            Err(InprocPublicationError::ExpectedGenerationNotCurrent)
        );
        assert!(
            registry
                .get_by_pubkey_in_namespace(namespace, &incumbent)
                .is_some_and(|live| live.same_inbox(&incumbent_sender)),
            "a refused replacement must leave the live generation in place"
        );

        // The live generation authorizes the takeover.
        assert_eq!(
            registry.replace_sender_in_namespace(
                namespace,
                name,
                (&incumbent, &incumbent_sender),
                successor,
                successor_sender.clone(),
            ),
            Ok(())
        );
        assert!(
            registry
                .get_by_pubkey_in_namespace(namespace, &incumbent)
                .is_none(),
            "the replaced generation must no longer be reachable"
        );
        assert!(
            registry
                .get_by_pubkey_in_namespace(namespace, &successor)
                .is_some_and(|live| live.same_inbox(&successor_sender))
        );
        assert_eq!(registry.peers_in_namespace(namespace).len(), 1);
    }

    /// ROW #292 gate: registration returns a typed [`RegistrationOutcome`] that
    /// surfaces its own rename, name occupancy and zero-pubkey rejection,
    /// instead of silently evicting and returning `()`.
    #[cfg(feature = "workspace-tests")]
    #[test]
    fn registration_outcome_is_typed_for_rename_and_rejection() {
        let registry = InprocRegistry::new();
        let keypair = make_keypair();
        let pubkey = keypair.public_key();
        let (_, sender1) = classified_inbox();
        let (_, sender2) = classified_inbox();
        let (_, sender3) = classified_inbox();

        // Clean first insert.
        assert_eq!(
            registry.register("agent-v1", pubkey, sender1),
            RegistrationOutcome::Registered
        );

        // Re-registering the SAME pubkey under a NEW name evicts the old name
        // and reports it typed.
        assert_eq!(
            registry.register("agent-v2", pubkey, sender2),
            RegistrationOutcome::ReplacedPubkey {
                evicted_name: "agent-v1".to_string()
            }
        );

        // Re-registering an existing NAME with a NEW pubkey is refused and
        // reports which key holds the live route.
        let other = make_keypair();
        let other_pubkey = other.public_key();
        assert_eq!(
            registry.register("agent-v2", other_pubkey, sender3),
            RegistrationOutcome::Rejected {
                reason: RegistrationRejection::NameOccupied {
                    holder_pubkey: pubkey
                }
            }
        );
        assert!(
            registry.contains(&pubkey),
            "the refused registration must leave the live route in place"
        );

        // A zero pubkey is refused fail-closed with a typed rejection, no
        // mutation.
        let (_, zero_sender) = classified_inbox();
        let zero_pubkey = PubKey::new([0u8; 32]);
        assert_eq!(
            registry.register("zero", zero_pubkey, zero_sender),
            RegistrationOutcome::Rejected {
                reason: RegistrationRejection::ZeroPubkey
            }
        );
        assert!(!registry.contains_name("zero"));
    }

    /// Test that the ABA scenario is handled correctly: after a name has been
    /// released and re-registered under a new identity, a late unregister from
    /// the old owner (its `Drop`) must be a safe no-op rather than unbinding
    /// the successor.
    #[cfg(feature = "workspace-tests")]
    #[test]
    fn test_registry_aba_scenario_safe() {
        let registry = InprocRegistry::new();
        let keypair_old = make_keypair();
        let pubkey_old = keypair_old.public_key();
        let keypair_new = make_keypair();
        let pubkey_new = keypair_new.public_key();
        let (_, sender_old) = classified_inbox();
        let (_, sender_new) = classified_inbox();

        // Step 1: Old runtime registers
        registry.register("agent", pubkey_old, sender_old.clone());
        assert!(registry.contains(&pubkey_old));

        // Step 2: The old route is released, then a new runtime takes the freed
        // name. A live name is never displaced, so release has to come first.
        assert!(registry.unregister_sender_in_namespace(
            DEFAULT_NAMESPACE,
            &pubkey_old,
            &sender_old
        ));
        assert_eq!(
            registry.register("agent", pubkey_new, sender_new),
            RegistrationOutcome::Registered
        );
        assert!(!registry.contains(&pubkey_old));
        assert!(registry.contains(&pubkey_new));

        // Step 3: Old runtime drops and calls unregister(pubkey_old)
        // This should be a no-op since pubkey_old was already released
        let removed = registry.unregister(&pubkey_old);
        assert!(
            !removed,
            "unregister of released pubkey should return false"
        );

        // New agent should still be registered (not affected by old unregister)
        assert!(
            registry.contains(&pubkey_new),
            "new agent should still be registered"
        );
        assert!(
            registry.contains_name("agent"),
            "name should still map to new agent"
        );

        // The name maps to the new identity.
        assert_eq!(
            registry.get_name_by_pubkey(&pubkey_new).as_deref(),
            Some("agent"),
            "name should map to the new pubkey"
        );
    }

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn test_registry_peer_names_in_namespace() {
        let registry = InprocRegistry::new();

        for i in 0..3 {
            let keypair = make_keypair();
            let (_, sender) = classified_inbox();
            registry.register_with_meta_in_namespace(
                "realm-names",
                format!("agent-{i}"),
                keypair.public_key(),
                sender,
                PeerMeta::default(),
            );
        }

        let names = registry.peer_names_in_namespace("realm-names");
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"agent-0".to_string()));
        assert!(names.contains(&"agent-1".to_string()));
        assert!(names.contains(&"agent-2".to_string()));
        assert!(registry.peer_names_in_namespace("realm-other").is_empty());
    }

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn test_registry_peers_snapshot() {
        let registry = InprocRegistry::new();
        let keypair = make_keypair();
        let pubkey = keypair.public_key();
        let (_, sender) = classified_inbox();
        registry.register("agent-a", pubkey, sender);

        let peers = registry.peers();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].name, "agent-a");
        assert_eq!(peers[0].pubkey, pubkey);
    }

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn test_registry_clear() {
        let registry = InprocRegistry::new();

        for i in 0..3 {
            let keypair = make_keypair();
            let (_, sender) = classified_inbox();
            registry.register(format!("agent-{i}"), keypair.public_key(), sender);
        }

        assert_eq!(registry.len(), 3);
        registry.clear();
        assert!(registry.is_empty());
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_registry_send_delivers_to_inbox() {
        let registry = InprocRegistry::new();

        // Set up receiver
        let receiver_keypair = make_keypair();
        let (inbox, sender, finalizer) = classified_inbox_with_runtime();
        registry.register("receiver", receiver_keypair.public_key(), sender);
        let finalize = spawn_runtime_finalization(Arc::new(inbox), finalizer);

        // Set up sender
        let sender_keypair = make_keypair();

        // Send a message (pubkey-keyed delivery)
        let result = registry
            .send_to_pubkey_in_namespace_with_id_wait(
                "",
                &sender_keypair,
                &receiver_keypair.public_key(),
                Uuid::new_v4(),
                MessageKind::Message {
                    objective_id: None,
                    content_taint: None,
                    blocks: None,
                    body: "hello inproc".to_string(),
                    handling_mode: None,
                    in_reply_to: None,
                },
                true,
                TEST_PARK_BOUND,
            )
            .await;
        let delivery = result.expect("runtime-bound Message must be durably admitted");
        assert!(matches!(
            delivery.resolved_outcome(),
            IngressDeliveryOutcome::DurablyResolved(
                meerkat_core::PeerIngressTerminalOutcomeKind::Accepted
            )
        ));

        let candidate = finalize.await.expect("runtime finalizer completes");
        assert_eq!(
            candidate.interaction.from,
            sender_keypair.public_key().to_pubkey_string()
        );
        match candidate.interaction.content {
            meerkat_core::InteractionContent::Message {
                body,
                blocks: None,
                in_reply_to: _,
            } => {
                assert_eq!(body, "hello inproc");
            }
            other => panic!("expected Message interaction, got {other:?}"),
        }
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn retrying_same_envelope_deduplicates_before_exact_claim_commit() {
        let registry = InprocRegistry::new();
        let receiver_keypair = make_keypair();
        let (inbox, sender, finalizer) = classified_inbox_with_runtime();
        let inbox = Arc::new(inbox);
        registry.register("receiver", receiver_keypair.public_key(), sender);
        let sender_keypair = make_keypair();
        let envelope_id = Uuid::new_v4();
        let message = || MessageKind::Message {
            objective_id: None,
            content_taint: None,
            blocks: None,
            body: "stable retry".to_string(),
            handling_mode: None,
            in_reply_to: None,
        };

        let first_finalize = spawn_runtime_finalization(Arc::clone(&inbox), Arc::clone(&finalizer));
        let first = registry
            .send_to_pubkey_in_namespace_with_id_wait(
                "",
                &sender_keypair,
                &receiver_keypair.public_key(),
                envelope_id,
                message(),
                true,
                TEST_PARK_BOUND,
            )
            .await
            .expect("first stable envelope delivery should resolve");
        first_finalize
            .await
            .expect("first actual-machine admission should complete");
        assert!(matches!(
            first.resolved_outcome(),
            IngressDeliveryOutcome::DurablyResolved(
                meerkat_core::PeerIngressTerminalOutcomeKind::Accepted
            )
        ));

        let retry_finalize = spawn_runtime_finalization(Arc::clone(&inbox), Arc::clone(&finalizer));
        let retry = registry
            .send_to_pubkey_in_namespace_with_id_wait(
                "",
                &sender_keypair,
                &receiver_keypair.public_key(),
                envelope_id,
                message(),
                true,
                TEST_PARK_BOUND,
            )
            .await
            .expect("same-envelope retry should resolve by durable deduplication");
        retry_finalize
            .await
            .expect("retry actual-machine admission should complete");
        assert!(matches!(
            retry.resolved_outcome(),
            IngressDeliveryOutcome::DurablyResolved(
                meerkat_core::PeerIngressTerminalOutcomeKind::Deduplicated
            )
        ));

        let projection = inbox
            .classified_snapshot()
            .expect("classified queue projection should exist");
        assert_eq!(projection.total_count, 0);
        assert_eq!(projection.durably_admitted_count, 2);
        assert_eq!(projection.terminal_outcomes.accepted, 1);
        assert_eq!(projection.terminal_outcomes.deduplicated, 1);
        let correlation = projection
            .last_delivery_correlation
            .expect("retry commit should publish stable delivery correlation");
        assert_eq!(
            correlation.raw_item_id,
            meerkat_core::InteractionId(envelope_id)
        );
        assert_eq!(
            correlation.outcome,
            meerkat_core::PeerIngressTerminalOutcomeKind::Deduplicated
        );
        assert!(correlation.existing_runtime_input_id.is_some());
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn response_preserves_legacy_queued_inproc_semantics() {
        let registry = InprocRegistry::new();
        let receiver_keypair = make_keypair();
        let in_reply_to = meerkat_core::InteractionId(Uuid::new_v4());
        let (mut inbox, sender) = classified_inbox();
        registry.register("receiver", receiver_keypair.public_key(), sender);
        let sender_keypair = make_keypair();
        let response_id = Uuid::new_v4();

        let delivery = registry
            .send_to_pubkey_in_namespace_with_id_wait(
                "",
                &sender_keypair,
                &receiver_keypair.public_key(),
                response_id,
                MessageKind::Response {
                    objective_id: None,
                    content_taint: None,
                    in_reply_to: in_reply_to.0,
                    status: crate::types::Status::Completed,
                    result: serde_json::json!({}),
                    blocks: None,
                    handling_mode: None,
                },
                true,
                TEST_PARK_BOUND,
            )
            .await
            .expect("Response must complete after queue admission");

        assert_eq!(delivery.envelope_id, response_id);
        assert!(matches!(
            delivery.resolved_outcome(),
            IngressDeliveryOutcome::Queued
        ));
        let mut entries = inbox.try_drain_classified();
        assert_eq!(entries.len(), 1);
        let entry = entries.pop().expect("queued Response entry");
        let InboxItem::External { envelope } = entry.item else {
            panic!("expected external Response envelope");
        };
        assert_eq!(envelope.id, response_id);
    }

    /// Bound used by the park tests. Short enough to keep the lane fast, and
    /// always paired with a much longer OUTER timeout so a regression to the
    /// unbounded park fails the assertion instead of hanging the lane.
    const SHORT_PARK_BOUND: Duration = Duration::from_millis(200);
    const OUTER_DEADLINE: Duration = Duration::from_secs(5);

    fn message_kind(body: &str) -> MessageKind {
        MessageKind::Message {
            objective_id: None,
            content_taint: None,
            blocks: None,
            body: body.to_string(),
            handling_mode: None,
            in_reply_to: None,
        }
    }

    /// The exact field defect: an in-process peer send waits for the RECEIVER's
    /// drain to commit its claim, so a drain that dies mid-claim used to park
    /// the sender forever. The claim drop releases the entry back to the queue
    /// head, nothing commits it, and nothing polls.
    ///
    /// The bounded park must return a fact that is neither a drop nor a
    /// success: the envelope IS queued and may still be drained later.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn parked_inproc_sender_resolves_when_receiver_drain_dies() {
        let registry = InprocRegistry::new();
        let receiver_keypair = make_keypair();
        let (inbox, sender, _finalizer) = classified_inbox_with_runtime();
        registry.register("receiver", receiver_keypair.public_key(), sender);
        let inbox = Arc::new(inbox);

        // A drain that dies while holding the exact claim: it takes the claim
        // and drops it without committing. `PeerIngressQueueClaim::drop`
        // releases the lease, which leaves the entry queued and the sender's
        // receipt unresolved forever.
        let dying_drain = {
            let inbox = Arc::clone(&inbox);
            tokio::spawn(async move {
                loop {
                    if let Some(claimed) = inbox.try_claim_one_classified() {
                        drop(crate::runtime::comms_runtime::test_peer_ingress_queue_claim(claimed));
                        return;
                    }
                    tokio::task::yield_now().await;
                }
            })
        };

        let sender_keypair = make_keypair();
        let envelope_id = Uuid::new_v4();
        let delivery = tokio::time::timeout(
            OUTER_DEADLINE,
            registry.send_to_pubkey_in_namespace_with_id_wait(
                "",
                &sender_keypair,
                &receiver_keypair.public_key(),
                envelope_id,
                message_kind("drain dies mid-claim"),
                true,
                SHORT_PARK_BOUND,
            ),
        )
        .await
        .expect("a dead receiver drain must not park the sender forever")
        .expect("the envelope was admitted, so this is not a typed send failure");

        assert_eq!(delivery.envelope_id, envelope_id);
        assert!(
            matches!(
                delivery.outcome,
                InprocDeliveryOutcome::Unconfirmed { park_bound } if park_bound == SHORT_PARK_BOUND
            ),
            "a dropped receiver claim is ambiguous delivery, not success: got {:?}",
            delivery.outcome
        );
        dying_drain.await.expect("dying drain task joins");

        let (_, queued) = inbox
            .peer_authority_test_snapshot()
            .expect("classified inbox exposes its queue depth");
        assert_eq!(
            queued, 1,
            "the envelope must still be queued: `Unconfirmed` claims admission, not loss"
        );
    }

    /// Companion: a drain that is alive but never claims (stalled, not dead)
    /// parks the sender at exactly the same await. Same bound, same fact.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn inproc_sender_park_is_bounded_when_drain_never_claims() {
        let registry = InprocRegistry::new();
        let receiver_keypair = make_keypair();
        let (inbox, sender, _finalizer) = classified_inbox_with_runtime();
        registry.register("receiver", receiver_keypair.public_key(), sender);

        let sender_keypair = make_keypair();
        let delivery = tokio::time::timeout(
            OUTER_DEADLINE,
            registry.send_to_pubkey_in_namespace_with_id_wait(
                "",
                &sender_keypair,
                &receiver_keypair.public_key(),
                Uuid::new_v4(),
                message_kind("nobody is draining"),
                true,
                SHORT_PARK_BOUND,
            ),
        )
        .await
        .expect("a stalled receiver drain must not park the sender forever")
        .expect("the envelope was admitted, so this is not a typed send failure");

        assert!(
            matches!(
                delivery.outcome,
                InprocDeliveryOutcome::Unconfirmed { park_bound } if park_bound == SHORT_PARK_BOUND
            ),
            "expected the bounded ambiguous fact, got {:?}",
            delivery.outcome
        );
        drop(inbox);
    }

    /// The second park in the same call: once timed-out senders have filled the
    /// receiver queue, capacity never opens again (only a dequeue fires the
    /// capacity notifier), so the capacity wait must share the same deadline.
    ///
    /// Expiry BEFORE admission is not ambiguous - nothing was delivered - so it
    /// keeps the existing typed `InboxFull` fact rather than claiming the
    /// envelope reached the peer.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn inproc_sender_capacity_park_is_bounded_when_receiver_never_drains() {
        let registry = InprocRegistry::new();
        let receiver_keypair = make_keypair();
        let (peer_comms_handle, _finalizer) =
            meerkat_runtime::test_peer_comms_handle_and_runtime_finalizer();
        let context = Arc::new(crate::classify::IngressClassificationContext {
            require_peer_auth: false,
            trusted_peers: Arc::new(RwLock::new(TrustStore::new())),
            peer_comms_handle: Arc::new(RwLock::new(Some(peer_comms_handle))),
            inproc_namespace: None,
            durable_runtime_consumer: true,
        });
        let (inbox, sender) = Inbox::new_classified_with_capacity_for_test(context, 1);
        registry.register("receiver", receiver_keypair.public_key(), sender);

        let sender_keypair = make_keypair();
        let first = tokio::time::timeout(
            OUTER_DEADLINE,
            registry.send_to_pubkey_in_namespace_with_id_wait(
                "",
                &sender_keypair,
                &receiver_keypair.public_key(),
                Uuid::new_v4(),
                message_kind("fills the only slot"),
                true,
                SHORT_PARK_BOUND,
            ),
        )
        .await
        .expect("first send must return at the bound")
        .expect("first send was admitted");
        assert!(matches!(
            first.outcome,
            InprocDeliveryOutcome::Unconfirmed { .. }
        ));

        let second = tokio::time::timeout(
            OUTER_DEADLINE,
            registry.send_to_pubkey_in_namespace_with_id_wait(
                "",
                &sender_keypair,
                &receiver_keypair.public_key(),
                Uuid::new_v4(),
                message_kind("waits on capacity that never opens"),
                true,
                SHORT_PARK_BOUND,
            ),
        )
        .await
        .expect("the capacity park must be bounded by the same deadline as the receipt park");

        assert!(
            matches!(
                second,
                Err(InprocSendError::IngressDropped(DropReason::InboxFull))
            ),
            "a send that never got admitted must carry the typed capacity refusal, \
             not an ambiguous delivery and not an unreachable peer: got {second:?}"
        );
        let (_, queued) = inbox
            .peer_authority_test_snapshot()
            .expect("classified inbox exposes its queue depth");
        assert_eq!(queued, 1, "the refused send must not have been queued");
    }

    /// The `Response` lane deliberately keeps capacity-only semantics: it
    /// returns on queue admission and never waits for the receiver's drain.
    ///
    /// The park bound here is deliberately huge, so the only way this test can
    /// finish inside its outer deadline is if `Response` never waits on a
    /// drain that does not exist.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn response_inproc_send_never_waits_for_a_receiver_drain() {
        let registry = InprocRegistry::new();
        let receiver_keypair = make_keypair();
        let (inbox, sender, _finalizer) = classified_inbox_with_runtime();
        registry.register("receiver", receiver_keypair.public_key(), sender);

        let sender_keypair = make_keypair();
        let response_id = Uuid::new_v4();
        let delivery = tokio::time::timeout(
            OUTER_DEADLINE,
            registry.send_to_pubkey_in_namespace_with_id_wait(
                "",
                &sender_keypair,
                &receiver_keypair.public_key(),
                response_id,
                MessageKind::Response {
                    objective_id: None,
                    content_taint: None,
                    in_reply_to: Uuid::new_v4(),
                    status: crate::types::Status::Completed,
                    result: serde_json::json!({}),
                    blocks: None,
                    handling_mode: None,
                },
                true,
                Duration::from_hours(1),
            ),
        )
        .await
        .expect("Response must return on admission, never on drain commit")
        .expect("Response admission succeeded");

        assert_eq!(delivery.envelope_id, response_id);
        assert!(matches!(
            delivery.resolved_outcome(),
            IngressDeliveryOutcome::Queued
        ));
        drop(inbox);
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn supervisor_bridge_request_is_auth_exempt_volatile_control() {
        let registry = InprocRegistry::new();
        let receiver_keypair = make_keypair();
        let (inbox, sender) = classified_inbox_with_auth(true);
        registry.register("receiver", receiver_keypair.public_key(), sender);
        let handoff = spawn_volatile_handoff(Arc::new(inbox));
        let sender_keypair = make_keypair();
        let request_id = Uuid::new_v4();

        let delivery = registry
            .send_to_pubkey_in_namespace_with_id_wait(
                "",
                &sender_keypair,
                &receiver_keypair.public_key(),
                request_id,
                MessageKind::Request {
                    objective_id: None,
                    content_taint: None,
                    intent: meerkat_core::SUPERVISOR_BRIDGE_INTENT.to_string(),
                    params: serde_json::json!({
                        "command": "bind_member",
                        "supervisor": {
                            "name": "mob/__mob_supervisor__",
                            "peer_id": sender_keypair.public_key().to_peer_id(),
                            "address": "inproc://mob/__mob_supervisor__"
                        },
                        "epoch": 1,
                        "protocol_version": 1,
                        "expected_peer_id": "peer-id",
                        "expected_address": "inproc://peer"
                    }),
                    blocks: None,
                    reply_endpoint: None,
                    handling_mode: None,
                },
                true,
                TEST_PARK_BOUND,
            )
            .await
            .expect("auth-exempt supervisor bridge request must hand off as volatile control");

        assert_eq!(delivery.envelope_id, request_id);
        assert!(matches!(
            delivery.resolved_outcome(),
            IngressDeliveryOutcome::VolatileHandedOff
        ));
        let entry = handoff.await.expect("volatile handoff task completes");
        assert_eq!(entry.class, meerkat_core::PeerInputClass::ActionableRequest);
        assert_eq!(
            entry.auth,
            meerkat_core::PeerIngressAuthDecision::Exempt(
                meerkat_core::PeerIngressAuthExemption::SupervisorBridge
            )
        );
        let InboxItem::External { envelope } = entry.item else {
            panic!("expected external supervisor bridge Request envelope");
        };
        assert_eq!(envelope.id, request_id);
    }

    #[tokio::test]
    async fn test_registry_send_peer_not_found() {
        let registry = InprocRegistry::new();
        let sender_keypair = make_keypair();
        let unknown = make_keypair().public_key();

        let result = registry
            .send_to_pubkey_in_namespace_with_id_wait(
                "",
                &sender_keypair,
                &unknown,
                Uuid::new_v4(),
                MessageKind::Message {
                    objective_id: None,
                    content_taint: None,
                    blocks: None,
                    body: "hello".to_string(),
                    handling_mode: None,
                    in_reply_to: None,
                },
                true,
                TEST_PARK_BOUND,
            )
            .await;

        assert!(matches!(result, Err(InprocSendError::PeerNotFound(_))));
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_registry_send_inbox_closed() {
        let registry = InprocRegistry::new();

        // Set up receiver but drop the inbox
        let receiver_keypair = make_keypair();
        let (inbox, sender) = classified_inbox();
        registry.register("receiver", receiver_keypair.public_key(), sender);
        drop(inbox); // Close the inbox

        let sender_keypair = make_keypair();

        let result = registry
            .send_to_pubkey_in_namespace_with_id_wait(
                "",
                &sender_keypair,
                &receiver_keypair.public_key(),
                Uuid::new_v4(),
                MessageKind::Message {
                    objective_id: None,
                    content_taint: None,
                    blocks: None,
                    body: "hello".to_string(),
                    handling_mode: None,
                    in_reply_to: None,
                },
                true,
                TEST_PARK_BOUND,
            )
            .await;

        assert!(matches!(result, Err(InprocSendError::InboxClosed)));
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_registry_namespace_isolation_for_lookup_and_send() {
        let registry = InprocRegistry::new();
        let receiver_keypair = make_keypair();
        let (inbox, sender, finalizer) = classified_inbox_with_runtime();
        registry.register_with_meta_in_namespace(
            "realm-a",
            "receiver",
            receiver_keypair.public_key(),
            sender,
            PeerMeta::default(),
        );
        let finalize = spawn_runtime_finalization(Arc::new(inbox), finalizer);

        // Default namespace cannot see realm-a registrations.
        assert!(
            registry
                .get_by_pubkey(&receiver_keypair.public_key())
                .is_none()
        );
        assert!(
            registry
                .get_by_pubkey_in_namespace("realm-a", &receiver_keypair.public_key())
                .is_some()
        );

        let sender_keypair = make_keypair();

        // Matching namespace succeeds.
        let ok = registry
            .send_to_pubkey_in_namespace_with_id_wait(
                "realm-a",
                &sender_keypair,
                &receiver_keypair.public_key(),
                Uuid::new_v4(),
                MessageKind::Message {
                    objective_id: None,
                    content_taint: None,
                    blocks: None,
                    body: "hello scoped".to_string(),
                    handling_mode: None,
                    in_reply_to: None,
                },
                true,
                TEST_PARK_BOUND,
            )
            .await;
        assert!(matches!(
            ok.expect("matching namespace must deliver")
                .resolved_outcome(),
            IngressDeliveryOutcome::DurablyResolved(
                meerkat_core::PeerIngressTerminalOutcomeKind::Accepted
            )
        ));

        // Different namespace cannot route to receiver.
        let wrong_ns = registry
            .send_to_pubkey_in_namespace_with_id_wait(
                "realm-b",
                &sender_keypair,
                &receiver_keypair.public_key(),
                Uuid::new_v4(),
                MessageKind::Message {
                    objective_id: None,
                    content_taint: None,
                    blocks: None,
                    body: "should not deliver".to_string(),
                    handling_mode: None,
                    in_reply_to: None,
                },
                true,
                TEST_PARK_BOUND,
            )
            .await;
        assert!(matches!(wrong_ns, Err(InprocSendError::PeerNotFound(_))));

        finalize.await.expect("runtime finalizer completes");
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_send_to_pubkey_in_namespace_ignores_display_name_collision() {
        let registry = InprocRegistry::new();
        let target_keypair = make_keypair();
        let target_pubkey = target_keypair.public_key();
        let shadow_keypair = make_keypair();
        let shadow_pubkey = shadow_keypair.public_key();
        let (target_inbox, target_sender, target_finalizer) = classified_inbox_with_runtime();
        let (mut shadow_inbox, shadow_sender) = classified_inbox();

        registry.register_with_meta_in_namespace(
            "",
            "canonical-target",
            target_pubkey,
            target_sender,
            PeerMeta::default(),
        );
        let finalize_target = spawn_runtime_finalization(Arc::new(target_inbox), target_finalizer);
        registry.register_with_meta_in_namespace(
            "",
            "shared-display-name",
            shadow_pubkey,
            shadow_sender,
            PeerMeta::default(),
        );

        let sender_keypair = make_keypair();
        let result = registry
            .send_to_pubkey_in_namespace_with_id_wait(
                "",
                &sender_keypair,
                &target_pubkey,
                Uuid::new_v4(),
                MessageKind::Message {
                    objective_id: None,
                    content_taint: None,
                    blocks: None,
                    body: "hello canonical".to_string(),
                    handling_mode: None,
                    in_reply_to: None,
                },
                true,
                TEST_PARK_BOUND,
            )
            .await;
        let delivery = result.expect("canonical target must receive");
        assert!(matches!(
            delivery.resolved_outcome(),
            IngressDeliveryOutcome::DurablyResolved(
                meerkat_core::PeerIngressTerminalOutcomeKind::Accepted
            )
        ));

        assert_eq!(shadow_inbox.try_drain_classified().len(), 0);
        let candidate = finalize_target
            .await
            .expect("target runtime finalizer completes");
        assert_eq!(candidate.interaction.id.0, delivery.envelope_id);
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn test_send_to_pubkey_any_namespace_rejects_ambiguous_identity() {
        let registry = InprocRegistry::new();
        let sender_keypair = make_keypair();
        let target_keypair = make_keypair();
        let target_pubkey = target_keypair.public_key();
        let (mut alpha_inbox, alpha_sender) = classified_inbox();
        let (mut beta_inbox, beta_sender) = classified_inbox();

        registry.register_with_meta_in_namespace(
            "realm-alpha",
            "alpha-target",
            target_pubkey,
            alpha_sender,
            PeerMeta::default(),
        );
        registry.register_with_meta_in_namespace(
            "realm-beta",
            "beta-target",
            target_pubkey,
            beta_sender,
            PeerMeta::default(),
        );

        let result = registry
            .send_to_pubkey_any_namespace_with_id_wait(
                &sender_keypair,
                &target_pubkey,
                Uuid::new_v4(),
                MessageKind::Message {
                    objective_id: None,
                    content_taint: None,
                    blocks: None,
                    body: "ambiguous identity".to_string(),
                    handling_mode: None,
                    in_reply_to: None,
                },
                true,
                TEST_PARK_BOUND,
            )
            .await;

        assert!(matches!(result, Err(InprocSendError::PeerNotFound(_))));
        assert!(alpha_inbox.try_drain_classified().is_empty());
        assert!(beta_inbox.try_drain_classified().is_empty());
    }

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn test_registry_same_name_can_exist_in_different_namespaces() {
        let registry = InprocRegistry::new();
        let kp_a = make_keypair();
        let kp_b = make_keypair();
        let (_, sender_a) = classified_inbox();
        let (_, sender_b) = classified_inbox();

        registry.register_with_meta_in_namespace(
            "realm-a",
            "shared-name",
            kp_a.public_key(),
            sender_a,
            PeerMeta::default(),
        );
        registry.register_with_meta_in_namespace(
            "realm-b",
            "shared-name",
            kp_b.public_key(),
            sender_b,
            PeerMeta::default(),
        );

        assert_eq!(
            registry
                .get_name_by_pubkey_in_namespace("realm-a", &kp_a.public_key())
                .as_deref(),
            Some("shared-name")
        );
        assert_eq!(
            registry
                .get_name_by_pubkey_in_namespace("realm-b", &kp_b.public_key())
                .as_deref(),
            Some("shared-name")
        );
        assert_ne!(kp_a.public_key(), kp_b.public_key());
        assert!(
            !registry.contains_name("shared-name"),
            "default namespace must not see namespaced registrations"
        );
    }

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn test_global_registry() {
        // Access global registry
        let registry = InprocRegistry::global();

        // Clear any existing state (from other tests)
        registry.clear();

        // Register a peer
        let keypair = make_keypair();
        let (_, sender) = classified_inbox();
        registry.register("global-test", keypair.public_key(), sender);

        // Verify it's accessible
        assert!(registry.contains_name("global-test"));

        // Clean up
        registry.unregister(&keypair.public_key());
    }

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn test_registry_register_with_meta() {
        let registry = InprocRegistry::new();
        let keypair = make_keypair();
        let pubkey = keypair.public_key();
        let (_, sender) = classified_inbox();

        let meta = PeerMeta::default()
            .with_description("Reviews code for style issues")
            .with_label("lang", "rust");

        registry.register_with_meta_in_namespace(
            DEFAULT_NAMESPACE,
            "reviewer",
            pubkey,
            sender,
            meta.clone(),
        );

        let peers = registry.peers();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].name, "reviewer");
        assert_eq!(peers[0].pubkey, pubkey);
        assert_eq!(peers[0].meta, meta);
    }

    #[cfg(feature = "workspace-tests")]
    #[test]
    fn test_registry_peers_returns_default_meta_for_plain_register() {
        let registry = InprocRegistry::new();
        let keypair = make_keypair();
        let pubkey = keypair.public_key();
        let (_, sender) = classified_inbox();

        registry.register("plain-agent", pubkey, sender);

        let peers = registry.peers();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].meta, PeerMeta::default());
    }
}

// Wake-route rows: they build classified inboxes with the workspace's test
// classification context, so they are workspace-only like the other
// classified-inbox rows above.
#[cfg(all(test, feature = "workspace-tests"))]
#[path = "inproc_wake_tests.rs"]
mod wake_route_tests;
