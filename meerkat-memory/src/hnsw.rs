//! HnswMemoryStore — HNSW-backed semantic memory store.
//!
//! Uses `hnsw_rs` for approximate nearest-neighbor search and `redb` for
//! metadata persistence. Embeddings use a simple bag-of-words TF approach
//! with cosine distance — upgrade to a proper embedding model for production.
//!
//! Index stored at `.rkat/memory/`.

use async_trait::async_trait;
use hnsw_rs::prelude::{DistCosine, Hnsw};
use meerkat_core::memory::{MemoryMetadata, MemoryResult, MemoryStore, MemoryStoreError};
use redb::{Database, ReadableTable, TableDefinition};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::RwLock;

/// Redb table: point ID (u64) → metadata JSON bytes.
const METADATA_TABLE: TableDefinition<u64, &[u8]> = TableDefinition::new("memory_metadata");

/// Redb table: point ID (u64) → original text bytes.
const TEXT_TABLE: TableDefinition<u64, &[u8]> = TableDefinition::new("memory_text");

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

/// HNSW-backed memory store with redb metadata persistence.
pub struct HnswMemoryStore {
    index: RwLock<Hnsw<'static, f32, DistCosine>>,
    db: Database,
    next_id: AtomicUsize,
    path: PathBuf,
}

impl HnswMemoryStore {
    /// Open or create a memory store at the given directory.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, MemoryStoreError> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir).map_err(MemoryStoreError::Io)?;

        let db_path = dir.join("memory.redb");
        let db =
            Database::create(&db_path).map_err(|e| MemoryStoreError::Index(e.to_string()))?;

        // Ensure tables exist
        let write_txn = db
            .begin_write()
            .map_err(|e| MemoryStoreError::Index(e.to_string()))?;
        {
            let _ = write_txn
                .open_table(METADATA_TABLE)
                .map_err(|e| MemoryStoreError::Index(e.to_string()))?;
            let _ = write_txn
                .open_table(TEXT_TABLE)
                .map_err(|e| MemoryStoreError::Index(e.to_string()))?;
        }
        write_txn
            .commit()
            .map_err(|e| MemoryStoreError::Index(e.to_string()))?;

        // Determine next_id from existing entries
        let next_id = {
            let read_txn = db
                .begin_read()
                .map_err(|e| MemoryStoreError::Index(e.to_string()))?;
            let table = read_txn
                .open_table(METADATA_TABLE)
                .map_err(|e| MemoryStoreError::Index(e.to_string()))?;
            let mut max_id = 0usize;
            let iter = table
                .iter()
                .map_err(|e| MemoryStoreError::Index(e.to_string()))?;
            for entry in iter {
                let (key, _) = entry.map_err(|e| MemoryStoreError::Index(e.to_string()))?;
                let id = key.value() as usize;
                if id >= max_id {
                    max_id = id + 1;
                }
            }
            max_id
        };

