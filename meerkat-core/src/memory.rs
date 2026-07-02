//! MemoryStore trait — semantic memory indexing for discarded conversation history.
//!
//! Implementations live in `meerkat-memory` crate.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Canonical semantic-memory owner.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryOwner {
    /// Session that owns the indexed memory shard.
    session_id: crate::types::SessionId,
}

impl MemoryOwner {
    pub fn canonical_session(session_id: crate::types::SessionId) -> Self {
        Self { session_id }
    }

    pub fn session_id(&self) -> &crate::types::SessionId {
        &self.session_id
    }

    fn includes(&self, metadata: &MemoryMetadata) -> bool {
        metadata.session_id == self.session_id
    }
}

/// Half-open range `[start, end)` of message offsets within a session's
/// history that a memory entry was derived from.
///
/// This is the typed source-provenance handle: it records the *origin* of the
/// indexed content (which messages it came from), independent of when the
/// entry happened to be indexed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageRange {
    start: u64,
    end: u64,
}

impl MessageRange {
    /// Construct a half-open range `[start, end)`.
    ///
    /// Fails closed when `start > end` rather than silently normalizing —
    /// an inverted range is a provenance bug at the call site.
    pub fn new(start: u64, end: u64) -> Result<Self, MemoryStoreError> {
        if start > end {
            return Err(MemoryStoreError::SourceRange { start, end });
        }
        Ok(Self { start, end })
    }

    /// A range covering a single message at `offset`.
    pub fn single(offset: u64) -> Self {
        Self {
            start: offset,
            end: offset.saturating_add(1),
        }
    }

    pub fn start(&self) -> u64 {
        self.start
    }

    pub fn end(&self) -> u64 {
        self.end
    }

    /// Number of source messages covered by this range.
    pub fn len(&self) -> u64 {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Whether this half-open range overlaps `other`.
    ///
    /// Half-open semantics: ranges that merely touch (`self.end ==
    /// other.start`) do not overlap, and an empty range overlaps nothing.
    pub fn overlaps(&self, other: &MessageRange) -> bool {
        !self.is_empty() && !other.is_empty() && self.start < other.end && other.start < self.end
    }
}

/// Typed origin of an indexed memory entry.
///
/// Memory provenance is a typed owner, not an absent/stringly fact: every
/// entry records *where* its content came from so retrieval can expose the
/// real source rather than a proxy (e.g. the turn at which compaction ran).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MemorySource {
    /// Content discarded during compaction, identified by the range of source
    /// session-history message offsets it was derived from.
    Compaction {
        /// Range of source message offsets this entry was derived from.
        source_range: MessageRange,
    },
}

impl MemorySource {
    /// The source message range, if this origin carries one.
    pub fn source_range(&self) -> Option<MessageRange> {
        match self {
            MemorySource::Compaction { source_range } => Some(*source_range),
        }
    }
}

/// Metadata associated with an indexed memory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMetadata {
    /// The session ID this memory originated from.
    pub session_id: crate::types::SessionId,
    /// Typed origin of the indexed content (the canonical source handle).
    pub source: MemorySource,
    /// When the memory was indexed.
    pub indexed_at: crate::time_compat::SystemTime,
}

/// A memory search result.
#[derive(Debug, Clone)]
pub struct MemoryResult {
    /// The text content of the memory.
    pub content: String,
    /// Metadata about the source.
    pub metadata: MemoryMetadata,
    /// Relevance score (0.0 = no match, 1.0 = perfect match).
    pub score: f32,
}

/// Typed owner/scope for semantic memory retrieval.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemorySearchScope {
    /// Canonical owner whose indexed memory is visible to this search.
    pub owner: MemoryOwner,
}

impl MemorySearchScope {
    pub fn for_session(session_id: crate::types::SessionId) -> Self {
        Self {
            owner: MemoryOwner::canonical_session(session_id),
        }
    }

    pub fn for_owner(owner: MemoryOwner) -> Self {
        Self { owner }
    }

    pub fn session_id(&self) -> &crate::types::SessionId {
        self.owner.session_id()
    }

    pub fn includes(&self, metadata: &MemoryMetadata) -> bool {
        self.owner.includes(metadata)
    }
}

/// Typed owner/scope for semantic memory indexing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryIndexScope {
    /// Canonical owner receiving the indexed memory projection.
    pub owner: MemoryOwner,
}

