//! Trust management for Meerkat comms.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::identity::{IdentityError, PubKey};

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
}

// Custom serde to serialize pubkey as "ed25519:..." string per spec
impl Serialize for TrustedPeer {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("TrustedPeer", 3)?;
        s.serialize_field("name", &self.name)?;
        s.serialize_field("pubkey", &self.pubkey.to_peer_id())?;
        s.serialize_field("addr", &self.addr)?;
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
        }
        let helper = TrustedPeerHelper::deserialize(deserializer)?;
        let pubkey =
            PubKey::from_peer_id(&helper.pubkey).map_err(serde::de::Error::custom)?;
        Ok(TrustedPeer {
            name: helper.name,
            pubkey,
            addr: helper.addr,
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

    /// Load trusted peers from a JSON file.
    pub fn load(path: &Path) -> Result<Self, TrustError> {
        let content = fs::read_to_string(path)?;
        let peers = serde_json::from_str(&content)?;
        Ok(peers)
    }

    /// Save trusted peers to a JSON file.
    pub fn save(&self, path: &Path) -> Result<(), TrustError> {
        let content = serde_json::to_string_pretty(self)?;
        fs::write(path, content)?;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_trusted_peer_fields() {
        let peer = TrustedPeer {
            name: "test-peer".to_string(),
            pubkey: PubKey::new([42u8; 32]),
            addr: "tcp://127.0.0.1:4200".to_string(),
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
                },
                TrustedPeer {
                    name: "peer2".to_string(),
                    pubkey: PubKey::new([2u8; 32]),
                    addr: "uds:///tmp/peer2.sock".to_string(),
                },
            ],
        };
        let json = serde_json::to_string_pretty(&peers).unwrap();
        let decoded: TrustedPeers = serde_json::from_str(&json).unwrap();
        assert_eq!(peers, decoded);
    }

    // Phase 2 tests

    #[test]
    fn test_trusted_peers_load() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("trusted_peers.json");
        let json = r#"{
            "peers": [
                { "name": "coding-meerkat", "pubkey": "ed25519:KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=", "addr": "uds:///tmp/meerkat-coding.sock" }
            ]
        }"#;
        fs::write(&path, json).unwrap();

        let peers = TrustedPeers::load(&path).unwrap();
        assert_eq!(peers.peers.len(), 1);
        assert_eq!(peers.peers[0].name, "coding-meerkat");
        assert_eq!(peers.peers[0].pubkey, PubKey::new([42u8; 32]));
    }

    #[test]
    fn test_trusted_peers_save() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("trusted_peers.json");

        let peers = TrustedPeers {
            peers: vec![TrustedPeer {
                name: "test-peer".to_string(),
                pubkey: PubKey::new([1u8; 32]),
                addr: "tcp://localhost:4200".to_string(),
            }],
        };
        peers.save(&path).unwrap();

        assert!(path.exists());
        let content = fs::read_to_string(&path).unwrap();
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
                },
                TrustedPeer {
                    name: "beta".to_string(),
                    pubkey: PubKey::new([2u8; 32]),
                    addr: "tcp://localhost:4202".to_string(),
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

    #[test]
    fn test_trusted_peers_persistence_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("trusted_peers.json");

        let original = TrustedPeers {
            peers: vec![
                TrustedPeer {
                    name: "peer1".to_string(),
                    pubkey: PubKey::new([1u8; 32]),
                    addr: "tcp://192.168.1.50:4200".to_string(),
                },
                TrustedPeer {
                    name: "peer2".to_string(),
                    pubkey: PubKey::new([2u8; 32]),
                    addr: "uds:///tmp/peer2.sock".to_string(),
                },
            ],
        };

        original.save(&path).unwrap();
        let loaded = TrustedPeers::load(&path).unwrap();
        assert_eq!(original, loaded);
    }
}
