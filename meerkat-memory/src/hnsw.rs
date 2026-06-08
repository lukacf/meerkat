//! HnswMemoryStore — HNSW-backed semantic memory store.
//!
//! Uses `hnsw_rs` for approximate nearest-neighbor search and SQLite for
//! metadata persistence. Embeddings use a simple bag-of-words TF approach
//! with cosine distance — upgrade to a proper embedding model for production.
//!
//! Index stored at `.rkat/memory/`.

use async_trait::async_trait;
use hnsw_rs::prelude::{DistCosine, Hnsw};
use meerkat_core::memory::{
    EmbeddingModel, HnswParams, MemoryIndexBatch, MemoryIndexReceipt, MemoryMetadata,
    MemoryRankingPolicy, MemoryResult, MemorySearchScope, MemoryStore, MemoryStoreError,
};
use meerkat_core::types::SessionId;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;

const SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;

const CREATE_MEMORY_SCHEMA_SQL: &str = r"
CREATE TABLE IF NOT EXISTS memory_metadata (
    point_id INTEGER PRIMARY KEY,
    metadata_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS memory_text (
    point_id INTEGER PRIMARY KEY,
    content BLOB NOT NULL
)";

/// Default vocabulary dimension for the bag-of-words embedding model.
const DEFAULT_VOCAB_DIM: usize = 4096;

/// Minimum max-elements hint for an allocated HNSW index.
const MIN_INDEX_ELEMENTS_HINT: usize = 1;

/// Default semantic-memory ranking policy: hash-based bag-of-words embeddings
/// with the baseline HNSW parameters.
pub fn default_ranking_policy() -> MemoryRankingPolicy {
    MemoryRankingPolicy::new(
        Arc::new(BagOfWordsEmbeddingModel::new(DEFAULT_VOCAB_DIM)),
        HnswParams::default(),
    )
}

/// Simple bag-of-words embedding model using hash-based dimensionality
/// reduction.
///
/// Each word is hashed to a bucket in `[0, vocab_dim)` and its presence
/// increments that dimension; the vector is then L2-normalized for cosine
/// distance compatibility.
///
/// This is a baseline. For production quality, substitute a proper embedding
/// model (e.g., sentence-transformers via ONNX runtime) by injecting a
/// different [`MemoryRankingPolicy`].
pub struct BagOfWordsEmbeddingModel {
    vocab_dim: usize,
}

impl BagOfWordsEmbeddingModel {
    pub fn new(vocab_dim: usize) -> Self {
        Self { vocab_dim }
    }
}

impl EmbeddingModel for BagOfWordsEmbeddingModel {
    fn dimension(&self) -> usize {
        self.vocab_dim
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        let mut vec = vec![0.0f32; self.vocab_dim];
        for word in text.split_whitespace() {
            let hash = word.bytes().fold(0usize, |acc, b| {
                acc.wrapping_mul(31)
                    .wrapping_add(b.to_ascii_lowercase() as usize)
            }) % self.vocab_dim;
            vec[hash] += 1.0;
        }

        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut vec {
                *x /= norm;
            }
        }

        vec
    }
}

fn open_connection(path: &Path) -> Result<Connection, MemoryStoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(MemoryStoreError::Io)?;
    }
    let conn = Connection::open(path).map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
    conn.busy_timeout(Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))
        .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
    conn.pragma_update(None, "synchronous", "FULL")
        .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
    conn.execute_batch(CREATE_MEMORY_SCHEMA_SQL)
        .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
    Ok(conn)
}

type MemoryHnswIndex = Hnsw<'static, f32, DistCosine>;

fn bounded_index_elements_hint(entries: usize) -> usize {
    entries.max(MIN_INDEX_ELEMENTS_HINT)
}

fn new_hnsw_index(max_elements_hint: usize, params: HnswParams) -> MemoryHnswIndex {
    Hnsw::<'static, f32, DistCosine>::new(
        params.max_nb_connection,
        bounded_index_elements_hint(max_elements_hint),
        params.max_layer,
        params.ef_construction,
        DistCosine {},
    )
}

