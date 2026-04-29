//! SimpleMemoryStore — basic in-memory keyword-matching memory store.
//!
//! This is a simple implementation that uses substring matching for search.
//! A production implementation would use vector embeddings (e.g., HNSW).

use async_trait::async_trait;
use meerkat_core::memory::{
    MemoryIndexBatch, MemoryIndexReceipt, MemoryIndexScope, MemoryMetadata, MemoryResult,
    MemorySearchScope, MemoryStore, MemoryStoreError,
};
use tokio::sync::RwLock;

/// Entry in the simple memory store.
#[derive(Debug, Clone)]
struct MemoryEntry {
    scope: MemoryIndexScope,
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
    async fn index_scoped_batch(
        &self,
        batch: MemoryIndexBatch,
    ) -> Result<MemoryIndexReceipt, MemoryStoreError> {
        let (receipt_scope, requests) = batch.into_parts();
        let indexed_entries = requests.len();
        let mut entries = self.entries.write().await;
        for request in requests {
            let (scope, content, metadata) = request.into_parts();
            entries.push(MemoryEntry {
                scope,
                content,
                metadata,
            });
        }
        Ok(MemoryIndexReceipt {
            scope: receipt_scope,
            indexed_entries,
        })
    }

    async fn search(
        &self,
        scope: &MemorySearchScope,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryResult>, MemoryStoreError> {
        let entries = self.entries.read().await;

        let query_lower = query.to_lowercase();
        let query_words: Vec<&str> = query_lower.split_whitespace().collect();

        let mut results: Vec<MemoryResult> = entries
            .iter()
            .filter(|entry| entry.scope.owner == scope.owner && scope.includes(&entry.metadata))
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
    use meerkat_core::memory::MemoryIndexRequest;
    use meerkat_core::types::SessionId;
    use std::time::SystemTime;

    fn meta(session_id: &SessionId) -> MemoryMetadata {
        MemoryMetadata {
            session_id: session_id.clone(),
            turn: Some(1),
            indexed_at: SystemTime::now(),
        }
    }

    fn request(content: impl Into<String>, session_id: &SessionId) -> MemoryIndexRequest {
        MemoryIndexRequest::new(
            MemoryIndexScope::for_session(session_id.clone()),
            content.into(),
            meta(session_id),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn test_index_and_search() {
        let store = SimpleMemoryStore::new();
        let session_id = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());
        let other_session_id = SessionId::new();

        store
            .index_scoped(request(
                "The user wants to implement a REST API",
                &session_id,
            ))
            .await
            .unwrap();
        store
            .index_scoped(request("Configuration uses TOML format", &session_id))
            .await
            .unwrap();
        store
            .index_scoped(request("Authentication uses JWT tokens", &other_session_id))
            .await
            .unwrap();

        {
            let entries = store.entries.read().await;
            assert!(
                entries
                    .iter()
                    .all(|entry| entry.scope.includes(&entry.metadata))
            );
            assert_eq!(entries[0].scope.session_id(), &session_id);
        }

        let results = store.search(&scope, "REST API", 10).await.unwrap();
        assert!(!results.is_empty());
        assert!(results[0].content.contains("REST API"));
        assert!(
            results
                .iter()
                .all(|result| scope.includes(&result.metadata))
        );
    }

    #[tokio::test]
    async fn test_search_empty_store() {
        let store = SimpleMemoryStore::new();
        let scope = MemorySearchScope::for_session(SessionId::new());
        let results = store.search(&scope, "anything", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_search_limit() {
        let store = SimpleMemoryStore::new();
        let session_id = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());

        for i in 0..10 {
            store
                .index_scoped(request(format!("Item {i} with keyword test"), &session_id))
                .await
                .unwrap();
        }

        let results = store.search(&scope, "test", 3).await.unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn test_search_no_match() {
        let store = SimpleMemoryStore::new();
        let session_id = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());
        store
            .index_scoped(request("Hello world", &session_id))
            .await
            .unwrap();

        let results = store.search(&scope, "quantum computing", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_index_request_rejects_metadata_outside_scope() {
        let session_id = SessionId::new();
        let other_session_id = SessionId::new();
        let error = MemoryIndexRequest::new(
            MemoryIndexScope::for_session(session_id),
            "outside scope".to_string(),
            meta(&other_session_id),
        )
        .unwrap_err();

        assert!(matches!(error, MemoryStoreError::Scope(_)));
    }
}
