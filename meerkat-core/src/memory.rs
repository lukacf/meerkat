//! MemoryStore trait — semantic memory indexing for discarded conversation history.
//!
//! Implementations live in `meerkat-memory` crate.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

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

/// Semantic memory store for indexing and searching conversation history.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait MemoryStore: Send + Sync {
    /// Index text content with associated metadata.
    async fn index(&self, content: &str, metadata: MemoryMetadata) -> Result<(), MemoryStoreError>;

    /// Search for memories matching the query.
    async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryResult>, MemoryStoreError>;
}

/// Errors from memory store operations.
#[derive(Debug, thiserror::Error)]
pub enum MemoryStoreError {
    #[error("Embedding error: {0}")]
    Embedding(String),

    #[error("Index error: {0}")]
    Index(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
