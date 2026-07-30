//! Authenticated, delta-sized HeadCanonical session-metadata projection.
//!
//! The live [`super::Session`] still exposes one ordinary JSON metadata map.
//! HeadCanonical persistence does not serialize that accumulated map at every
//! boundary. Instead, every carried key is one canonical value cell in a
//! fixed-depth sparse Merkle map keyed by a domain-separated SHA-256 of the
//! exact UTF-8 key bytes.
//!
//! `exact_root_digest` authenticates the exact canonical value bytes stored by
//! the backend. There is deliberately no second "semantic" identity: metadata
//! authority is the bytes the store committed, not a representation-normalized
//! interpretation of those bytes.
//!
//! An ordinary mutation path-copies exactly 256 in-memory nodes and carries a
//! standard sibling proof. Stores verify that proof against their current
//! compact head before changing the named cell. They persist the changed cell
//! and a small lineage journal, not the path nodes, so database/WAL work stays
//! proportional to changed metadata while cold materialization may rebuild and
//! verify the complete map.

use std::collections::BTreeSet;
use std::sync::{Arc, LazyLock, OnceLock};

use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest as _, Sha256};

const KEY_ROUTE_DOMAIN: &[u8] = b"rkat:session-head-metadata:key-route:v1\0";
const VALUE_EXACT_DOMAIN: &[u8] = b"rkat:session-head-metadata:value-exact:v1\0";
const LEAF_EXACT_DOMAIN: &[u8] = b"rkat:session-head-metadata:leaf-exact:v1\0";
const NODE_EXACT_DOMAIN: &[u8] = b"rkat:session-head-metadata:node-exact:v1\0";
const EMPTY_LEAF_EXACT_DOMAIN: &[u8] = b"rkat:session-head-metadata:empty-leaf-exact:v1\0";
const TREE_DEPTH: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct MerkleHash([u8; 32]);

impl std::fmt::Debug for MerkleHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&encode_hex(&self.0))
    }
}

impl MerkleHash {
    fn digest(parts: &[&[u8]]) -> Self {
        let mut hasher = Sha256::new();
        for part in parts {
            hasher.update(part);
        }
        Self(hasher.finalize().into())
    }

    fn parse_hex(value: &str) -> Option<Self> {
        decode_hex_32(value).map(Self)
    }
}

fn encode_hex(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(64);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_hex_32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut decoded = [0u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = decode_hex_nibble(pair[0])?;
        let low = decode_hex_nibble(pair[1])?;
        decoded[index] = (high << 4) | low;
    }
    Some(decoded)
}

fn decode_hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

/// The exact root of the HeadCanonical metadata sparse Merkle map.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SessionHeadMetadataDigest(String);

impl SessionHeadMetadataDigest {
    const PREFIX: &'static str = "head-metadata-smt-sha256:";

    fn from_hash(hash: MerkleHash) -> Self {
        Self(format!("{}{}", Self::PREFIX, encode_hex(&hash.0)))
    }

    fn hash(&self) -> Option<MerkleHash> {
        self.0
            .strip_prefix(Self::PREFIX)
            .and_then(MerkleHash::parse_hex)
    }