/// Rebuild a single scoped HNSW index from the rows that currently survive in
/// SQLite for `session_id`.
///
/// This is the per-scope analogue of the full re-index performed by
/// [`HnswMemoryStore::open`]: it derives the live index purely from committed
/// DB state, so callers can repair a scope after a partial in-memory insert
/// (where the DB rows were rolled back) without leaving phantom neighbor slots
/// that point at DB-deleted records.
fn rebuild_scoped_index_from_db(
    conn: &Connection,
    session_id: &SessionId,
    embedding_model: &dyn EmbeddingModel,
    params: HnswParams,
) -> Result<ScopedHnswIndex, MemoryStoreError> {
    let mut rows = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT t.point_id, t.content, m.metadata_json \
                 FROM memory_text t \
                 JOIN memory_metadata m ON m.point_id = t.point_id \
                 ORDER BY t.point_id ASC",
            )
            .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
        let mapped = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
        for row in mapped {
            rows.push(row.map_err(|e| MemoryStoreError::Storage(e.to_string()))?);
        }
    }

    let mut scoped: Vec<(usize, String)> = Vec::new();
    for (point_id, text, metadata_json) in rows {
        let metadata: MemoryMetadata = serde_json::from_slice(&metadata_json)
            .map_err(|e| MemoryStoreError::Embedding(e.to_string()))?;
        if &metadata.session_id != session_id {
            continue;
        }
        let point_id =
            usize::try_from(point_id).map_err(|_| MemoryStoreError::PointIdOutOfRange)?;
        scoped.push((point_id, String::from_utf8_lossy(&text).into_owned()));
    }

    let index = ScopedHnswIndex::new(scoped.len(), params);
    for (point_id, text) in &scoped {
        let embedding = embedding_model.embed(text);
        index.insert(&embedding, *point_id);
    }
    Ok(index)
}

struct ScopedHnswIndex {
    index: MemoryHnswIndex,
    #[cfg(test)]
    max_elements_hint: usize,
}

impl ScopedHnswIndex {
    fn new(max_elements_hint: usize, params: HnswParams) -> Self {
        let max_elements_hint = bounded_index_elements_hint(max_elements_hint);
        Self {
            index: new_hnsw_index(max_elements_hint, params),
            #[cfg(test)]
            max_elements_hint,
        }
    }

    fn insert(&self, embedding: &[f32], point_id: usize) {
        self.index.insert((embedding, point_id));
    }
}

/// HNSW-backed memory store with SQLite metadata persistence.
pub struct HnswMemoryStore {
    // SAFETY NOTE: we use `'static` because `hnsw_rs` 0.3 copies inserted vectors
    // into owned internal storage. If a future `hnsw_rs` release changes this to
    // borrow caller memory, this type must be revisited before upgrading.
    indices: Arc<std::sync::RwLock<HashMap<SessionId, ScopedHnswIndex>>>,
    db_path: PathBuf,
    next_id: AtomicUsize,
    insert_lock: Mutex<()>,
    path: PathBuf,
    /// Injected typed ranking policy: the authority for embedding generation
    /// and HNSW index parameters (not store-local constants).
    policy: MemoryRankingPolicy,
    /// Test-only fault injector: when set, the in-memory HNSW insert loop fails
    /// after the configured number of successful inserts within a batch,
    /// simulating a partial mid-batch failure so the rollback/repair path can be
    /// exercised deterministically. Production builds never carry this field.
    #[cfg(test)]
    fail_hnsw_insert_after: Arc<std::sync::atomic::AtomicI64>,
}

impl HnswMemoryStore {
    /// Open or create a memory store at the given directory using the default
    /// ranking policy ([`default_ranking_policy`]).
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, MemoryStoreError> {
        Self::open_with_policy(dir, default_ranking_policy())
    }

