//! MemoryStore trait — semantic memory indexing for discarded conversation history.
//!
//! Implementations live in `meerkat-memory` crate.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Typed session-snapshot outbox carrier for compaction projection intent.
pub const SESSION_COMPACTION_PROJECTION_INTENTS_KEY: &str = "session_compaction_projection_intents";

/// Durable identity of the semantic-memory projection paired with one
/// authoritative compaction transcript rewrite.
///
/// The identity is derived from the exact [`crate::TranscriptRewriteCommit`]
/// rather than a turn counter, wall clock, or locally minted batch id. That
/// makes stage/finalize/recovery idempotent across cancellation and process
/// loss while keeping the transcript rewrite as the semantic owner.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct CompactionProjectionId {
    session_id: crate::types::SessionId,
    parent_revision: String,
    revision: String,
    /// Canonical identity of the exact semantic rewrite commit. Wall-clock
    /// `committed_at` is deliberately excluded so a cancelled retry of the
    /// same rewrite remains idempotent.
    commit_fingerprint: String,
}

#[derive(Serialize)]
struct CompactionCommitFingerprint<'a> {
    selection: &'a crate::TranscriptRewriteSelection,
    original_span_digest: &'a str,
    replacement_digest: &'a str,
    messages_before: usize,
    messages_after: usize,
    actor: &'a Option<String>,
}

/// Pre-typed-selection fingerprint retained strictly as a durable decoder for
/// compaction intents written before the semantic authority field existed.
#[derive(Serialize)]
struct LegacyCompactionCommitFingerprint<'a> {
    selection: &'a crate::TranscriptRewriteSelection,
    original_span_digest: &'a str,
    replacement_digest: &'a str,
    messages_before: usize,
    messages_after: usize,
    reason: &'a crate::TranscriptRewriteReason,
    actor: &'a Option<String>,
}

/// Exact post-commit projection work carried by the session snapshot into the
/// runtime's atomic-apply outbox.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactionProjectionIntent {
    pub projection: CompactionProjectionId,
    pub summary_tokens: u64,
    pub messages_before: usize,
    pub messages_after: usize,
}

impl CompactionProjectionId {
    /// Mint the projection identity for the exact validated compaction rewrite.
    ///
    /// This is crate-private: a durable semantic tag is sufficient to validate
    /// an already-carried ID during recovery, but never to mint a new ID. Only
    /// the core compaction owner can supply the opaque witness, which is
    /// rechecked against the commit's exact pre/post transcript digests.
    pub(crate) fn from_validated_transcript_rewrite(
        session_id: crate::types::SessionId,
        commit: &crate::TranscriptRewriteCommit,
        authority: &crate::agent::compact::ValidatedCompactionRewrite,
    ) -> Option<Self> {
        if !authority.authorizes_commit(commit) {
            return None;
        }
        Self::derive_from_typed_transcript_rewrite(session_id, commit)
    }

    fn derive_from_typed_transcript_rewrite(
        session_id: crate::types::SessionId,
        commit: &crate::TranscriptRewriteCommit,
    ) -> Option<Self> {
        if commit.selection.semantic() != crate::TranscriptRewriteSemantic::Compaction {
            return None;
        }
        let canonical = serde_json::to_vec(&CompactionCommitFingerprint {
            selection: &commit.selection,
            original_span_digest: &commit.original_span_digest,
            replacement_digest: &commit.replacement_digest,
            messages_before: commit.messages_before,
            messages_after: commit.messages_after,
            actor: &commit.actor,
        })
        .ok()?;
        let digest = Sha256::digest(canonical);
        let mut commit_fingerprint = String::with_capacity("sha256:".len() + digest.len() * 2);
        commit_fingerprint.push_str("sha256:");
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in digest {
            commit_fingerprint.push(HEX[(byte >> 4) as usize] as char);
            commit_fingerprint.push(HEX[(byte & 0x0f) as usize] as char);
        }
        Some(Self {
            session_id,
            parent_revision: commit.parent_revision.clone(),
            revision: commit.revision.clone(),
            commit_fingerprint,
        })
    }

