//! Trust management for Meerkat comms.

use std::collections::BTreeMap;
use std::io;
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::sync::Arc;

use meerkat_core::comms::{GeneratedCommsTrustAuthoritySourceKind, PeerAddress, PeerId, PeerName};
use parking_lot::RwLock;
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
    /// A trust entry carried the all-zero public key sentinel instead of real
    /// Ed25519 key material.
    #[error("trusted peer pubkey must be non-zero for {name}")]
    ZeroPubkey { name: String },
    /// A raw [`TrustEntry`] tried to bind a routing identity that does not
    /// derive from its signing key.
    #[error(
        "trusted peer id {peer_id} does not match pubkey-derived id {derived_peer_id} for {name}"
    )]
    PeerIdPubkeyMismatch {
        name: String,
        peer_id: PeerId,
        derived_peer_id: PeerId,
    },
    /// A persisted or wire trust row carried a display name that fails
    /// [`PeerName`] validation. The row is rejected fail-closed rather than
    /// coerced into a sanitized name.
    #[error("invalid trusted peer name {name:?}: {detail}")]
    InvalidPeerName { name: String, detail: String },
    /// A persisted or wire trust row carried an address that fails
    /// [`PeerAddress`] validation. The row is rejected fail-closed rather
    /// than reinterpreted as a default transport.
    #[error("invalid trusted peer address for {name}: {detail}")]
    InvalidPeerAddress { name: String, detail: String },
    /// A generated authority source tried to re-add a peer row with different
    /// routing material. Callers must use the owning generated rebind/update
    /// protocol instead of letting a trust repair rewrite the row.
    #[error(
        "generated trust source {source_kind:?} for {peer_id} already owns different trust material"
    )]
    ConflictingGeneratedTrustSource {
        peer_id: PeerId,
        source_kind: GeneratedCommsTrustAuthoritySourceKind,
    },
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

impl TrustEntry {
    fn validate(&self) -> Result<(), TrustError> {
        if self.pubkey.is_zero() {
            return Err(TrustError::ZeroPubkey {
                name: self.name.as_str().to_string(),
            });
        }
        let derived_peer_id = self.pubkey.to_peer_id();
        if derived_peer_id != self.peer_id {
            return Err(TrustError::PeerIdPubkeyMismatch {
                name: self.name.as_str().to_string(),
                peer_id: self.peer_id,
                derived_peer_id,
            });
        }
        Ok(())
    }
}

/// Durable wire shape of one trust row in `trusted_peers.json`.
///
/// The persisted format is intentionally stable: `{ name, pubkey, addr, meta }`
/// with the pubkey rendered as an `ed25519:<base64>` string. Loading derives
/// the canonical [`PeerId`] from the pubkey and rejects (typed, fail-closed)
/// any row whose name, address, or key material does not validate.
#[derive(Serialize, Deserialize)]
struct PersistedTrustEntry {
    name: String,
    pubkey: String,
    addr: String,
    #[serde(default)]
    meta: PeerMeta,
}

#[derive(Serialize, Deserialize)]
struct PersistedTrustStore {
    peers: Vec<PersistedTrustEntry>,
}

impl PersistedTrustEntry {
    fn from_entry(entry: &TrustEntry) -> Self {
        Self {
            name: entry.name.as_string(),
            pubkey: entry.pubkey.to_pubkey_string(),
            addr: entry.address.to_string(),
            meta: entry.meta.clone(),
        }
    }

    fn into_entry(self) -> Result<TrustEntry, TrustError> {
        let pubkey = PubKey::from_pubkey_string(&self.pubkey)?;
        let name =
            PeerName::new(self.name.clone()).map_err(|detail| TrustError::InvalidPeerName {
                name: self.name.clone(),
                detail,
            })?;
        let address =
            PeerAddress::parse(&self.addr).map_err(|err| TrustError::InvalidPeerAddress {
                name: self.name.clone(),
                detail: err.to_string(),
            })?;
        let entry = TrustEntry {
            peer_id: pubkey.to_peer_id(),
            name,
            pubkey,
            address,
            meta: self.meta,
        };
        entry.validate()?;
        Ok(entry)
    }
}

