//! Trust management for Meerkat comms.

use std::io;
use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::identity::{IdentityError, PubKey};
use crate::peer_meta::PeerMeta;

/// Errors that can occur during trust operations.
#[derive(Debug, Error)]
pub enum TrustError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Invalid peer ID: {0}")]
    InvalidPeerId(#[from] IdentityError),
}

/// A trusted peer in the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedPeer {
    /// Human-readable name for the peer.
    pub name: String,
    /// The peer's public key.
    pub pubkey: PubKey,
    /// Address to reach the peer (e.g., "uds:///tmp/meerkat.sock" or "tcp://host:port").
    pub addr: String,
    /// Friendly metadata for peer discovery.
    pub meta: PeerMeta,
}

// Custom serde to serialize pubkey as "ed25519:..." string per spec
impl Serialize for TrustedPeer {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        // Always emit 4 fields; meta uses skip_serializing_if internally
        // but at the struct level we always include it for forward compat.
        let mut s = serializer.serialize_struct("TrustedPeer", 4)?;
        s.serialize_field("name", &self.name)?;
        s.serialize_field("pubkey", &self.pubkey.to_peer_id())?;
        s.serialize_field("addr", &self.addr)?;
        s.serialize_field("meta", &self.meta)?;
        s.end()
    }
}

impl<'de> Deserialize<'de> for TrustedPeer {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct TrustedPeerHelper {
            name: String,
            pubkey: String,
            addr: String,
            /// Backward compat: missing meta deserializes as Default.
            #[serde(default)]
            meta: PeerMeta,
        }
        let helper = TrustedPeerHelper::deserialize(deserializer)?;
        let pubkey = PubKey::from_peer_id(&helper.pubkey).map_err(serde::de::Error::custom)?;
        Ok(TrustedPeer {
            name: helper.name,
            pubkey,
            addr: helper.addr,
            meta: helper.meta,
        })
    }
}

/// Collection of trusted peers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedPeers {
    /// List of trusted peers.
    pub peers: Vec<TrustedPeer>,
}

impl TrustedPeers {
    /// Create an empty trusted peers list.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns true if there are no trusted peers.
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// Returns true if there is at least one trusted peer.
    pub fn has_peers(&self) -> bool {
        !self.peers.is_empty()
    }

