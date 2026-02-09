//! SimpleMemoryStore — basic in-memory keyword-matching memory store.
//!
//! This is a simple implementation that uses substring matching for search.
//! A production implementation would use vector embeddings (e.g., HNSW).

use async_trait::async_trait;
use meerkat_core::memory::{MemoryMetadata, MemoryResult, MemoryStore, MemoryStoreError};
use tokio::sync::RwLock;

/// Entry in the simple memory store.
#[derive(Debug, Clone)]
struct MemoryEntry {
    content: String,
    metadata: MemoryMetadata,
}

/// Simple in-memory store using substring matching.
///
/// This is a **test-only** implementation. Production use cases should use a
/// vector-embedding-based store (e.g. HNSW). The substring matching here is
/// not suitable for semantic search.
pub struct SimpleMemoryStore {
    entries: RwLock<Vec<MemoryEntry>>,
}

impl SimpleMemoryStore {
    /// Create a new empty memory store.
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(Vec::new()),
        }
    }
}

impl Default for SimpleMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MemoryStore for SimpleMemoryStore {
    async fn index(&self, content: &str, metadata: MemoryMetadata) -> Result<(), MemoryStoreError> {
        let mut entries = self.entries.write().await;
        entries.push(MemoryEntry {
            content: content.to_string(),
            metadata,
        });
        Ok(())
    }

    async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryResult>, MemoryStoreError> {
        let entries = self.entries.read().await;

        let query_lower = query.to_lowercase();
        let query_words: Vec<&str> = query_lower.split_whitespace().collect();

        let mut results: Vec<MemoryResult> = entries
            .iter()
            .filter_map(|entry| {
                let content_lower = entry.content.to_lowercase();
                let matching_words = query_words
                    .iter()
                    .filter(|w| content_lower.contains(**w))
                    .count();

                if matching_words == 0 {
                    return None;
                }

                let score = matching_words as f32 / query_words.len().max(1) as f32;
                Some(MemoryResult {
                    content: entry.content.clone(),
                    metadata: entry.metadata.clone(),
                    score,
                })
            })
            .collect();

        // Sort by score descending
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(limit);

        Ok(results)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::types::SessionId;
    use std::time::SystemTime;

    fn meta(_label: &str) -> MemoryMetadata {
        MemoryMetadata {
            session_id: SessionId::new(),
            turn: Some(1),
            indexed_at: SystemTime::now(),
        }
    }

    #[tokio::test]
    async fn test_index_and_search() {
        let store = SimpleMemoryStore::new();

        store
            .index("The user wants to implement a REST API", meta("s1"))
            .await
            .unwrap();
        store
            .index("Configuration uses TOML format", meta("s2"))
            .await
            .unwrap();
        store
            .index("Authentication uses JWT tokens", meta("s3"))
            .await
            .unwrap();

        let results = store.search("REST API", 10).await.unwrap();
        assert!(!results.is_empty());
        assert!(results[0].content.contains("REST API"));
    }

    #[tokio::test]
    async fn test_search_empty_store() {
        let store = SimpleMemoryStore::new();
        let results = store.search("anything", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_search_limit() {
        let store = SimpleMemoryStore::new();

        for i in 0..10 {
            store
                .index(&format!("Item {} with keyword test", i), meta("s1"))
                .await
                .unwrap();
        }

        let results = store.search("test", 3).await.unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn test_search_no_match() {
        let store = SimpleMemoryStore::new();
        store.index("Hello world", meta("s1")).await.unwrap();

        let results = store.search("quantum computing", 10).await.unwrap();
        assert!(results.is_empty());
    }
}