    /// Open or create a memory store at the given directory with an injected
    /// typed ranking policy that owns embedding generation and HNSW parameters.
    pub fn open_with_policy(
        dir: impl AsRef<Path>,
        policy: MemoryRankingPolicy,
    ) -> Result<Self, MemoryStoreError> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir).map_err(MemoryStoreError::Io)?;
        let hnsw_params = policy.hnsw_params();

        let db_path = dir.join("memory.sqlite3");
        let conn = open_connection(&db_path)?;

        let next_id = {
            let max_id: Option<i64> = conn
                .query_row("SELECT MAX(point_id) FROM memory_metadata", [], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(|e| MemoryStoreError::Storage(e.to_string()))?
                .flatten();
            match max_id {
                Some(value) => {
                    usize::try_from(value + 1).map_err(|_| MemoryStoreError::PointIdOutOfRange)?
                }
                None => 0,
            }
        };

        let mut session_counts = HashMap::<SessionId, usize>::new();
        {
            let mut count_stmt = conn
                .prepare("SELECT metadata_json FROM memory_metadata")
                .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
            let metadata_rows = count_stmt
                .query_map([], |row| row.get::<_, Vec<u8>>(0))
                .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
            for metadata_json in metadata_rows {
                let metadata_json =
                    metadata_json.map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
                let metadata: MemoryMetadata = serde_json::from_slice(&metadata_json)
                    .map_err(|e| MemoryStoreError::Embedding(e.to_string()))?;
                *session_counts.entry(metadata.session_id).or_default() += 1;
            }
        }

        let mut indices = HashMap::<SessionId, ScopedHnswIndex>::new();

        let mut stmt = conn
            .prepare(
                "SELECT t.point_id, t.content, m.metadata_json \
                 FROM memory_text t \
                 JOIN memory_metadata m ON m.point_id = t.point_id \
                 ORDER BY t.point_id ASC",
            )
            .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
        for row in rows {
            let (point_id, text, metadata_json) =
                row.map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
            let point_id =
                usize::try_from(point_id).map_err(|_| MemoryStoreError::PointIdOutOfRange)?;
            let metadata: MemoryMetadata = serde_json::from_slice(&metadata_json)
                .map_err(|e| MemoryStoreError::Embedding(e.to_string()))?;
            let text = String::from_utf8_lossy(&text);
            let embedding = policy.embed(&text);
            let max_elements_hint = *session_counts
                .get(&metadata.session_id)
                .unwrap_or(&MIN_INDEX_ELEMENTS_HINT);
            indices
                .entry(metadata.session_id)
                .or_insert_with(|| ScopedHnswIndex::new(max_elements_hint, hnsw_params))
                .insert(&embedding, point_id);
        }

        Ok(Self {
            indices: Arc::new(std::sync::RwLock::new(indices)),
            db_path,
            next_id: AtomicUsize::new(next_id),
            insert_lock: Mutex::new(()),
            path: dir.to_path_buf(),
            policy,
            #[cfg(test)]
            fail_hnsw_insert_after: Arc::new(std::sync::atomic::AtomicI64::new(-1)),
        })
    }

    /// Get the storage directory path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used)]
    fn hnsw_index_count(&self) -> usize {
        self.indices.read().unwrap().len()
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used)]
    fn hnsw_point_count(&self) -> usize {
        self.indices
            .read()
            .unwrap()
            .values()
            .map(|index| index.index.get_nb_point())
            .sum()
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used)]
    fn hnsw_index_hints(&self) -> Vec<usize> {
        self.indices
            .read()
            .unwrap()
            .values()
            .map(|index| index.max_elements_hint)
            .collect()
    }

    /// Test-only: arm the in-memory HNSW insert loop to fail after `after`
    /// successful inserts within the next batch (`-1` disables injection).
    #[cfg(test)]
    fn arm_hnsw_insert_failure_after(&self, after: i64) {
        self.fail_hnsw_insert_after
            .store(after, std::sync::atomic::Ordering::Release);
    }
}

