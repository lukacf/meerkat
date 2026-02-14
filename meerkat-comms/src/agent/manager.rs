//! CommsManager - Central manager for agent communication.
//!
//! The CommsManager holds the keypair, trusted peers, inbox, and router,
//! providing a unified interface for comms operations.

use std::sync::Arc;

use crate::{CommsConfig, Inbox, InboxSender, Keypair, Router, TrustedPeers};

use super::types::CommsMessage;

/// Configuration for CommsManager.
///
/// Note: This struct takes ownership of the keypair since Keypair
/// intentionally doesn't implement Clone for security reasons.
pub struct CommsManagerConfig {
    /// Our keypair for signing messages.
    pub keypair: Keypair,
    /// List of trusted peers.
    pub trusted_peers: TrustedPeers,
    /// Comms configuration (timeouts, limits).
    pub comms_config: CommsConfig,
}

impl CommsManagerConfig {
    /// Create a new config with generated keypair and empty trusted peers.
    pub fn new() -> Self {
        Self {
            keypair: Keypair::generate(),
            trusted_peers: TrustedPeers::new(),
            comms_config: CommsConfig::default(),
        }
    }

    /// Create a new config with the given keypair.
    pub fn with_keypair(keypair: Keypair) -> Self {
        Self {
            keypair,
            trusted_peers: TrustedPeers::new(),
            comms_config: CommsConfig::default(),
        }
    }

    /// Set the trusted peers.
    pub fn trusted_peers(mut self, trusted_peers: TrustedPeers) -> Self {
        self.trusted_peers = trusted_peers;
        self
    }

    /// Set the comms config.
    pub fn comms_config(mut self, config: CommsConfig) -> Self {
        self.comms_config = config;
        self
    }
}

impl Default for CommsManagerConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Central manager for agent communication.
///
/// Holds all the components needed for inter-agent communication:
/// - Keypair for signing
/// - Trusted peers for peer resolution
/// - Inbox for receiving messages
/// - Router for sending messages
pub struct CommsManager {
    /// Our keypair for signing messages.
    keypair: Arc<Keypair>,
    /// List of trusted peers.
    trusted_peers: Arc<TrustedPeers>,
    /// The inbox for receiving messages.
    inbox: Inbox,
    /// The inbox sender (for listeners to use).
    inbox_sender: InboxSender,
    /// The router for sending messages.
    router: Arc<Router>,
}

impl CommsManager {
    /// Create a new CommsManager with the given configuration.
    pub fn new(config: CommsManagerConfig) -> std::io::Result<Self> {
        let (inbox, inbox_sender) = Inbox::new();
        let trusted_peers = Arc::new(config.trusted_peers.clone());

        let router = Router::new(
            config.keypair,
            config.trusted_peers,
            config.comms_config,
            inbox_sender.clone(),
            true,
        );
        let keypair = router.keypair_arc();
        let router = Arc::new(router);

        Ok(Self {
            keypair,
            trusted_peers,
            inbox,
            inbox_sender,
            router,
        })
    }

    pub fn keypair(&self) -> &Keypair {
        self.keypair.as_ref()
    }

    pub fn keypair_arc(&self) -> Arc<Keypair> {
        self.keypair.clone()
    }

    /// Get the public key.
    pub fn public_key(&self) -> crate::PubKey {
        self.keypair.public_key()
    }

    /// Get the trusted peers.
    pub fn trusted_peers(&self) -> &Arc<TrustedPeers> {
        &self.trusted_peers
    }

    /// Get the inbox sender (for listeners to use).
    pub fn inbox_sender(&self) -> &InboxSender {
        &self.inbox_sender
    }

    /// Get the router for sending messages.
    pub fn router(&self) -> &Arc<Router> {
        &self.router
    }

    /// Drain all available messages from the inbox.
    ///
    /// This is a non-blocking operation that returns all currently
    /// available messages, converting them to CommsMessage format.
    ///
    /// Messages that cannot be converted (unknown peers, acks) are dropped.
    pub fn drain_messages(&mut self) -> Vec<CommsMessage> {
        let items = self.inbox.try_drain();
        items
            .iter()
            .filter_map(|item| CommsMessage::from_inbox_item(item, &self.trusted_peers, true))
            .collect()
    }