impl MemoryIndexScope {
    pub fn for_session(session_id: crate::types::SessionId) -> Self {
        Self {
            owner: MemoryOwner::canonical_session(session_id),
        }
    }

    pub fn for_owner(owner: MemoryOwner) -> Self {
        Self { owner }
    }

    pub fn session_id(&self) -> &crate::types::SessionId {
        self.owner.session_id()
    }

    pub fn includes(&self, metadata: &MemoryMetadata) -> bool {
        self.owner.includes(metadata)
    }
}

/// One scoped semantic-memory indexing request.
///
/// The request carries the typed [`MemoryIndexableContent`] decision rather
/// than a flattened `String`, so the store — not the producer — owns the
/// include/exclude policy. An `Excluded(_)` request reaches the store with its
/// typed exclusion reason intact; the store decides what (if anything) to
/// index. This removes the empty-string "not indexable" convention from the
/// producer/store seam.
#[derive(Debug, Clone)]
pub struct MemoryIndexRequest {
    scope: MemoryIndexScope,
    content: crate::types::MemoryIndexableContent,
    metadata: MemoryMetadata,
}

impl MemoryIndexRequest {
    pub fn new(
        scope: MemoryIndexScope,
        content: crate::types::MemoryIndexableContent,
        metadata: MemoryMetadata,
    ) -> Result<Self, MemoryStoreError> {
        if !scope.includes(&metadata) {
            return Err(MemoryStoreError::Scope(format!(
                "memory metadata session {} is outside indexing scope {}",
                metadata.session_id,
                scope.session_id()
            )));
        }
        Ok(Self {
            scope,
            content,
            metadata,
        })
    }

    pub fn scope(&self) -> &MemoryIndexScope {
        &self.scope
    }

    /// The typed indexability decision the store owns.
    pub fn content(&self) -> &crate::types::MemoryIndexableContent {
        &self.content
    }

    /// Borrow the indexable text, or `None` when the message is excluded.
    pub fn indexable_text(&self) -> Option<&str> {
        self.content.indexable_text()
    }

    pub fn metadata(&self) -> &MemoryMetadata {
        &self.metadata
    }

    pub fn into_parts(
        self,
    ) -> (
        MemoryIndexScope,
        crate::types::MemoryIndexableContent,
        MemoryMetadata,
    ) {
        (self.scope, self.content, self.metadata)
    }
}

/// Atomic scoped semantic-memory indexing batch.
#[derive(Debug, Clone)]
pub struct MemoryIndexBatch {
    scope: MemoryIndexScope,
    requests: Vec<MemoryIndexRequest>,
}

impl MemoryIndexBatch {
    pub fn new(
        scope: MemoryIndexScope,
        requests: Vec<MemoryIndexRequest>,
    ) -> Result<Self, MemoryStoreError> {
        for request in &requests {
            if request.scope() != &scope {
                return Err(MemoryStoreError::Scope(format!(
                    "memory index request scope {} is outside batch scope {}",
                    request.scope().session_id(),
                    scope.session_id()
                )));
            }
        }
        Ok(Self { scope, requests })
    }

    pub fn single(request: MemoryIndexRequest) -> Self {
        Self {
            scope: request.scope.clone(),
            requests: vec![request],
        }
    }

    pub fn scope(&self) -> &MemoryIndexScope {
        &self.scope
    }

    pub fn len(&self) -> usize {
        self.requests.len()
    }

    pub fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }

    pub fn into_parts(self) -> (MemoryIndexScope, Vec<MemoryIndexRequest>) {
        (self.scope, self.requests)
    }
}

/// Successful delivery receipt for a scoped memory index request.
#[derive(Debug, Clone)]
pub struct MemoryIndexReceipt {
    pub scope: MemoryIndexScope,
    pub indexed_entries: usize,
}

/// Typed compaction-to-memory delivery outcome.
#[derive(Debug)]
pub enum MemoryIndexDelivery {
    NoStore {
        scope: MemoryIndexScope,
    },
    Delivered(MemoryIndexReceipt),
    Rejected {
        scope: MemoryIndexScope,
        attempted_entries: usize,
        error: MemoryStoreError,
    },
}

/// Successful receipt for an all-or-nothing scope drop.
#[derive(Debug, Clone)]
pub struct MemoryScopeDropReceipt {
    /// Owner whose indexed entries were dropped.
    pub owner: MemoryOwner,
    /// Number of durable entries removed by the drop.
    pub dropped_entries: usize,
}