    /// Parse one canonical sparse-Merkle metadata digest.
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        let Some(hex) = value.strip_prefix(Self::PREFIX) else {
            return Err(format!("invalid HeadCanonical metadata digest `{value}`"));
        };
        if MerkleHash::parse_hex(hex).is_none() {
            return Err(format!("invalid HeadCanonical metadata digest `{value}`"));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionHeadMetadataDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<'de> Deserialize<'de> for SessionHeadMetadataDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// Exact identity of one canonical metadata value cell.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SessionHeadMetadataValueDigest(String);

impl SessionHeadMetadataValueDigest {
    const PREFIX: &'static str = "head-metadata-value-sha256:";

    fn from_hash(hash: MerkleHash) -> Self {
        Self(format!("{}{}", Self::PREFIX, encode_hex(&hash.0)))
    }

    fn hash(&self) -> Option<MerkleHash> {
        self.0
            .strip_prefix(Self::PREFIX)
            .and_then(MerkleHash::parse_hex)
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        let Some(hex) = value.strip_prefix(Self::PREFIX) else {
            return Err(format!(
                "invalid HeadCanonical metadata value digest `{value}`"
            ));
        };
        if MerkleHash::parse_hex(hex).is_none() {
            return Err(format!(
                "invalid HeadCanonical metadata value digest `{value}`"
            ));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionHeadMetadataValueDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<'de> Deserialize<'de> for SessionHeadMetadataValueDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// Versioned identity of the complete carried metadata key/value set.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct SessionHeadMetadataIdentity {
    format_version: u16,
    entry_count: u64,
    exact_root_digest: SessionHeadMetadataDigest,
}

impl SessionHeadMetadataIdentity {
    /// First published per-key sparse-Merkle metadata identity.
    ///
    /// Released 0.8.10 heads carried inline metadata and therefore had no
    /// authenticated-map format number. Unreleased whole-map candidates do
    /// not reserve a ghost predecessor version.
    pub const FORMAT_V1: u16 = 1;

    /// Canonical identity of the empty authenticated metadata map.
    ///
    /// A projection with no predecessor always begins at this identity. This
    /// is deliberately derived by the same sparse-tree implementation as all
    /// other roots rather than duplicated as a wire constant.
    #[must_use]
    pub fn canonical_empty() -> Self {
        SparseMetadataTree::default().identity()
    }

    /// Whether this identity is the one canonical empty-map identity.
    #[must_use]
    pub fn is_canonical_empty(&self) -> bool {
        self == &Self::canonical_empty()
    }

    #[must_use]
    pub const fn format_version(&self) -> u16 {
        self.format_version
    }

    #[must_use]
    pub const fn entry_count(&self) -> u64 {
        self.entry_count
    }

    #[must_use]
    pub fn exact_root_digest(&self) -> &SessionHeadMetadataDigest {
        &self.exact_root_digest
    }

    fn from_tree(tree: &SparseMetadataTree) -> Self {
        Self {
            format_version: Self::FORMAT_V1,
            entry_count: tree.entry_count,
            exact_root_digest: SessionHeadMetadataDigest::from_hash(tree.root_hash()),
        }
    }

    fn root(&self) -> Option<MerkleHash> {
        self.exact_root_digest.hash()
    }
}

/// Exact byte-derived identity of one metadata value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct SessionHeadMetadataCellIdentity {
    exact_value_digest: SessionHeadMetadataValueDigest,
}

impl SessionHeadMetadataCellIdentity {
    #[must_use]
    pub fn new(exact_value_digest: SessionHeadMetadataValueDigest) -> Self {
        Self { exact_value_digest }
    }

    #[must_use]
    pub fn exact_value_digest(&self) -> &SessionHeadMetadataValueDigest {
        &self.exact_value_digest
    }
}

/// Canonical bytes and parsed value of one carried metadata key.
#[derive(Debug, Clone)]
pub struct SessionHeadMetadataCell {
    key: Arc<str>,
    key_route: [u8; 32],
    identity: SessionHeadMetadataCellIdentity,
    canonical_json: Arc<[u8]>,
    value: Arc<serde_json::Value>,
}

impl SessionHeadMetadataCell {
    pub(crate) fn from_value(
        key: &str,
        value: &serde_json::Value,
    ) -> Result<Self, serde_json::Error> {
        let mut canonical_json = Vec::new();
        crate::digest_observability::write_canonical_json(value, &mut canonical_json)?;
        #[cfg(test)]
        super::SESSION_HEAD_METADATA_CANONICALIZATION_COUNT
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let exact_hash = MerkleHash::digest(&[VALUE_EXACT_DOMAIN, &canonical_json]);
        crate::digest_observability::record_content_digest_computation();
        crate::digest_observability::record_content_digest_bytes(canonical_json.len() as u64);

        let key_route = metadata_key_route(key);
        Ok(Self {
            key: Arc::from(key),
            key_route,
            identity: SessionHeadMetadataCellIdentity::new(
                SessionHeadMetadataValueDigest::from_hash(exact_hash),
            ),
            canonical_json: Arc::from(canonical_json),
            value: Arc::new(value.clone()),
        })
    }

    /// Validate one exact persisted value cell before admitting it into a
    /// materialized sparse-Merkle snapshot.
    pub fn from_canonical_json(
        key: String,
        expected_identity: SessionHeadMetadataCellIdentity,
        canonical_json: Arc<[u8]>,
    ) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_slice(&canonical_json)
            .map_err(|error| format!("invalid HeadCanonical metadata cell JSON: {error}"))?;
        let derived = Self::from_value(&key, &value).map_err(|error| {
            format!("failed to canonicalize HeadCanonical metadata cell: {error}")
        })?;
        if derived.canonical_json.as_ref() != canonical_json.as_ref() {
            return Err("HeadCanonical metadata cell is not canonical JSON".to_string());
        }
        if derived.identity != expected_identity {
            return Err(format!(
                "HeadCanonical metadata cell identity {:?} differs from expected {:?}",
                derived.identity, expected_identity
            ));
        }
        Ok(derived)
    }

    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    #[must_use]
    pub fn key_route(&self) -> &[u8; 32] {
        &self.key_route
    }

    #[must_use]
    pub fn identity(&self) -> &SessionHeadMetadataCellIdentity {
        &self.identity
    }

    #[must_use]
    pub fn canonical_json(&self) -> &[u8] {
        &self.canonical_json
    }

    #[must_use]
    pub fn value(&self) -> &serde_json::Value {
        &self.value
    }
}

#[derive(Debug, Clone)]
struct SparseMetadataNode {
    hash: MerkleHash,
    kind: SparseMetadataNodeKind,
}

#[derive(Debug, Clone)]
enum SparseMetadataNodeKind {
    Branch {
        left: Option<Arc<SparseMetadataNode>>,
        right: Option<Arc<SparseMetadataNode>>,
    },
    Leaf {
        key: Arc<str>,
        key_route: [u8; 32],
        identity: SessionHeadMetadataCellIdentity,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct SparseMetadataTree {
    root: Option<Arc<SparseMetadataNode>>,
    entry_count: u64,
}

impl Default for SparseMetadataTree {
    fn default() -> Self {
        Self {
            root: None,
            entry_count: 0,
        }
    }
}

impl SparseMetadataTree {
    fn root_hash(&self) -> MerkleHash {
        self.root
            .as_ref()
            .map_or_else(|| default_hash(0), |node| node.hash)
    }

    pub(crate) fn identity(&self) -> SessionHeadMetadataIdentity {
        SessionHeadMetadataIdentity::from_tree(self)
    }

    fn cell_identity(&self, key: &str) -> Result<Option<SessionHeadMetadataCellIdentity>, String> {
        let route = metadata_key_route(key);
        let mut node = self.root.as_ref();
        for depth in 0..TREE_DEPTH {
            node = match node.map(|node| &node.kind) {
                None => return Ok(None),
                Some(SparseMetadataNodeKind::Branch { left, right }) => {
                    if route_bit(&route, depth) {
                        right.as_ref()
                    } else {
                        left.as_ref()
                    }
                }
                Some(SparseMetadataNodeKind::Leaf { .. }) => {
                    return Err("HeadCanonical sparse-Merkle tree has a premature leaf".to_string());
                }
            };
        }
        match node.map(|node| &node.kind) {
            None => Ok(None),
            Some(SparseMetadataNodeKind::Leaf {
                key: observed_key,
                key_route,
                identity,
            }) if key_route == &route && observed_key.as_ref() == key => Ok(Some(identity.clone())),
            Some(SparseMetadataNodeKind::Leaf {
                key: observed_key,
                key_route,
                ..
            }) if key_route == &route => Err(format!(
                "HeadCanonical metadata key-hash collision between `{observed_key}` and `{key}`"
            )),
            Some(SparseMetadataNodeKind::Leaf { .. }) => {
                Err("HeadCanonical sparse-Merkle leaf is on the wrong route".to_string())
            }
            Some(SparseMetadataNodeKind::Branch { .. }) => {
                Err("HeadCanonical sparse-Merkle tree has a branch below depth 256".to_string())
            }
        }
    }

    pub(crate) fn apply(
        &self,
        key: &str,
        expected: Option<&SessionHeadMetadataCellIdentity>,
        successor: Option<&SessionHeadMetadataCellIdentity>,
    ) -> Result<(Self, SessionHeadMetadataProof), String> {
        let route = metadata_key_route(key);
        let mut siblings = Vec::with_capacity(TREE_DEPTH);
        let observed = observe_and_prove(self.root.as_ref(), 0, key, &route, &mut siblings)?;
        if observed.as_ref() != expected {
            return Err(format!(
                "HeadCanonical metadata cell `{key}` expected predecessor {:?}, observed {:?}",
                expected, observed
            ));
        }
        let root = update_node(self.root.as_ref(), 0, key, &route, successor)?;
        let entry_count = match (expected, successor) {
            (None, Some(_)) => self
                .entry_count
                .checked_add(1)
                .ok_or_else(|| "HeadCanonical metadata entry count overflow".to_string())?,
            (Some(_), None) => self
                .entry_count
                .checked_sub(1)
                .ok_or_else(|| "HeadCanonical metadata entry count underflow".to_string())?,
            _ => self.entry_count,
        };
        Ok((
            Self { root, entry_count },
            SessionHeadMetadataProof {
                siblings: siblings.into(),
            },
        ))
    }

    pub(crate) fn verify_identity(&self, expected: &SessionHeadMetadataIdentity) -> bool {
        &self.identity() == expected
    }
}

/// Standard fixed-depth sparse-Merkle membership/non-membership proof.
#[derive(Debug, Clone)]
pub struct SessionHeadMetadataProof {
    siblings: Arc<[MerkleHash]>,
}

impl SessionHeadMetadataProof {
    fn verify_transition(
        &self,
        key: &str,
        predecessor: Option<&SessionHeadMetadataCellIdentity>,
        successor: Option<&SessionHeadMetadataCellIdentity>,
        predecessor_identity: &SessionHeadMetadataIdentity,
        successor_identity: &SessionHeadMetadataIdentity,
    ) -> bool {
        if self.siblings.len() != TREE_DEPTH
            || predecessor_identity.format_version != SessionHeadMetadataIdentity::FORMAT_V1
            || successor_identity.format_version != SessionHeadMetadataIdentity::FORMAT_V1
        {
            return false;
        }
        let Some(expected_root) = predecessor_identity.root() else {
            return false;
        };
        let Some(successor_root) = successor_identity.root() else {
            return false;
        };
        let route = metadata_key_route(key);
        let old_leaf = leaf_or_default_hash(key, &route, predecessor);
        let new_leaf = leaf_or_default_hash(key, &route, successor);
        let old_root = fold_proof(&route, &self.siblings, old_leaf);
        let new_root = fold_proof(&route, &self.siblings, new_leaf);
        let expected_count = match (predecessor, successor) {
            (None, Some(_)) => predecessor_identity.entry_count.checked_add(1),
            (Some(_), None) => predecessor_identity.entry_count.checked_sub(1),
            _ => Some(predecessor_identity.entry_count),
        };
        old_root == expected_root
            && new_root == successor_root
            && expected_count == Some(successor_identity.entry_count)
    }
}

/// One sealed key transition and its authenticated predecessor proof.
#[derive(Debug, Clone)]
pub struct SessionHeadMetadataCellMutation {
    key: Arc<str>,
    predecessor: Option<SessionHeadMetadataCellIdentity>,
    successor: Option<Arc<SessionHeadMetadataCell>>,
    predecessor_identity: SessionHeadMetadataIdentity,
    successor_identity: SessionHeadMetadataIdentity,
    proof: SessionHeadMetadataProof,
}

impl SessionHeadMetadataCellMutation {
    pub(crate) fn new(
        key: &str,
        predecessor: Option<SessionHeadMetadataCellIdentity>,
        successor: Option<Arc<SessionHeadMetadataCell>>,
        predecessor_identity: SessionHeadMetadataIdentity,
        successor_identity: SessionHeadMetadataIdentity,
        proof: SessionHeadMetadataProof,
    ) -> Result<Self, String> {
        let successor_identity_cell = successor.as_deref().map(SessionHeadMetadataCell::identity);
        if !proof.verify_transition(
            key,
            predecessor.as_ref(),
            successor_identity_cell,
            &predecessor_identity,
            &successor_identity,
        ) {
            return Err(format!(
                "HeadCanonical metadata mutation for `{key}` has an invalid sparse-Merkle proof"
            ));
        }
        Ok(Self {
            key: Arc::from(key),
            predecessor,
            successor,
            predecessor_identity,
            successor_identity,
            proof,
        })
    }

    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    #[must_use]
    pub fn key_route(&self) -> [u8; 32] {
        metadata_key_route(&self.key)
    }

    #[must_use]
    pub fn predecessor(&self) -> Option<&SessionHeadMetadataCellIdentity> {
        self.predecessor.as_ref()
    }

    #[must_use]
    pub fn successor(&self) -> Option<&Arc<SessionHeadMetadataCell>> {
        self.successor.as_ref()
    }

    #[must_use]
    pub fn predecessor_identity(&self) -> &SessionHeadMetadataIdentity {
        &self.predecessor_identity
    }

    #[must_use]
    pub fn successor_identity(&self) -> &SessionHeadMetadataIdentity {
        &self.successor_identity
    }

    #[must_use]
    pub fn verify(&self) -> bool {
        self.proof.verify_transition(
            &self.key,
            self.predecessor.as_ref(),
            self.successor
                .as_deref()
                .map(SessionHeadMetadataCell::identity),
            &self.predecessor_identity,
            &self.successor_identity,
        )
    }
}

/// Sealed sparse-Merkle transition carried by one prepared HeadCanonical head.
#[derive(Debug, Clone)]
pub struct SessionHeadMetadataProjection {
    predecessor_identity: Option<SessionHeadMetadataIdentity>,
    successor_identity: SessionHeadMetadataIdentity,
    mutations: Arc<[SessionHeadMetadataCellMutation]>,
    successor_tree: SparseMetadataTree,
    snapshot_cells: Option<Arc<[Arc<SessionHeadMetadataCell>]>>,
    mutation_epoch: u64,
}

impl PartialEq for SessionHeadMetadataProjection {
    fn eq(&self, other: &Self) -> bool {
        self.predecessor_identity == other.predecessor_identity
            && self.successor_identity == other.successor_identity
            && self
                .mutations
                .iter()
                .map(|mutation| {
                    (
                        mutation.key(),
                        mutation.predecessor(),
                        mutation.successor().map(|cell| cell.identity()),
                    )
                })
                .eq(other.mutations.iter().map(|mutation| {
                    (
                        mutation.key(),
                        mutation.predecessor(),
                        mutation.successor().map(|cell| cell.identity()),
                    )
                }))
    }
}

impl Eq for SessionHeadMetadataProjection {}

impl SessionHeadMetadataProjection {
    pub(crate) fn from_transition(
        predecessor_identity: Option<SessionHeadMetadataIdentity>,
        successor_tree: SparseMetadataTree,
        mutations: Vec<SessionHeadMetadataCellMutation>,
        snapshot_cells: Option<Vec<Arc<SessionHeadMetadataCell>>>,
        mutation_epoch: u64,
    ) -> Result<Self, String> {
        let successor_identity = successor_tree.identity();
        let empty_identity = SessionHeadMetadataIdentity::canonical_empty();
        let mut cursor = predecessor_identity
            .clone()
            .unwrap_or_else(|| empty_identity.clone());
        for mutation in &mutations {
            if !mutation.verify() || &cursor != mutation.predecessor_identity() {
                return Err(
                    "HeadCanonical metadata mutations do not form one authenticated chain"
                        .to_string(),
                );
            }
            cursor = mutation.successor_identity().clone();
        }
        if mutations.is_empty() {
            if cursor != successor_identity {
                return Err(
                    "empty HeadCanonical metadata mutation changed the authenticated root"
                        .to_string(),
                );
            }
        } else if cursor != successor_identity {
            return Err(
                "HeadCanonical metadata mutation chain does not reach its successor root"
                    .to_string(),
            );
        }
        Ok(Self {
            predecessor_identity,
            successor_identity,
            mutations: mutations.into(),
            successor_tree,
            snapshot_cells: snapshot_cells.map(Arc::from),
            mutation_epoch,
        })
    }

    /// Rebuild and verify a complete cold-loaded metadata snapshot.
    pub fn from_snapshot(
        expected_identity: SessionHeadMetadataIdentity,
        mut cells: Vec<Arc<SessionHeadMetadataCell>>,
    ) -> Result<Self, String> {
        cells.sort_by(|left, right| left.key().cmp(right.key()));
        if cells.windows(2).any(|pair| pair[0].key() == pair[1].key()) {
            return Err("HeadCanonical metadata snapshot contains a duplicate key".to_string());
        }
        let mut tree = SparseMetadataTree::default();
        let mut mutations = Vec::with_capacity(cells.len());
        let mut predecessor_identity = tree.identity();
        for cell in &cells {
            let (successor_tree, proof) = tree.apply(cell.key(), None, Some(cell.identity()))?;
            let successor_identity = successor_tree.identity();
            mutations.push(SessionHeadMetadataCellMutation::new(
                cell.key(),
                None,
                Some(Arc::clone(cell)),
                predecessor_identity,
                successor_identity.clone(),
                proof,
            )?);
            tree = successor_tree;
            predecessor_identity = successor_identity;
        }
        if !tree.verify_identity(&expected_identity) {
            return Err(format!(
                "HeadCanonical metadata snapshot root {:?} differs from expected {:?}",
                tree.identity(),
                expected_identity
            ));
        }
        Ok(Self {
            predecessor_identity: None,
            successor_identity: expected_identity,
            mutations: mutations.into(),
            successor_tree: tree,
            snapshot_cells: Some(cells.into()),
            mutation_epoch: 0,
        })
    }

    #[must_use]
    pub fn predecessor_identity(&self) -> Option<&SessionHeadMetadataIdentity> {
        self.predecessor_identity.as_ref()
    }

    #[must_use]
    pub fn identity(&self) -> &SessionHeadMetadataIdentity {
        &self.successor_identity
    }

    #[must_use]
    pub fn mutations(&self) -> &[SessionHeadMetadataCellMutation] {
        &self.mutations
    }

    #[must_use]
    pub const fn mutation_epoch(&self) -> u64 {
        self.mutation_epoch
    }

    #[must_use]
    pub fn is_full_snapshot(&self) -> bool {
        self.snapshot_cells.is_some()
    }

    pub fn materialized_values(
        &self,
    ) -> Result<serde_json::Map<String, serde_json::Value>, String> {
        let cells = self.snapshot_cells.as_ref().ok_or_else(|| {
            "delta-only HeadCanonical metadata projection cannot materialize a complete map"
                .to_string()
        })?;
        Ok(cells
            .iter()
            .map(|cell| (cell.key().to_string(), cell.value().clone()))
            .collect())
    }

    pub(crate) fn successor_tree(&self) -> &SparseMetadataTree {
        &self.successor_tree
    }

    pub(crate) fn snapshot_cells(&self) -> Option<&[Arc<SessionHeadMetadataCell>]> {
        self.snapshot_cells.as_deref()
    }
}

pub(crate) fn build_metadata_projection(
    baseline_tree: &SparseMetadataTree,
    predecessor_identity: Option<SessionHeadMetadataIdentity>,
    cells: Vec<(String, Option<Arc<SessionHeadMetadataCell>>)>,
    full_snapshot: bool,
    mutation_epoch: u64,
) -> Result<SessionHeadMetadataProjection, String> {
    let mut tree = baseline_tree.clone();
    let mut cursor = predecessor_identity
        .clone()
        .unwrap_or_else(|| baseline_tree.identity());
    let mut mutations = Vec::new();
    let mut snapshot_cells = full_snapshot.then(Vec::new);
    for (key, successor) in cells {
        let predecessor = tree.cell_identity(&key)?;
        if predecessor.as_ref() == successor.as_deref().map(SessionHeadMetadataCell::identity) {
            if let (Some(snapshot), Some(cell)) = (snapshot_cells.as_mut(), successor) {
                snapshot.push(cell);
            }
            continue;
        }
        let (successor_tree, proof) = tree.apply(
            &key,
            predecessor.as_ref(),
            successor.as_deref().map(SessionHeadMetadataCell::identity),
        )?;
        let successor_identity = successor_tree.identity();
        mutations.push(SessionHeadMetadataCellMutation::new(
            &key,
            predecessor,
            successor.clone(),
            cursor,
            successor_identity.clone(),
            proof,
        )?);
        if let (Some(snapshot), Some(cell)) = (snapshot_cells.as_mut(), successor) {
            snapshot.push(cell);
        }
        tree = successor_tree;
        cursor = successor_identity;
    }
    SessionHeadMetadataProjection::from_transition(
        predecessor_identity,
        tree,
        mutations,
        snapshot_cells,
        mutation_epoch,
    )
}

/// Actor-local structural owner of HeadCanonical metadata preparation.
///
/// `baseline_tree` describes the exact physical head last acknowledged by the
/// actor and is persistent through `Arc`-backed nodes, so cloning the tracker
/// does not copy an accumulated key index. `dirty_keys` is coalesced mutation
/// intent since that acknowledgement. The ordinary prepare path never diffs
/// or clones the complete JSON map.
#[derive(Debug, Clone)]
pub(crate) struct SessionHeadMetadataTracker {
    baseline_identity: Option<SessionHeadMetadataIdentity>,
    baseline_tree: SparseMetadataTree,
    dirty_keys: BTreeSet<String>,
    mutation_epoch: u64,
    invalid: bool,
    prepared: OnceLock<Arc<SessionHeadMetadataProjection>>,
}

impl Default for SessionHeadMetadataTracker {
    fn default() -> Self {
        Self {
            baseline_identity: None,
            baseline_tree: SparseMetadataTree::default(),
            dirty_keys: BTreeSet::new(),
            mutation_epoch: 0,
            invalid: false,
            prepared: OnceLock::new(),
        }
    }
}

impl SessionHeadMetadataTracker {
    pub(crate) fn mark_key_mutated(&mut self, key: &str) {
        if !super::head_canonical_metadata_cell_carries_key(key) {
            return;
        }
        self.dirty_keys.insert(key.to_string());
        match self.mutation_epoch.checked_add(1) {
            Some(next) => self.mutation_epoch = next,
            None => self.invalid = true,
        }
        self.prepared = OnceLock::new();
    }

    pub(crate) fn projection(
        &self,
        metadata: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Arc<SessionHeadMetadataProjection>, String> {
        if self.invalid {
            return Err(
                "HeadCanonical metadata mutation epoch overflowed or was poisoned".to_string(),
            );
        }
        if let Some(projection) = self.prepared.get() {
            return Ok(Arc::clone(projection));
        }

        let (keys, full_snapshot) = match self.baseline_identity.as_ref() {
            Some(_) => (self.dirty_keys.iter().cloned().collect::<Vec<_>>(), false),
            None => (
                metadata
                    .keys()
                    .filter(|key| super::head_canonical_metadata_cell_carries_key(key))
                    .cloned()
                    .collect::<Vec<_>>(),
                true,
            ),
        };
        let cells = keys
            .into_iter()
            .map(|key| {
                let cell = metadata
                    .get(&key)
                    .map(|value| {
                        SessionHeadMetadataCell::from_value(&key, value)
                            .map(Arc::new)
                            .map_err(|error| error.to_string())
                    })
                    .transpose()?;
                Ok((key, cell))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let projection = Arc::new(build_metadata_projection(
            &self.baseline_tree,
            self.baseline_identity.clone(),
            cells,
            full_snapshot,
            self.mutation_epoch,
        )?);
        let _ = self.prepared.set(Arc::clone(&projection));
        Ok(self.prepared.get().map_or(projection, Arc::clone))
    }

    pub(crate) fn install_snapshot(
        &mut self,
        projection: &Arc<SessionHeadMetadataProjection>,
    ) -> Result<(), String> {
        if projection.snapshot_cells().is_none() {
            return Err(
                "cannot install a delta-only HeadCanonical metadata projection as a cold baseline"
                    .to_string(),
            );
        }
        if projection.predecessor_identity().is_some() {
            return Err(
                "cold HeadCanonical metadata baseline unexpectedly names a predecessor".to_string(),
            );
        }
        self.baseline_identity = Some(projection.identity().clone());
        self.baseline_tree = projection.successor_tree().clone();
        self.dirty_keys.clear();
        self.prepared = OnceLock::new();
        self.invalid = false;
        Ok(())
    }

    pub(crate) fn acknowledge(
        &mut self,
        projection: &Arc<SessionHeadMetadataProjection>,
        metadata: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        self.validate_acknowledgement(projection, metadata)?;
        self.baseline_identity = Some(projection.identity().clone());
        self.baseline_tree = projection.successor_tree().clone();
        self.dirty_keys.clear();
        // A successor preparation must now name the newly acknowledged root as
        // its predecessor, not reuse the just-applied delta carrier.
        self.prepared = OnceLock::new();
        Ok(())
    }

    pub(crate) fn validate_acknowledgement(
        &self,
        projection: &Arc<SessionHeadMetadataProjection>,
        metadata: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        if self.invalid || projection.mutation_epoch() != self.mutation_epoch {
            return Err(
                "HeadCanonical metadata changed after its prepared transition was sealed"
                    .to_string(),
            );
        }
        if projection.predecessor_identity() != self.baseline_identity.as_ref() {
            return Err(
                "HeadCanonical metadata acknowledgement does not extend the actor baseline"
                    .to_string(),
            );
        }
        let current = self.projection(metadata)?;
        if current.identity() != projection.identity()
            || current.mutations().len() != projection.mutations().len()
            || !current
                .mutations()
                .iter()
                .zip(projection.mutations())
                .all(|(left, right)| {
                    left.key() == right.key()
                        && left.predecessor() == right.predecessor()
                        && left.successor().map(|cell| cell.identity())
                            == right.successor().map(|cell| cell.identity())
                })
        {
            return Err(
                "HeadCanonical metadata acknowledgement differs from the live prepared transition"
                    .to_string(),
            );
        }
        Ok(())
    }
}

fn metadata_key_route(key: &str) -> [u8; 32] {
    MerkleHash::digest(&[KEY_ROUTE_DOMAIN, key.as_bytes()]).0
}

fn route_bit(route: &[u8; 32], depth: usize) -> bool {
    let byte = route[depth / 8];
    let mask = 1u8 << (7 - (depth % 8));
    byte & mask != 0
}

fn default_hash(depth: usize) -> MerkleHash {
    static DEFAULT_HASHES: LazyLock<[MerkleHash; TREE_DEPTH + 1]> = LazyLock::new(|| {
        let mut hashes = [MerkleHash([0; 32]); TREE_DEPTH + 1];
        hashes[TREE_DEPTH] = MerkleHash::digest(&[EMPTY_LEAF_EXACT_DOMAIN]);
        for node_depth in (0..TREE_DEPTH).rev() {
            let child = hashes[node_depth + 1];
            let depth_bytes = u16::try_from(node_depth).unwrap_or(u16::MAX).to_be_bytes();
            hashes[node_depth] =
                MerkleHash::digest(&[NODE_EXACT_DOMAIN, &depth_bytes, &child.0, &child.0]);
        }
        hashes
    });
    DEFAULT_HASHES[depth]
}

fn leaf_or_default_hash(
    key: &str,
    route: &[u8; 32],
    identity: Option<&SessionHeadMetadataCellIdentity>,
) -> MerkleHash {
    let Some(identity) = identity else {
        return default_hash(TREE_DEPTH);
    };
    let value_digest = identity
        .exact_value_digest
        .hash()
        .unwrap_or_else(|| MerkleHash::digest(&[b"invalid-exact-value-digest"]));
    let key_len = u64::try_from(key.len()).unwrap_or(u64::MAX).to_be_bytes();
    MerkleHash::digest(&[
        LEAF_EXACT_DOMAIN,
        &key_len,
        key.as_bytes(),
        route,
        &value_digest.0,
    ])
}

fn branch_hash(
    depth: usize,
    left: Option<&Arc<SparseMetadataNode>>,
    right: Option<&Arc<SparseMetadataNode>>,
) -> MerkleHash {
    let default = default_hash(depth + 1);
    let left_hash = left.map_or(default, |node| node.hash);
    let right_hash = right.map_or(default, |node| node.hash);
    let depth_bytes = u16::try_from(depth).unwrap_or(u16::MAX).to_be_bytes();
    MerkleHash::digest(&[NODE_EXACT_DOMAIN, &depth_bytes, &left_hash.0, &right_hash.0])
}

fn observe_and_prove(
    node: Option<&Arc<SparseMetadataNode>>,
    depth: usize,
    key: &str,
    route: &[u8; 32],
    siblings: &mut Vec<MerkleHash>,
) -> Result<Option<SessionHeadMetadataCellIdentity>, String> {
    if depth == TREE_DEPTH {
        return match node.map(|node| &node.kind) {
            None => Ok(None),
            Some(SparseMetadataNodeKind::Leaf {
                key: observed_key,
                key_route,
                identity,
            }) if key_route == route && observed_key.as_ref() == key => Ok(Some(identity.clone())),
            Some(SparseMetadataNodeKind::Leaf {
                key: observed_key,
                key_route,
                ..
            }) if key_route == route => Err(format!(
                "HeadCanonical metadata key-hash collision between `{observed_key}` and `{key}`"
            )),
            Some(SparseMetadataNodeKind::Leaf { .. }) => {
                Err("HeadCanonical sparse-Merkle leaf is on the wrong route".to_string())
            }
            Some(SparseMetadataNodeKind::Branch { .. }) => {
                Err("HeadCanonical sparse-Merkle tree has a branch below depth 256".to_string())
            }
        };
    }
    let (left, right) = match node.map(|node| &node.kind) {
        None => (None, None),
        Some(SparseMetadataNodeKind::Branch { left, right }) => (left.as_ref(), right.as_ref()),
        Some(SparseMetadataNodeKind::Leaf { .. }) => {
            return Err("HeadCanonical sparse-Merkle tree has a premature leaf".to_string());
        }
    };
    let default = default_hash(depth + 1);
    if route_bit(route, depth) {
        siblings.push(left.map_or(default, |node| node.hash));
        observe_and_prove(right, depth + 1, key, route, siblings)
    } else {
        siblings.push(right.map_or(default, |node| node.hash));
        observe_and_prove(left, depth + 1, key, route, siblings)
    }
}

fn update_node(
    node: Option<&Arc<SparseMetadataNode>>,
    depth: usize,
    key: &str,
    route: &[u8; 32],
    successor: Option<&SessionHeadMetadataCellIdentity>,
) -> Result<Option<Arc<SparseMetadataNode>>, String> {
    if depth == TREE_DEPTH {
        return match successor {
            None => Ok(None),
            Some(identity) => {
                if let Some(SparseMetadataNodeKind::Leaf {
                    key: observed_key,
                    key_route,
                    ..
                }) = node.map(|node| &node.kind)
                    && key_route == route
                    && observed_key.as_ref() != key
                {
                    return Err(format!(
                        "HeadCanonical metadata key-hash collision between `{observed_key}` and `{key}`"
                    ));
                }
                let hash = leaf_or_default_hash(key, route, Some(identity));
                Ok(Some(Arc::new(SparseMetadataNode {
                    hash,
                    kind: SparseMetadataNodeKind::Leaf {
                        key: Arc::from(key),
                        key_route: *route,
                        identity: identity.clone(),
                    },
                })))
            }
        };
    }
    let (mut left, mut right) = match node.map(|node| &node.kind) {
        None => (None, None),
        Some(SparseMetadataNodeKind::Branch { left, right }) => (
            left.as_ref().map(Arc::clone),
            right.as_ref().map(Arc::clone),
        ),
        Some(SparseMetadataNodeKind::Leaf { .. }) => {
            return Err("HeadCanonical sparse-Merkle tree has a premature leaf".to_string());
        }
    };
    if route_bit(route, depth) {
        right = update_node(right.as_ref(), depth + 1, key, route, successor)?;
    } else {
        left = update_node(left.as_ref(), depth + 1, key, route, successor)?;
    }
    if left.is_none() && right.is_none() {
        return Ok(None);
    }
    let hash = branch_hash(depth, left.as_ref(), right.as_ref());
    Ok(Some(Arc::new(SparseMetadataNode {
        hash,
        kind: SparseMetadataNodeKind::Branch { left, right },
    })))
}

fn fold_proof(route: &[u8; 32], siblings: &[MerkleHash], mut hash: MerkleHash) -> MerkleHash {
    for depth in (0..TREE_DEPTH).rev() {
        let sibling = siblings[depth];
        let depth_bytes = u16::try_from(depth).unwrap_or(u16::MAX).to_be_bytes();
        if route_bit(route, depth) {
            hash = MerkleHash::digest(&[NODE_EXACT_DOMAIN, &depth_bytes, &sibling.0, &hash.0]);
        } else {
            hash = MerkleHash::digest(&[NODE_EXACT_DOMAIN, &depth_bytes, &hash.0, &sibling.0]);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(key: &str, value: serde_json::Value) -> Arc<SessionHeadMetadataCell> {
        Arc::new(
            SessionHeadMetadataCell::from_value(key, &value)
                .unwrap_or_else(|error| panic!("test metadata cell must serialize: {error}")),
        )
    }

    #[test]
    fn sparse_merkle_update_and_delete_proofs_are_exact() {
        let first = cell("a", serde_json::json!({"v": 1}));
        let second = cell("z", serde_json::json!([1, 2, 3]));
        let empty = SparseMetadataTree::default();
        let empty_identity = empty.identity();
        let (one, first_proof) = empty
            .apply("a", None, Some(first.identity()))
            .unwrap_or_else(|error| panic!("insert proof must derive: {error}"));
        assert!(first_proof.verify_transition(
            "a",
            None,
            Some(first.identity()),
            &empty_identity,
            &one.identity(),
        ));
        let (two, second_proof) = one
            .apply("z", None, Some(second.identity()))
            .unwrap_or_else(|error| panic!("second insert proof must derive: {error}"));
        assert!(second_proof.verify_transition(
            "z",
            None,
            Some(second.identity()),
            &one.identity(),
            &two.identity(),
        ));
        let (back_to_one, delete_proof) = two
            .apply("z", Some(second.identity()), None)
            .unwrap_or_else(|error| panic!("delete proof must derive: {error}"));
        assert!(delete_proof.verify_transition(
            "z",
            Some(second.identity()),
            None,
            &two.identity(),
            &back_to_one.identity(),
        ));
        assert_eq!(one.identity(), back_to_one.identity());
    }

    #[test]
    fn persisted_identities_expose_only_exact_byte_digests() {
        let cell = cell("caller", serde_json::json!({"value": 1}));
        let cell_identity = serde_json::to_value(cell.identity())
            .unwrap_or_else(|error| panic!("cell identity must serialize: {error}"));
        assert_eq!(
            cell_identity
                .as_object()
                .map(|object| object.keys().cloned().collect::<Vec<_>>()),
            Some(vec!["exact_value_digest".to_string()])
        );

        let (tree, _) = SparseMetadataTree::default()
            .apply(cell.key(), None, Some(cell.identity()))
            .unwrap_or_else(|error| panic!("insert proof must derive: {error}"));
        let root_identity = serde_json::to_value(tree.identity())
            .unwrap_or_else(|error| panic!("root identity must serialize: {error}"));
        assert_eq!(
            root_identity
                .as_object()
                .map(|object| object.keys().cloned().collect::<BTreeSet<_>>()),
            Some(BTreeSet::from([
                "entry_count".to_string(),
                "exact_root_digest".to_string(),
                "format_version".to_string(),
            ]))
        );
    }

    #[test]
    fn canonical_key_bytes_are_bound_in_leaf_hash() {
        let left = cell("caller/a", serde_json::json!(1));
        let right = cell("caller/b", serde_json::json!(1));
        let (left_tree, _) = SparseMetadataTree::default()
            .apply(left.key(), None, Some(left.identity()))
            .unwrap_or_else(|error| panic!("left insert must derive: {error}"));
        let (right_tree, _) = SparseMetadataTree::default()
            .apply(right.key(), None, Some(right.identity()))
            .unwrap_or_else(|error| panic!("right insert must derive: {error}"));
        assert_ne!(left_tree.identity(), right_tree.identity());
    }

    #[test]
    fn nonempty_create_projection_proves_from_canonical_empty_root() {
        let created = cell("caller", serde_json::json!({"value": 1}));
        let projection = build_metadata_projection(
            &SparseMetadataTree::default(),
            None,
            vec![("caller".to_string(), Some(Arc::clone(&created)))],
            true,
            0,
        )
        .unwrap_or_else(|error| panic!("nonempty create projection must derive: {error}"));
        assert!(projection.predecessor_identity().is_none());
        assert_eq!(
            projection.identity().format_version(),
            SessionHeadMetadataIdentity::FORMAT_V1
        );
        assert_eq!(projection.identity().entry_count(), 1);
        assert_eq!(projection.mutations().len(), 1);
        assert!(projection.mutations()[0].verify());
    }

    #[test]
    fn tracker_stable_boundary_touches_zero_values_and_changed_key_only() {
        let mut metadata = serde_json::Map::new();
        metadata.insert("a".to_string(), serde_json::json!({"value": 1}));
        metadata.insert(
            "large-cold".to_string(),
            serde_json::json!({"payload": "x".repeat(16_384)}),
        );
        let mut tracker = SessionHeadMetadataTracker::default();
        let created = tracker
            .projection(&metadata)
            .unwrap_or_else(|error| panic!("create projection must derive: {error}"));
        tracker
            .install_snapshot(&created)
            .unwrap_or_else(|error| panic!("create baseline must install: {error}"));

        super::super::reset_session_head_metadata_canonicalization_count();
        let stable = tracker
            .projection(&metadata)
            .unwrap_or_else(|error| panic!("stable projection must derive: {error}"));
        assert!(stable.mutations().is_empty());
        assert_eq!(
            super::super::session_head_metadata_canonicalization_count(),
            0
        );

        metadata.insert("a".to_string(), serde_json::json!({"value": 2}));
        tracker.mark_key_mutated("a");
        let changed = tracker
            .projection(&metadata)
            .unwrap_or_else(|error| panic!("changed projection must derive: {error}"));
        assert_eq!(changed.mutations().len(), 1);
        assert_eq!(changed.mutations()[0].key(), "a");
        assert_eq!(
            super::super::session_head_metadata_canonicalization_count(),
            1
        );
    }

    #[test]
    fn tracker_epoch_refuses_change_then_revert_aba() {
        let mut metadata = serde_json::Map::new();
        metadata.insert("a".to_string(), serde_json::json!(1));
        let mut tracker = SessionHeadMetadataTracker::default();
        let created = tracker
            .projection(&metadata)
            .unwrap_or_else(|error| panic!("create projection must derive: {error}"));
        tracker
            .install_snapshot(&created)
            .unwrap_or_else(|error| panic!("create baseline must install: {error}"));

        metadata.insert("a".to_string(), serde_json::json!(2));
        tracker.mark_key_mutated("a");
        let prepared = tracker
            .projection(&metadata)
            .unwrap_or_else(|error| panic!("changed projection must derive: {error}"));
        metadata.insert("a".to_string(), serde_json::json!(1));
        tracker.mark_key_mutated("a");
        assert!(tracker.acknowledge(&prepared, &metadata).is_err());
    }
}
