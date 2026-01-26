//! In-process message transport for sub-agent communication.
//!
//! This module provides a process-global registry that allows agents within
//! the same process to communicate without network sockets. Messages are
//! delivered directly via in-memory channels.
//!
//! # Usage
//!
//! ```ignore
//! // Register an agent's inbox
//! let (inbox, sender) = Inbox::new();
//! InprocRegistry::global().register("my-agent", pubkey, sender);
//!
//! // Send to an inproc peer (via Router with inproc:// address)
//! router.send("my-agent", MessageKind::Message { body: "hello".into() }).await?;
//!
//! // Unregister when done
//! InprocRegistry::global().unregister(&pubkey);
//! ```

use std::collections::HashMap;
use std::sync::OnceLock;

use parking_lot::RwLock;
use uuid::Uuid;

use crate::identity::{Keypair, PubKey, Signature};
use crate::inbox::InboxSender;
use crate::types::{Envelope, InboxItem, MessageKind};

/// Global inproc registry instance.
static GLOBAL_REGISTRY: OnceLock<InprocRegistry> = OnceLock::new();

/// Registry entry for an inproc peer.
#[derive(Clone)]
struct InprocPeer {
    name: String,
    pubkey: PubKey,
    sender: InboxSender,
}

/// Internal state protected by a single lock to prevent deadlocks.
struct RegistryState {
    /// Map from pubkey to peer entry.
    peers: HashMap<PubKey, InprocPeer>,
    /// Map from name to pubkey for name-based lookup.
    names: HashMap<String, PubKey>,
}

impl RegistryState {
    fn new() -> Self {
        Self {
            peers: HashMap::new(),
            names: HashMap::new(),
        }
    }
}

/// Process-global registry for in-process agent communication.
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
            state: RwLock::new(RegistryState::new()),
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
        let name = name.into();
        let peer = InprocPeer {
            name: name.clone(),
            pubkey,
            sender,
        };

        let mut state = self.state.write();

        // If this pubkey was registered under a different name, remove old name mapping
        let old_name_to_remove = state
            .peers
            .get(&pubkey)
            .filter(|old_peer| old_peer.name != name)
            .map(|old_peer| old_peer.name.clone());
        if let Some(old_name) = old_name_to_remove {
            state.names.remove(&old_name);
        }

        // If this name was registered to a different pubkey, remove the old pubkey entry
        // This prevents stale pubkeys from remaining reachable
        let old_pubkey_to_remove = state
            .names
            .get(&name)
            .filter(|&&old_pk| old_pk != pubkey)
            .copied();
        if let Some(old_pubkey) = old_pubkey_to_remove {
            state.peers.remove(&old_pubkey);
        }

        state.peers.insert(pubkey, peer);
        state.names.insert(name, pubkey);
    }

    /// Unregister an agent by pubkey.
    ///
    /// Returns true if the agent was found and removed.
    pub fn unregister(&self, pubkey: &PubKey) -> bool {
        let mut state = self.state.write();
        if let Some(peer) = state.peers.remove(pubkey) {
            state.names.remove(&peer.name);
            true
        } else {
            false
        }
    }

    /// Look up an inproc peer by name.
    pub fn get_by_name(&self, name: &str) -> Option<(PubKey, InboxSender)> {
        let state = self.state.read();
        let pubkey = state.names.get(name).copied()?;
        let peer = state.peers.get(&pubkey)?;
        Some((peer.pubkey, peer.sender.clone()))
    }

    /// Look up an inproc peer by pubkey.
    pub fn get_by_pubkey(&self, pubkey: &PubKey) -> Option<InboxSender> {
        self.state
            .read()
            .peers
            .get(pubkey)
            .map(|p| p.sender.clone())
    }

    /// Check if a peer is registered.
    pub fn contains(&self, pubkey: &PubKey) -> bool {
        self.state.read().peers.contains_key(pubkey)
    }

    /// Check if a peer name is registered.
    pub fn contains_name(&self, name: &str) -> bool {
        self.state.read().names.contains_key(name)
    }

    /// Get the number of registered peers.
    pub fn len(&self) -> usize {
        self.state.read().peers.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.state.read().peers.is_empty()
    }

    /// Clear all registrations (primarily for testing).
    pub fn clear(&self) {
        let mut state = self.state.write();
        state.peers.clear();
        state.names.clear();
    }

    /// Send a message directly to an inproc peer.
    ///
    /// This bypasses network transport entirely, delivering the envelope
    /// directly to the peer's inbox.
    ///
    /// Returns Ok(()) if the message was delivered, or an error if:
    /// - The peer is not found in the registry
    /// - The peer's inbox has been closed
    pub fn send(
        &self,
        from_keypair: &Keypair,
        to_name: &str,
        kind: MessageKind,
    ) -> Result<(), InprocSendError> {
        // Look up the peer
        let (to_pubkey, sender) = self
            .get_by_name(to_name)
            .ok_or_else(|| InprocSendError::PeerNotFound(to_name.to_string()))?;

        // Create and sign the envelope
        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: from_keypair.public_key(),
            to: to_pubkey,
            kind,
            sig: Signature::new([0u8; 64]),
        };
        envelope.sign(from_keypair);

        // Deliver directly to inbox
        sender
            .send(InboxItem::External { envelope })
            .map_err(|_| InprocSendError::InboxClosed)?;

        Ok(())
    }

    /// List all registered peer names.
    pub fn peer_names(&self) -> Vec<String> {
        self.state.read().names.keys().cloned().collect()
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
}

#[cfg(test)]
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
            registry.register(format!("agent-{}", i), keypair.public_key(), sender);
        }

        let names = registry.peer_names();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"agent-0".to_string()));
        assert!(names.contains(&"agent-1".to_string()));
        assert!(names.contains(&"agent-2".to_string()));
    }

    #[test]
    fn test_registry_clear() {
        let registry = InprocRegistry::new();

        for i in 0..3 {
            let keypair = make_keypair();
            let (_, sender) = Inbox::new();
            registry.register(format!("agent-{}", i), keypair.public_key(), sender);
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
                body: "hello inproc".to_string(),
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
                    MessageKind::Message { body } => {
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
                body: "hello".to_string(),
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
                body: "hello".to_string(),
            },
        );

        assert!(matches!(result, Err(InprocSendError::InboxClosed)));
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
}