/// Trust store keyed by canonical [`PeerId`].
///
/// `PeerName` is **not** a routing key. Duplicate `PeerName` across entries is
/// legal — duplicate `PeerId` is a hard error. Reverse lookups by name go
/// through [`resolve_name`](Self::resolve_name) which returns a typed
/// ambiguity error when the name maps to more than one entry.
///
/// This type is THE owner of comms trust state: the router's live trust
/// projection, ingress classification/admission, and the durable
/// `trusted_peers.json` projection all key on it. Mutation of live trust
/// flows exclusively through the generated
/// `CommsRuntime::apply_trust_mutation` seam.
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
        entry.validate()?;
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
    pub fn upsert(&mut self, entry: TrustEntry) -> Result<Option<TrustEntry>, TrustError> {
        entry.validate()?;
        Ok(self.entries.insert(entry.peer_id, entry))
    }

    /// Number of trusted peers.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if no peers are trusted.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// True if at least one peer is trusted.
    pub fn has_peers(&self) -> bool {
        !self.entries.is_empty()
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

    /// Parse the durable `trusted_peers.json` representation.
    ///
    /// Every row is validated fail-closed: invalid key material, names,
    /// addresses, and duplicate canonical identities are typed rejections,
    /// never silently coerced or dropped.
    pub fn from_persisted_json(content: &str) -> Result<Self, TrustError> {
        let persisted: PersistedTrustStore = serde_json::from_str(content)?;
        let mut store = Self::new();
        for row in persisted.peers {
            store.insert(row.into_entry()?)?;
        }
        Ok(store)
    }

    fn to_persisted(&self) -> PersistedTrustStore {
        PersistedTrustStore {
            peers: self
                .entries
                .values()
                .map(PersistedTrustEntry::from_entry)
                .collect(),
        }
    }

    /// Load the trust store from a JSON file, or return empty if not found.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_or_default(path: &Path) -> Result<Self, TrustError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        Self::from_persisted_json(&content)
    }

    /// Load the trust store from a JSON file.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn load(path: &Path) -> Result<Self, TrustError> {
        let content = tokio::fs::read_to_string(path).await?;
        Self::from_persisted_json(&content)
    }

    /// Save the trust store to a JSON file.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn save(&self, path: &Path) -> Result<(), TrustError> {
        let content = serde_json::to_string_pretty(&self.to_persisted())?;
        tokio::fs::write(path, content).await?;
        Ok(())
    }
}

impl Serialize for TrustStore {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.to_persisted().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for TrustStore {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let persisted = PersistedTrustStore::deserialize(deserializer)?;
        let mut store = Self::new();
        for row in persisted.peers {
            let entry = row.into_entry().map_err(serde::de::Error::custom)?;
            store.insert(entry).map_err(serde::de::Error::custom)?;
        }
        Ok(store)
    }
}

/// Read-only handle to the router-owned live trust projection.
///
/// The lock itself is not exposed publicly because trust changes affect
/// admission and routing semantics. Mutation is restricted to the generated
/// `CommsRuntime::apply_trust_mutation` path.
#[derive(Debug, Clone)]
pub struct TrustedPeersView {
    inner: Arc<RwLock<TrustStore>>,
}

impl TrustedPeersView {
    pub(crate) fn new(inner: Arc<RwLock<TrustStore>>) -> Self {
        Self { inner }
    }

    pub fn snapshot(&self) -> TrustStore {
        self.inner.read().clone()
    }

    pub fn entries(&self) -> Vec<TrustEntry> {
        self.inner.read().entries().cloned().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.read().is_empty()
    }

    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    pub fn contains(&self, peer_id: &PeerId) -> bool {
        self.inner.read().contains(peer_id)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn entry(name: &str, pubkey_bytes: [u8; 32], addr: &str) -> TrustEntry {
        let pubkey = PubKey::new(pubkey_bytes);
        TrustEntry {
            peer_id: pubkey.to_peer_id(),
            name: PeerName::new(name).unwrap(),
            pubkey,
            address: PeerAddress::parse(addr).unwrap(),
            meta: PeerMeta::default(),
        }
    }

    #[test]
    fn test_trust_entry_fields() {
        let entry = entry("test-peer", [42u8; 32], "tcp://127.0.0.1:4200");
        assert_eq!(entry.name.as_str(), "test-peer");
        assert_eq!(entry.pubkey.as_bytes()[0], 42);
        assert_eq!(entry.address.to_string(), "tcp://127.0.0.1:4200");
        assert_eq!(entry.peer_id, entry.pubkey.to_peer_id());
    }

    #[test]
    fn test_trust_store_json_roundtrip() {
        let mut store = TrustStore::new();
        store
            .insert(entry("peer1", [1u8; 32], "tcp://192.168.1.50:4200"))
            .unwrap();
        store
            .insert(entry("peer2", [2u8; 32], "uds:///tmp/peer2.sock"))
            .unwrap();

        let json = serde_json::to_string_pretty(&store).unwrap();
        let decoded: TrustStore = serde_json::from_str(&json).unwrap();
        assert_eq!(store, decoded);
    }

    #[tokio::test]
    async fn test_trust_store_load() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("trusted_peers.json");
        let json = r#"{
            "peers": [
                { "name": "coding-meerkat", "pubkey": "ed25519:KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=", "addr": "uds:///tmp/meerkat-coding.sock" }
            ]
        }"#;
        tokio::fs::write(&path, json).await.unwrap();