    /// Validate this identity against a typed compaction commit, admitting the
    /// exact legacy fingerprint only for backward-compatible persisted data.
    pub(crate) fn matches_transcript_rewrite(
        &self,
        session_id: &crate::types::SessionId,
        commit: &crate::TranscriptRewriteCommit,
    ) -> bool {
        if Self::derive_from_typed_transcript_rewrite(session_id.clone(), commit).as_ref()
            == Some(self)
        {
            return true;
        }
        if commit.selection.semantic() != crate::TranscriptRewriteSemantic::Compaction {
            return false;
        }
        Self::legacy_from_typed_compaction(session_id.clone(), commit).as_ref() == Some(self)
    }

    fn legacy_from_typed_compaction(
        session_id: crate::types::SessionId,
        commit: &crate::TranscriptRewriteCommit,
    ) -> Option<Self> {
        if commit.selection.semantic() != crate::TranscriptRewriteSemantic::Compaction {
            return None;
        }
        let (start, end) = commit.selection.bounds();
        let legacy_selection = crate::TranscriptRewriteSelection::MessageRange { start, end };
        let canonical = serde_json::to_vec(&LegacyCompactionCommitFingerprint {
            selection: &legacy_selection,
            original_span_digest: &commit.original_span_digest,
            replacement_digest: &commit.replacement_digest,
            messages_before: commit.messages_before,
            messages_after: commit.messages_after,
            reason: &commit.reason,
            actor: &commit.actor,
        })
        .ok()?;
        let digest = Sha256::digest(canonical);
        let mut fingerprint = String::with_capacity("sha256:".len() + digest.len() * 2);
        fingerprint.push_str("sha256:");
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in digest {
            fingerprint.push(HEX[(byte >> 4) as usize] as char);
            fingerprint.push(HEX[(byte & 0x0f) as usize] as char);
        }
        Some(Self {
            session_id,
            parent_revision: commit.parent_revision.clone(),
            revision: commit.revision.clone(),
            commit_fingerprint: fingerprint,
        })
    }

    pub fn session_id(&self) -> &crate::types::SessionId {
        &self.session_id
    }

    pub fn parent_revision(&self) -> &str {
        &self.parent_revision
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }

    pub fn commit_fingerprint(&self) -> &str {
        &self.commit_fingerprint
    }
}

/// Store behavior for compaction-derived semantic-memory projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionProjectionPersistence {
    /// Store does not implement the compaction projection lifecycle. Recall
    /// and explicit indexing may still work, but compaction must preserve the
    /// transcript rather than publishing an unpaired memory projection.
    Unsupported,
    /// Process-local store with no durable crash window. The agent may publish
    /// the batch immediately after its in-memory transcript rewrite succeeds.
    EphemeralImmediate,
    /// Durable store. Batches must first be persisted invisibly, then finalized
    /// only after the runtime atomically commits the paired transcript rewrite.
    DurableStaged,
}

/// Resultful runtime handoff that authorizes a durable transcript+memory pair.
///
/// Runtime-backed construction injects this handle from the runtime epoch.
/// Standalone construction has no coordinator and therefore fails closed for
/// [`CompactionProjectionPersistence::DurableStaged`] stores.
pub trait CompactionCommitCoordinator: Send + Sync {
    fn authorize_projection(
        &self,
        projection: &CompactionProjectionId,
    ) -> Result<(), CompactionCommitCoordinationError>;
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum CompactionCommitCoordinationError {
    #[error(
        "compaction projection session mismatch: coordinator owns {expected}, projection owns {actual}"
    )]
    SessionMismatch {
        expected: crate::types::SessionId,
        actual: crate::types::SessionId,
    },
    #[error("compaction projection coordinator rejected the handoff: {0}")]
    Rejected(String),
}

