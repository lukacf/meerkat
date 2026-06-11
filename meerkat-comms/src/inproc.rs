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
//! let (inbox, sender) = Inbox::new();
//! InprocRegistry::global().register("my-agent", pubkey, sender);
//!
//! // Delivery is pubkey-keyed: the Router resolves a trusted peer's
//! // signing key and delivers through the namespace-scoped
//! // send_to_pubkey_*_wait owners.
//!
//! // Unregister when done
//! InprocRegistry::global().unregister(&pubkey);
//! ```

use std::collections::HashMap;
use std::sync::OnceLock;

use parking_lot::RwLock;
use uuid::Uuid;

use crate::identity::{Keypair, PubKey, Signature};
use crate::inbox::{AdmissionOutcome, DropReason, InboxSender};
use crate::peer_meta::PeerMeta;
use crate::types::{Envelope, InboxItem, MessageKind};

const DEFAULT_NAMESPACE: &str = "";

/// Snapshot of an inproc peer returned by [`InprocRegistry::peers()`].
#[derive(Debug, Clone)]
pub struct InprocPeerInfo {
    pub name: String,
    pub pubkey: PubKey,
    pub meta: PeerMeta,
}

/// Why a registration was rejected without mutating the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationRejection {
    /// The supplied pubkey was the all-zero key, which can never identify a
    /// distinct peer and is refused fail-closed.
    ZeroPubkey,
}

/// Typed result of registering an inproc peer.
///
/// Registration is not always a clean insert: re-registering an existing
/// pubkey under a new name evicts the old name mapping, and re-registering an
/// existing name with a new pubkey evicts the old pubkey entry. Both evictions
/// can also happen at once. Callers (runtime constructors, metadata refresh)
/// must observe these facts rather than assume a clean success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationOutcome {
    /// The peer was inserted without displacing any existing route.
    Registered,
    /// This pubkey was already registered under a different name; the old name
    /// mapping was removed and replaced with the new name.
    ReplacedPubkey { evicted_name: String },
    /// This name was already bound to a different pubkey; the old pubkey entry
    /// was evicted so the stale key is no longer reachable.
    EvictedName { evicted_pubkey: PubKey },
    /// Both evictions happened: this pubkey's old name was removed AND this
    /// name's old pubkey was evicted in the same registration.
    ReplacedPubkeyAndEvictedName {
        evicted_name: String,
        evicted_pubkey: PubKey,
    },
    /// The registration was refused without mutating the registry.
    Rejected { reason: RegistrationRejection },
}

impl RegistrationOutcome {
    /// Whether the registration displaced an existing route (either eviction).
    pub fn displaced_existing(&self) -> bool {
        matches!(
            self,
            Self::ReplacedPubkey { .. }
                | Self::EvictedName { .. }
                | Self::ReplacedPubkeyAndEvictedName { .. }
        )
    }

    /// Whether the registration was rejected (no mutation occurred).
    pub fn is_rejected(&self) -> bool {
        matches!(self, Self::Rejected { .. })
    }
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

/// Internal namespace state protected by a single lock to prevent deadlocks.
#[derive(Default)]
struct NamespaceState {
    /// Map from pubkey to peer entry.
    peers: HashMap<PubKey, InprocPeer>,
    /// Map from name to pubkey for name-based lookup.
    names: HashMap<String, PubKey>,
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
    /// Returns a typed [`RegistrationOutcome`] that surfaces route displacement
    /// explicitly: re-registering an existing pubkey under a new name evicts
    /// the old name mapping ([`RegistrationOutcome::ReplacedPubkey`]); a name
    /// rebound to a new pubkey evicts the old pubkey entry
    /// ([`RegistrationOutcome::EvictedName`]); both can happen at once
    /// ([`RegistrationOutcome::ReplacedPubkeyAndEvictedName`]). A zero pubkey is
    /// refused without mutation ([`RegistrationOutcome::Rejected`]). Callers
    /// must observe displacement/rejection rather than assume a clean success.
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

        // If this pubkey was registered under a different name, remove old name mapping
        let evicted_name = namespace_state
            .peers
            .get(&pubkey)
            .filter(|old_peer| old_peer.name != name)
            .map(|old_peer| old_peer.name.clone());
        if let Some(old_name) = &evicted_name {
            namespace_state.names.remove(old_name);
        }

        // If this name was registered to a different pubkey, remove the old pubkey entry
        // This prevents stale pubkeys from remaining reachable
        let evicted_pubkey = namespace_state
            .names
            .get(&name)
            .filter(|&&old_pk| old_pk != pubkey)
            .copied();
        if let Some(old_pubkey) = evicted_pubkey {
            namespace_state.peers.remove(&old_pubkey);
        }

        namespace_state.peers.insert(pubkey, peer);
        namespace_state.names.insert(name, pubkey);

        match (evicted_name, evicted_pubkey) {
            (None, None) => RegistrationOutcome::Registered,
            (Some(evicted_name), None) => RegistrationOutcome::ReplacedPubkey { evicted_name },
            (None, Some(evicted_pubkey)) => RegistrationOutcome::EvictedName { evicted_pubkey },
            (Some(evicted_name), Some(evicted_pubkey)) => {
                RegistrationOutcome::ReplacedPubkeyAndEvictedName {
                    evicted_name,
                    evicted_pubkey,
                }
            }
        }
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