/// Typed request for one page of scoped memory enumeration.
///
/// `offset` (and the resulting page's `next_offset`) count RAW scope rows in
/// durable-id order, not post-filter records: paging stays deterministic and
/// iteration stays complete even when `source_overlap` / `indexed_after`
/// filter rows out of a page, so a page may carry fewer than `limit` records.
#[derive(Debug, Clone, Copy)]
pub struct MemoryEnumerationRequest {
    /// Maximum number of raw scope rows scanned for this page.
    pub limit: usize,
    /// Raw scope-row offset (durable-id order) to start scanning from.
    pub offset: usize,
    /// Admit only records whose typed source message range overlaps this
    /// half-open range.
    pub source_overlap: Option<MessageRange>,
    /// Admit only records indexed strictly after this instant. Source-range
    /// offsets restart per compaction generation; scopes are append-only, so
    /// the previous generation's `indexed_at` high-water disambiguates them.
    pub indexed_after: Option<crate::time_compat::SystemTime>,
}

impl MemoryEnumerationRequest {
    /// Whether `metadata` survives this request's post-deserialize filters.
    ///
    /// The filter semantics have exactly one owner (this method) so every
    /// store applies them identically: `source_overlap` admits records whose
    /// typed source range overlaps the requested half-open range (a source
    /// without a range never overlaps); `indexed_after` admits records
    /// indexed strictly after the given instant.
    pub fn admits(&self, metadata: &MemoryMetadata) -> bool {
        if let Some(range) = self.source_overlap {
            match metadata.source.source_range() {
                Some(source_range) if source_range.overlaps(&range) => {}
                _ => return false,
            }
        }
        if let Some(after) = self.indexed_after
            && metadata.indexed_at <= after
        {
            return false;
        }
        true
    }
}

/// One page of scoped memory enumeration.
#[derive(Debug, Clone)]
pub struct MemoryEnumerationPage {
    /// Records surviving the request's post-filters, in durable-id order.
    pub records: Vec<MemoryRecord>,
    /// Raw scope-row offset of the next page, or `None` when no raw scope
    /// rows remain past this page.
    pub next_offset: Option<usize>,
}

/// One enumerated memory record: content plus typed metadata, no ranking
/// score (enumeration is provenance-ordered, not relevance-ranked).
#[derive(Debug, Clone)]
pub struct MemoryRecord {
    /// The text content of the memory.
    pub content: String,
    /// Metadata about the source.
    pub metadata: MemoryMetadata,
}

/// Typed embedding model contract owning vector generation.
///
/// The model is the authority for how text becomes a ranking vector; stores
/// consume an injected model rather than hard-coding an embedding scheme.
pub trait EmbeddingModel: Send + Sync {
    /// Dimensionality of vectors produced by [`EmbeddingModel::embed`].
    ///
    /// Must be stable for the lifetime of the model and match the length of
    /// every returned vector.
    fn dimension(&self) -> usize;

    /// Embed `text` into a ranking vector of length [`EmbeddingModel::dimension`].
    fn embed(&self, text: &str) -> Vec<f32>;
}

/// Typed HNSW index parameters.
///
/// These were previously store-local magic constants; they are now an
/// injected, typed part of the ranking policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HnswParams {
    /// Maximum neighbors per layer.
    pub max_nb_connection: usize,
    /// Maximum number of layers.
    pub max_layer: usize,
    /// Construction-time exploration factor.
    pub ef_construction: usize,
    /// Query-time exploration factor floor.
    pub ef_search: usize,
}

impl Default for HnswParams {
    fn default() -> Self {
        Self {
            max_nb_connection: 16,
            max_layer: 16,
            ef_construction: 200,
            ef_search: 200,
        }
    }
}

/// Typed ranking policy: the authority for embedding generation and index
/// parameters used by a semantic memory store.
#[derive(Clone)]
pub struct MemoryRankingPolicy {
    embedding_model: std::sync::Arc<dyn EmbeddingModel>,
    hnsw_params: HnswParams,
}

impl MemoryRankingPolicy {
    /// Build a ranking policy from an embedding model and index parameters.
    pub fn new(
        embedding_model: std::sync::Arc<dyn EmbeddingModel>,
        hnsw_params: HnswParams,
    ) -> Self {
        Self {
            embedding_model,
            hnsw_params,
        }
    }

