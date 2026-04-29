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

/// Metadata associated with an indexed memory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMetadata {
    /// The session ID this memory originated from.
    pub session_id: crate::types::SessionId,
    /// Turn number within the session.
    pub turn: Option<u32>,
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
#[derive(Debug, Clone)]
pub struct MemoryIndexRequest {
    scope: MemoryIndexScope,
    content: String,
    metadata: MemoryMetadata,
}

impl MemoryIndexRequest {
    pub fn new(
        scope: MemoryIndexScope,
        content: String,
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

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn metadata(&self) -> &MemoryMetadata {
        &self.metadata
    }

    pub fn into_parts(self) -> (MemoryIndexScope, String, MemoryMetadata) {
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
}

/// Errors from memory store operations.
#[derive(Debug, thiserror::Error)]
pub enum MemoryStoreError {
    #[error("Scope error: {0}")]
    Scope(String),

    #[error("Embedding error: {0}")]
    Embedding(String),

    #[error("Index error: {0}")]
    Index(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
