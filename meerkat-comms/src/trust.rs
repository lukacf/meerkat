//! Trust management for Meerkat comms.

use std::collections::BTreeMap;
use std::io;
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

use meerkat_core::comms::{PeerAddress, PeerId, PeerName};
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
    /// A [`TrustStore`] insert was rejected because the [`PeerId`] already
    /// resolves to a different entry. Duplicate [`PeerName`] is legal; a
    /// duplicate [`PeerId`] is structurally impossible by design.
    #[error("duplicate peer id: {peer_id}")]
    DuplicatePeerId { peer_id: PeerId },
}

/// Error resolving a [`PeerName`] to a routing-key [`PeerId`].
///
/// Two entries may legitimately share a name; the resolver refuses to guess.
/// Callers that receive `Ambiguous` must disambiguate by other means (ask the
/// user, narrow by label, use the [`PeerId`] directly) — the router itself
/// never collapses ambiguous names onto a single routing key.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum TrustResolveError {
    #[error("no trusted peer with name {0:?}")]
    NotFound(PeerName),
    #[error("ambiguous peer name {name:?}: {} candidates", candidates.len())]
    Ambiguous {
        name: PeerName,
        candidates: Vec<PeerId>,
    },
}

/// A single entry in a [`TrustStore`]: everything we know about one trusted
/// peer, keyed in the store by its canonical [`PeerId`].
///
/// `name` is display metadata only — the store intentionally allows multiple
/// entries to share a `name`. Routing is by `peer_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustEntry {
    /// Canonical routing identity (the store key).
    pub peer_id: PeerId,
    /// Display-only slug. Not unique across entries.
    pub name: PeerName,
    /// Signing public key for admission / verification.
    pub pubkey: PubKey,
    /// Typed transport address.
    pub address: PeerAddress,
    /// Discovery metadata (description, labels).
    pub meta: PeerMeta,
}

/// Trust store keyed by canonical [`PeerId`].
///
/// Wave-B V5 dogma: `PeerName` is **not** a routing key. Duplicate `PeerName`
/// across entries is legal — duplicate `PeerId` is a hard error. Reverse
/// lookups by name go through [`resolve_name`](Self::resolve_name) which
/// returns a typed ambiguity error when the name maps to more than one entry.
///
/// This type is the dogma-pure replacement for the legacy [`TrustedPeers`]
/// `Vec<TrustedPeer>` collection. The two are intentionally parallel for
/// Wave-B: consumers still own `TrustedPeers` in Wave-B's allowlist, but all
/// new code keys trust by `PeerId` through this store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustStore {
    entries: BTreeMap<PeerId, TrustEntry>,
}

impl TrustStore {
    /// Create an empty trust store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a new trust entry. Returns an error if a different entry with
    /// the same [`PeerId`] already exists.
    ///
    /// Use [`upsert`](Self::upsert) if replacing an existing entry is the
    /// intended behaviour.
    pub fn insert(&mut self, entry: TrustEntry) -> Result<(), TrustError> {
        if self.entries.contains_key(&entry.peer_id) {
            return Err(TrustError::DuplicatePeerId {
                peer_id: entry.peer_id,
            });
        }
        self.entries.insert(entry.peer_id, entry);
        Ok(())
    }

    /// Insert or replace a trust entry keyed by [`PeerId`].
    ///
    /// Unlike [`insert`](Self::insert), duplicate `PeerId` is not an error —
    /// the existing entry is returned.
    pub fn upsert(&mut self, entry: TrustEntry) -> Option<TrustEntry> {
        self.entries.insert(entry.peer_id, entry)
    }

    /// Number of trusted peers.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if no peers are trusted.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Look up a trust entry by its canonical [`PeerId`].
    pub fn get(&self, peer_id: &PeerId) -> Option<&TrustEntry> {
        self.entries.get(peer_id)
    }