        let store = TrustStore::load(&path).await.unwrap();
        assert_eq!(store.len(), 1);
        let expected_pubkey = PubKey::new([42u8; 32]);
        let loaded = store.get(&expected_pubkey.to_peer_id()).unwrap();
        assert_eq!(loaded.name.as_str(), "coding-meerkat");
        assert_eq!(loaded.pubkey, expected_pubkey);
    }

    #[tokio::test]
    async fn test_trust_store_save() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("trusted_peers.json");

        let mut store = TrustStore::new();
        store
            .insert(entry("test-peer", [1u8; 32], "tcp://localhost:4200"))
            .unwrap();
        store.save(&path).await.unwrap();

        assert!(path.exists());
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(content.contains("test-peer"));
        assert!(content.contains("ed25519:"));
    }

    #[tokio::test]
    async fn test_trust_store_persistence_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("trusted_peers.json");

        let mut original = TrustStore::new();
        original
            .insert(entry("peer1", [1u8; 32], "tcp://192.168.1.50:4200"))
            .unwrap();
        original
            .insert(entry("peer2", [2u8; 32], "uds:///tmp/peer2.sock"))
            .unwrap();

        original.save(&path).await.unwrap();
        let loaded = TrustStore::load(&path).await.unwrap();
        assert_eq!(original, loaded);
    }

    #[test]
    fn test_contains_found_and_not_found() {
        let mut store = TrustStore::new();
        let trusted = entry("trusted", [42u8; 32], "tcp://localhost:4200");
        let trusted_id = trusted.peer_id;
        store.insert(trusted).unwrap();

        assert!(store.contains(&trusted_id));
        let unknown = PubKey::new([99u8; 32]).to_peer_id();
        assert!(!store.contains(&unknown));
        assert!(store.get(&unknown).is_none());
        assert_eq!(store.get(&trusted_id).unwrap().name.as_str(), "trusted");
    }

    #[test]
    fn test_zero_pubkey_entry_is_rejected() {
        let zero = PubKey::new([0u8; 32]);
        let raw = TrustEntry {
            peer_id: zero.to_peer_id(),
            name: PeerName::new("zero").unwrap(),
            pubkey: zero,
            address: PeerAddress::parse("inproc://zero").unwrap(),
            meta: PeerMeta::default(),
        };

        let mut store = TrustStore::new();
        assert!(matches!(
            store.insert(raw.clone()),
            Err(TrustError::ZeroPubkey { .. })
        ));
        assert!(matches!(
            store.upsert(raw),
            Err(TrustError::ZeroPubkey { .. })
        ));
        assert!(
            store.is_empty(),
            "zero-pubkey rows must never enter the canonical trust store"
        );
    }

    #[test]
    fn test_persisted_duplicate_canonical_identity_is_rejected() {
        // Two rows with the same signing key are the same canonical identity;
        // the durable representation must reject the duplicate fail-closed.
        let json = r#"{
            "peers": [
                { "name": "primary", "pubkey": "ed25519:KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=", "addr": "inproc://primary" },
                { "name": "stale-shadow", "pubkey": "ed25519:KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=", "addr": "inproc://stale-shadow" }
            ]
        }"#;
        let expected_id = PubKey::new([42u8; 32]).to_peer_id();
        let err = TrustStore::from_persisted_json(json)
            .expect_err("duplicate canonical identities must be rejected");
        assert!(
            matches!(err, TrustError::DuplicatePeerId { peer_id } if peer_id == expected_id),
            "expected duplicate PeerId rejection, got {err:?}"
        );
    }

    #[test]
    fn test_persisted_invalid_address_is_rejected() {
        let json = r#"{
            "peers": [
                { "name": "bad-addr", "pubkey": "ed25519:KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=", "addr": "localhost:4200" }
            ]
        }"#;
        let err =
            TrustStore::from_persisted_json(json).expect_err("schemeless address must be rejected");
        assert!(matches!(err, TrustError::InvalidPeerAddress { .. }));
    }

    #[test]
    fn test_persisted_invalid_name_is_rejected() {
        let json = r#"{
            "peers": [
                { "name": "", "pubkey": "ed25519:KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=", "addr": "inproc://x" }
            ]
        }"#;
        let err = TrustStore::from_persisted_json(json).expect_err("empty name must be rejected");
        assert!(matches!(err, TrustError::InvalidPeerName { .. }));
    }

    #[test]
    fn test_persisted_zero_pubkey_is_rejected() {
        let json = r#"{
            "peers": [
                { "name": "zero-peer", "pubkey": "ed25519:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=", "addr": "inproc://zero-peer" }
            ]
        }"#;
        let err = TrustStore::from_persisted_json(json)
            .expect_err("zero pubkey must be rejected at load");
        assert!(matches!(err, TrustError::ZeroPubkey { .. }));
    }

    #[test]
    fn test_json_format_matches_spec() {
        // Per DESIGN-COMMS.md, the durable JSON format is:
        // { "peers": [{ "name": "...", "pubkey": "ed25519:...", "addr": "..." }] }
        let mut store = TrustStore::new();
        store
            .insert(entry(
                "coding-meerkat",
                [7u8; 32],
                "uds:///tmp/meerkat-coding.sock",
            ))
            .unwrap();
        let json = serde_json::to_string_pretty(&store).unwrap();

        assert!(json.contains("\"peers\""));
        assert!(json.contains("\"name\""));
        assert!(json.contains("\"pubkey\""));
        assert!(json.contains("\"addr\""));
        assert!(json.contains("ed25519:"));
        assert!(json.contains("coding-meerkat"));
        assert!(json.contains("uds:///tmp/meerkat-coding.sock"));

        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let pubkey_str = parsed["peers"][0]["pubkey"].as_str().unwrap();
        assert!(pubkey_str.starts_with("ed25519:"));
    }

    #[test]
    fn test_upsert_adds_new_entry() {
        let mut store = TrustStore::new();
        assert_eq!(store.len(), 0);

        let new_entry = entry("new-peer", [42u8; 32], "uds:///tmp/new.sock");
        let replaced = store.upsert(new_entry).expect("valid entry should upsert");
        assert!(replaced.is_none());
        assert_eq!(store.len(), 1);
        assert_eq!(
            store
                .get(&PubKey::new([42u8; 32]).to_peer_id())
                .unwrap()
                .name
                .as_str(),
            "new-peer"
        );
    }

    #[test]
    fn test_upsert_replaces_existing_entry() {
        let mut store = TrustStore::new();
        store
            .insert(entry("original", [42u8; 32], "uds:///tmp/original.sock"))
            .unwrap();

        // Same pubkey (same canonical identity), different name/addr.
        let replaced = store
            .upsert(entry("updated", [42u8; 32], "uds:///tmp/updated.sock"))
            .expect("valid entry should upsert");
        assert_eq!(replaced.unwrap().name.as_str(), "original");

        assert_eq!(store.len(), 1);
        let live = store.get(&PubKey::new([42u8; 32]).to_peer_id()).unwrap();
        assert_eq!(live.name.as_str(), "updated");
        assert_eq!(live.address.to_string(), "uds:///tmp/updated.sock");
    }

    #[test]
    fn test_remove_existing_and_nonexistent() {
        let mut store = TrustStore::new();
        store
            .insert(entry("peer1", [1u8; 32], "tcp://localhost:4201"))
            .unwrap();
        store
            .insert(entry("peer2", [2u8; 32], "tcp://localhost:4202"))
            .unwrap();

        let removed = store.remove(&PubKey::new([1u8; 32]).to_peer_id());
        assert_eq!(
            removed.map(|e| e.name.as_string()).as_deref(),
            Some("peer1")
        );
        assert_eq!(store.len(), 1);

        let missing = store.remove(&PubKey::new([99u8; 32]).to_peer_id());
        assert!(missing.is_none());
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn test_is_empty_and_has_peers_and_len() {
        let mut store = TrustStore::new();
        assert!(store.is_empty());
        assert!(!store.has_peers());
        assert_eq!(store.len(), 0);

        store
            .insert(entry("peer1", [1u8; 32], "tcp://localhost:4201"))
            .unwrap();
        assert!(!store.is_empty());
        assert!(store.has_peers());
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn test_trust_store_rejects_peer_id_pubkey_mismatch() {
        let trusted_pubkey = PubKey::new([8u8; 32]);
        let mismatched_peer_id = PubKey::new([7u8; 32]).to_peer_id();
        let mut store = TrustStore::new();

        let result = store.insert(TrustEntry {
            peer_id: mismatched_peer_id,
            name: PeerName::new("mismatched-peer").unwrap(),
            pubkey: trusted_pubkey,
            address: PeerAddress::parse("inproc://mismatched-peer").unwrap(),
            meta: PeerMeta::default(),
        });

        assert!(
            matches!(
                result,
                Err(TrustError::PeerIdPubkeyMismatch {
                    peer_id,
                    derived_peer_id,
                    ..
                }) if peer_id == mismatched_peer_id
                    && derived_peer_id == trusted_pubkey.to_peer_id()
            ),
            "raw TrustEntry authority must reject peer ids that do not derive from the pubkey"
        );
        assert!(
            store.is_empty(),
            "mismatched raw trust entry must not enter the canonical trust store"
        );

        let result = store.upsert(TrustEntry {
            peer_id: mismatched_peer_id,
            name: PeerName::new("mismatched-peer").unwrap(),
            pubkey: trusted_pubkey,
            address: PeerAddress::parse("inproc://mismatched-peer").unwrap(),
            meta: PeerMeta::default(),
        });

        assert!(
            matches!(
                result,
                Err(TrustError::PeerIdPubkeyMismatch {
                    peer_id,
                    derived_peer_id,
                    ..
                }) if peer_id == mismatched_peer_id
                    && derived_peer_id == trusted_pubkey.to_peer_id()
            ),
            "raw TrustEntry upsert must reject peer ids that do not derive from the pubkey"
        );
        assert!(
            store.is_empty(),
            "mismatched raw trust entry must not enter the canonical trust store through upsert"
        );
    }

    #[test]
    fn test_trust_store_duplicate_peer_id_uses_pubkey_derived_id() {
        let pubkey = PubKey::new([9u8; 32]);
        let peer_id = pubkey.to_peer_id();
        let mut store = TrustStore::new();

        store
            .insert(entry("first-peer", [9u8; 32], "inproc://first-peer"))
            .expect("derived peer id should insert");

        let result = store.insert(entry(
            "duplicate-peer",
            [9u8; 32],
            "inproc://duplicate-peer",
        ));

        assert!(
            matches!(result, Err(TrustError::DuplicatePeerId { peer_id: id }) if id == peer_id)
        );
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn test_trust_entry_with_meta_roundtrip() {
        let meta = PeerMeta::default()
            .with_description("Reviews code")
            .with_label("lang", "rust");

        let mut reviewer = entry("reviewer", [42u8; 32], "inproc://reviewer");
        reviewer.meta = meta.clone();
        let mut store = TrustStore::new();
        store.insert(reviewer).unwrap();

        let json = serde_json::to_string(&store).unwrap();
        let decoded: TrustStore = serde_json::from_str(&json).unwrap();
        let decoded_entry = decoded.get(&PubKey::new([42u8; 32]).to_peer_id()).unwrap();
        assert_eq!(decoded_entry.meta, meta);
        assert_eq!(
            decoded_entry.meta.description.as_deref(),
            Some("Reviews code")
        );
        assert_eq!(
            decoded_entry.meta.labels.get("lang").map(String::as_str),
            Some("rust")
        );
    }

    #[test]
    fn test_persisted_entry_without_meta_defaults() {
        // Rows persisted before PeerMeta existed have no "meta" field; the
        // durable format remains readable and defaults the metadata.
        let json = r#"{
            "peers": [
                { "name": "durable-peer", "pubkey": "ed25519:KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=", "addr": "tcp://127.0.0.1:4200" }
            ]
        }"#;
        let store = TrustStore::from_persisted_json(json).unwrap();
        let loaded = store.get(&PubKey::new([42u8; 32]).to_peer_id()).unwrap();
        assert_eq!(loaded.name.as_str(), "durable-peer");
        assert_eq!(loaded.meta, PeerMeta::default());
    }

    #[test]
    fn test_resolve_name_unique_ambiguous_and_missing() {
        let mut store = TrustStore::new();
        store
            .insert(entry("alpha", [1u8; 32], "tcp://localhost:4201"))
            .unwrap();
        store
            .insert(entry("shared", [2u8; 32], "tcp://localhost:4202"))
            .unwrap();
        store
            .insert(entry("shared", [3u8; 32], "tcp://localhost:4203"))
            .unwrap();

        assert_eq!(
            store.resolve_name(&PeerName::new("alpha").unwrap()),
            Ok(PubKey::new([1u8; 32]).to_peer_id())
        );
        assert!(matches!(
            store.resolve_name(&PeerName::new("shared").unwrap()),
            Err(TrustResolveError::Ambiguous { candidates, .. }) if candidates.len() == 2
        ));
        assert!(matches!(
            store.resolve_name(&PeerName::new("gamma").unwrap()),
            Err(TrustResolveError::NotFound(_))
        ));
    }
}