    /// Backpressured pubkey-keyed delivery across all namespaces.
    ///
    /// Runtime-originated peer sends should await receiver capacity instead of
    /// turning a transient full inbox into semantic message loss.
    pub(crate) async fn send_to_pubkey_any_namespace_with_id_wait(
        &self,
        from_keypair: &Keypair,
        to_pubkey: &PubKey,
        envelope_id: Uuid,
        kind: MessageKind,
        sign_envelope: bool,
    ) -> Result<uuid::Uuid, InprocSendError> {
        let sender = self
            .get_by_pubkey_any_namespace(to_pubkey)
            .ok_or_else(|| InprocSendError::PeerNotFound(to_pubkey.to_peer_id().to_string()))?;

        Self::deliver_to_sender_wait(
            from_keypair,
            *to_pubkey,
            sender,
            envelope_id,
            kind,
            sign_envelope,
        )
        .await
    }

    /// Namespace-scoped variant of
    /// [`Self::send_to_pubkey_any_namespace_with_id_wait`]: the destination is
    /// resolved exactly once, *inside* `namespace`, and that resolved sender is
    /// the delivery target.
    ///
    /// This is the single-resolution send for namespace-isolated routers. The
    /// namespace is the delivery authority, so the destination must not be
    /// re-derived from the global registry between an isolation check and the
    /// inbox handoff — a second any-namespace lookup would open a window where
    /// the peer re-registers elsewhere and delivery crosses the namespace
    /// boundary.
    pub(crate) async fn send_to_pubkey_in_namespace_with_id_wait(
        &self,
        namespace: &str,
        from_keypair: &Keypair,
        to_pubkey: &PubKey,
        envelope_id: Uuid,
        kind: MessageKind,
        sign_envelope: bool,
    ) -> Result<uuid::Uuid, InprocSendError> {
        let sender = self
            .get_by_pubkey_in_namespace(namespace, to_pubkey)
            .ok_or_else(|| InprocSendError::PeerNotFound(to_pubkey.to_peer_id().to_string()))?;

        Self::deliver_to_sender_wait(
            from_keypair,
            *to_pubkey,
            sender,
            envelope_id,
            kind,
            sign_envelope,
        )
        .await
    }

    async fn deliver_to_sender_wait(
        from_keypair: &Keypair,
        to_pubkey: PubKey,
        sender: InboxSender,
        envelope_id: Uuid,
        kind: MessageKind,
        sign_envelope: bool,
    ) -> Result<uuid::Uuid, InprocSendError> {
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
        match sender.send_wait(InboxItem::External { envelope }).await {
            AdmissionOutcome::Admitted => {}
            AdmissionOutcome::Dropped {
                reason: DropReason::SessionClosed,
            } => return Err(InprocSendError::InboxClosed),
            AdmissionOutcome::Dropped {
                reason: DropReason::InboxFull,
            } => return Err(InprocSendError::InboxFull),
            AdmissionOutcome::Dropped { reason } => {
                return Err(InprocSendError::IngressDropped(reason));
            }
        }

        Ok(envelope_id)
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
    #[error("Inproc peer not found: {0}")]
    PeerNotFound(String),
    #[error("Peer inbox has been closed")]
    InboxClosed,
    #[error("Peer inbox is full")]
    InboxFull,
    #[error("Peer inbox dropped ingress: {0:?}")]
    IngressDropped(crate::inbox::DropReason),
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::classify::test_support;
    use crate::inbox::Inbox;
    use crate::trust::TrustStore;

    fn classified_inbox() -> (Inbox, crate::InboxSender) {
        Inbox::new_classified(test_support::classification_context(
            TrustStore::new(),
            false,
        ))
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

    #[test]
    fn test_registry_replace_on_same_name_different_pubkey() {
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
        registry.register("my-agent", pubkey2, sender2);

        // Old pubkey should be evicted, new pubkey should exist
        assert!(!registry.contains(&pubkey1), "old pubkey should be evicted");
        assert!(registry.contains(&pubkey2));
        assert!(registry.contains_name("my-agent"));
        assert_eq!(registry.len(), 1);

        // The name maps to the new identity.
        assert_eq!(
            registry.get_name_by_pubkey(&pubkey2).as_deref(),
            Some("my-agent")
        );
    }

    /// ROW #292 gate: registration returns a typed [`RegistrationOutcome`] that
    /// surfaces route displacement and zero-pubkey rejection, instead of
    /// silently evicting and returning `()`.
    #[test]
    fn registration_outcome_is_typed_for_displacement_and_rejection() {
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

        // Re-registering an existing NAME with a NEW pubkey evicts the old
        // pubkey and reports it typed.
        let other = make_keypair();
        let other_pubkey = other.public_key();
        match registry.register("agent-v2", other_pubkey, sender3) {
            RegistrationOutcome::EvictedName { evicted_pubkey } => {
                assert_eq!(evicted_pubkey, pubkey);
            }
            other => panic!("expected EvictedName, got {other:?}"),
        }

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

    /// Test that the ABA scenario is handled correctly:
    /// When a new agent registers with the same name, the old agent's
    /// unregister call (on Drop) should be a safe no-op.
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
        registry.register("agent", pubkey_old, sender_old);
        assert!(registry.contains(&pubkey_old));

        // Step 2: New runtime registers same name (evicts old)
        registry.register("agent", pubkey_new, sender_new);
        assert!(
            !registry.contains(&pubkey_old),
            "old pubkey should be evicted"
        );
        assert!(registry.contains(&pubkey_new));

        // Step 3: Old runtime drops and calls unregister(pubkey_old)
        // This should be a no-op since pubkey_old was already evicted
        let removed = registry.unregister(&pubkey_old);
        assert!(!removed, "unregister of evicted pubkey should return false");

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

    #[tokio::test]
    async fn test_registry_send_delivers_to_inbox() {
        let registry = InprocRegistry::new();

        // Set up receiver
        let receiver_keypair = make_keypair();
        let (mut inbox, sender) = classified_inbox();
        registry.register("receiver", receiver_keypair.public_key(), sender);

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
                    blocks: None,
                    body: "hello inproc".to_string(),
                    handling_mode: None,
                },
                true,
            )
            .await;
        assert!(result.is_ok());

        // Verify message was received
        let items = inbox.try_drain_classified();
        assert_eq!(items.len(), 1);

        match &items[0].item {
            InboxItem::External { envelope } => {
                assert_eq!(envelope.from, sender_keypair.public_key());
                assert_eq!(envelope.to, receiver_keypair.public_key());
                match &envelope.kind {
                    MessageKind::Message {
                        blocks: None, body, ..
                    } => {
                        assert_eq!(body, "hello inproc");
                    }
                    _ => panic!("expected Message kind"),
                }
                // Verify signature
                assert!(envelope.verify());
            }
            _ => panic!("expected External inbox item"),
        }
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
                    blocks: None,
                    body: "hello".to_string(),
                    handling_mode: None,
                },
                true,
            )
            .await;

