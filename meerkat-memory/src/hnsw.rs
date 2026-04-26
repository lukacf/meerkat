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
    MemoryMetadata, MemoryResult, MemorySearchScope, MemoryStore, MemoryStoreError,
};
use rusqlite::{Connection, OptionalExtension, params};
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

/// Vocabulary dimension for bag-of-words vectors.
const VOCAB_DIM: usize = 4096;

/// Maximum neighbors per layer in HNSW.
const MAX_NB_CONNECTION: usize = 16;
/// Maximum HNSW layers.
const MAX_LAYER: usize = 16;
/// Construction-time exploration factor.
const EF_CONSTRUCTION: usize = 200;
/// Default max elements hint.
const DEFAULT_MAX_ELEMENTS: usize = 100_000;

fn open_connection(path: &Path) -> Result<Connection, MemoryStoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(MemoryStoreError::Io)?;
    }
    let conn = Connection::open(path).map_err(|e| MemoryStoreError::Index(e.to_string()))?;
    conn.busy_timeout(Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))
        .map_err(|e| MemoryStoreError::Index(e.to_string()))?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| MemoryStoreError::Index(e.to_string()))?;
    conn.pragma_update(None, "synchronous", "FULL")
        .map_err(|e| MemoryStoreError::Index(e.to_string()))?;
    conn.execute_batch(CREATE_MEMORY_SCHEMA_SQL)
        .map_err(|e| MemoryStoreError::Index(e.to_string()))?;
    Ok(conn)
}

/// HNSW-backed memory store with SQLite metadata persistence.
pub struct HnswMemoryStore {
    // SAFETY NOTE: we use `'static` because `hnsw_rs` 0.3 copies inserted vectors
    // into owned internal storage. If a future `hnsw_rs` release changes this to
    // borrow caller memory, this type must be revisited before upgrading.
    index: Arc<std::sync::RwLock<Hnsw<'static, f32, DistCosine>>>,
    db_path: PathBuf,
    next_id: AtomicUsize,
    insert_lock: Mutex<()>,
    path: PathBuf,
}

impl HnswMemoryStore {
    /// Open or create a memory store at the given directory.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, MemoryStoreError> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir).map_err(MemoryStoreError::Io)?;

        let db_path = dir.join("memory.sqlite3");
        let conn = open_connection(&db_path)?;

        let next_id = {
            let max_id: Option<i64> = conn
                .query_row("SELECT MAX(point_id) FROM memory_metadata", [], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(|e| MemoryStoreError::Index(e.to_string()))?
                .flatten();
            match max_id {
                Some(value) => usize::try_from(value + 1)
                    .map_err(|_| MemoryStoreError::Index("point ID out of range".to_string()))?,
                None => 0,
            }
        };

        let hnsw = Hnsw::<'static, f32, DistCosine>::new(
            MAX_NB_CONNECTION,
            DEFAULT_MAX_ELEMENTS,
            MAX_LAYER,
            EF_CONSTRUCTION,
            DistCosine {},
        );