#[async_trait]
impl MemoryStore for HnswMemoryStore {
    async fn index_scoped_batch(
        &self,
        batch: MemoryIndexBatch,
    ) -> Result<MemoryIndexReceipt, MemoryStoreError> {
        let (receipt_scope, requests) = batch.into_parts();
        let mut entries = Vec::with_capacity(requests.len());
        for request in requests {
            let (_scope, content, metadata) = request.into_parts();
            // Store-side include/exclude gate (#319): the producer marks each
            // message Indexable(text) or Excluded(reason) via the typed
            // MemoryIndexableContent. The store indexes the former and skips
            // the latter, rather than the producer pre-flattening to a String
            // and dropping empties blindly.
            if !content.is_indexable() {
                continue;
            }
            let text = content.into_indexable_text();
            let meta_json = serde_json::to_vec(&metadata)
                .map_err(|e| MemoryStoreError::Embedding(e.to_string()))?;
            let embedding = self.policy.embed(&text);
            entries.push((text, meta_json, embedding));
        }
        let indexed_entries = entries.len();
        if indexed_entries == 0 {
            return Ok(MemoryIndexReceipt {
                scope: receipt_scope,
                indexed_entries: 0,
            });
        }

        let db_path = self.db_path.clone();
        let indices = Arc::clone(&self.indices);
        let session_id = receipt_scope.session_id().clone();
        let hnsw_params = self.policy.hnsw_params();
        let embedding_model = Arc::clone(self.policy.embedding_model());
        #[cfg(test)]
        let fail_hnsw_insert_after = Arc::clone(&self.fail_hnsw_insert_after);

        // Keep point ID allocation coupled to a successful write.
        let _guard = self.insert_lock.lock().await;
        let point_id = self.next_id.load(Ordering::Acquire);
        let next_id = point_id
            .checked_add(indexed_entries)
            .ok_or(MemoryStoreError::PointIdOverflow)?;
        let point_ids: Vec<i64> = (point_id..next_id)
            .map(|id| i64::try_from(id).map_err(|_| MemoryStoreError::PointIdOutOfRange))
            .collect::<Result<_, _>>()?;

        let insert_result = tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&db_path)?;
            let tx = conn
                .transaction()
                .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
            for (point_id_i64, (content, meta_json, _embedding)) in point_ids.iter().zip(&entries) {
                tx.execute(
                    "INSERT INTO memory_metadata (point_id, metadata_json) VALUES (?1, ?2)",
                    params![point_id_i64, meta_json],
                )
                .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
                tx.execute(
                    "INSERT INTO memory_text (point_id, content) VALUES (?1, ?2)",
                    params![point_id_i64, content.as_bytes()],
                )
                .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
            }
            tx.commit()
                .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;

            let index_result = (|| {
                let mut indices = indices
                    .write()
                    .map_err(|_| MemoryStoreError::LockPoisoned)?;
                let index = indices
                    .entry(session_id.clone())
                    .or_insert_with(|| ScopedHnswIndex::new(indexed_entries, hnsw_params));
                // The enumerate index is consumed only by the `cfg(test)`
                // fault-injection seam below; in non-test builds it is unused.
                #[allow(clippy::unused_enumerate_index)]
                for (_ordinal, (point_id, (_content, _meta_json, embedding))) in
                    point_ids.iter().zip(&entries).enumerate()
                {
                    // Test-only fault injection: simulate a partial mid-batch
                    // HNSW failure after `fail_hnsw_insert_after` successful
                    // inserts so the rollback/repair path can be exercised.
                    #[cfg(test)]
                    {
                        let fail_after =
                            fail_hnsw_insert_after.load(std::sync::atomic::Ordering::Acquire);
                        if fail_after >= 0 && _ordinal as i64 >= fail_after {
                            return Err(MemoryStoreError::LockPoisoned);
                        }
                    }
                    let point_id = usize::try_from(*point_id)
                        .map_err(|_| MemoryStoreError::PointIdOutOfRange)?;
                    index.insert(embedding, point_id);
                }
                Ok::<(), MemoryStoreError>(())
            })();