    /// The embedding model that owns vector generation.
    pub fn embedding_model(&self) -> &std::sync::Arc<dyn EmbeddingModel> {
        &self.embedding_model
    }

    /// The typed HNSW index parameters.
    pub fn hnsw_params(&self) -> HnswParams {
        self.hnsw_params
    }

    /// Embedding dimension (delegates to the model).
    pub fn dimension(&self) -> usize {
        self.embedding_model.dimension()
    }

    /// Embed `text` via the policy's model.
    pub fn embed(&self, text: &str) -> Vec<f32> {
        self.embedding_model.embed(text)
    }
}

impl std::fmt::Debug for MemoryRankingPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryRankingPolicy")
            .field("dimension", &self.embedding_model.dimension())
            .field("hnsw_params", &self.hnsw_params)
            .finish()
    }
}

/// Semantic memory store for indexing and searching conversation history.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait MemoryStore: Send + Sync {
    /// Index a typed, owner-scoped memory request.
    async fn index_scoped(
        &self,
        request: MemoryIndexRequest,
    ) -> Result<MemoryIndexReceipt, MemoryStoreError> {
        self.index_scoped_batch(MemoryIndexBatch::single(request))
            .await
    }

    /// Atomically index a typed, owner-scoped memory batch.
    ///
    /// Implementations must either make every request in the batch visible or
    /// make none of them visible.
    async fn index_scoped_batch(
        &self,
        batch: MemoryIndexBatch,
    ) -> Result<MemoryIndexReceipt, MemoryStoreError>;

    /// Semantic search: return up to `limit` results ordered by relevance.
    async fn search(
        &self,
        scope: &MemorySearchScope,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryResult>, MemoryStoreError>;

    /// Atomically drop every indexed entry owned by `owner`.
    ///
    /// All-or-nothing per scope: either every durable entry for the owner is
    /// removed or none are. Other owners' entries are untouched. Unsupported
    /// by default for stores without a deletion capability.
    async fn drop_scope(
        &self,
        owner: &MemoryOwner,
    ) -> Result<MemoryScopeDropReceipt, MemoryStoreError> {
        let _ = owner;
        Err(MemoryStoreError::Unsupported {
            operation: "drop_scope",
        })
    }

    /// Enumerate one page of a scope's records in durable-id order.
    ///
    /// Paging counts raw scope rows (see [`MemoryEnumerationRequest`]); the
    /// `source_overlap` / `indexed_after` filters run on typed metadata after
    /// deserialization, so a page may return fewer than `limit` records.
    /// Corrupt durable rows propagate as typed faults, never silently
    /// skipped. Unsupported by default.
    async fn enumerate_scoped(
        &self,
        scope: &MemorySearchScope,
        request: MemoryEnumerationRequest,
    ) -> Result<MemoryEnumerationPage, MemoryStoreError> {
        let _ = (scope, request);
        Err(MemoryStoreError::Unsupported {
            operation: "enumerate_scoped",
        })
    }
}

/// Errors from memory store operations.
///
/// Each underlying failure is a distinct typed variant so callers can
/// distinguish (e.g.) a poisoned index lock from a storage fault from an
/// embedding serialization error without parsing message substrings.
#[derive(Debug, thiserror::Error)]
pub enum MemoryStoreError {
    /// An indexing/search scope did not contain the supplied metadata.
    #[error("Scope error: {0}")]
    Scope(String),

    /// An inverted message source range (`start > end`).
    #[error("invalid memory source range: start {start} > end {end}")]
    SourceRange { start: u64, end: u64 },

    /// Embedding/metadata serialization or deserialization failed.
    #[error("Embedding error: {0}")]
    Embedding(String),

    /// The backing index/metadata store reported a failure.
    #[error("Storage error: {0}")]
    Storage(String),

    /// The in-memory index lock was poisoned by a panicking holder.
    #[error("memory index lock poisoned")]
    LockPoisoned,

    /// A point ID could not be represented in the target integer width.
    #[error("memory point ID out of range")]
    PointIdOutOfRange,

    /// Allocating the next point ID would overflow the ID space.
    #[error("memory point ID overflow")]
    PointIdOverflow,

    /// A background store task failed to join.
    #[error("memory store task join failed: {0}")]
    TaskJoin(String),

