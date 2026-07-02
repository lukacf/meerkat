//! SimpleMemoryStore — basic in-memory keyword-matching memory store.
//!
//! This is a simple implementation that uses substring matching for search.
//! A production implementation would use vector embeddings (e.g., HNSW).

use async_trait::async_trait;
use meerkat_core::memory::{
    MemoryEnumerationPage, MemoryEnumerationRequest, MemoryIndexBatch, MemoryIndexReceipt,
    MemoryIndexScope, MemoryMetadata, MemoryOwner, MemoryRecord, MemoryResult,
    MemoryScopeDropReceipt, MemorySearchScope, MemoryStore, MemoryStoreError,
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
        let mut entries = self.entries.write().await;
        let mut indexed_entries = 0usize;
        for request in requests {
            let (scope, content, metadata) = request.into_parts();
            // Store-side include/exclude gate (#319): skip content the producer
            // marked non-indexable (Excluded), index the rest.
            if !content.is_indexable() {
                continue;
            }
            entries.push(MemoryEntry {
                scope,
                content: content.into_indexable_text(),
                metadata,
            });
            indexed_entries += 1;
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

    async fn drop_scope(
        &self,
        owner: &MemoryOwner,
    ) -> Result<MemoryScopeDropReceipt, MemoryStoreError> {
        let mut entries = self.entries.write().await;
        let before = entries.len();
        entries.retain(|entry| entry.scope.owner != *owner);
        Ok(MemoryScopeDropReceipt {
            owner: owner.clone(),
            dropped_entries: before - entries.len(),
        })
    }

    async fn enumerate_scoped(
        &self,
        scope: &MemorySearchScope,
        request: MemoryEnumerationRequest,
    ) -> Result<MemoryEnumerationPage, MemoryStoreError> {
        if request.limit == 0 {
            return Err(MemoryStoreError::EnumerationLimitZero);
        }
        let entries = self.entries.read().await;

        // Raw scope rows in insertion order; paging counts raw rows while the
        // typed-metadata filters run afterwards on the selected window
        // (parity with the durable store's SQL LIMIT/OFFSET semantics).
        let scoped: Vec<&MemoryEntry> = entries
            .iter()
            .filter(|entry| entry.scope.owner == scope.owner && scope.includes(&entry.metadata))
            .collect();
        let window_start = request.offset.min(scoped.len());
        let window_end = request
            .offset
            .saturating_add(request.limit)
            .min(scoped.len());
        let rows_scanned = window_end - window_start;

        let records = scoped[window_start..window_end]
            .iter()
            .filter(|entry| request.admits(&entry.metadata))
            .map(|entry| MemoryRecord {
                content: entry.content.clone(),
                metadata: entry.metadata.clone(),
            })
            .collect();
        let next_offset =
            (window_end < scoped.len()).then(|| request.offset.saturating_add(rows_scanned));

        Ok(MemoryEnumerationPage {
            records,
            next_offset,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::memory::{MemoryIndexRequest, MemorySource, MessageRange};
    use meerkat_core::types::SessionId;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn meta(session_id: &SessionId) -> MemoryMetadata {
        MemoryMetadata {
            session_id: session_id.clone(),
            source: MemorySource::Compaction {
                source_range: MessageRange::single(0),
            },
            indexed_at: SystemTime::now(),
        }
    }

    fn request(content: impl Into<String>, session_id: &SessionId) -> MemoryIndexRequest {
        MemoryIndexRequest::new(
            MemoryIndexScope::for_session(session_id.clone()),
            meerkat_core::MemoryIndexableContent::Indexable(content.into()),
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
            meerkat_core::MemoryIndexableContent::Indexable("outside scope".to_string()),
            meta(&other_session_id),
        )
        .unwrap_err();

        assert!(matches!(error, MemoryStoreError::Scope(_)));
    }

    fn request_with(
        content: impl Into<String>,
        session_id: &SessionId,
        source_range: MessageRange,
        indexed_at: SystemTime,
    ) -> MemoryIndexRequest {
        MemoryIndexRequest::new(
            MemoryIndexScope::for_session(session_id.clone()),
            meerkat_core::MemoryIndexableContent::Indexable(content.into()),
            MemoryMetadata {
                session_id: session_id.clone(),
                source: MemorySource::Compaction { source_range },
                indexed_at,
            },
        )
        .unwrap()
    }

    fn enumeration(limit: usize, offset: usize) -> MemoryEnumerationRequest {
        MemoryEnumerationRequest {
            limit,
            offset,
            source_overlap: None,
            indexed_after: None,
        }
    }

    /// Parity with HnswMemoryStore: drop removes exactly the owner's entries
    /// and reports their count; other scopes are untouched.
    #[tokio::test]
    async fn test_drop_scope_removes_only_owner_entries() {
        let store = SimpleMemoryStore::new();
        let session_a = SessionId::new();
        let session_b = SessionId::new();
        let scope_a = MemorySearchScope::for_session(session_a.clone());
        let scope_b = MemorySearchScope::for_session(session_b.clone());

        store
            .index_scoped(request("doomed alpha entry", &session_a))
            .await
            .unwrap();
        store
            .index_scoped(request("doomed beta entry", &session_a))
            .await
            .unwrap();
        store
            .index_scoped(request("surviving gamma entry", &session_b))
            .await
            .unwrap();

        let receipt = store
            .drop_scope(&MemoryOwner::canonical_session(session_a.clone()))
            .await
            .unwrap();
        assert_eq!(receipt.dropped_entries, 2);
        assert_eq!(receipt.owner.session_id(), &session_a);

        let dropped = store.search(&scope_a, "doomed", 10).await.unwrap();
        assert!(dropped.is_empty());
        let surviving = store.search(&scope_b, "surviving gamma", 10).await.unwrap();
        assert_eq!(surviving.len(), 1);

        // Dropping the same scope again is a zero-count no-op.
        let repeat = store
            .drop_scope(&MemoryOwner::canonical_session(session_a))
            .await
            .unwrap();
        assert_eq!(repeat.dropped_entries, 0);
    }

    /// Parity with HnswMemoryStore: enumeration pages raw scope rows in
    /// insertion order with deterministic raw-offset accounting.
    #[tokio::test]
    async fn test_enumerate_scoped_pages_in_insertion_order() {
        let store = SimpleMemoryStore::new();
        let session_id = SessionId::new();
        let other_session = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());

        let texts = ["entry zero", "entry one", "entry two", "entry three"];
        for (i, text) in texts.iter().enumerate() {
            store
                .index_scoped(request(*text, &session_id))
                .await
                .unwrap();
            store
                .index_scoped(request(format!("interloper {i}"), &other_session))
                .await
                .unwrap();
        }

        let first = store
            .enumerate_scoped(&scope, enumeration(3, 0))
            .await
            .unwrap();
        assert_eq!(first.records.len(), 3);
        assert_eq!(first.records[0].content, "entry zero");
        assert_eq!(first.records[1].content, "entry one");
        assert_eq!(first.records[2].content, "entry two");
        assert_eq!(first.next_offset, Some(3));

        let last = store
            .enumerate_scoped(&scope, enumeration(3, 3))
            .await
            .unwrap();
        assert_eq!(last.records.len(), 1);
        assert_eq!(last.records[0].content, "entry three");
        assert_eq!(last.next_offset, None);

        let beyond = store
            .enumerate_scoped(&scope, enumeration(3, 9))
            .await
            .unwrap();
        assert!(beyond.records.is_empty());
        assert_eq!(beyond.next_offset, None);
    }

    /// Parity with HnswMemoryStore: filters run post-window on typed
    /// metadata, so a page may return fewer than `limit` records while
    /// `next_offset` advances by raw rows scanned.
    #[tokio::test]
    async fn test_enumerate_scoped_filters_apply_after_raw_paging() {
        let store = SimpleMemoryStore::new();
        let session_id = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());
        let early = UNIX_EPOCH + Duration::from_secs(1_000);
        let late = UNIX_EPOCH + Duration::from_secs(2_000);

        store
            .index_scoped(request_with(
                "covers zero to five early",
                &session_id,
                MessageRange::new(0, 5).unwrap(),
                early,
            ))
            .await
            .unwrap();
        store
            .index_scoped(request_with(
                "covers five to ten late",
                &session_id,
                MessageRange::new(5, 10).unwrap(),
                late,
            ))
            .await
            .unwrap();
        store
            .index_scoped(request_with(
                "covers ten to fifteen late",
                &session_id,
                MessageRange::new(10, 15).unwrap(),
                late,
            ))
            .await
            .unwrap();

        // source_overlap admits only the middle record; all three raw rows
        // are scanned.
        let overlap = store
            .enumerate_scoped(
                &scope,
                MemoryEnumerationRequest {
                    limit: 10,
                    offset: 0,
                    source_overlap: Some(MessageRange::new(6, 8).unwrap()),
                    indexed_after: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(overlap.records.len(), 1);
        assert_eq!(overlap.records[0].content, "covers five to ten late");
        assert_eq!(overlap.next_offset, None);

        // indexed_after is strict: the boundary instant is excluded.
        let after = store
            .enumerate_scoped(
                &scope,
                MemoryEnumerationRequest {
                    limit: 10,
                    offset: 0,
                    source_overlap: None,
                    indexed_after: Some(early),
                },
            )
            .await
            .unwrap();
        assert_eq!(after.records.len(), 2);
        assert!(after.records.iter().all(|r| r.content.contains("late")));

        // Zero-limit pages cannot advance the cursor and are rejected with
        // the typed error (a follow-`next_offset` loop would never
        // terminate) — parity with the durable store.
        let error = store
            .enumerate_scoped(&scope, enumeration(0, 1))
            .await
            .expect_err("limit zero must be rejected");
        assert!(matches!(
            error,
            meerkat_core::memory::MemoryStoreError::EnumerationLimitZero
        ));
    }
}