        // Build HNSW index — reload from redb if entries exist
        let hnsw = Hnsw::<'static, f32, DistCosine>::new(
            MAX_NB_CONNECTION,
            DEFAULT_MAX_ELEMENTS,
            MAX_LAYER,
            EF_CONSTRUCTION,
            DistCosine {},
        );

        // Re-index existing entries
        if next_id > 0 {
            let read_txn = db
                .begin_read()
                .map_err(|e| MemoryStoreError::Index(e.to_string()))?;
            let text_table = read_txn
                .open_table(TEXT_TABLE)
                .map_err(|e| MemoryStoreError::Index(e.to_string()))?;

            for point_id in 0..next_id {
                if let Some(text_guard) = text_table
                    .get(point_id as u64)
                    .map_err(|e| MemoryStoreError::Index(e.to_string()))?
                {
                    let text = String::from_utf8_lossy(text_guard.value());
                    let embedding = text_to_embedding(&text);
                    hnsw.insert((&embedding, point_id));
                }
            }
        }

        Ok(Self {
            index: RwLock::new(hnsw),
            db,
            next_id: AtomicUsize::new(next_id),
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
    async fn index(
        &self,
        content: &str,
        metadata: MemoryMetadata,
    ) -> Result<(), MemoryStoreError> {
        let point_id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let embedding = text_to_embedding(content);

        // Persist metadata and text to redb
        let meta_json = serde_json::to_vec(&metadata)
            .map_err(|e| MemoryStoreError::Embedding(e.to_string()))?;
        let text_bytes = content.as_bytes();

        let write_txn = self
            .db
            .begin_write()
            .map_err(|e| MemoryStoreError::Index(e.to_string()))?;
        {
            let mut meta_table = write_txn
                .open_table(METADATA_TABLE)
                .map_err(|e| MemoryStoreError::Index(e.to_string()))?;
            let mut text_table = write_txn
                .open_table(TEXT_TABLE)
                .map_err(|e| MemoryStoreError::Index(e.to_string()))?;

            meta_table
                .insert(point_id as u64, meta_json.as_slice())
                .map_err(|e| MemoryStoreError::Index(e.to_string()))?;
            text_table
                .insert(point_id as u64, text_bytes)
                .map_err(|e| MemoryStoreError::Index(e.to_string()))?;
        }
        write_txn
            .commit()
            .map_err(|e| MemoryStoreError::Index(e.to_string()))?;

        // Insert into HNSW index
        let index = self.index.write().await;
        index.insert((&embedding, point_id));

        Ok(())
    }

    async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryResult>, MemoryStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let embedding = text_to_embedding(query);
        let index = self.index.read().await;
        let ef_search = limit.max(EF_CONSTRUCTION);
        let neighbors = index.search(&embedding, limit, ef_search);

        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| MemoryStoreError::Index(e.to_string()))?;
        let meta_table = read_txn
            .open_table(METADATA_TABLE)
            .map_err(|e| MemoryStoreError::Index(e.to_string()))?;
        let text_table = read_txn
            .open_table(TEXT_TABLE)
            .map_err(|e| MemoryStoreError::Index(e.to_string()))?;

        let mut results = Vec::with_capacity(neighbors.len());
        for neighbor in &neighbors {
            let point_id = neighbor.d_id as u64;

            let content = match text_table
                .get(point_id)
                .map_err(|e| MemoryStoreError::Index(e.to_string()))?
            {
                Some(guard) => String::from_utf8_lossy(guard.value()).into_owned(),
                None => continue,
            };

            let metadata = match meta_table
                .get(point_id)
                .map_err(|e| MemoryStoreError::Index(e.to_string()))?
            {
                Some(guard) => serde_json::from_slice(guard.value())
                    .map_err(|e| MemoryStoreError::Embedding(e.to_string()))?,
                None => continue,
            };

            // HNSW distance is cosine distance (0 = identical, 2 = opposite).
            // Convert to a 0..1 similarity score.
            let score = 1.0 - (neighbor.distance / 2.0);

            results.push(MemoryResult {
                content,
                metadata,
                score,
            });
        }

        Ok(results)
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
fn text_to_embedding(text: &str) -> Vec<f32> {
    let mut vec = vec![0.0f32; VOCAB_DIM];

    for word in text.split_whitespace() {
        let word_lower = word.to_lowercase();
        // Simple hash: sum of byte values mod VOCAB_DIM
        let hash = word_lower.bytes().fold(0usize, |acc, b| {
            acc.wrapping_mul(31).wrapping_add(b as usize)
        }) % VOCAB_DIM;
        vec[hash] += 1.0;
    }

    // L2 normalize
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

    fn meta() -> MemoryMetadata {
        MemoryMetadata {
            session_id: SessionId::new(),
            turn: Some(1),
            indexed_at: SystemTime::now(),
        }
    }

    #[tokio::test]
    async fn test_hnsw_index_and_search() {
        let dir = TempDir::new().unwrap();
        let store = HnswMemoryStore::open(dir.path().join("memory")).unwrap();

        store
            .index("The user wants to implement a REST API with authentication", meta())
            .await
            .unwrap();
        store
            .index("Configuration files use TOML format for settings", meta())
            .await
            .unwrap();
        store
            .index("JWT tokens handle authentication and authorization", meta())
            .await
            .unwrap();

        let results = store.search("REST API authentication", 10).await.unwrap();
        assert!(!results.is_empty());
        // The most relevant result should mention REST API or authentication
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

        let results = store.search("anything", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_hnsw_search_limit() {
        let dir = TempDir::new().unwrap();
        let store = HnswMemoryStore::open(dir.path().join("memory")).unwrap();

        for i in 0..10 {
            store
                .index(&format!("Item {} with keyword test data", i), meta())
                .await
                .unwrap();
        }

        let results = store.search("test", 3).await.unwrap();
        assert!(results.len() <= 3);
    }

    #[tokio::test]
    async fn test_hnsw_persists_across_reopen() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");

        // Index some data
        {
            let store = HnswMemoryStore::open(&memory_dir).unwrap();
            store
                .index("Persistent memory entry about Rust programming", meta())
                .await
                .unwrap();
        }

        // Reopen and search
        {
            let store = HnswMemoryStore::open(&memory_dir).unwrap();
            let results = store.search("Rust programming", 5).await.unwrap();
            assert!(!results.is_empty(), "Data should survive reopen");
            assert!(results[0].content.contains("Rust"));
        }
    }

    #[tokio::test]
    async fn test_hnsw_score_range() {
        let dir = TempDir::new().unwrap();
        let store = HnswMemoryStore::open(dir.path().join("memory")).unwrap();

        store
            .index("Exact match query text here", meta())
            .await
            .unwrap();

        let results = store.search("Exact match query text here", 1).await.unwrap();
        assert!(!results.is_empty());
        // Score should be close to 1.0 for exact match
        assert!(
            results[0].score > 0.9,
            "Exact match should have high score, got: {}",
            results[0].score
        );
    }
}