    /// Durable memory text bytes are not valid UTF-8. Corrupt durable bytes
    /// are a typed store-corruption fault, never lossy-decoded into
    /// searchable/returned memory content.
    #[error("memory text corruption at point {point_id}: stored bytes are not valid UTF-8")]
    TextCorruption { point_id: i64 },

    /// The live nearest-neighbor index referenced a point that has no durable
    /// row. The index and the durable store have diverged; results derived
    /// from the divergent candidate set must not be silently filtered.
    #[error(
        "memory index/store divergence at point {point_id}: live index references a missing durable row"
    )]
    IndexDivergence { point_id: i64 },

    /// The scoped live index is poisoned: a failed batch could not be
    /// repaired from durable state, so reads fail closed until the scope is
    /// rebuilt (next successful index attempt or store reopen).
    #[error("memory scope index is poisoned pending rebuild from durable state")]
    ScopePoisoned,

    /// A failed batch could not be rolled back/repaired from durable state.
    /// The scoped live index is poisoned (fails closed) until rebuilt.
    #[error(
        "memory scope repair failed after partial index failure: {repair} (original failure: {original})"
    )]
    ScopeRepairFailed {
        original: Box<MemoryStoreError>,
        repair: Box<MemoryStoreError>,
    },

    /// The store does not implement the requested optional operation.
    #[error("memory store operation '{operation}' is unsupported by this store")]
    Unsupported { operation: &'static str },

    /// An enumeration request carried `limit == 0`, which cannot advance the
    /// raw-row cursor: `next_offset` would equal the request offset and a
    /// standard follow-`next_offset` pagination loop would never terminate.
    #[error("memory enumeration limit must be non-zero")]
    EnumerationLimitZero,

    /// An underlying filesystem operation failed.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl MemoryStoreError {
    /// Stable discriminant for the failure class.
    ///
    /// Callers (e.g. tool surfaces) use this to preserve the typed distinction
    /// downstream without parsing message substrings.
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::Scope(_) => "memory_scope",
            Self::SourceRange { .. } => "memory_source_range",
            Self::Embedding(_) => "memory_embedding",
            Self::Storage(_) => "memory_storage",
            Self::LockPoisoned => "memory_lock_poisoned",
            Self::PointIdOutOfRange => "memory_point_id_out_of_range",
            Self::PointIdOverflow => "memory_point_id_overflow",
            Self::TaskJoin(_) => "memory_task_join",
            Self::TextCorruption { .. } => "memory_text_corruption",
            Self::IndexDivergence { .. } => "memory_index_divergence",
            Self::ScopePoisoned => "memory_scope_poisoned",
            Self::ScopeRepairFailed { .. } => "memory_scope_repair_failed",
            Self::Unsupported { .. } => "memory_unsupported",
            Self::EnumerationLimitZero => "memory_enumeration_limit_zero",
            Self::Io(_) => "memory_io",
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::time_compat::{Duration, UNIX_EPOCH};

    fn range(start: u64, end: u64) -> MessageRange {
        MessageRange::new(start, end).unwrap()
    }

    #[test]
    fn overlaps_is_half_open() {
        // Proper overlap.
        assert!(range(0, 5).overlaps(&range(4, 6)));
        assert!(range(4, 6).overlaps(&range(0, 5)));
        // Containment overlaps.
        assert!(range(0, 10).overlaps(&range(3, 4)));
        assert!(range(3, 4).overlaps(&range(0, 10)));
        // Touching ranges do not overlap (half-open).
        assert!(!range(0, 5).overlaps(&range(5, 10)));
        assert!(!range(5, 10).overlaps(&range(0, 5)));
        // Disjoint ranges do not overlap.
        assert!(!range(0, 2).overlaps(&range(7, 9)));
    }

    #[test]
    fn empty_range_never_overlaps() {
        assert!(!range(3, 3).overlaps(&range(0, 10)));
        assert!(!range(0, 10).overlaps(&range(3, 3)));
        assert!(!range(3, 3).overlaps(&range(3, 3)));
    }

    #[test]
    fn unsupported_error_code_is_stable() {
        assert_eq!(
            MemoryStoreError::Unsupported {
                operation: "drop_scope",
            }
            .error_code(),
            "memory_unsupported"
        );
    }

    fn metadata_at(
        indexed_at: crate::time_compat::SystemTime,
        source: MemorySource,
    ) -> MemoryMetadata {
        MemoryMetadata {
            session_id: crate::types::SessionId::new(),
            source,
            indexed_at,
        }
    }

    #[test]
    fn enumeration_request_admits_on_source_overlap() {
        let request = MemoryEnumerationRequest {
            limit: 10,
            offset: 0,
            source_overlap: Some(range(4, 6)),
            indexed_after: None,
        };
        let overlapping = metadata_at(
            UNIX_EPOCH,
            MemorySource::Compaction {
                source_range: range(0, 5),
            },
        );
        let disjoint = metadata_at(
            UNIX_EPOCH,
            MemorySource::Compaction {
                source_range: range(6, 9),
            },
        );
        assert!(request.admits(&overlapping));
        assert!(!request.admits(&disjoint));
    }

    #[test]
    fn enumeration_request_indexed_after_is_strict() {
        let boundary = UNIX_EPOCH + Duration::from_secs(100);
        let request = MemoryEnumerationRequest {
            limit: 10,
            offset: 0,
            source_overlap: None,
            indexed_after: Some(boundary),
        };
        let at_boundary = metadata_at(
            boundary,
            MemorySource::Compaction {
                source_range: range(0, 1),
            },
        );
        let after_boundary = metadata_at(
            boundary + Duration::from_secs(1),
            MemorySource::Compaction {
                source_range: range(0, 1),
            },
        );
        let before_boundary = metadata_at(
            UNIX_EPOCH,
            MemorySource::Compaction {
                source_range: range(0, 1),
            },
        );
        assert!(!request.admits(&at_boundary));
        assert!(request.admits(&after_boundary));
        assert!(!request.admits(&before_boundary));
    }

    #[test]
    fn enumeration_request_filters_compose() {
        let request = MemoryEnumerationRequest {
            limit: 10,
            offset: 0,
            source_overlap: Some(range(0, 5)),
            indexed_after: Some(UNIX_EPOCH + Duration::from_secs(100)),
        };
        let both = metadata_at(
            UNIX_EPOCH + Duration::from_secs(200),
            MemorySource::Compaction {
                source_range: range(2, 3),
            },
        );
        let wrong_range = metadata_at(
            UNIX_EPOCH + Duration::from_secs(200),
            MemorySource::Compaction {
                source_range: range(5, 9),
            },
        );
        let too_early = metadata_at(
            UNIX_EPOCH,
            MemorySource::Compaction {
                source_range: range(2, 3),
            },
        );
        assert!(request.admits(&both));
        assert!(!request.admits(&wrong_range));
        assert!(!request.admits(&too_early));
    }

    /// Minimal store implementing only the required trait surface, pinning
    /// the fail-closed `Unsupported` defaults for the optional lifecycle
    /// methods.
    struct MinimalStore;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl MemoryStore for MinimalStore {
        async fn index_scoped_batch(
            &self,
            batch: MemoryIndexBatch,
        ) -> Result<MemoryIndexReceipt, MemoryStoreError> {
            let (scope, requests) = batch.into_parts();
            Ok(MemoryIndexReceipt {
                scope,
                indexed_entries: requests.len(),
            })
        }

        async fn search(
            &self,
            _scope: &MemorySearchScope,
            _query: &str,
            _limit: usize,
        ) -> Result<Vec<MemoryResult>, MemoryStoreError> {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn drop_scope_default_is_typed_unsupported() {
        let store = MinimalStore;
        let owner = MemoryOwner::canonical_session(crate::types::SessionId::new());
        let error = store.drop_scope(&owner).await.unwrap_err();
        assert!(matches!(
            error,
            MemoryStoreError::Unsupported {
                operation: "drop_scope",
            }
        ));
        assert_eq!(error.error_code(), "memory_unsupported");
    }

    #[tokio::test]
    async fn enumerate_scoped_default_is_typed_unsupported() {
        let store = MinimalStore;
        let scope = MemorySearchScope::for_session(crate::types::SessionId::new());
        let error = store
            .enumerate_scoped(
                &scope,
                MemoryEnumerationRequest {
                    limit: 10,
                    offset: 0,
                    source_overlap: None,
                    indexed_after: None,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            MemoryStoreError::Unsupported {
                operation: "enumerate_scoped",
            }
        ));
        assert_eq!(error.error_code(), "memory_unsupported");
    }
}