/// Receipt for an invisible durable stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionStageReceipt {
    pub projection: CompactionProjectionId,
    pub staged_entries: usize,
}

/// Receipt for idempotent stage reconciliation at agent-build ingress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CompactionStageReconcileReceipt {
    pub retained_committed: usize,
    pub aborted_orphans: usize,
}

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
    /// Compaction projection durability contract for this store.
    fn compaction_projection_persistence(&self) -> CompactionProjectionPersistence {
        // Unknown/custom stores fail closed without making legacy recall-only
        // implementations pretend to support durable staging/reconciliation.
        CompactionProjectionPersistence::Unsupported
    }

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

    /// Persist a compaction batch durably but invisibly.
    ///
    /// Durable stores override this. Search/enumeration must not observe any
    /// staged row until [`MemoryStore::finalize_compaction_batch`] succeeds.
    async fn stage_compaction_batch(
        &self,
        projection: CompactionProjectionId,
        batch: MemoryIndexBatch,
    ) -> Result<CompactionStageReceipt, MemoryStoreError> {
        let _ = (projection, batch);
        Err(MemoryStoreError::Unsupported {
            operation: "stage_compaction_batch",
        })
    }

    /// Idempotently publish one previously staged durable batch.
    async fn finalize_compaction_batch(
        &self,
        projection: &CompactionProjectionId,
    ) -> Result<MemoryIndexReceipt, MemoryStoreError> {
        let _ = projection;
        Err(MemoryStoreError::Unsupported {
            operation: "finalize_compaction_batch",
        })
    }

    /// Idempotently discard one uncommitted invisible stage.
    async fn abort_compaction_batch(
        &self,
        projection: &CompactionProjectionId,
    ) -> Result<(), MemoryStoreError> {
        let _ = projection;
        Err(MemoryStoreError::Unsupported {
            operation: "abort_compaction_batch",
        })
    }

    /// Reconcile durable invisible stages against authoritative transcript
    /// rewrite identities at agent-build ingress.
    ///
    /// Stages absent from `committed` are crash/cancellation orphans and are
    /// aborted. Matching stages remain invisible for the runtime outbox owner
    /// to finalize.
    async fn reconcile_compaction_stages(
        &self,
        owner: &MemoryOwner,
        committed: &[CompactionProjectionId],
    ) -> Result<CompactionStageReconcileReceipt, MemoryStoreError> {
        let _ = (owner, committed);
        Err(MemoryStoreError::Unsupported {
            operation: "reconcile_compaction_stages",
        })
    }

    /// Semantic search: return up to `limit` results ordered by relevance.
    async fn search(
        &self,
        scope: &MemorySearchScope,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryResult>, MemoryStoreError>;

    /// Atomically and permanently drop every indexed entry owned by `owner`.
    ///
    /// All-or-nothing per scope: either every durable entry for the owner is
    /// removed or none are. Durable staging implementations must retain a
    /// deletion-wins tombstone so stale stage/finalize/reconcile/index work
    /// cannot republish this identity after concurrency or restart; a pending
    /// finalize may acknowledge the deletion with a zero-entry success. Other
    /// owners' entries are untouched. Unsupported by default for stores
    /// without a deletion capability.
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

    fn compaction_commit(
        committed_at: crate::time_compat::SystemTime,
    ) -> (
        crate::TranscriptRewriteCommit,
        crate::agent::compact::ValidatedCompactionRewrite,
    ) {
        let mut session = crate::Session::new();
        session.push(crate::types::Message::User(
            crate::types::UserMessage::text("verbose context one"),
        ));
        session.push(crate::types::Message::User(
            crate::types::UserMessage::text("verbose context two"),
        ));
        let replacement = vec![crate::types::Message::User(
            crate::types::UserMessage::compaction_summary("compacted context"),
        )];
        let authority = crate::agent::compact::ValidatedCompactionRewrite::for_test(
            session.messages(),
            &replacement,
        )
        .unwrap();
        let mut commit = session
            .replace_messages_for_compaction_internal(replacement, &authority)
            .unwrap()
            .unwrap();
        commit.committed_at = committed_at;
        (commit, authority)
    }

    #[test]
    fn projection_identity_fingerprints_semantic_commit_but_excludes_wall_time() {
        let session_id = crate::types::SessionId::new();
        let (first_commit, first_authority) =
            compaction_commit(UNIX_EPOCH + Duration::from_secs(1));
        let first = CompactionProjectionId::from_validated_transcript_rewrite(
            session_id.clone(),
            &first_commit,
            &first_authority,
        )
        .unwrap();
        let (retry_commit, retry_authority) =
            compaction_commit(UNIX_EPOCH + Duration::from_secs(2));
        let retry = CompactionProjectionId::from_validated_transcript_rewrite(
            session_id.clone(),
            &retry_commit,
            &retry_authority,
        )
        .unwrap();
        assert_eq!(first, retry, "wall time must not break cancellation retry");

        let (mut distinct_commit, distinct_authority) =
            compaction_commit(UNIX_EPOCH + Duration::from_secs(1));
        distinct_commit.actor = Some("agent-b".to_string());
        let distinct = CompactionProjectionId::from_validated_transcript_rewrite(
            session_id,
            &distinct_commit,
            &distinct_authority,
        )
        .unwrap();
        assert_ne!(first, distinct, "semantic commit fields must fence aliases");
        assert_ne!(first.commit_fingerprint(), distinct.commit_fingerprint());

        let (mut presentation_only, presentation_authority) =
            compaction_commit(UNIX_EPOCH + Duration::from_secs(3));
        presentation_only.reason = crate::TranscriptRewriteReason {
            kind: "context_reduction".to_string(),
            note: Some("free-form audit wording changed".to_string()),
        };
        let presentation_only = CompactionProjectionId::from_validated_transcript_rewrite(
            first.session_id().clone(),
            &presentation_only,
            &presentation_authority,
        )
        .unwrap();
        assert_eq!(
            first, presentation_only,
            "free-form audit reason must not participate in projection authority or identity"
        );
    }

    #[test]
    fn free_form_compaction_reason_cannot_mint_projection_authority() {
        let (mut generic, authority) = compaction_commit(UNIX_EPOCH);
        generic.selection = crate::TranscriptRewriteSelection::MessageRange { start: 0, end: 2 };
        generic.reason = crate::TranscriptRewriteReason::new("compaction");
        assert!(
            CompactionProjectionId::from_validated_transcript_rewrite(
                crate::types::SessionId::new(),
                &generic,
                &authority,
            )
            .is_none(),
            "display reason text is never a compaction witness"
        );
    }

    #[test]
    fn typed_compaction_commit_accepts_exact_pre_semantic_fingerprint_for_prior_data() {
        let session_id = crate::types::SessionId::new();
        let (commit, authority) = compaction_commit(UNIX_EPOCH);
        let current = CompactionProjectionId::from_validated_transcript_rewrite(
            session_id.clone(),
            &commit,
            &authority,
        )
        .unwrap();
        let legacy =
            CompactionProjectionId::legacy_from_typed_compaction(session_id.clone(), &commit)
                .unwrap();
        assert_ne!(
            legacy, current,
            "typed semantic changes the canonical fingerprint"
        );
        assert!(legacy.matches_transcript_rewrite(&session_id, &commit));
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
        assert_eq!(
            store.compaction_projection_persistence(),
            CompactionProjectionPersistence::Unsupported
        );
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
    async fn reconcile_compaction_stages_default_is_typed_unsupported() {
        let store = MinimalStore;
        let owner = MemoryOwner::canonical_session(crate::types::SessionId::new());
        let error = store
            .reconcile_compaction_stages(&owner, &[])
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            MemoryStoreError::Unsupported {
                operation: "reconcile_compaction_stages",
            }
        ));
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