        let mut stmt = conn
            .prepare("SELECT point_id, content FROM memory_text ORDER BY point_id ASC")
            .map_err(|e| MemoryStoreError::Index(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(|e| MemoryStoreError::Index(e.to_string()))?;
        for row in rows {
            let (point_id, text) = row.map_err(|e| MemoryStoreError::Index(e.to_string()))?;
            let point_id = usize::try_from(point_id)
                .map_err(|_| MemoryStoreError::Index("point ID out of range".to_string()))?;
            let text = String::from_utf8_lossy(&text);
            let embedding = text_to_embedding(&text);
            hnsw.insert((&embedding, point_id));
        }

        Ok(Self {
            index: Arc::new(std::sync::RwLock::new(hnsw)),
            db_path,
            next_id: AtomicUsize::new(next_id),
            insert_lock: Mutex::new(()),
            path: dir.to_path_buf(),
        })
    }

    /// Get the storage directory path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[async_trait]
impl MemoryStore for HnswMemoryStore {
    async fn index(&self, content: &str, metadata: MemoryMetadata) -> Result<(), MemoryStoreError> {
        let meta_json = serde_json::to_vec(&metadata)
            .map_err(|e| MemoryStoreError::Embedding(e.to_string()))?;
        let content = content.to_owned();
        let db_path = self.db_path.clone();
        let index = Arc::clone(&self.index);

        // Keep point ID allocation coupled to a successful write.
        let _guard = self.insert_lock.lock().await;
        let point_id = self.next_id.load(Ordering::Acquire);
        let point_id_i64 = i64::try_from(point_id)
            .map_err(|_| MemoryStoreError::Index("point ID out of range".to_string()))?;

        tokio::task::spawn_blocking(move || {
            let embedding = text_to_embedding(&content);
            let mut conn = open_connection(&db_path)?;
            let tx = conn
                .transaction()
                .map_err(|e| MemoryStoreError::Index(e.to_string()))?;
            tx.execute(
                "INSERT INTO memory_metadata (point_id, metadata_json) VALUES (?1, ?2)",
                params![point_id_i64, meta_json],
            )
            .map_err(|e| MemoryStoreError::Index(e.to_string()))?;
            tx.execute(
                "INSERT INTO memory_text (point_id, content) VALUES (?1, ?2)",
                params![point_id_i64, content.as_bytes()],
            )
            .map_err(|e| MemoryStoreError::Index(e.to_string()))?;
            tx.commit()
                .map_err(|e| MemoryStoreError::Index(e.to_string()))?;

            let index = index
                .write()
                .map_err(|_| MemoryStoreError::Index("HNSW index lock poisoned".to_string()))?;
            index.insert((&embedding, point_id));

            Ok::<(), MemoryStoreError>(())
        })
        .await
        .map_err(|e| MemoryStoreError::Index(format!("index task join failed: {e}")))??;

        let next_id = point_id
            .checked_add(1)
            .ok_or_else(|| MemoryStoreError::Index("point ID overflow".to_string()))?;
        self.next_id.store(next_id, Ordering::Release);
        Ok(())
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
        let index = Arc::clone(&self.index);

        tokio::task::spawn_blocking(move || {
            let embedding = text_to_embedding(&query);
            let candidate_limit = limit.saturating_mul(8).max(limit).max(1);
            let ef_search = candidate_limit.max(EF_CONSTRUCTION);
            let neighbors = {
                let index = index
                    .read()
                    .map_err(|_| MemoryStoreError::Index("HNSW index lock poisoned".to_string()))?;
                index.search(&embedding, candidate_limit, ef_search)
            };

            let conn = open_connection(&db_path)?;
            let mut results = Vec::with_capacity(neighbors.len());
            for neighbor in &neighbors {
                let point_id = i64::try_from(neighbor.d_id)
                    .map_err(|_| MemoryStoreError::Index("point ID out of range".to_string()))?;

                let content = match conn
                    .query_row(
                        "SELECT content FROM memory_text WHERE point_id = ?1",
                        params![point_id],
                        |row| row.get::<_, Vec<u8>>(0),
                    )
                    .optional()
                    .map_err(|e| MemoryStoreError::Index(e.to_string()))?
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
                    .map_err(|e| MemoryStoreError::Index(e.to_string()))?
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
                if results.len() >= limit {
                    break;
                }
            }

            Ok::<Vec<MemoryResult>, MemoryStoreError>(results)
        })
        .await
        .map_err(|e| MemoryStoreError::Index(format!("search task join failed: {e}")))?
    }
}

/// Simple bag-of-words embedding using hash-based dimensionality reduction.
///
/// Each word is hashed to a bucket in `[0, VOCAB_DIM)` and its presence
/// increments that dimension. The vector is then L2-normalized for cosine
/// distance compatibility.
///
/// This is a baseline. For production quality, replace with a proper
/// embedding model (e.g., sentence-transformers via ONNX runtime).
fn text_to_embedding(text: &str) -> [f32; VOCAB_DIM] {
    let mut vec = [0.0f32; VOCAB_DIM];
    for word in text.split_whitespace() {
        let hash = word.bytes().fold(0usize, |acc, b| {
            acc.wrapping_mul(31)
                .wrapping_add(b.to_ascii_lowercase() as usize)
        }) % VOCAB_DIM;
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::types::SessionId;
    use std::time::SystemTime;
    use tempfile::TempDir;

    fn meta(session_id: &SessionId) -> MemoryMetadata {
        MemoryMetadata {
            session_id: session_id.clone(),
            turn: Some(1),
            indexed_at: SystemTime::now(),
        }
    }

    #[tokio::test]
    async fn test_hnsw_index_and_search() {
        let dir = TempDir::new().unwrap();
        let store = HnswMemoryStore::open(dir.path().join("memory")).unwrap();
        let session_id = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());

        store
            .index(
                "The user wants to implement a REST API with authentication",
                meta(&session_id),
            )
            .await
            .unwrap();
        store
            .index(
                "Configuration files use TOML format for settings",
                meta(&session_id),
            )
            .await
            .unwrap();
        store
            .index(
                "JWT tokens handle authentication and authorization",
                meta(&SessionId::new()),
            )
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
                .index(
                    &format!("Item {i} with keyword test data"),
                    meta(&session_id),
                )
                .await
                .unwrap();
        }

        let results = store.search(&scope, "test", 3).await.unwrap();
        assert!(results.len() <= 3);
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
                .index(
                    "Persistent memory entry about Rust programming",
                    meta(&session_id),
                )
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
            .index("Exact match query text here", meta(&session_id))
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
}