    /// Returns the number of trusted peers.
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// Load trusted peers from a JSON file, or return empty if not found.
    pub fn load_or_default(path: &Path) -> Result<Self, TrustError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        let peers = serde_json::from_str(&content)?;
        Ok(peers)
    }

    /// Load trusted peers from a JSON file.
    pub async fn load(path: &Path) -> Result<Self, TrustError> {
        let content = tokio::fs::read_to_string(path).await?;
        let peers = serde_json::from_str(&content)?;
        Ok(peers)
    }

    /// Save trusted peers to a JSON file.
    pub async fn save(&self, path: &Path) -> Result<(), TrustError> {
        let content = serde_json::to_string_pretty(self)?;
        tokio::fs::write(path, content).await?;
        Ok(())
    }

    /// Check if a public key is in the trusted list.
    pub fn is_trusted(&self, pubkey: &PubKey) -> bool {
        self.peers.iter().any(|p| &p.pubkey == pubkey)
    }

    /// Get a peer by their public key.
    pub fn get_peer(&self, pubkey: &PubKey) -> Option<&TrustedPeer> {
        self.peers.iter().find(|p| &p.pubkey == pubkey)
    }

    /// Get a peer by their name.
    pub fn get_by_name(&self, name: &str) -> Option<&TrustedPeer> {
        self.peers.iter().find(|p| p.name == name)
    }

    /// Add or update a trusted peer.
    ///
    /// If a peer with the same pubkey already exists, it will be updated.
    /// Otherwise, a new peer will be added.
    pub fn upsert(&mut self, peer: TrustedPeer) {
        if let Some(existing) = self.peers.iter_mut().find(|p| p.pubkey == peer.pubkey) {
            *existing = peer;
        } else {
            self.peers.push(peer);
        }
    }

    /// Remove a peer by pubkey.
    pub fn remove(&mut self, pubkey: &PubKey) -> bool {
        let len_before = self.peers.len();
        self.peers.retain(|p| &p.pubkey != pubkey);
        self.peers.len() != len_before
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_trusted_peer_fields() {
        let peer = TrustedPeer {
            name: "test-peer".to_string(),
            pubkey: PubKey::new([42u8; 32]),
            addr: "tcp://127.0.0.1:4200".to_string(),
            meta: PeerMeta::default(),
        };
        assert_eq!(peer.name, "test-peer");
        assert_eq!(peer.pubkey.as_bytes()[0], 42);
        assert_eq!(peer.addr, "tcp://127.0.0.1:4200");
    }

    #[test]
    fn test_trusted_peers_fields() {
        let peers = TrustedPeers {
            peers: vec![TrustedPeer {
                name: "peer1".to_string(),
                pubkey: PubKey::new([1u8; 32]),
                addr: "uds:///tmp/test.sock".to_string(),
                meta: crate::PeerMeta::default(),
            }],
        };
        assert_eq!(peers.peers.len(), 1);
        assert_eq!(peers.peers[0].name, "peer1");
    }

    #[test]
    fn test_trusted_peer_json_roundtrip() {
        let peer = TrustedPeer {
            name: "coding-meerkat".to_string(),
            pubkey: PubKey::new([7u8; 32]),
            addr: "uds:///tmp/meerkat-coding.sock".to_string(),
            meta: crate::PeerMeta::default(),
        };
        let json = serde_json::to_string(&peer).unwrap();
        let decoded: TrustedPeer = serde_json::from_str(&json).unwrap();
        assert_eq!(peer, decoded);
    }

    #[test]
    fn test_trusted_peers_json_roundtrip() {
        let peers = TrustedPeers {
            peers: vec![
                TrustedPeer {
                    name: "peer1".to_string(),
                    pubkey: PubKey::new([1u8; 32]),
                    addr: "tcp://192.168.1.50:4200".to_string(),
                    meta: crate::PeerMeta::default(),
                },
                TrustedPeer {
                    name: "peer2".to_string(),
                    pubkey: PubKey::new([2u8; 32]),
                    addr: "uds:///tmp/peer2.sock".to_string(),
                    meta: crate::PeerMeta::default(),
                },
            ],
        };
        let json = serde_json::to_string_pretty(&peers).unwrap();
        let decoded: TrustedPeers = serde_json::from_str(&json).unwrap();
        assert_eq!(peers, decoded);
    }

    // Phase 2 tests

    #[tokio::test]
    async fn test_trusted_peers_load() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("trusted_peers.json");
        let json = r#"{
            "peers": [
                { "name": "coding-meerkat", "pubkey": "ed25519:KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=", "addr": "uds:///tmp/meerkat-coding.sock" }
            ]
        }"#;
        tokio::fs::write(&path, json).await.unwrap();

        let peers = TrustedPeers::load(&path).await.unwrap();
        assert_eq!(peers.peers.len(), 1);
        assert_eq!(peers.peers[0].name, "coding-meerkat");
        assert_eq!(peers.peers[0].pubkey, PubKey::new([42u8; 32]));
    }

    #[tokio::test]
    async fn test_trusted_peers_save() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("trusted_peers.json");

        let peers = TrustedPeers {
            peers: vec![TrustedPeer {
                name: "test-peer".to_string(),
                pubkey: PubKey::new([1u8; 32]),
                addr: "tcp://localhost:4200".to_string(),
                meta: crate::PeerMeta::default(),
            }],
        };
        peers.save(&path).await.unwrap();

        assert!(path.exists());
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(content.contains("test-peer"));
        assert!(content.contains("ed25519:"));
    }

    #[test]
    fn test_is_trusted_found() {
        let pubkey = PubKey::new([42u8; 32]);
        let peers = TrustedPeers {
            peers: vec![TrustedPeer {
                name: "trusted".to_string(),
                pubkey,
                addr: "tcp://localhost:4200".to_string(),
                meta: crate::PeerMeta::default(),
            }],
        };
        assert!(peers.is_trusted(&pubkey));
    }

    #[test]
    fn test_is_trusted_not_found() {
        let peers = TrustedPeers {
            peers: vec![TrustedPeer {
                name: "trusted".to_string(),
                pubkey: PubKey::new([1u8; 32]),
                addr: "tcp://localhost:4200".to_string(),
                meta: crate::PeerMeta::default(),
            }],
        };
        let unknown = PubKey::new([99u8; 32]);
        assert!(!peers.is_trusted(&unknown));
    }

    #[test]
    fn test_get_peer() {
        let pubkey = PubKey::new([42u8; 32]);
        let peers = TrustedPeers {
            peers: vec![TrustedPeer {
                name: "the-peer".to_string(),
                pubkey,
                addr: "tcp://localhost:4200".to_string(),
                meta: crate::PeerMeta::default(),
            }],
        };
        let found = peers.get_peer(&pubkey);
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "the-peer");

        let unknown = PubKey::new([99u8; 32]);
        assert!(peers.get_peer(&unknown).is_none());
    }

    #[test]
    fn test_get_by_name() {
        let peers = TrustedPeers {
            peers: vec![
                TrustedPeer {
                    name: "alpha".to_string(),
                    pubkey: PubKey::new([1u8; 32]),
                    addr: "tcp://localhost:4201".to_string(),
                    meta: crate::PeerMeta::default(),
                },
                TrustedPeer {
                    name: "beta".to_string(),
                    pubkey: PubKey::new([2u8; 32]),
                    addr: "tcp://localhost:4202".to_string(),
                    meta: crate::PeerMeta::default(),
                },
            ],
        };
        let found = peers.get_by_name("beta");
        assert!(found.is_some());
        assert_eq!(found.unwrap().pubkey, PubKey::new([2u8; 32]));

        assert!(peers.get_by_name("gamma").is_none());
    }

    #[test]
    fn test_json_format_matches_spec() {
        // Per DESIGN-COMMS.md, the JSON format should be:
        // { "peers": [{ "name": "...", "pubkey": "ed25519:...", "addr": "..." }] }
        let peers = TrustedPeers {
            peers: vec![TrustedPeer {
                name: "coding-meerkat".to_string(),
                pubkey: PubKey::new([7u8; 32]),
                addr: "uds:///tmp/meerkat-coding.sock".to_string(),
                meta: crate::PeerMeta::default(),
            }],
        };
        let json = serde_json::to_string_pretty(&peers).unwrap();

        // Verify structure matches spec
        assert!(json.contains("\"peers\""));
        assert!(json.contains("\"name\""));
        assert!(json.contains("\"pubkey\""));
        assert!(json.contains("\"addr\""));
        assert!(json.contains("ed25519:"));
        assert!(json.contains("coding-meerkat"));
        assert!(json.contains("uds:///tmp/meerkat-coding.sock"));

        // Verify it can be parsed back
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let pubkey_str = parsed["peers"][0]["pubkey"].as_str().unwrap();
        assert!(pubkey_str.starts_with("ed25519:"));
    }

    #[tokio::test]
    async fn test_trusted_peers_persistence_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("trusted_peers.json");

        let original = TrustedPeers {
            peers: vec![
                TrustedPeer {
                    name: "peer1".to_string(),
                    pubkey: PubKey::new([1u8; 32]),
                    addr: "tcp://192.168.1.50:4200".to_string(),
                    meta: crate::PeerMeta::default(),
                },
                TrustedPeer {
                    name: "peer2".to_string(),
                    pubkey: PubKey::new([2u8; 32]),
                    addr: "uds:///tmp/peer2.sock".to_string(),
                    meta: crate::PeerMeta::default(),
                },
            ],
        };

        original.save(&path).await.unwrap();
        let loaded = TrustedPeers::load(&path).await.unwrap();
        assert_eq!(original, loaded);
    }

    #[test]
    fn test_upsert_adds_new_peer() {
        let mut peers = TrustedPeers::new();
        assert_eq!(peers.peers.len(), 0);

        let peer = TrustedPeer {
            name: "new-peer".to_string(),
            pubkey: PubKey::new([42u8; 32]),
            addr: "uds:///tmp/new.sock".to_string(),
            meta: crate::PeerMeta::default(),
        };
        peers.upsert(peer);

        assert_eq!(peers.peers.len(), 1);
        assert_eq!(peers.peers[0].name, "new-peer");
    }

    #[test]
    fn test_upsert_updates_existing_peer() {
        let mut peers = TrustedPeers {
            peers: vec![TrustedPeer {
                name: "original".to_string(),
                pubkey: PubKey::new([42u8; 32]),
                addr: "uds:///tmp/original.sock".to_string(),
                meta: crate::PeerMeta::default(),
            }],
        };

        // Same pubkey, different name/addr
        let updated = TrustedPeer {
            name: "updated".to_string(),
            pubkey: PubKey::new([42u8; 32]),
            addr: "uds:///tmp/updated.sock".to_string(),
            meta: crate::PeerMeta::default(),
        };
        peers.upsert(updated);

        assert_eq!(peers.peers.len(), 1);
        assert_eq!(peers.peers[0].name, "updated");
        assert_eq!(peers.peers[0].addr, "uds:///tmp/updated.sock");
    }

    #[test]
    fn test_remove_existing_peer() {
        let mut peers = TrustedPeers {
            peers: vec![
                TrustedPeer {
                    name: "peer1".to_string(),
                    pubkey: PubKey::new([1u8; 32]),
                    addr: "tcp://localhost:4201".to_string(),
                    meta: crate::PeerMeta::default(),
                },
                TrustedPeer {
                    name: "peer2".to_string(),
                    pubkey: PubKey::new([2u8; 32]),
                    addr: "tcp://localhost:4202".to_string(),
                    meta: crate::PeerMeta::default(),
                },
            ],
        };

        let removed = peers.remove(&PubKey::new([1u8; 32]));
        assert!(removed);
        assert_eq!(peers.peers.len(), 1);
        assert_eq!(peers.peers[0].name, "peer2");
    }

    #[test]
    fn test_is_empty() {
        let peers = TrustedPeers::new();
        assert!(peers.is_empty());

        let peers_with_one = TrustedPeers {
            peers: vec![TrustedPeer {
                name: "peer1".to_string(),
                pubkey: PubKey::new([1u8; 32]),
                addr: "tcp://localhost:4201".to_string(),
                meta: crate::PeerMeta::default(),
            }],
        };
        assert!(!peers_with_one.is_empty());
    }

    #[test]
    fn test_has_peers() {
        let peers = TrustedPeers::new();
        assert!(!peers.has_peers());

        let peers_with_one = TrustedPeers {
            peers: vec![TrustedPeer {
                name: "peer1".to_string(),
                pubkey: PubKey::new([1u8; 32]),
                addr: "tcp://localhost:4201".to_string(),
                meta: crate::PeerMeta::default(),
            }],
        };
        assert!(peers_with_one.has_peers());
    }

    #[test]
    fn test_len() {
        let peers = TrustedPeers::new();
        assert_eq!(peers.len(), 0);

        let peers_with_two = TrustedPeers {
            peers: vec![
                TrustedPeer {
                    name: "peer1".to_string(),
                    pubkey: PubKey::new([1u8; 32]),
                    addr: "tcp://localhost:4201".to_string(),
                    meta: crate::PeerMeta::default(),
                },
                TrustedPeer {
                    name: "peer2".to_string(),
                    pubkey: PubKey::new([2u8; 32]),
                    addr: "tcp://localhost:4202".to_string(),
                    meta: crate::PeerMeta::default(),
                },
            ],
        };
        assert_eq!(peers_with_two.len(), 2);
    }

    #[test]
    fn test_remove_nonexistent_peer() {
        let mut peers = TrustedPeers {
            peers: vec![TrustedPeer {
                name: "peer1".to_string(),
                pubkey: PubKey::new([1u8; 32]),
                addr: "tcp://localhost:4201".to_string(),
                meta: crate::PeerMeta::default(),
            }],
        };

        let removed = peers.remove(&PubKey::new([99u8; 32]));
        assert!(!removed);
        assert_eq!(peers.peers.len(), 1);
    }

    #[test]
    fn test_trusted_peer_with_meta() {
        let meta = PeerMeta::default()
            .with_description("Reviews code")
            .with_label("lang", "rust");

        let peer = TrustedPeer {
            name: "reviewer".to_string(),
            pubkey: PubKey::new([42u8; 32]),
            addr: "inproc://reviewer".to_string(),
            meta: meta.clone(),
        };

        let json = serde_json::to_string(&peer).unwrap();
        let decoded: TrustedPeer = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.meta, meta);
        assert_eq!(decoded.meta.description.as_deref(), Some("Reviews code"));
        assert_eq!(
            decoded.meta.labels.get("lang").map(String::as_str),
            Some("rust")
        );
    }

    #[test]
    fn test_trusted_peer_without_meta_backward_compat() {
        // Pre-PeerMeta JSON (no "meta" field) — should deserialize with Default meta.
        let json = r#"{
            "name": "legacy-peer",
            "pubkey": "ed25519:KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=",
            "addr": "tcp://127.0.0.1:4200"
        }"#;
        let peer: TrustedPeer = serde_json::from_str(json).unwrap();
        assert_eq!(peer.name, "legacy-peer");
        assert_eq!(peer.meta, PeerMeta::default());
    }
}