    /// True if a peer is trusted.
    pub fn contains(&self, peer_id: &PeerId) -> bool {
        self.entries.contains_key(peer_id)
    }

    /// Remove a trust entry by [`PeerId`]. Returns the removed entry if
    /// present.
    pub fn remove(&mut self, peer_id: &PeerId) -> Option<TrustEntry> {
        self.entries.remove(peer_id)
    }

    /// Iterate over all trust entries.
    pub fn entries(&self) -> impl Iterator<Item = &TrustEntry> {
        self.entries.values()
    }

    /// Resolve a display [`PeerName`] to its canonical [`PeerId`].
    ///
    /// Returns [`TrustResolveError::NotFound`] if no entry has this name,
    /// or [`TrustResolveError::Ambiguous`] if more than one entry shares
    /// the name. The store does **not** pick one for you — duplicate names
    /// are a discovery concern, not a routing concern.
    pub fn resolve_name(&self, name: &PeerName) -> Result<PeerId, TrustResolveError> {
        let mut hits = self
            .entries
            .values()
            .filter(|e| &e.name == name)
            .map(|e| e.peer_id);
        let first = match hits.next() {
            Some(id) => id,
            None => return Err(TrustResolveError::NotFound(name.clone())),
        };
        let mut candidates = vec![first];
        for extra in hits {
            candidates.push(extra);
        }
        if candidates.len() == 1 {
            Ok(first)
        } else {
            Err(TrustResolveError::Ambiguous {
                name: name.clone(),
                candidates,
            })
        }
    }
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
        s.serialize_field("pubkey", &self.pubkey.to_pubkey_string())?;
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
        let pubkey =
            PubKey::from_pubkey_string(&helper.pubkey).map_err(serde::de::Error::custom)?;
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
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_or_default(path: &Path) -> Result<Self, TrustError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        let peers = serde_json::from_str(&content)?;
        Ok(peers)
    }

    /// Load trusted peers from a JSON file.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn load(path: &Path) -> Result<Self, TrustError> {
        let content = tokio::fs::read_to_string(path).await?;
        let peers = serde_json::from_str(&content)?;
        Ok(peers)
    }

    /// Save trusted peers to a JSON file.
    #[cfg(not(target_arch = "wasm32"))]
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

    /// Remove a peer by pubkey.
    pub fn remove(&mut self, pubkey: &PubKey) -> bool {
        let len_before = self.peers.len();
        self.peers.retain(|p| &p.pubkey != pubkey);
        self.peers.len() != len_before
    }

    /// Insert or replace a peer, keyed by `pubkey`.
    pub fn upsert(&mut self, peer: TrustedPeer) {
        if let Some(existing) = self.peers.iter_mut().find(|p| p.pubkey == peer.pubkey) {
            *existing = peer;
        } else {
            self.peers.push(peer);
        }
    }

    /// Lookup helper used by tests and legacy callers.
    ///
    /// **Do not call this for routing decisions.** Name-keyed lookup is a
    /// V5-dogma violation — use [`TrustStore::resolve_name`] or store by
    /// [`PeerId`] at the call site. This method is retained only for
    /// display-side callers during the Wave-B cutover.
    pub fn get_by_name(&self, name: &str) -> Option<&TrustedPeer> {
        self.peers.iter().find(|p| p.name == name)
    }

    /// Canonical routing lookup: find the trusted peer whose derived
    /// [`PeerId`] matches `peer_id`.
    ///
    /// `PeerId` is deterministic over the signing pubkey (UUIDv5; see
    /// [`crate::router::peer_id_from_pubkey`]), so the match is stable and
    /// unambiguous even when multiple entries share a [`crate::peer_meta::PeerMeta`]
    /// display name.
    pub fn find_by_peer_id(&self, peer_id: &PeerId) -> Option<&TrustedPeer> {
        self.peers
            .iter()
            .find(|p| crate::router::peer_id_from_pubkey(&p.pubkey) == *peer_id)
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