    /// Wait for the next message from the inbox.
    ///
    /// This blocks until a message is available, then returns it.
    /// Returns `None` if the inbox is closed.
    ///
    /// Note: This will skip acks and messages from unknown peers.
    pub async fn recv_message(&mut self) -> Option<CommsMessage> {
        loop {
            let item = self.inbox.recv().await?;
            if let Some(msg) = CommsMessage::from_inbox_item(&item, &self.trusted_peers, true) {
                return Some(msg);
            }
            // Skip acks and unknown peers, continue waiting
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::{Envelope, InboxItem, MessageKind, Signature, TrustedPeer};
    use uuid::Uuid;

    fn make_keypair() -> Keypair {
        Keypair::generate()
    }

    fn make_trusted_peers(name: &str, pubkey: &crate::PubKey) -> TrustedPeers {
        TrustedPeers {
            peers: vec![TrustedPeer {
                name: name.to_string(),
                pubkey: *pubkey,
                addr: "tcp://127.0.0.1:4200".to_string(),
                meta: crate::PeerMeta::default(),
            }],
        }
    }

    #[test]
    fn test_comms_manager_struct() {
        let config = CommsManagerConfig::new();
        let manager = CommsManager::new(config).unwrap();
        // Verify all fields are accessible
        let _ = manager.keypair();
        let _ = manager.public_key();
        let _ = manager.trusted_peers();
        let _ = manager.inbox_sender();
        let _ = manager.router();
    }

    #[test]
    fn test_comms_manager_new() {
        let keypair = make_keypair();
        let pubkey = keypair.public_key();
        let trusted = make_trusted_peers("test-peer", &pubkey);

        let config = CommsManagerConfig::with_keypair(keypair).trusted_peers(trusted);

        let manager = CommsManager::new(config).unwrap();

        assert_eq!(manager.trusted_peers().peers.len(), 1);
        assert_eq!(manager.trusted_peers().peers[0].name, "test-peer");
    }

    #[test]
    fn test_comms_manager_drain_empty() {
        let config = CommsManagerConfig::new();
        let mut manager = CommsManager::new(config).unwrap();

        let messages = manager.drain_messages();
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn test_comms_manager_drain() {
        let sender = make_keypair();
        let sender_pubkey = sender.public_key();
        let our_keypair = make_keypair();
        let our_pubkey = our_keypair.public_key();
        let trusted = make_trusted_peers("sender-agent", &sender_pubkey);

        let config = CommsManagerConfig::with_keypair(our_keypair).trusted_peers(trusted);

        let mut manager = CommsManager::new(config).unwrap();

        // Send a message to the inbox
        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: sender_pubkey,
            to: our_pubkey,
            kind: MessageKind::Message {
                body: "hello from sender".to_string(),
            },
            sig: Signature::new([0u8; 64]),
        };
        envelope.sign(&sender);

        manager
            .inbox_sender()
            .send(InboxItem::External { envelope })
            .unwrap();

        // Give tokio a moment to process
        tokio::task::yield_now().await;

        // Drain messages
        let messages = manager.drain_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].from_peer, "sender-agent");
    }

    #[test]
    fn test_comms_manager_router_access() {
        let config = CommsManagerConfig::new();
        let manager = CommsManager::new(config).unwrap();

        // Router should be accessible
        let router = manager.router();
        // We can't easily test router functionality here without network,
        // but we can verify it exists
        let _ = Arc::strong_count(router);
    }

    #[test]
    fn test_comms_manager_config_builder() {
        let keypair = make_keypair();
        let keypair_pubkey = keypair.public_key();
        let trusted = TrustedPeers::new();
        let comms_config = CommsConfig {
            ack_timeout_secs: 60,
            max_message_bytes: 2_000_000,
        };

        let config = CommsManagerConfig::with_keypair(keypair)
            .trusted_peers(trusted)
            .comms_config(comms_config);

        // Config took ownership of keypair, verify via manager
        let manager = CommsManager::new(config).unwrap();
        assert_eq!(manager.public_key(), keypair_pubkey);
    }
}
