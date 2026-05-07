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
//! // Send to an inproc peer (via Router with inproc:// address)
//! router.send(
//!     "my-agent",
//!     MessageKind::Message {
//!         blocks: None,
//!         body: "hello".into(),
//!         handling_mode: None,
//!     },
//! ).await?;
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
    /// If an agent with the same pubkey already exists, it will be replaced.
    /// If an agent with the same name but different pubkey exists, the old
    /// agent will be evicted (both from peers and names maps).
    pub fn register(&self, name: impl Into<String>, pubkey: PubKey, sender: InboxSender) {
        self.register_with_meta_in_namespace(
            DEFAULT_NAMESPACE,
            name,
            pubkey,
            sender,
            PeerMeta::default(),
        );
    }

    /// Register an agent's inbox with associated [`PeerMeta`].
    pub fn register_with_meta(
        &self,
        name: impl Into<String>,
        pubkey: PubKey,
        sender: InboxSender,
        meta: PeerMeta,
    ) {
        self.register_with_meta_in_namespace(DEFAULT_NAMESPACE, name, pubkey, sender, meta);
    }

    /// Register an agent's inbox within an explicit namespace.
    pub fn register_with_meta_in_namespace(
        &self,
        namespace: &str,
        name: impl Into<String>,
        pubkey: PubKey,
        sender: InboxSender,
        meta: PeerMeta,
    ) {
        let name = name.into();
        if pubkey.is_zero() {
            tracing::warn!(
                inproc_namespace = %namespace,
                peer_name = %name,
                "rejecting zero-pubkey inproc registration"
            );
            return;
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
        let old_name_to_remove = namespace_state
            .peers
            .get(&pubkey)
            .filter(|old_peer| old_peer.name != name)
            .map(|old_peer| old_peer.name.clone());
        if let Some(old_name) = old_name_to_remove {
            namespace_state.names.remove(&old_name);
        }

        // If this name was registered to a different pubkey, remove the old pubkey entry
        // This prevents stale pubkeys from remaining reachable
        let old_pubkey_to_remove = namespace_state
            .names
            .get(&name)
            .filter(|&&old_pk| old_pk != pubkey)
            .copied();
        if let Some(old_pubkey) = old_pubkey_to_remove {
            namespace_state.peers.remove(&old_pubkey);
        }

        namespace_state.peers.insert(pubkey, peer);
        namespace_state.names.insert(name, pubkey);
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

    /// Look up an inproc peer by name.
    pub fn get_by_name(&self, name: &str) -> Option<(PubKey, InboxSender)> {
        self.get_by_name_in_namespace(DEFAULT_NAMESPACE, name)
    }

    /// Look up an inproc peer by name in an explicit namespace.
    pub fn get_by_name_in_namespace(
        &self,
        namespace: &str,
        name: &str,
    ) -> Option<(PubKey, InboxSender)> {
        let state = self.state.read();
        let namespace_state = state.namespace(namespace)?;
        let pubkey = namespace_state.names.get(name).copied()?;
        let peer = namespace_state.peers.get(&pubkey)?;
        Some((peer.pubkey, peer.sender.clone()))
    }

    /// Look up an inproc peer by name AND pubkey across ALL namespaces.
    ///
    /// Constrains the match by pubkey to avoid ambiguity when multiple
    /// namespaces contain peers with the same name but different keys.
    pub fn get_by_name_and_pubkey_any_namespace(
        &self,
        name: &str,
        expected_pubkey: &PubKey,
    ) -> Option<(PubKey, InboxSender)> {
        if expected_pubkey.is_zero() {
            return None;
        }
        let state = self.state.read();
        let mut found = None;
        let mut pubkey_registrations = 0;
        for namespace_state in state.namespaces.values() {
            if namespace_state.peers.contains_key(expected_pubkey) {
                pubkey_registrations += 1;
            }
            if let Some(&pubkey) = namespace_state.names.get(name)
                && pubkey == *expected_pubkey
                && let Some(peer) = namespace_state.peers.get(&pubkey)
            {
                if found.is_some() {
                    return None;
                }
                found = Some((peer.pubkey, peer.sender.clone()));
            }
        }
        if pubkey_registrations == 1 {
            found
        } else {
            None
        }
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

    /// Count live registrations for a canonical pubkey across all namespaces.
    pub(crate) fn pubkey_registration_count_any_namespace(&self, pubkey: &PubKey) -> usize {
        if pubkey.is_zero() {
            return 0;
        }
        self.state
            .read()
            .namespaces
            .values()
            .filter(|namespace_state| namespace_state.peers.contains_key(pubkey))
            .count()
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

    /// Send a message directly to an inproc peer.
    ///
    /// This bypasses network transport entirely, delivering the envelope
    /// directly to the peer's inbox.
    ///
    /// Returns the generated envelope ID when the message was delivered, or
    /// an error if:
    /// - The peer is not found in the registry
    /// - The peer's inbox has been closed
    pub fn send(
        &self,
        from_keypair: &Keypair,
        to_name: &str,
        kind: MessageKind,
    ) -> Result<uuid::Uuid, InprocSendError> {
        self.send_with_signature_in_namespace(DEFAULT_NAMESPACE, from_keypair, to_name, kind, true)
    }

    /// Send a message directly to an inproc peer.
    ///
    /// If `sign_envelope` is false, the envelope is sent unsigned.
    pub fn send_with_signature(
        &self,
        from_keypair: &Keypair,
        to_name: &str,
        kind: MessageKind,
        sign_envelope: bool,
    ) -> Result<uuid::Uuid, InprocSendError> {
        self.send_with_signature_in_namespace(
            DEFAULT_NAMESPACE,
            from_keypair,
            to_name,
            kind,
            sign_envelope,
        )
    }

    /// Send a message to an inproc peer, searching ALL namespaces.
    ///
    /// Used as a fallback for cross-mob communication when the recipient is
    /// in a different namespace (realm) than the sender. The lookup is
    /// constrained by `expected_pubkey` to prevent misdelivery when multiple
    /// namespaces contain peers with the same name but different identities.
    pub fn send_cross_namespace(
        &self,
        from_keypair: &Keypair,
        to_name: &str,
        expected_pubkey: &PubKey,
        kind: MessageKind,
        sign_envelope: bool,
    ) -> Result<uuid::Uuid, InprocSendError> {
        self.send_cross_namespace_with_id(
            from_keypair,
            to_name,
            expected_pubkey,
            Uuid::new_v4(),
            kind,
            sign_envelope,
        )
    }

    /// Send a message to an inproc peer across namespaces using a caller-chosen envelope id.
    pub fn send_cross_namespace_with_id(
        &self,
        from_keypair: &Keypair,
        to_name: &str,
        expected_pubkey: &PubKey,
        envelope_id: Uuid,
        kind: MessageKind,
        sign_envelope: bool,
    ) -> Result<uuid::Uuid, InprocSendError> {
        let (to_pubkey, sender) = self
            .get_by_name_and_pubkey_any_namespace(to_name, expected_pubkey)
            .ok_or_else(|| InprocSendError::PeerNotFound(to_name.to_string()))?;

        Self::deliver_to_sender(
            from_keypair,
            to_pubkey,
            sender,
            envelope_id,
            kind,
            sign_envelope,
        )
    }

    /// Send a message to an inproc peer by pubkey, searching ALL namespaces.
    ///
    /// Router call sites already resolved the destination from canonical trust
    /// state, so delivery must use that identity rather than a display name.
    pub(crate) fn send_to_pubkey_any_namespace_with_id(
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

        Self::deliver_to_sender(
            from_keypair,
            *to_pubkey,
            sender,
            envelope_id,
            kind,
            sign_envelope,
        )
    }

    /// Send a message directly to an inproc peer within a namespace.
    pub fn send_with_signature_in_namespace(
        &self,
        namespace: &str,
        from_keypair: &Keypair,
        to_name: &str,
        kind: MessageKind,
        sign_envelope: bool,
    ) -> Result<uuid::Uuid, InprocSendError> {
        self.send_with_signature_in_namespace_with_id(
            namespace,
            from_keypair,
            to_name,
            Uuid::new_v4(),
            kind,
            sign_envelope,
        )
    }

    /// Send a message directly to an inproc peer within a namespace using a caller-chosen envelope id.
    pub fn send_with_signature_in_namespace_with_id(
        &self,
        namespace: &str,
        from_keypair: &Keypair,
        to_name: &str,
        envelope_id: Uuid,
        kind: MessageKind,
        sign_envelope: bool,
    ) -> Result<uuid::Uuid, InprocSendError> {
        // Look up the peer
        let (to_pubkey, sender) = self
            .get_by_name_in_namespace(namespace, to_name)
            .ok_or_else(|| InprocSendError::PeerNotFound(to_name.to_string()))?;
        if to_pubkey.is_zero() {
            return Err(InprocSendError::PeerNotFound(to_name.to_string()));
        }

        Self::deliver_to_sender(
            from_keypair,
            to_pubkey,
            sender,
            envelope_id,
            kind,
            sign_envelope,
        )
    }

    /// Send a message directly to an inproc peer within a namespace by pubkey.
    #[cfg(test)]
    fn send_to_pubkey_in_namespace_with_id(
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

        Self::deliver_to_sender(
            from_keypair,
            *to_pubkey,
            sender,
            envelope_id,
            kind,
            sign_envelope,
        )
    }

    fn deliver_to_sender(
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

        // Deliver directly to inbox
        let envelope_id = envelope.id;
        match sender.send_classified(InboxItem::External { envelope }) {
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

    /// List all registered peer names.
    pub fn peer_names(&self) -> Vec<String> {
        self.peer_names_in_namespace(DEFAULT_NAMESPACE)
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
    use crate::inbox::Inbox;

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
        let (_, sender) = Inbox::new();

        registry.register("test-agent", pubkey, sender);

        assert!(!registry.is_empty());
        assert_eq!(registry.len(), 1);
        assert!(registry.contains(&pubkey));
        assert!(registry.contains_name("test-agent"));

        // Lookup by name
        let (found_pubkey, _) = registry.get_by_name("test-agent").unwrap();
        assert_eq!(found_pubkey, pubkey);

        // Lookup by pubkey
        assert!(registry.get_by_pubkey(&pubkey).is_some());
    }

    #[test]
    fn test_registry_rejects_zero_pubkey_registration() {
        let registry = InprocRegistry::new();
        let (_, sender) = Inbox::new();
        let zero_pubkey = PubKey::new([0u8; 32]);

        registry.register("zero-agent", zero_pubkey, sender);

        assert!(registry.is_empty());
        assert!(!registry.contains_name("zero-agent"));
        assert!(registry.get_by_name("zero-agent").is_none());
        assert!(registry.get_by_pubkey(&zero_pubkey).is_none());
    }

    #[test]
    fn test_registry_zero_pubkey_registration_does_not_shadow_valid_name() {
        let registry = InprocRegistry::new();
        let valid_keypair = make_keypair();
        let valid_pubkey = valid_keypair.public_key();
        let (_, valid_sender) = Inbox::new();
        let (_, zero_sender) = Inbox::new();
        let zero_pubkey = PubKey::new([0u8; 32]);

        registry.register("stable-agent", valid_pubkey, valid_sender);
        registry.register("stable-agent", zero_pubkey, zero_sender);

        assert_eq!(registry.len(), 1);
        assert!(registry.contains(&valid_pubkey));
        assert!(registry.contains_name("stable-agent"));
        assert!(registry.get_by_pubkey(&valid_pubkey).is_some());
        assert!(registry.get_by_pubkey(&zero_pubkey).is_none());

        let (found_pubkey, _) = registry
            .get_by_name("stable-agent")
            .expect("valid name mapping should remain");
        assert_eq!(found_pubkey, valid_pubkey);
    }

    #[test]
    fn test_registry_unregister() {
        let registry = InprocRegistry::new();
        let keypair = make_keypair();
        let pubkey = keypair.public_key();
        let (_, sender) = Inbox::new();

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
        let (_, sender1) = Inbox::new();
        let (_, sender2) = Inbox::new();

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
        let (_, sender1) = Inbox::new();
        let (_, sender2) = Inbox::new();

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

        // Lookup should return the new pubkey
        let (found_pubkey, _) = registry.get_by_name("my-agent").unwrap();
        assert_eq!(found_pubkey, pubkey2);
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
        let (_, sender_old) = Inbox::new();
        let (_, sender_new) = Inbox::new();

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

        // Verify the correct pubkey is returned for lookup
        let (found_pubkey, _) = registry.get_by_name("agent").unwrap();
        assert_eq!(found_pubkey, pubkey_new, "lookup should return new pubkey");
    }

    #[test]
    fn test_registry_peer_names() {
        let registry = InprocRegistry::new();

        for i in 0..3 {
            let keypair = make_keypair();
            let (_, sender) = Inbox::new();
            registry.register(format!("agent-{i}"), keypair.public_key(), sender);
        }

        let names = registry.peer_names();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"agent-0".to_string()));
        assert!(names.contains(&"agent-1".to_string()));
        assert!(names.contains(&"agent-2".to_string()));
    }

    #[test]
    fn test_registry_peers_snapshot() {
        let registry = InprocRegistry::new();
        let keypair = make_keypair();
        let pubkey = keypair.public_key();
        let (_, sender) = Inbox::new();
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
            let (_, sender) = Inbox::new();
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
        let (mut inbox, sender) = Inbox::new();
        registry.register("receiver", receiver_keypair.public_key(), sender);

        // Set up sender
        let sender_keypair = make_keypair();

        // Send a message
        let result = registry.send(
            &sender_keypair,
            "receiver",
            MessageKind::Message {
                blocks: None,
                body: "hello inproc".to_string(),
                handling_mode: None,
            },
        );
        assert!(result.is_ok());

        // Verify message was received
        let items = inbox.try_drain();
        assert_eq!(items.len(), 1);

        match &items[0] {
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

    #[test]
    fn test_registry_send_peer_not_found() {
        let registry = InprocRegistry::new();
        let sender_keypair = make_keypair();

        let result = registry.send(
            &sender_keypair,
            "nonexistent",
            MessageKind::Message {
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
            },
        );

        assert!(matches!(result, Err(InprocSendError::PeerNotFound(_))));
    }

    #[test]
    fn test_registry_send_inbox_closed() {
        let registry = InprocRegistry::new();

        // Set up receiver but drop the inbox
        let receiver_keypair = make_keypair();
        let (inbox, sender) = Inbox::new();
        registry.register("receiver", receiver_keypair.public_key(), sender);
        drop(inbox); // Close the inbox

        let sender_keypair = make_keypair();

        let result = registry.send(
            &sender_keypair,
            "receiver",
            MessageKind::Message {
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
            },
        );

        assert!(matches!(result, Err(InprocSendError::InboxClosed)));
    }

    #[tokio::test]
    async fn test_registry_namespace_isolation_for_lookup_and_send() {
        let registry = InprocRegistry::new();
        let receiver_keypair = make_keypair();
        let (mut inbox, sender) = Inbox::new();
        registry.register_with_meta_in_namespace(
            "realm-a",
            "receiver",
            receiver_keypair.public_key(),
            sender,
            PeerMeta::default(),
        );

        // Default namespace cannot see realm-a registrations.
        assert!(registry.get_by_name("receiver").is_none());
        assert!(
            registry
                .get_by_name_in_namespace("realm-a", "receiver")
                .is_some()
        );

        let sender_keypair = make_keypair();

        // Matching namespace succeeds.
        let ok = registry.send_with_signature_in_namespace(
            "realm-a",
            &sender_keypair,
            "receiver",
            MessageKind::Message {
                blocks: None,
                body: "hello scoped".to_string(),
                handling_mode: None,
            },
            true,
        );
        assert!(ok.is_ok());

        // Different namespace cannot route to receiver.
        let wrong_ns = registry.send_with_signature_in_namespace(
            "realm-b",
            &sender_keypair,
            "receiver",
            MessageKind::Message {
                blocks: None,
                body: "should not deliver".to_string(),
                handling_mode: None,
            },
            true,
        );
        assert!(matches!(wrong_ns, Err(InprocSendError::PeerNotFound(_))));

        let items = inbox.try_drain();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn test_send_to_pubkey_in_namespace_ignores_display_name_collision() {
        let registry = InprocRegistry::new();
        let target_keypair = make_keypair();
        let target_pubkey = target_keypair.public_key();
        let shadow_keypair = make_keypair();
        let shadow_pubkey = shadow_keypair.public_key();
        let (mut target_inbox, target_sender) = Inbox::new();
        let (mut shadow_inbox, shadow_sender) = Inbox::new();

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
        let result = registry.send_to_pubkey_in_namespace_with_id(
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
        );
        assert!(result.is_ok());

        assert_eq!(shadow_inbox.try_drain().len(), 0);
        let items = target_inbox.try_drain();
        assert_eq!(items.len(), 1);
        let InboxItem::External { envelope } = &items[0] else {
            panic!("expected external envelope");
        };
        assert_eq!(envelope.to, target_pubkey);
    }

    #[test]
    fn test_send_to_pubkey_any_namespace_rejects_ambiguous_identity() {
        let registry = InprocRegistry::new();
        let sender_keypair = make_keypair();
        let target_keypair = make_keypair();
        let target_pubkey = target_keypair.public_key();
        let (mut alpha_inbox, alpha_sender) = Inbox::new();
        let (mut beta_inbox, beta_sender) = Inbox::new();

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

        let result = registry.send_to_pubkey_any_namespace_with_id(
            &sender_keypair,
            &target_pubkey,
            Uuid::new_v4(),
            MessageKind::Message {
                blocks: None,
                body: "ambiguous identity".to_string(),
                handling_mode: None,
            },
            true,
        );

        assert!(matches!(result, Err(InprocSendError::PeerNotFound(_))));
        assert!(alpha_inbox.try_drain().is_empty());
        assert!(beta_inbox.try_drain().is_empty());
    }

    #[test]
    fn test_registry_same_name_can_exist_in_different_namespaces() {
        let registry = InprocRegistry::new();
        let kp_a = make_keypair();
        let kp_b = make_keypair();
        let (_, sender_a) = Inbox::new();
        let (_, sender_b) = Inbox::new();

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

        let (found_a, _) = registry
            .get_by_name_in_namespace("realm-a", "shared-name")
            .expect("realm-a peer should exist");
        let (found_b, _) = registry
            .get_by_name_in_namespace("realm-b", "shared-name")
            .expect("realm-b peer should exist");
        assert_ne!(found_a, found_b);
        assert!(registry.get_by_name("shared-name").is_none());
    }

    #[test]
    fn test_global_registry() {
        // Access global registry
        let registry = InprocRegistry::global();

        // Clear any existing state (from other tests)
        registry.clear();

        // Register a peer
        let keypair = make_keypair();
        let (_, sender) = Inbox::new();
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
        let (_, sender) = Inbox::new();

        let meta = PeerMeta::default()
            .with_description("Reviews code for style issues")
            .with_label("lang", "rust");

        registry.register_with_meta("reviewer", pubkey, sender, meta.clone());

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
        let (_, sender) = Inbox::new();

        registry.register("plain-agent", pubkey, sender);

        let peers = registry.peers();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].meta, PeerMeta::default());
    }

    /// Regression: send_cross_namespace must reject delivery when the resolved
    /// peer's pubkey doesn't match the expected pubkey. Without the pubkey
    /// guard, a name collision across namespaces can misdeliver messages.
    #[test]
    fn test_send_cross_namespace_rejects_pubkey_mismatch() {
        let registry = InprocRegistry::new();
        let sender_kp = make_keypair();

        // Register "ambassador" in namespace "mob:alpha" with key A
        let key_a = make_keypair();
        let (_inbox_a, sender_a) = Inbox::new();
        registry.register_with_meta_in_namespace(
            "mob:alpha",
            "ambassador",
            key_a.public_key(),
            sender_a,
            PeerMeta::default(),
        );

        // Register "ambassador" (same name!) in namespace "mob:beta" with key B
        let key_b = make_keypair();
        let (_inbox_b, sender_b) = Inbox::new();
        registry.register_with_meta_in_namespace(
            "mob:beta",
            "ambassador",
            key_b.public_key(),
            sender_b,
            PeerMeta::default(),
        );

        // Send cross-namespace expecting key A → should succeed (matches alpha)
        let result_a = registry.send_cross_namespace(
            &sender_kp,
            "ambassador",
            &key_a.public_key(),
            MessageKind::Request {
                intent: "test".into(),
                params: serde_json::json!({}),
                blocks: None,
                handling_mode: None,
            },
            false,
        );
        assert!(result_a.is_ok(), "should deliver when pubkey matches");

        // Send cross-namespace with an unrelated key that doesn't match
        // either registered "ambassador". The pubkey guard must reject it.
        let unrelated_key = make_keypair();
        let result_mismatch = registry.send_cross_namespace(
            &sender_kp,
            "ambassador",
            &unrelated_key.public_key(),
            MessageKind::Request {
                intent: "test".into(),
                params: serde_json::json!({}),
                blocks: None,
                handling_mode: None,
            },
            false,
        );
        assert!(
            matches!(result_mismatch, Err(InprocSendError::PeerNotFound(_))),
            "must reject when resolved pubkey doesn't match expected: {result_mismatch:?}"
        );
    }

    #[test]
    fn test_send_cross_namespace_rejects_duplicate_name_pubkey_across_namespaces() {
        let registry = InprocRegistry::new();
        let sender_kp = make_keypair();
        let target_key = make_keypair();
        let target_pubkey = target_key.public_key();
        let (mut inbox_a, sender_a) = Inbox::new();
        let (mut inbox_b, sender_b) = Inbox::new();

        registry.register_with_meta_in_namespace(
            "mob:alpha",
            "ambassador",
            target_pubkey,
            sender_a,
            PeerMeta::default(),
        );
        registry.register_with_meta_in_namespace(
            "mob:beta",
            "ambassador",
            target_pubkey,
            sender_b,
            PeerMeta::default(),
        );

        let result = registry.send_cross_namespace(
            &sender_kp,
            "ambassador",
            &target_pubkey,
            MessageKind::Request {
                intent: "test".into(),
                params: serde_json::json!({}),
                blocks: None,
                handling_mode: None,
            },
            false,
        );

        assert!(
            matches!(result, Err(InprocSendError::PeerNotFound(_))),
            "same name/pubkey across namespaces must fail closed instead of picking a namespace: {result:?}"
        );
        assert!(
            inbox_a.try_drain().is_empty(),
            "must not deliver to the first duplicate namespace"
        );
        assert!(
            inbox_b.try_drain().is_empty(),
            "must not deliver to the second duplicate namespace"
        );
    }

    #[test]
    fn test_send_cross_namespace_rejects_duplicate_pubkey_with_different_names_across_namespaces() {
        let registry = InprocRegistry::new();
        let sender_kp = make_keypair();
        let target_key = make_keypair();
        let target_pubkey = target_key.public_key();
        let (mut alpha_inbox, alpha_sender) = Inbox::new();
        let (mut beta_inbox, beta_sender) = Inbox::new();

        registry.register_with_meta_in_namespace(
            "mob:alpha",
            "ambassador",
            target_pubkey,
            alpha_sender,
            PeerMeta::default(),
        );
        registry.register_with_meta_in_namespace(
            "mob:beta",
            "observer",
            target_pubkey,
            beta_sender,
            PeerMeta::default(),
        );

        let result = registry.send_cross_namespace(
            &sender_kp,
            "ambassador",
            &target_pubkey,
            MessageKind::Request {
                intent: "test".into(),
                params: serde_json::json!({}),
                blocks: None,
                handling_mode: None,
            },
            false,
        );

        assert!(
            matches!(result, Err(InprocSendError::PeerNotFound(_))),
            "duplicate pubkey across namespaces must fail closed even when display names differ: {result:?}"
        );
        assert!(
            alpha_inbox.try_drain().is_empty(),
            "must not deliver to the named namespace when canonical pubkey is ambiguous"
        );
        assert!(
            beta_inbox.try_drain().is_empty(),
            "must not deliver to the differently named duplicate namespace"
        );
    }

    #[test]
    fn test_send_cross_namespace_with_id_rejects_duplicate_pubkey_different_names() {
        let registry = InprocRegistry::new();
        let sender_kp = make_keypair();
        let target_key = make_keypair();
        let target_pubkey = target_key.public_key();
        let (mut alpha_inbox, alpha_sender) = Inbox::new();
        let (mut beta_inbox, beta_sender) = Inbox::new();
        let envelope_id = Uuid::new_v4();

        registry.register_with_meta_in_namespace(
            "mob:alpha",
            "ambassador",
            target_pubkey,
            alpha_sender,
            PeerMeta::default(),
        );
        registry.register_with_meta_in_namespace(
            "mob:beta",
            "observer",
            target_pubkey,
            beta_sender,
            PeerMeta::default(),
        );

        let result = registry.send_cross_namespace_with_id(
            &sender_kp,
            "ambassador",
            &target_pubkey,
            envelope_id,
            MessageKind::Request {
                intent: "test".into(),
                params: serde_json::json!({}),
                blocks: None,
                handling_mode: None,
            },
            false,
        );

        assert!(
            matches!(result, Err(InprocSendError::PeerNotFound(_))),
            "duplicate pubkey across namespaces must fail closed on send_cross_namespace_with_id: {result:?}"
        );
        assert!(
            alpha_inbox.try_drain().is_empty(),
            "must not deliver the caller-chosen envelope id to the named namespace"
        );
        assert!(
            beta_inbox.try_drain().is_empty(),
            "must not deliver the caller-chosen envelope id to the differently named duplicate namespace"
        );
    }
}