            if let Err(error) = index_result {
                let mut cleanup = open_connection(&db_path)?;
                let tx = cleanup
                    .transaction()
                    .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
                for point_id_i64 in &point_ids {
                    tx.execute(
                        "DELETE FROM memory_metadata WHERE point_id = ?1",
                        params![point_id_i64],
                    )
                    .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
                    tx.execute(
                        "DELETE FROM memory_text WHERE point_id = ?1",
                        params![point_id_i64],
                    )
                    .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
                }
                tx.commit()
                    .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;

                // `hnsw_rs` cannot remove already-inserted points, so a partial
                // insert above may have left live neighbor slots for point_ids
                // we just deleted from the DB. Rebuild the scope's live index
                // purely from the surviving committed rows so a subsequent
                // search in this same process never selects phantom (DB-deleted)
                // neighbor slots. This mirrors the full re-index that crash
                // recovery performs on `open`. Fail closed: if the rebuild
                // cannot be derived from the DB, surface the typed fault rather
                // than leaving a stale live index.
                let repaired = rebuild_scoped_index_from_db(
                    &cleanup,
                    &session_id,
                    embedding_model.as_ref(),
                    hnsw_params,
                )?;
                let mut indices = indices
                    .write()
                    .map_err(|_| MemoryStoreError::LockPoisoned)?;
                indices.insert(session_id, repaired);
                return Err(error);
            }