        assert!(matches!(result, Err(InprocSendError::PeerNotFound(_))));
    }

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
                    blocks: None,
                    body: "hello".to_string(),
                    handling_mode: None,
                },
                true,
            )
            .await;

        assert!(matches!(result, Err(InprocSendError::InboxClosed)));
    }

    #[tokio::test]
    async fn test_registry_namespace_isolation_for_lookup_and_send() {
        let registry = InprocRegistry::new();
        let receiver_keypair = make_keypair();
        let (mut inbox, sender) = classified_inbox();
        registry.register_with_meta_in_namespace(
            "realm-a",
            "receiver",
            receiver_keypair.public_key(),
            sender,
            PeerMeta::default(),
        );

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
                    blocks: None,
                    body: "hello scoped".to_string(),
                    handling_mode: None,
                },
                true,
            )
            .await;
        assert!(ok.is_ok());

        // Different namespace cannot route to receiver.
        let wrong_ns = registry
            .send_to_pubkey_in_namespace_with_id_wait(
                "realm-b",
                &sender_keypair,
                &receiver_keypair.public_key(),
                Uuid::new_v4(),
                MessageKind::Message {
                    blocks: None,
                    body: "should not deliver".to_string(),
                    handling_mode: None,
                },
                true,
            )
            .await;
        assert!(matches!(wrong_ns, Err(InprocSendError::PeerNotFound(_))));

        let items = inbox.try_drain_classified();
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn test_send_to_pubkey_in_namespace_ignores_display_name_collision() {
        let registry = InprocRegistry::new();
        let target_keypair = make_keypair();
        let target_pubkey = target_keypair.public_key();
        let shadow_keypair = make_keypair();
        let shadow_pubkey = shadow_keypair.public_key();
        let (mut target_inbox, target_sender) = classified_inbox();
        let (mut shadow_inbox, shadow_sender) = classified_inbox();

        registry.register_with_meta_in_namespace(
            "",
            "canonical-target",
            target_pubkey,
            target_sender,
            PeerMeta::default(),
        );
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
                    blocks: None,
                    body: "hello canonical".to_string(),
                    handling_mode: None,
                },
                true,
            )
            .await;
        assert!(result.is_ok());

        assert_eq!(shadow_inbox.try_drain_classified().len(), 0);
        let items = target_inbox.try_drain_classified();
        assert_eq!(items.len(), 1);
        let InboxItem::External { envelope } = &items[0].item else {
            panic!("expected external envelope");
        };
        assert_eq!(envelope.to, target_pubkey);
    }

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
                    blocks: None,
                    body: "ambiguous identity".to_string(),
                    handling_mode: None,
                },
                true,
            )
            .await;

        assert!(matches!(result, Err(InprocSendError::PeerNotFound(_))));
        assert!(alpha_inbox.try_drain_classified().is_empty());
        assert!(beta_inbox.try_drain_classified().is_empty());
    }

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