            Ok::<(), MemoryStoreError>(())
        })
        .await
        .map_err(|e| MemoryStoreError::TaskJoin(format!("index task join failed: {e}")))?;

        if let Err(error) = insert_result {
            self.next_id.store(next_id, Ordering::Release);
            return Err(error);
        }

        self.next_id.store(next_id, Ordering::Release);
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
        if limit == 0 {
            return Ok(Vec::new());
        }
        let query = query.to_owned();
        let scope = scope.clone();
        let db_path = self.db_path.clone();
        let indices = Arc::clone(&self.indices);
        let embedding_model = Arc::clone(self.policy.embedding_model());
        let ef_search = self.policy.hnsw_params().ef_search;

        tokio::task::spawn_blocking(move || {
            let embedding = embedding_model.embed(&query);
            let neighbors = {
                let indices = indices.read().map_err(|_| MemoryStoreError::LockPoisoned)?;
                let Some(index) = indices.get(scope.session_id()) else {
                    return Ok(Vec::new());
                };
                index.index.search(&embedding, limit, limit.max(ef_search))
            };

            let conn = open_connection(&db_path)?;
            let mut results = Vec::with_capacity(neighbors.len());
            for neighbor in &neighbors {
                let point_id = i64::try_from(neighbor.d_id)
                    .map_err(|_| MemoryStoreError::PointIdOutOfRange)?;

                let content = match conn
                    .query_row(
                        "SELECT content FROM memory_text WHERE point_id = ?1",
                        params![point_id],
                        |row| row.get::<_, Vec<u8>>(0),
                    )
                    .optional()
                    .map_err(|e| MemoryStoreError::Storage(e.to_string()))?
                {
                    Some(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                    None => continue,
                };

                let metadata = match conn
                    .query_row(
                        "SELECT metadata_json FROM memory_metadata WHERE point_id = ?1",
                        params![point_id],
                        |row| row.get::<_, Vec<u8>>(0),
                    )
                    .optional()
                    .map_err(|e| MemoryStoreError::Storage(e.to_string()))?
                {
                    Some(bytes) => serde_json::from_slice(&bytes)
                        .map_err(|e| MemoryStoreError::Embedding(e.to_string()))?,
                    None => continue,
                };
                if !scope.includes(&metadata) {
                    continue;
                }

                // HNSW distance is cosine distance (0 = identical, 2 = opposite).
                // Convert to a 0..1 similarity score.
                let score = 1.0 - (neighbor.distance / 2.0);

                results.push(MemoryResult {
                    content,
                    metadata,
                    score,
                });
            }

            Ok::<Vec<MemoryResult>, MemoryStoreError>(results)
        })
        .await
        .map_err(|e| MemoryStoreError::TaskJoin(format!("search task join failed: {e}")))?
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::memory::{
        MemoryIndexBatch, MemoryIndexRequest, MemoryIndexScope, MemoryMetadata, MemorySource,
        MessageRange,
    };
    use meerkat_core::types::SessionId;
    use std::time::SystemTime;
    use tempfile::TempDir;

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
    async fn test_hnsw_index_and_search() {
        let dir = TempDir::new().unwrap();
        let store = HnswMemoryStore::open(dir.path().join("memory")).unwrap();
        let session_id = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());
        let other_session_id = SessionId::new();

        store
            .index_scoped(request(
                "The user wants to implement a REST API with authentication",
                &session_id,
            ))
            .await
            .unwrap();
        store
            .index_scoped(request(
                "Configuration files use TOML format for settings",
                &session_id,
            ))
            .await
            .unwrap();
        store
            .index_scoped(request(
                "JWT tokens handle authentication and authorization",
                &other_session_id,
            ))
            .await
            .unwrap();

        let results = store
            .search(&scope, "REST API authentication", 10)
            .await
            .unwrap();
        assert!(!results.is_empty());
        assert!(
            results
                .iter()
                .all(|result| scope.includes(&result.metadata))
        );
        assert!(
            results[0].content.contains("REST") || results[0].content.contains("authentication"),
            "Top result should be relevant: {}",
            results[0].content
        );
    }

    #[tokio::test]
    async fn test_hnsw_search_empty_store() {
        let dir = TempDir::new().unwrap();
        let store = HnswMemoryStore::open(dir.path().join("memory")).unwrap();
        let scope = MemorySearchScope::for_session(SessionId::new());

        let results = store.search(&scope, "anything", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_hnsw_search_limit() {
        let dir = TempDir::new().unwrap();
        let store = HnswMemoryStore::open(dir.path().join("memory")).unwrap();
        let session_id = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());

        for i in 0..10 {
            store
                .index_scoped(request(
                    format!("Item {i} with keyword test data"),
                    &session_id,
                ))
                .await
                .unwrap();
        }

        let results = store.search(&scope, "test", 3).await.unwrap();
        assert!(results.len() <= 3);
    }

    #[tokio::test]
    async fn test_hnsw_search_scopes_before_candidate_selection() {
        let dir = TempDir::new().unwrap();
        let store = HnswMemoryStore::open(dir.path().join("memory")).unwrap();
        let session_id = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());

        for _ in 0..32 {
            let other_session_id = SessionId::new();
            store
                .index_scoped(request(
                    "needle recall exact global candidate",
                    &other_session_id,
                ))
                .await
                .unwrap();
        }
        store
            .index_scoped(request("needle recall scoped survivor", &session_id))
            .await
            .unwrap();

        let results = store
            .search(&scope, "needle recall exact global candidate", 1)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].metadata.session_id, session_id);
        assert!(
            results[0].content.contains("scoped survivor"),
            "scoped candidates must be ranked before the limit is applied"
        );
    }

    /// Gate (#240): a partial in-memory HNSW failure mid-batch must not leave
    /// phantom neighbor slots. The DB rows for the failed batch are rolled back,
    /// so the live scoped index is repaired (rebuilt from surviving DB rows) and
    /// a subsequent search in the same live process selects no DB-deleted point.
    #[tokio::test]
    async fn test_partial_hnsw_failure_repairs_live_index_no_phantom_neighbors() {
        let dir = TempDir::new().unwrap();
        let store = HnswMemoryStore::open(dir.path().join("memory")).unwrap();
        let session_id = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());

        // A clean committed entry that must remain the only live candidate.
        store
            .index_scoped(request("alpha survivor entry one", &session_id))
            .await
            .unwrap();
        assert_eq!(store.hnsw_point_count(), 1);

        // Arm a partial mid-batch failure: the first insert of the next batch
        // lands in the live HNSW, the rest fail, and the whole batch's DB rows
        // are rolled back. Without repair, the live index would retain a slot
        // for the now-DB-deleted first point.
        store.arm_hnsw_insert_failure_after(1);
        let batch = MemoryIndexBatch::new(
            MemoryIndexScope::for_session(session_id.clone()),
            vec![
                request("beta doomed entry two", &session_id),
                request("gamma doomed entry three", &session_id),
                request("delta doomed entry four", &session_id),
            ],
        )
        .unwrap();
        let err = store.index_scoped_batch(batch).await.unwrap_err();
        assert_eq!(err.error_code(), "memory_lock_poisoned");

        // Disarm injection for subsequent searches.
        store.arm_hnsw_insert_failure_after(-1);

        // The live index now matches the DB exactly: only the one survivor.
        assert_eq!(
            store.hnsw_point_count(),
            1,
            "repaired live index must contain exactly the surviving DB rows"
        );

        // Search against the text of a DB-deleted (doomed) entry. The repaired
        // index must not surface a phantom neighbor slot for the rolled-back
        // point; only DB-present content can be returned.
        let results = store
            .search(&scope, "beta doomed entry two", 10)
            .await
            .unwrap();
        for result in &results {
            assert!(
                result.content.contains("survivor"),
                "search must only return DB-present content, got: {}",
                result.content
            );
        }
        assert!(
            results.iter().all(|r| scope.includes(&r.metadata)),
            "all results stay within the requested scope"
        );

        // The survivor is still discoverable by its own text.
        let survivor = store
            .search(&scope, "alpha survivor entry one", 1)
            .await
            .unwrap();
        assert_eq!(survivor.len(), 1);
        assert!(survivor[0].content.contains("survivor"));
    }

    #[tokio::test]
    async fn test_hnsw_many_small_scopes_use_bounded_index_hints_across_reopen() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let mut session_ids = Vec::new();

        {
            let store = HnswMemoryStore::open(&memory_dir).unwrap();
            for i in 0..48 {
                let session_id = SessionId::new();
                store
                    .index_scoped(request(
                        format!("single scoped memory entry {i}"),
                        &session_id,
                    ))
                    .await
                    .unwrap();
                session_ids.push(session_id);
            }

            assert_eq!(
                store.hnsw_index_count(),
                session_ids.len(),
                "one-entry scopes keep separate scoped indexes for recall"
            );
            assert!(
                store
                    .hnsw_index_hints()
                    .iter()
                    .all(|hint| *hint == MIN_INDEX_ELEMENTS_HINT),
                "many one-entry scopes must not allocate oversized HNSW indexes"
            );
            assert_eq!(store.hnsw_point_count(), session_ids.len());
        }

        {
            let store = HnswMemoryStore::open(&memory_dir).unwrap();
            assert_eq!(
                store.hnsw_index_count(),
                session_ids.len(),
                "reopen keeps scoped indexes for recall"
            );
            assert!(
                store
                    .hnsw_index_hints()
                    .iter()
                    .all(|hint| *hint == MIN_INDEX_ELEMENTS_HINT),
                "reopen must rebuild one-entry scoped indexes with bounded hints"
            );
            assert_eq!(store.hnsw_point_count(), session_ids.len());

            let last_session = session_ids.last().unwrap();
            let results = store
                .search(
                    &MemorySearchScope::for_session(last_session.clone()),
                    "single scoped memory entry 47",
                    1,
                )
                .await
                .unwrap();
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].metadata.session_id, *last_session);
        }
    }

    #[tokio::test]
    async fn test_hnsw_persists_across_reopen() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let session_id = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());

        {
            let store = HnswMemoryStore::open(&memory_dir).unwrap();
            store
                .index_scoped(request(
                    "Persistent memory entry about Rust programming",
                    &session_id,
                ))
                .await
                .unwrap();
        }

        {
            let store = HnswMemoryStore::open(&memory_dir).unwrap();
            let results = store.search(&scope, "Rust programming", 5).await.unwrap();
            assert!(!results.is_empty(), "Data should survive reopen");
            assert!(results[0].content.contains("Rust"));
        }
    }

    #[tokio::test]
    async fn test_hnsw_score_range() {
        let dir = TempDir::new().unwrap();
        let store = HnswMemoryStore::open(dir.path().join("memory")).unwrap();
        let session_id = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());

        store
            .index_scoped(request("Exact match query text here", &session_id))
            .await
            .unwrap();

        let results = store
            .search(&scope, "Exact match query text here", 1)
            .await
            .unwrap();
        assert!(!results.is_empty());
        assert!(
            results[0].score > 0.9,
            "Exact match should have high score, got: {}",
            results[0].score
        );
        assert!(results[0].score <= 1.0);
        assert!(results[0].score >= 0.0);
    }

    /// Embedding model that maps every input to a single fixed bucket, so all
    /// content collapses to one ranking vector regardless of text.
    struct ConstantEmbeddingModel {
        dim: usize,
    }

    impl EmbeddingModel for ConstantEmbeddingModel {
        fn dimension(&self) -> usize {
            self.dim
        }

        fn embed(&self, _text: &str) -> Vec<f32> {
            let mut v = vec![0.0f32; self.dim];
            v[0] = 1.0;
            v
        }
    }

    #[tokio::test]
    async fn test_injected_policy_is_ranking_authority() {
        let session_id = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());

        // Default policy: distinct text embeds distinctly, so an unrelated query
        // does not perfectly match indexed content.
        let default_score = {
            let dir = TempDir::new().unwrap();
            let store = HnswMemoryStore::open(dir.path().join("memory")).unwrap();
            store
                .index_scoped(request("alpha beta gamma", &session_id))
                .await
                .unwrap();
            let results = store
                .search(&scope, "completely unrelated query", 1)
                .await
                .unwrap();
            results.first().map(|r| r.score)
        };

        // Injected constant policy: every text collapses to the same vector, so
        // the unrelated query matches the indexed content with score ~1.0.
        let constant_score = {
            let dir = TempDir::new().unwrap();
            let policy = MemoryRankingPolicy::new(
                Arc::new(ConstantEmbeddingModel { dim: 16 }),
                HnswParams::default(),
            );
            let store =
                HnswMemoryStore::open_with_policy(dir.path().join("memory"), policy).unwrap();
            store
                .index_scoped(request("alpha beta gamma", &session_id))
                .await
                .unwrap();
            let results = store
                .search(&scope, "completely unrelated query", 1)
                .await
                .unwrap();
            results.first().map(|r| r.score)
        };

        // The injected policy is the authority: ranking output differs from the
        // default hard-coded scheme.
        let constant_score = constant_score.expect("constant policy matches all content");
        assert!(
            constant_score > 0.99,
            "constant embedding policy must rank unrelated text as a match, got {constant_score}"
        );
        assert!(
            default_score.map(|s| s < 0.99).unwrap_or(true),
            "default policy must not rank unrelated text as a perfect match"
        );
    }

    #[tokio::test]
    async fn test_distinct_failures_surface_as_distinct_typed_variants() {
        // Embedding/metadata serialization failures vs storage faults are
        // distinguishable typed variants, not a single stringly arm.
        assert_eq!(
            MemoryStoreError::Embedding("x".into()).error_code(),
            "memory_embedding"
        );
        assert_eq!(
            MemoryStoreError::Storage("x".into()).error_code(),
            "memory_storage"
        );
        assert_eq!(
            MemoryStoreError::LockPoisoned.error_code(),
            "memory_lock_poisoned"
        );
        assert_eq!(
            MemoryStoreError::PointIdOverflow.error_code(),
            "memory_point_id_overflow"
        );
        assert_eq!(
            MemoryStoreError::PointIdOutOfRange.error_code(),
            "memory_point_id_out_of_range"
        );
    }
}
