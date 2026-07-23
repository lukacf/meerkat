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
    CompactionProjectionId, CompactionProjectionPersistence, CompactionStageReceipt,
    CompactionStageReconcileReceipt, EmbeddingModel, HnswParams, MemoryEnumerationPage,
    MemoryEnumerationRequest, MemoryIndexBatch, MemoryIndexReceipt, MemoryMetadata, MemoryOwner,
    MemoryRankingPolicy, MemoryRecord, MemoryResult, MemoryScopeDropReceipt, MemorySearchScope,
    MemoryStore, MemoryStoreError,
};
use meerkat_core::types::SessionId;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Base durable shape shared with pre-`session_id` binaries; the idempotent
/// ledger migration 0002 upgrades it in place on open.
const CREATE_MEMORY_SCHEMA_SQL: &str = r"
CREATE TABLE IF NOT EXISTS memory_metadata (
    point_id INTEGER PRIMARY KEY,
    metadata_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS memory_text (
    point_id INTEGER PRIMARY KEY,
    content BLOB NOT NULL
)";

/// Covering index for the `session_id` projection column: every per-scope SQL
/// operation (lazy scope load, enumerate, drop) filters on it.
const CREATE_MEMORY_SESSION_INDEX_SQL: &str =
    "CREATE INDEX IF NOT EXISTS idx_memory_metadata_session_id ON memory_metadata (session_id)";

/// Durable point-ID allocator: a single-row high-water counter. IDs are
/// allocated transactionally with the row writes and never reused (dropping a
/// scope does not return its IDs), so stale live indexes surface as typed
/// [`MemoryStoreError::IndexDivergence`] instead of silently rebinding a
/// recycled ID.
const CREATE_MEMORY_ALLOCATOR_SCHEMA_SQL: &str = "CREATE TABLE IF NOT EXISTS memory_allocator (
    id INTEGER PRIMARY KEY CHECK (id = 0),
    high_water INTEGER NOT NULL
)";

/// Seed the allocator once from the existing rows (`MAX(point_id) + 1`, or 0
/// for an empty store); a no-op when the allocator row already exists.
const SEED_MEMORY_ALLOCATOR_SQL: &str = "INSERT INTO memory_allocator (id, high_water) \
     SELECT 0, COALESCE((SELECT MAX(point_id) + 1 FROM memory_metadata), 0) \
     WHERE NOT EXISTS (SELECT 1 FROM memory_allocator WHERE id = 0)";

/// Invisible durable compaction batches. `state = 'staged'` rows are never
/// consulted by search/enumeration; finalization moves their entries into the
/// ordinary memory tables and flips the state in one SQLite transaction.
const CREATE_COMPACTION_STAGE_SCHEMA_SQL: &str = r"
CREATE TABLE IF NOT EXISTS memory_compaction_stage (
    session_id TEXT NOT NULL,
    parent_revision TEXT NOT NULL,
    revision TEXT NOT NULL,
    commit_fingerprint TEXT NOT NULL,
    entries_json BLOB NOT NULL,
    payload_digest BLOB NOT NULL,
    indexed_entries INTEGER NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('staged', 'finalized')),
    PRIMARY KEY (session_id, parent_revision, revision, commit_fingerprint)
);
CREATE INDEX IF NOT EXISTS idx_memory_compaction_stage_session
    ON memory_compaction_stage (session_id, state);
CREATE TABLE IF NOT EXISTS memory_scope_tombstone (
    session_id TEXT PRIMARY KEY,
    drop_generation INTEGER NOT NULL CHECK (drop_generation > 0)
)";

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct DurableCompactionStageEntry {
    content: String,
    metadata: MemoryMetadata,
}

#[derive(serde::Serialize)]
struct DurableCompactionStageDigestEntry<'a> {
    content: &'a str,
    session_id: &'a SessionId,
    source: &'a meerkat_core::memory::MemorySource,
}

fn compaction_stage_payload_digest(
    entries: &[DurableCompactionStageEntry],
) -> Result<Vec<u8>, MemoryStoreError> {
    // `indexed_at` is publication metadata, not semantic identity. A dropped
    // stage await can leave the first insertion durable while the Agent later
    // retries the same content-derived transcript rewrite with a newly minted
    // commit timestamp. Excluding that volatile field makes the retry exact
    // and idempotent while the first durable stage retains its timestamp.
    let canonical = entries
        .iter()
        .map(|entry| DurableCompactionStageDigestEntry {
            content: &entry.content,
            session_id: &entry.metadata.session_id,
            source: &entry.metadata.source,
        })
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&canonical)
        .map_err(|error| MemoryStoreError::Embedding(error.to_string()))?;
    Ok(Sha256::digest(bytes).to_vec())
}

/// Whether the canonical session scope has been durably deleted.
///
/// A tombstone is permanent because session identity is never reused. Every
/// publisher checks it inside the same SQLite write transaction as its rows,
/// making scope deletion win regardless of process restart or cross-instance
/// interleaving.
fn scope_is_tombstoned(
    conn: &Connection,
    session_id: &SessionId,
) -> Result<bool, MemoryStoreError> {
    conn.query_row(
        "SELECT 1 FROM memory_scope_tombstone WHERE session_id = ?1",
        params![session_id.to_string()],
        |_| Ok(()),
    )
    .optional()
    .map(|row| row.is_some())
    .map_err(|error| MemoryStoreError::Storage(error.to_string()))
}

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

/// The memory store's schema domain in the per-file migration ledger.
///
/// Migration 0001 is the base durable shape shared with pre-`session_id`
/// binaries; 0002 lifts the historical in-place upgrade (projection column +
/// covering index + allocator + compaction staging) into a once-per-file
/// migration. The `table_info` guard keeps 0002 convergent on files of any
/// vintage; the NULL `session_id` backfill deliberately stays OUT of the
/// migration — [`backfill_null_session_ids`] already heals NULL rows on
/// every open and before each per-scope operation (old binaries keep writing
/// NULL rows after the migration, so a one-shot backfill could never be the
/// invariant anyway).
const MEMORY_DOMAIN: meerkat_sqlite::SchemaDomain = meerkat_sqlite::SchemaDomain {
    name: "memory",
    migrations: &[
        meerkat_sqlite::Migration {
            version: 1,
            name: "base-schema",
            apply: migration_0001_memory_schema,
        },
        meerkat_sqlite::Migration {
            version: 2,
            name: "scoped-projection-and-staging",
            apply: migration_0002_scoped_projection,
        },
    ],
};

fn migration_0001_memory_schema(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(CREATE_MEMORY_SCHEMA_SQL)
}

fn migration_0002_scoped_projection(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    let has_session_id = {
        let mut stmt = tx.prepare("PRAGMA table_info(memory_metadata)")?;
        let column_names = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        column_names.iter().any(|name| name == "session_id")
    };
    if !has_session_id {
        tx.execute("ALTER TABLE memory_metadata ADD COLUMN session_id TEXT", [])?;
    }
    tx.execute(CREATE_MEMORY_SESSION_INDEX_SQL, [])?;
    tx.execute(CREATE_MEMORY_ALLOCATOR_SCHEMA_SQL, [])?;
    tx.execute(SEED_MEMORY_ALLOCATOR_SQL, [])?;
    tx.execute_batch(CREATE_COMPACTION_STAGE_SCHEMA_SQL)?;
    Ok(())
}

/// Per-operation connection: fence guard lives exactly as long as the
/// connection it admits.
struct MemoryConn {
    conn: Connection,
    _guard: meerkat_sqlite::OperationGuard,
}

impl std::ops::Deref for MemoryConn {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.conn
    }
}

impl std::ops::DerefMut for MemoryConn {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

fn open_connection(path: &Path) -> Result<MemoryConn, MemoryStoreError> {
    let guard = meerkat_sqlite::OperationGuard::for_database(path)
        .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
    let mut conn = meerkat_sqlite::open_with(
        path,
        meerkat_sqlite::ConnectionProfile::PRIMARY,
        meerkat_sqlite::OpenOptions {
            // Future-schema refusal precedes the Primary profile's WAL
            // conversion.
            schema_preflight: &[&MEMORY_DOMAIN],
            ..meerkat_sqlite::OpenOptions::default()
        },
    )
    .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
    meerkat_sqlite::apply_domain_migrations(&mut conn, &MEMORY_DOMAIN)
        .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
    Ok(MemoryConn {
        conn,
        _guard: guard,
    })
}

/// Idempotent NULL `session_id` backfill from the typed durable metadata.
///
/// Old-binary writers INSERT with the original column list and leave the
/// projection column NULL; those rows must never become invisible to scoped
/// SQL, so this heal runs on every open AND before each per-scope SQL
/// operation (lazy scope load, drop, enumerate) — cheap when zero NULL rows
/// remain (a single indexed probe). It runs statement-at-a-time (no
/// transaction of its own) so it composes with the migration's transaction;
/// partial application outside a transaction is safe because the backfill is
/// idempotent and re-run before every scoped read.
///
/// A NULL row whose `metadata_json` does not deserialize is deliberately a
/// store-wide typed fault (it gates every scoped operation): the corrupt row
/// cannot be attributed to any scope, so per-scope poisoning cannot contain
/// it, and skipping or quarantining it would silently serve a store known to
/// hold undecodable durable rows — the same fail-closed posture as
/// [`MemoryStoreError::TextCorruption`].
fn backfill_null_session_ids(conn: &Connection) -> Result<(), MemoryStoreError> {
    let mut null_rows = Vec::new();
    {
        let mut stmt = conn
            .prepare("SELECT point_id, metadata_json FROM memory_metadata WHERE session_id IS NULL")
            .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
        let mapped = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
        for row in mapped {
            null_rows.push(row.map_err(|e| MemoryStoreError::Storage(e.to_string()))?);
        }
    }
    for (point_id, metadata_json) in null_rows {
        let metadata: MemoryMetadata = serde_json::from_slice(&metadata_json)
            .map_err(|e| MemoryStoreError::Embedding(e.to_string()))?;
        conn.execute(
            "UPDATE memory_metadata SET session_id = ?1 WHERE point_id = ?2",
            params![metadata.session_id.to_string(), point_id],
        )
        .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
    }
    Ok(())
}

/// Allocate `count` monotonically increasing point IDs from the durable
/// allocator inside the caller's insert transaction.
///
/// The high-water mark never decreases (dropping a scope does not return its
/// IDs), so point IDs are never reused and cross-instance stale live indexes
/// surface as typed [`MemoryStoreError::IndexDivergence`] instead of silently
/// rebinding a recycled ID. The allocation base also self-heals past
/// `MAX(point_id)` so rows written by an old binary's in-memory allocator can
/// never wedge the store on a permanent primary-key collision.
fn allocate_point_ids(tx: &Transaction<'_>, count: usize) -> Result<Vec<i64>, MemoryStoreError> {
    let high_water: i64 = tx
        .query_row(
            "SELECT high_water FROM memory_allocator WHERE id = 0",
            [],
            |row| row.get(0),
        )
        .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
    let max_row: Option<i64> = tx
        .query_row("SELECT MAX(point_id) FROM memory_metadata", [], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|e| MemoryStoreError::Storage(e.to_string()))?
        .flatten();
    let row_floor = match max_row {
        Some(max) => max
            .checked_add(1)
            .ok_or(MemoryStoreError::PointIdOverflow)?,
        None => 0,
    };
    let base = high_water.max(row_floor);
    let count = i64::try_from(count).map_err(|_| MemoryStoreError::PointIdOutOfRange)?;
    let next_high = base
        .checked_add(count)
        .ok_or(MemoryStoreError::PointIdOverflow)?;
    tx.execute(
        "UPDATE memory_allocator SET high_water = ?1 WHERE id = 0",
        params![next_high],
    )
    .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
    Ok((base..next_high).collect())
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

/// Build (or rebuild) a single scoped HNSW index from the rows that currently
/// survive in SQLite for `session_id`.
///
/// This is both the lazy per-scope loader (scope indexes are built on first
/// use, never on open) and the repair path after a partial in-memory insert
/// (where the DB rows were rolled back): the live index derives purely from
/// committed DB state, leaving no phantom neighbor slots that point at
/// DB-deleted records. NULL projection cells are healed first so rows written
/// by an old binary are never invisible to the indexed scoped SELECT.
fn rebuild_scoped_index_from_db(
    conn: &Connection,
    session_id: &SessionId,
    embedding_model: &dyn EmbeddingModel,
    params: HnswParams,
) -> Result<ScopedHnswIndex, MemoryStoreError> {
    backfill_null_session_ids(conn)?;
    let mut scoped: Vec<(i64, Vec<u8>)> = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT t.point_id, t.content \
                 FROM memory_text t \
                 JOIN memory_metadata m ON m.point_id = t.point_id \
                 WHERE m.session_id = ?1 \
                 ORDER BY t.point_id ASC",
            )
            .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
        let mapped = stmt
            .query_map(params![session_id.to_string()], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
        for row in mapped {
            scoped.push(row.map_err(|e| MemoryStoreError::Storage(e.to_string()))?);
        }
    }

    let index = ScopedHnswIndex::new(scoped.len(), params);
    for (point_id, text) in scoped {
        let text = decode_memory_text(point_id, text)?;
        let point_id =
            usize::try_from(point_id).map_err(|_| MemoryStoreError::PointIdOutOfRange)?;
        let embedding = embedding_model.embed(&text);
        index.insert(&embedding, point_id);
    }
    Ok(index)
}

/// Decode durable memory text bytes, failing closed on corruption.
///
/// Corrupt durable bytes must surface as the typed
/// [`MemoryStoreError::TextCorruption`] fault — never lossy-decoded into
/// searchable/returned memory content.
fn decode_memory_text(point_id: i64, bytes: Vec<u8>) -> Result<String, MemoryStoreError> {
    String::from_utf8(bytes).map_err(|_| MemoryStoreError::TextCorruption { point_id })
}

/// Live state of one session-scoped HNSW index.
///
/// `Poisoned` is the fail-closed marker for a scope whose live index could not
/// be repaired from durable state after a partial batch failure: reads return
/// the typed [`MemoryStoreError::ScopePoisoned`] fault instead of serving a
/// candidate set known to diverge from the durable store. The next index
/// attempt (or a store reopen) rebuilds the scope from durable rows.
enum ScopedIndexState {
    Live(ScopedHnswIndex),
    Poisoned,
}

#[cfg(test)]
struct BlockingMutationPause {
    reached: std::sync::Barrier,
    release: std::sync::Barrier,
}

#[cfg(test)]
impl BlockingMutationPause {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            reached: std::sync::Barrier::new(2),
            release: std::sync::Barrier::new(2),
        })
    }
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
///
/// Manual `Debug`: the scoped HNSW indices are large native structures with
/// no useful (or derivable) `Debug` form, so the projection prints the
/// durable identity facts only.
pub struct HnswMemoryStore {
    // SAFETY NOTE: we use `'static` because `hnsw_rs` 0.3 copies inserted vectors
    // into owned internal storage. If a future `hnsw_rs` release changes this to
    // borrow caller memory, this type must be revisited before upgrading.
    indices: Arc<std::sync::RwLock<HashMap<SessionId, ScopedIndexState>>>,
    db_path: PathBuf,
    insert_lock: Arc<Mutex<()>>,
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
    /// One-shot deterministic cancellation windows used by crash-safety
    /// tests. Production builds carry neither hook.
    #[cfg(test)]
    stage_commit_pause: std::sync::Mutex<Option<Arc<BlockingMutationPause>>>,
    #[cfg(test)]
    finalize_publish_pause: std::sync::Mutex<Option<Arc<BlockingMutationPause>>>,
}

impl std::fmt::Debug for HnswMemoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HnswMemoryStore")
            .field("db_path", &self.db_path)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl HnswMemoryStore {
    /// Open or create a memory store at the given directory using the default
    /// ranking policy ([`default_ranking_policy`]).
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, MemoryStoreError> {
        Self::open_with_policy(dir, default_ranking_policy())
    }

    /// Open or create a memory store at the given directory with an injected
    /// typed ranking policy that owns embedding generation and HNSW parameters.
    ///
    /// Opening is cheap: it runs the idempotent durable-format migration and
    /// the NULL projection-cell heal only — no scope scans and no embedding
    /// work. Per-scope live indexes are built lazily on first use (search /
    /// index / enumerate / drop).
    pub fn open_with_policy(
        dir: impl AsRef<Path>,
        policy: MemoryRankingPolicy,
    ) -> Result<Self, MemoryStoreError> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir).map_err(MemoryStoreError::Io)?;

        let db_path = dir.join("memory.sqlite3");
        // `open_connection` brings the schema domain up to date.
        let conn = open_connection(&db_path)?;
        // Heal on every open: rows an old binary wrote since the last heal
        // must never stay invisible to scoped SQL.
        backfill_null_session_ids(&conn)?;

        Ok(Self {
            indices: Arc::new(std::sync::RwLock::new(HashMap::new())),
            db_path,
            insert_lock: Arc::new(Mutex::new(())),
            path: dir.to_path_buf(),
            policy,
            #[cfg(test)]
            fail_hnsw_insert_after: Arc::new(std::sync::atomic::AtomicI64::new(-1)),
            #[cfg(test)]
            stage_commit_pause: std::sync::Mutex::new(None),
            #[cfg(test)]
            finalize_publish_pause: std::sync::Mutex::new(None),
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
            .map(|state| match state {
                ScopedIndexState::Live(index) => index.index.get_nb_point(),
                ScopedIndexState::Poisoned => 0,
            })
            .sum()
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used)]
    fn hnsw_index_hints(&self) -> Vec<usize> {
        self.indices
            .read()
            .unwrap()
            .values()
            .filter_map(|state| match state {
                ScopedIndexState::Live(index) => Some(index.max_elements_hint),
                ScopedIndexState::Poisoned => None,
            })
            .collect()
    }

    /// Test-only: arm the in-memory HNSW insert loop to fail after `after`
    /// successful inserts within the next batch (`-1` disables injection).
    #[cfg(test)]
    fn arm_hnsw_insert_failure_after(&self, after: i64) {
        self.fail_hnsw_insert_after
            .store(after, std::sync::atomic::Ordering::Release);
    }

    #[cfg(test)]
    fn arm_stage_commit_pause(&self) -> Arc<BlockingMutationPause> {
        let pause = BlockingMutationPause::new();
        *self
            .stage_commit_pause
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&pause));
        pause
    }

    #[cfg(test)]
    fn arm_finalize_publish_pause(&self) -> Arc<BlockingMutationPause> {
        let pause = BlockingMutationPause::new();
        *self
            .finalize_publish_pause
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&pause));
        pause
    }
}

#[async_trait]
impl MemoryStore for HnswMemoryStore {
    fn compaction_projection_persistence(&self) -> CompactionProjectionPersistence {
        CompactionProjectionPersistence::DurableStaged
    }

    async fn stage_compaction_batch(
        &self,
        projection: CompactionProjectionId,
        batch: MemoryIndexBatch,
    ) -> Result<CompactionStageReceipt, MemoryStoreError> {
        let (scope, requests) = batch.into_parts();
        if scope.session_id() != projection.session_id() {
            return Err(MemoryStoreError::Scope(format!(
                "compaction projection session {} is outside batch scope {}",
                projection.session_id(),
                scope.session_id()
            )));
        }
        let mut entries = Vec::new();
        for request in requests {
            let (_scope, content, metadata) = request.into_parts();
            if content.is_indexable() {
                entries.push(DurableCompactionStageEntry {
                    content: content.into_indexable_text(),
                    metadata,
                });
            }
        }
        let staged_entries = entries.len();
        let entries_json = serde_json::to_vec(&entries)
            .map_err(|error| MemoryStoreError::Embedding(error.to_string()))?;
        let payload_digest = compaction_stage_payload_digest(&entries)?;
        let indexed_entries =
            i64::try_from(staged_entries).map_err(|_| MemoryStoreError::PointIdOutOfRange)?;
        let db_path = self.db_path.clone();
        let projection_for_write = projection.clone();
        #[cfg(test)]
        let stage_commit_pause = self
            .stage_commit_pause
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let publication_guard = Arc::clone(&self.insert_lock).lock_owned().await;
        let tombstoned = tokio::task::spawn_blocking(move || -> Result<bool, MemoryStoreError> {
            let _publication_guard = publication_guard;
            let mut conn = open_connection(&db_path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
            if scope_is_tombstoned(&tx, projection_for_write.session_id())? {
                // A stale agent may retry its pre-delete stage after the
                // canonical session owner is gone. Acknowledge the exact
                // request as a zero stage so the caller can unwind normally,
                // but never recreate durable or live projection state.
                tx.execute(
                    "DELETE FROM memory_compaction_stage WHERE session_id = ?1",
                    params![projection_for_write.session_id().to_string()],
                )
                .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
                tx.commit()
                    .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
                return Ok(true);
            }
            let existing = tx
                .query_row(
                    "SELECT payload_digest, indexed_entries, state FROM memory_compaction_stage \
                     WHERE session_id = ?1 AND parent_revision = ?2 AND revision = ?3 \
                       AND commit_fingerprint = ?4",
                    params![
                        projection_for_write.session_id().to_string(),
                        projection_for_write.parent_revision(),
                        projection_for_write.revision(),
                        projection_for_write.commit_fingerprint(),
                    ],
                    |row| {
                        Ok((
                            row.get::<_, Vec<u8>>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
            if let Some((existing_digest, existing_count, state)) = existing {
                let compatible = existing_count == indexed_entries
                    && existing_digest == payload_digest
                    && matches!(state.as_str(), "staged" | "finalized");
                if !compatible {
                    return Err(MemoryStoreError::Storage(
                        "conflicting durable compaction stage for the same transcript rewrite"
                            .to_string(),
                    ));
                }
                tx.commit()
                    .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
                return Ok(false);
            }
            tx.execute(
                "INSERT INTO memory_compaction_stage \
                 (session_id, parent_revision, revision, commit_fingerprint, entries_json, payload_digest, indexed_entries, state) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'staged')",
                params![
                    projection_for_write.session_id().to_string(),
                    projection_for_write.parent_revision(),
                    projection_for_write.revision(),
                    projection_for_write.commit_fingerprint(),
                    entries_json,
                    payload_digest,
                    indexed_entries,
                ],
            )
            .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
            tx.commit()
                .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
            #[cfg(test)]
            if let Some(pause) = stage_commit_pause {
                pause.reached.wait();
                pause.release.wait();
            }
            Ok(false)
        })
        .await
        .map_err(|error| MemoryStoreError::TaskJoin(format!("stage task join failed: {error}")))??;

        Ok(CompactionStageReceipt {
            projection,
            staged_entries: if tombstoned { 0 } else { staged_entries },
        })
    }

    async fn finalize_compaction_batch(
        &self,
        projection: &CompactionProjectionId,
    ) -> Result<MemoryIndexReceipt, MemoryStoreError> {
        let receipt_scope =
            meerkat_core::memory::MemoryIndexScope::for_session(projection.session_id().clone());
        let db_path = self.db_path.clone();
        let indices = Arc::clone(&self.indices);
        let projection = projection.clone();
        let embedding_model = Arc::clone(self.policy.embedding_model());
        let hnsw_params = self.policy.hnsw_params();
        #[cfg(test)]
        let finalize_publish_pause = self
            .finalize_publish_pause
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let publication_guard = Arc::clone(&self.insert_lock).lock_owned().await;
        let indexed_entries =
            tokio::task::spawn_blocking(move || -> Result<usize, MemoryStoreError> {
                let _publication_guard = publication_guard;
                let mut conn = open_connection(&db_path)?;
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
                if scope_is_tombstoned(&tx, projection.session_id())? {
                    // The RuntimeStore may still carry a pending outbox row
                    // when session deletion wins. Return a successful zero
                    // publication so that authority can acknowledge/clear the
                    // outbox, while defensively purging any stale rows left by
                    // an interleaving or older writer.
                    backfill_null_session_ids(&tx)?;
                    let session_param = projection.session_id().to_string();
                    tx.execute(
                        "DELETE FROM memory_text WHERE point_id IN \
                         (SELECT point_id FROM memory_metadata WHERE session_id = ?1)",
                        params![session_param],
                    )
                    .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
                    tx.execute(
                        "DELETE FROM memory_metadata WHERE session_id = ?1",
                        params![session_param],
                    )
                    .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
                    tx.execute(
                        "DELETE FROM memory_compaction_stage WHERE session_id = ?1",
                        params![session_param],
                    )
                    .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
                    tx.commit()
                        .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
                    indices
                        .write()
                        .map_err(|_| MemoryStoreError::LockPoisoned)?
                        .remove(projection.session_id());
                    return Ok(0);
                }
                let (entries_json, indexed_entries_i64, state) = tx
                    .query_row(
                        "SELECT entries_json, indexed_entries, state FROM memory_compaction_stage \
                         WHERE session_id = ?1 AND parent_revision = ?2 AND revision = ?3 \
                           AND commit_fingerprint = ?4",
                        params![
                            projection.session_id().to_string(),
                            projection.parent_revision(),
                            projection.revision(),
                            projection.commit_fingerprint(),
                        ],
                        |row| {
                            Ok((
                                row.get::<_, Vec<u8>>(0)?,
                                row.get::<_, i64>(1)?,
                                row.get::<_, String>(2)?,
                            ))
                        },
                    )
                    .optional()
                    .map_err(|error| MemoryStoreError::Storage(error.to_string()))?
                    .ok_or_else(|| {
                        MemoryStoreError::Storage(format!(
                            "missing durable compaction stage for committed rewrite {}",
                            projection.revision()
                        ))
                    })?;
                let indexed_entries = usize::try_from(indexed_entries_i64)
                    .map_err(|_| MemoryStoreError::PointIdOutOfRange)?;
                if state == "staged" {
                    let entries: Vec<DurableCompactionStageEntry> =
                        serde_json::from_slice(&entries_json)
                            .map_err(|error| MemoryStoreError::Embedding(error.to_string()))?;
                    if entries.len() != indexed_entries {
                        return Err(MemoryStoreError::Storage(
                            "durable compaction stage entry count is corrupt".to_string(),
                        ));
                    }
                    let point_ids = allocate_point_ids(&tx, indexed_entries)?;
                    let session_param = projection.session_id().to_string();
                    for (point_id, entry) in point_ids.iter().zip(&entries) {
                        let metadata_json = serde_json::to_vec(&entry.metadata)
                            .map_err(|error| MemoryStoreError::Embedding(error.to_string()))?;
                        tx.execute(
                            "INSERT INTO memory_metadata (point_id, metadata_json, session_id) \
                             VALUES (?1, ?2, ?3)",
                            params![point_id, metadata_json, session_param],
                        )
                        .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
                        tx.execute(
                            "INSERT INTO memory_text (point_id, content) VALUES (?1, ?2)",
                            params![point_id, entry.content.as_bytes()],
                        )
                        .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
                    }
                    tx.execute(
                        "UPDATE memory_compaction_stage \
                         SET state = 'finalized', entries_json = ?5 \
                         WHERE session_id = ?1 AND parent_revision = ?2 AND revision = ?3 \
                           AND commit_fingerprint = ?4",
                        params![
                            projection.session_id().to_string(),
                            projection.parent_revision(),
                            projection.revision(),
                            projection.commit_fingerprint(),
                            b"[]".as_slice(),
                        ],
                    )
                    .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
                } else if state != "finalized" {
                    return Err(MemoryStoreError::Storage(format!(
                        "unknown durable compaction stage state '{state}'"
                    )));
                }
                tx.commit()
                    .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;

                // Cancellation-safe publication: this blocking owner continues
                // after an awaiting future is dropped, and rebuilds the live
                // index from the just-committed durable rows before returning.
                let rebuilt = rebuild_scoped_index_from_db(
                    &conn,
                    projection.session_id(),
                    embedding_model.as_ref(),
                    hnsw_params,
                );
                #[cfg(test)]
                if let Some(pause) = finalize_publish_pause {
                    pause.reached.wait();
                    pause.release.wait();
                }
                let mut live = indices
                    .write()
                    .map_err(|_| MemoryStoreError::LockPoisoned)?;
                match rebuilt {
                    Ok(rebuilt) => {
                        live.insert(
                            projection.session_id().clone(),
                            ScopedIndexState::Live(rebuilt),
                        );
                    }
                    Err(error) => {
                        live.insert(projection.session_id().clone(), ScopedIndexState::Poisoned);
                        return Err(error);
                    }
                }
                Ok(indexed_entries)
            })
            .await
            .map_err(|error| {
                MemoryStoreError::TaskJoin(format!("finalize task join failed: {error}"))
            })??;

        Ok(MemoryIndexReceipt {
            scope: receipt_scope,
            indexed_entries,
        })
    }

    async fn abort_compaction_batch(
        &self,
        projection: &CompactionProjectionId,
    ) -> Result<(), MemoryStoreError> {
        let db_path = self.db_path.clone();
        let projection = projection.clone();
        let publication_guard = Arc::clone(&self.insert_lock).lock_owned().await;
        tokio::task::spawn_blocking(move || -> Result<(), MemoryStoreError> {
            let _publication_guard = publication_guard;
            let conn = open_connection(&db_path)?;
            conn.execute(
                "DELETE FROM memory_compaction_stage \
                 WHERE session_id = ?1 AND parent_revision = ?2 AND revision = ?3 \
                   AND commit_fingerprint = ?4 \
                   AND state = 'staged'",
                params![
                    projection.session_id().to_string(),
                    projection.parent_revision(),
                    projection.revision(),
                    projection.commit_fingerprint(),
                ],
            )
            .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|error| {
            MemoryStoreError::TaskJoin(format!("abort task join failed: {error}"))
        })??;
        Ok(())
    }

    async fn reconcile_compaction_stages(
        &self,
        owner: &MemoryOwner,
        committed: &[CompactionProjectionId],
    ) -> Result<CompactionStageReconcileReceipt, MemoryStoreError> {
        let committed = committed
            .iter()
            .filter(|projection| projection.session_id() == owner.session_id())
            .map(|projection| {
                (
                    projection.parent_revision().to_string(),
                    projection.revision().to_string(),
                    projection.commit_fingerprint().to_string(),
                )
            })
            .collect::<std::collections::HashSet<_>>();
        let db_path = self.db_path.clone();
        let session_id = owner.session_id().clone();
        let publication_guard = Arc::clone(&self.insert_lock).lock_owned().await;
        tokio::task::spawn_blocking(
            move || -> Result<CompactionStageReconcileReceipt, MemoryStoreError> {
                let _publication_guard = publication_guard;
                let mut conn = open_connection(&db_path)?;
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
                if scope_is_tombstoned(&tx, &session_id)? {
                    // Empty and non-empty RuntimeStore outbox authority both
                    // reconcile successfully after canonical scope deletion.
                    // Finalize will likewise acknowledge each exact pending
                    // projection with a zero publication, allowing the outbox
                    // owner to clear it without resurrecting memory.
                    backfill_null_session_ids(&tx)?;
                    let session_param = session_id.to_string();
                    tx.execute(
                        "DELETE FROM memory_text WHERE point_id IN \
                         (SELECT point_id FROM memory_metadata WHERE session_id = ?1)",
                        params![session_param],
                    )
                    .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
                    tx.execute(
                        "DELETE FROM memory_metadata WHERE session_id = ?1",
                        params![session_param],
                    )
                    .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
                    tx.execute(
                        "DELETE FROM memory_compaction_stage WHERE session_id = ?1",
                        params![session_param],
                    )
                    .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
                    tx.commit()
                        .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
                    return Ok(CompactionStageReconcileReceipt::default());
                }
                let mut staged = Vec::new();
                {
                    let mut statement = tx
                        .prepare(
                            "SELECT parent_revision, revision, commit_fingerprint FROM memory_compaction_stage \
                             WHERE session_id = ?1 AND state = 'staged'",
                        )
                        .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
                    let rows = statement
                        .query_map(params![session_id.to_string()], |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                            ))
                        })
                        .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
                    for row in rows {
                        staged.push(
                            row.map_err(|error| MemoryStoreError::Storage(error.to_string()))?,
                        );
                    }
                }
                let mut receipt = CompactionStageReconcileReceipt::default();
                for (parent_revision, revision, commit_fingerprint) in staged {
                    if committed.contains(&(
                        parent_revision.clone(),
                        revision.clone(),
                        commit_fingerprint.clone(),
                    )) {
                        receipt.retained_committed += 1;
                    } else {
                        tx.execute(
                            "DELETE FROM memory_compaction_stage \
                             WHERE session_id = ?1 AND parent_revision = ?2 AND revision = ?3 \
                               AND commit_fingerprint = ?4 \
                               AND state = 'staged'",
                            params![
                                session_id.to_string(),
                                parent_revision,
                                revision,
                                commit_fingerprint,
                            ],
                        )
                        .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
                        receipt.aborted_orphans += 1;
                    }
                }
                tx.commit()
                    .map_err(|error| MemoryStoreError::Storage(error.to_string()))?;
                Ok(receipt)
            },
        )
        .await
        .map_err(|error| {
            MemoryStoreError::TaskJoin(format!("reconcile task join failed: {error}"))
        })?
    }

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

        // Serialize in-process inserts so live index updates apply in commit
        // order.
        let publication_guard = Arc::clone(&self.insert_lock).lock_owned().await;

        let insert_result = tokio::task::spawn_blocking(move || {
            let _publication_guard = publication_guard;
            let mut conn = open_connection(&db_path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
            if scope_is_tombstoned(&tx, &session_id)? {
                tx.commit()
                    .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
                indices
                    .write()
                    .map_err(|_| MemoryStoreError::LockPoisoned)?
                    .remove(&session_id);
                return Ok::<usize, MemoryStoreError>(0);
            }
            // Transactional point-ID allocation: IDs come from the durable
            // allocator inside the same transaction as the row writes, so
            // concurrent instances over the same file can never allocate
            // colliding IDs and dropped scopes never recycle theirs. A batch
            // that fails after commit burns its IDs (the rollback below never
            // rewinds the high-water mark).
            let point_ids = allocate_point_ids(&tx, indexed_entries)?;
            let session_param = session_id.to_string();
            for (point_id_i64, (content, meta_json, _embedding)) in point_ids.iter().zip(&entries) {
                tx.execute(
                    "INSERT INTO memory_metadata (point_id, metadata_json, session_id) \
                     VALUES (?1, ?2, ?3)",
                    params![point_id_i64, meta_json, session_param],
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
                // Self-heal: a previously poisoned scope is rebuilt wholesale
                // from the committed durable rows (which now include this
                // batch), restoring the durable-derived live index instead of
                // failing the scope forever.
                if matches!(indices.get(&session_id), Some(ScopedIndexState::Poisoned)) {
                    let rebuilt = rebuild_scoped_index_from_db(
                        &conn,
                        &session_id,
                        embedding_model.as_ref(),
                        hnsw_params,
                    )?;
                    indices.insert(session_id.clone(), ScopedIndexState::Live(rebuilt));
                    return Ok(());
                }
                let state = match indices.entry(session_id.clone()) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        // Lazy loading: an existing-but-unloaded scope must be
                        // built from the committed durable rows (which now
                        // include this batch) — starting from an empty live
                        // index here would silently hide the scope's prior
                        // entries from search.
                        let rebuilt = rebuild_scoped_index_from_db(
                            &conn,
                            &session_id,
                            embedding_model.as_ref(),
                            hnsw_params,
                        )?;
                        entry.insert(ScopedIndexState::Live(rebuilt));
                        return Ok(());
                    }
                };
                let ScopedIndexState::Live(index) = state else {
                    // Unreachable: the poisoned case returned above.
                    return Err(MemoryStoreError::ScopePoisoned);
                };
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
                // Roll back this batch's committed rows, then repair the live
                // scoped index purely from the surviving durable rows.
                // `hnsw_rs` cannot remove already-inserted points, so a partial
                // insert above may have left live neighbor slots for point_ids
                // the rollback deletes; the rebuild mirrors the full re-index
                // crash recovery performs on `open`.
                let repair_result = (|| -> Result<ScopedHnswIndex, MemoryStoreError> {
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
                    rebuild_scoped_index_from_db(
                        &cleanup,
                        &session_id,
                        embedding_model.as_ref(),
                        hnsw_params,
                    )
                })();

                // Fail closed in BOTH repair outcomes: a repaired scope serves
                // the durable-derived index; an unrepairable scope is poisoned
                // so reads surface the typed fault instead of phantom
                // (rolled-back) neighbor slots — the stale live index is never
                // left serving.
                let mut indices = indices
                    .write()
                    .map_err(|_| MemoryStoreError::LockPoisoned)?;
                match repair_result {
                    Ok(repaired) => {
                        indices.insert(session_id, ScopedIndexState::Live(repaired));
                        return Err(error);
                    }
                    Err(repair) => {
                        indices.insert(session_id, ScopedIndexState::Poisoned);
                        return Err(MemoryStoreError::ScopeRepairFailed {
                            original: Box::new(error),
                            repair: Box::new(repair),
                        });
                    }
                }
            }

            Ok::<usize, MemoryStoreError>(indexed_entries)
        })
        .await
        .map_err(|e| MemoryStoreError::TaskJoin(format!("index task join failed: {e}")))?;
        let published_entries = insert_result?;

        Ok(MemoryIndexReceipt {
            scope: receipt_scope,
            indexed_entries: published_entries,
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
        // First pass serves an already-loaded scope without touching the
        // insert lock. An unloaded scope reports back instead of loading
        // inline: the lazy rebuild-and-publish must be serialized with the
        // index/drop publishers (below), or a rebuild snapshot taken while a
        // batch commit is in flight would publish a stale live index.
        match self.search_pass(scope, query, limit, None).await? {
            Some(results) => Ok(results),
            None => {
                let publication_guard = Arc::clone(&self.insert_lock).lock_owned().await;
                match self
                    .search_pass(scope, query, limit, Some(publication_guard))
                    .await?
                {
                    Some(results) => Ok(results),
                    // Structurally unreachable: the load pass always loads.
                    // Fail closed rather than serve an empty result for a
                    // scope whose durable rows were never consulted.
                    None => Err(MemoryStoreError::Storage(
                        "scope load pass did not publish a live index".to_string(),
                    )),
                }
            }
        }
    }

    async fn enumerate_scoped(
        &self,
        scope: &MemorySearchScope,
        request: MemoryEnumerationRequest,
    ) -> Result<MemoryEnumerationPage, MemoryStoreError> {
        self.enumerate_scoped_impl(scope, request).await
    }

    async fn drop_scope(
        &self,
        owner: &MemoryOwner,
    ) -> Result<MemoryScopeDropReceipt, MemoryStoreError> {
        self.drop_scope_impl(owner).await
    }
}

impl HnswMemoryStore {
    /// One search attempt over the scope's live index.
    ///
    /// Returns `Ok(None)` when the scope has no live index and
    /// `load_if_missing` is false — the caller then retries under the insert
    /// lock so the lazy rebuild is serialized with the index/drop publishers.
    async fn search_pass(
        &self,
        scope: &MemorySearchScope,
        query: &str,
        limit: usize,
        publication_guard: Option<tokio::sync::OwnedMutexGuard<()>>,
    ) -> Result<Option<Vec<MemoryResult>>, MemoryStoreError> {
        let load_if_missing = publication_guard.is_some();
        let query = query.to_owned();
        let scope = scope.clone();
        let db_path = self.db_path.clone();
        let indices = Arc::clone(&self.indices);
        let embedding_model = Arc::clone(self.policy.embedding_model());
        let hnsw_params = self.policy.hnsw_params();
        let ef_search = hnsw_params.ef_search;

        tokio::task::spawn_blocking(move || {
            // When this is the lazy publication pass, ownership of the guard
            // lives in the blocking owner. Dropping the caller's await cannot
            // let a newer writer publish and then be overwritten by this
            // older rebuild snapshot.
            let _publication_guard = publication_guard;
            let embedding = embedding_model.embed(&query);
            let conn = open_connection(&db_path)?;
            if scope_is_tombstoned(&conn, scope.session_id())? {
                // Cross-instance finalization can publish a stale local index
                // after another instance commits the deletion tombstone. The
                // durable tombstone is checked before every cached-index read,
                // so that stale projection is discarded rather than served.
                indices
                    .write()
                    .map_err(|_| MemoryStoreError::LockPoisoned)?
                    .remove(scope.session_id());
                return Ok(Some(Vec::new()));
            }
            // Fast path: an already-loaded scope serves under the shared read
            // lock. An unloaded scope falls through to the lazy load below —
            // it must never early-return empty, which would silently hide the
            // scope's durable entries.
            let loaded_neighbors = {
                let indices = indices.read().map_err(|_| MemoryStoreError::LockPoisoned)?;
                match indices.get(scope.session_id()) {
                    None => None,
                    // Fail closed: a poisoned scope's candidate set is known to
                    // diverge from durable truth; surface the typed fault
                    // rather than serving phantom or partial results.
                    Some(ScopedIndexState::Poisoned) => {
                        return Err(MemoryStoreError::ScopePoisoned);
                    }
                    Some(ScopedIndexState::Live(index)) => {
                        Some(index.index.search(&embedding, limit, limit.max(ef_search)))
                    }
                }
            };
            let neighbors = match loaded_neighbors {
                Some(neighbors) => neighbors,
                None if !load_if_missing => return Ok(None),
                None => {
                    // Lazy per-scope load: build the live index from durable
                    // rows, publish it, and search it. The caller holds the
                    // insert lock, so no index/drop publisher can interleave
                    // between the rebuild snapshot and the publish. A scope
                    // published while we waited for the lock wins the map
                    // entry; this rebuild is then dropped. The
                    // poison/self-heal lifecycle applies to the published
                    // entry exactly as it does to eagerly-loaded scopes.
                    let rebuilt = rebuild_scoped_index_from_db(
                        &conn,
                        scope.session_id(),
                        embedding_model.as_ref(),
                        hnsw_params,
                    )?;
                    let mut indices = indices
                        .write()
                        .map_err(|_| MemoryStoreError::LockPoisoned)?;
                    match indices
                        .entry(scope.session_id().clone())
                        .or_insert(ScopedIndexState::Live(rebuilt))
                    {
                        ScopedIndexState::Poisoned => {
                            return Err(MemoryStoreError::ScopePoisoned);
                        }
                        ScopedIndexState::Live(index) => {
                            index.index.search(&embedding, limit, limit.max(ef_search))
                        }
                    }
                }
            };
            let mut results = Vec::with_capacity(neighbors.len());
            for neighbor in &neighbors {
                let point_id = i64::try_from(neighbor.d_id)
                    .map_err(|_| MemoryStoreError::PointIdOutOfRange)?;

                // A live neighbor slot without a durable row is index/store
                // divergence: fail closed with the typed fault instead of
                // silently skipping a candidate that consumed a
                // nearest-neighbor slot.
                let content = match conn
                    .query_row(
                        "SELECT content FROM memory_text WHERE point_id = ?1",
                        params![point_id],
                        |row| row.get::<_, Vec<u8>>(0),
                    )
                    .optional()
                    .map_err(|e| MemoryStoreError::Storage(e.to_string()))?
                {
                    Some(bytes) => decode_memory_text(point_id, bytes)?,
                    None => return Err(MemoryStoreError::IndexDivergence { point_id }),
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
                    None => return Err(MemoryStoreError::IndexDivergence { point_id }),
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

            Ok::<Option<Vec<MemoryResult>>, MemoryStoreError>(Some(results))
        })
        .await
        .map_err(|e| MemoryStoreError::TaskJoin(format!("search task join failed: {e}")))?
    }

    async fn drop_scope_impl(
        &self,
        owner: &MemoryOwner,
    ) -> Result<MemoryScopeDropReceipt, MemoryStoreError> {
        let owner = owner.clone();
        let db_path = self.db_path.clone();
        let indices = Arc::clone(&self.indices);

        // Serialize with in-process inserts so a concurrent batch cannot
        // publish live points for rows this drop is deleting.
        let publication_guard = Arc::clone(&self.insert_lock).lock_owned().await;

        tokio::task::spawn_blocking(
            move || -> Result<MemoryScopeDropReceipt, MemoryStoreError> {
                let _publication_guard = publication_guard;
                let mut conn = open_connection(&db_path)?;
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
                // Heal INSIDE the delete transaction: a mixed-version writer
                // inserting NULL-projection rows between an outside heal and
                // the scoped DELETE would leave its rows invisible to (and
                // surviving) the drop. The backfill is statement-at-a-time by
                // design so it composes with this caller transaction.
                backfill_null_session_ids(&tx)?;
                let session_param = owner.session_id().to_string();
                tx.execute(
                    "INSERT INTO memory_scope_tombstone (session_id, drop_generation) \
                     VALUES (?1, 1) \
                     ON CONFLICT(session_id) DO UPDATE \
                     SET drop_generation = memory_scope_tombstone.drop_generation + 1",
                    params![session_param],
                )
                .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
                // One transaction over both tables makes the drop all-or-nothing:
                // text rows go first (they are reachable only through the metadata
                // rows' point_ids), then the indexed metadata rows, whose count is
                // the receipt.
                tx.execute(
                    "DELETE FROM memory_text WHERE point_id IN \
                 (SELECT point_id FROM memory_metadata WHERE session_id = ?1)",
                    params![session_param],
                )
                .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
                let dropped_entries = tx
                    .execute(
                        "DELETE FROM memory_metadata WHERE session_id = ?1",
                        params![session_param],
                    )
                    .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
                // Scope deletion also removes every invisible/finalized stage.
                // The separate scope tombstone is deliberately retained: a
                // pending runtime outbox or stale process can then receive a
                // successful zero-finalize acknowledgement without recreating
                // the stage or its visible rows.
                tx.execute(
                    "DELETE FROM memory_compaction_stage WHERE session_id = ?1",
                    params![session_param],
                )
                .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
                tx.commit()
                    .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;

                // Drop the in-memory scope entry (loaded, poisoned, or absent);
                // the next use lazily rebuilds from the now-empty durable rows.
                // `hnsw_rs` cannot remove points from a live index, so removing
                // the whole scope entry is the only shape that leaves no phantom
                // neighbors. The allocator's high-water mark is untouched: the
                // dropped IDs are never reused.
                let mut indices = indices
                    .write()
                    .map_err(|_| MemoryStoreError::LockPoisoned)?;
                indices.remove(owner.session_id());

                Ok(MemoryScopeDropReceipt {
                    owner,
                    dropped_entries,
                })
            },
        )
        .await
        .map_err(|e| MemoryStoreError::TaskJoin(format!("drop task join failed: {e}")))?
    }

    async fn enumerate_scoped_impl(
        &self,
        scope: &MemorySearchScope,
        request: MemoryEnumerationRequest,
    ) -> Result<MemoryEnumerationPage, MemoryStoreError> {
        if request.limit == 0 {
            return Err(MemoryStoreError::EnumerationLimitZero);
        }
        let scope = scope.clone();
        let db_path = self.db_path.clone();

        tokio::task::spawn_blocking(move || -> Result<MemoryEnumerationPage, MemoryStoreError> {
            let conn = open_connection(&db_path)?;
            if scope_is_tombstoned(&conn, scope.session_id())? {
                return Ok(MemoryEnumerationPage {
                    records: Vec::new(),
                    next_offset: None,
                });
            }
            // Heal first: old-binary rows with a NULL projection cell must be
            // visible to the scoped page SELECT below.
            backfill_null_session_ids(&conn)?;

            // Page over RAW scope rows in durable-id order. One extra row is
            // fetched purely to learn whether more raw rows remain; it is
            // never decoded or returned. Clamping to i64::MAX is lossless:
            // SQLite cannot hold more rows than that, so no reachable row is
            // ever excluded by the clamp.
            let fetch_limit = i64::try_from(request.limit.saturating_add(1)).unwrap_or(i64::MAX);
            let fetch_offset = i64::try_from(request.offset).unwrap_or(i64::MAX);

            let mut raw_rows: Vec<(i64, Option<Vec<u8>>, Vec<u8>)> = Vec::new();
            {
                let mut stmt = conn
                    .prepare(
                        "SELECT m.point_id, t.content, m.metadata_json \
                         FROM memory_metadata m \
                         LEFT JOIN memory_text t ON t.point_id = m.point_id \
                         WHERE m.session_id = ?1 \
                         ORDER BY m.point_id ASC \
                         LIMIT ?2 OFFSET ?3",
                    )
                    .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
                let mapped = stmt
                    .query_map(
                        params![scope.session_id().to_string(), fetch_limit, fetch_offset],
                        |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, Option<Vec<u8>>>(1)?,
                                row.get::<_, Vec<u8>>(2)?,
                            ))
                        },
                    )
                    .map_err(|e| MemoryStoreError::Storage(e.to_string()))?;
                for row in mapped {
                    raw_rows.push(row.map_err(|e| MemoryStoreError::Storage(e.to_string()))?);
                }
            }

            let more_raw_rows_remain = raw_rows.len() > request.limit;
            raw_rows.truncate(request.limit);
            let rows_scanned = raw_rows.len();

            let mut records = Vec::new();
            for (point_id, content, metadata_json) in raw_rows {
                // Fail closed on divergent/corrupt durable rows: enumeration
                // never silently skips a row it cannot serve. A metadata row
                // without its text row is index/store divergence; undecodable
                // text bytes are typed corruption.
                let content = match content {
                    Some(bytes) => decode_memory_text(point_id, bytes)?,
                    None => return Err(MemoryStoreError::IndexDivergence { point_id }),
                };
                let metadata: MemoryMetadata = serde_json::from_slice(&metadata_json)
                    .map_err(|e| MemoryStoreError::Embedding(e.to_string()))?;
                // Post-deserialize filters on typed metadata; filtered rows
                // still count toward the raw-row offset.
                if !request.admits(&metadata) {
                    continue;
                }
                records.push(MemoryRecord { content, metadata });
            }

            let next_offset =
                more_raw_rows_remain.then(|| request.offset.saturating_add(rows_scanned));

            Ok(MemoryEnumerationPage {
                records,
                next_offset,
            })
        })
        .await
        .map_err(|e| MemoryStoreError::TaskJoin(format!("enumerate task join failed: {e}")))?
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
    use std::time::Duration;
    use std::time::{SystemTime, UNIX_EPOCH};
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

    /// Create a pre-`session_id` durable file by hand: the original two-table
    /// schema with old-shape column-list INSERTs, exactly as a pre-migration
    /// binary would leave it.
    fn create_pre_migration_db(db_path: &std::path::Path, rows: &[(i64, &SessionId, &str)]) {
        let conn = Connection::open(db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE memory_metadata (
                 point_id INTEGER PRIMARY KEY,
                 metadata_json BLOB NOT NULL
             );
             CREATE TABLE memory_text (
                 point_id INTEGER PRIMARY KEY,
                 content BLOB NOT NULL
             )",
        )
        .unwrap();
        for (point_id, session_id, text) in rows {
            let meta_json = serde_json::to_vec(&meta(session_id)).unwrap();
            conn.execute(
                "INSERT INTO memory_metadata (point_id, metadata_json) VALUES (?1, ?2)",
                params![point_id, meta_json],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO memory_text (point_id, content) VALUES (?1, ?2)",
                params![point_id, text.as_bytes()],
            )
            .unwrap();
        }
    }

    /// Insert a row the way an old (pre-migration) binary would after the
    /// schema migrated: an explicit column-list INSERT leaving the projection
    /// column NULL.
    fn insert_old_binary_row(
        db_path: &std::path::Path,
        point_id: i64,
        session_id: &SessionId,
        text: &str,
    ) {
        let conn = Connection::open(db_path).unwrap();
        let meta_json = serde_json::to_vec(&meta(session_id)).unwrap();
        conn.execute(
            "INSERT INTO memory_metadata (point_id, metadata_json) VALUES (?1, ?2)",
            params![point_id, meta_json],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO memory_text (point_id, content) VALUES (?1, ?2)",
            params![point_id, text.as_bytes()],
        )
        .unwrap();
    }

    fn query_i64(db_path: &std::path::Path, sql: &str) -> i64 {
        let conn = Connection::open(db_path).unwrap();
        conn.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    fn compaction_projection(
        session_id: &SessionId,
        parent_revision: &str,
        revision: &str,
    ) -> CompactionProjectionId {
        compaction_projection_with_actor(session_id, parent_revision, revision, "memory-stage-test")
    }

    fn compaction_projection_with_actor(
        session_id: &SessionId,
        parent_revision: &str,
        revision: &str,
        actor: &str,
    ) -> CompactionProjectionId {
        serde_json::from_value(serde_json::json!({
            "session_id": session_id,
            "parent_revision": parent_revision,
            "revision": revision,
            "commit_fingerprint": format!("sha256:test-fixture-{actor}"),
        }))
        .expect("persisted compaction projection fixture")
    }

    async fn wait_for_pause(pause: &Arc<BlockingMutationPause>) {
        let pause = Arc::clone(pause);
        tokio::task::spawn_blocking(move || {
            pause.reached.wait();
        })
        .await
        .unwrap();
    }

    async fn release_pause(pause: &Arc<BlockingMutationPause>) {
        let pause = Arc::clone(pause);
        tokio::task::spawn_blocking(move || {
            pause.release.wait();
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn durable_compaction_stage_is_invisible_across_reopen_and_finalize_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let session_id = SessionId::new();
        let projection = compaction_projection(&session_id, "parent-a", "revision-a");
        let scope = MemorySearchScope::for_session(session_id.clone());

        {
            let store = HnswMemoryStore::open(&memory_dir).unwrap();
            let receipt = store
                .stage_compaction_batch(
                    projection.clone(),
                    MemoryIndexBatch::single(request("invisible staged memory", &session_id)),
                )
                .await
                .unwrap();
            assert_eq!(receipt.staged_entries, 1);
            assert!(
                store
                    .enumerate_scoped(&scope, enumeration(10, 0))
                    .await
                    .unwrap()
                    .records
                    .is_empty(),
                "a staged batch must not be visible in-process"
            );
        }

        let store = HnswMemoryStore::open(&memory_dir).unwrap();
        assert!(
            store
                .enumerate_scoped(&scope, enumeration(10, 0))
                .await
                .unwrap()
                .records
                .is_empty(),
            "a cold reopen must not publish an uncommitted stage"
        );
        let reconcile = store
            .reconcile_compaction_stages(
                &MemoryOwner::canonical_session(session_id.clone()),
                std::slice::from_ref(&projection),
            )
            .await
            .unwrap();
        assert_eq!(reconcile.retained_committed, 1);
        assert_eq!(reconcile.aborted_orphans, 0);

        store.finalize_compaction_batch(&projection).await.unwrap();
        store.finalize_compaction_batch(&projection).await.unwrap();
        let page = store
            .enumerate_scoped(&scope, enumeration(10, 0))
            .await
            .unwrap();
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.records[0].content, "invisible staged memory");

        // Finalized tombstones retain the exact payload digest: an identical
        // retry is accepted, while a conflicting reuse of the rewrite id is
        // rejected without adding a second visible row.
        store
            .stage_compaction_batch(
                projection.clone(),
                MemoryIndexBatch::single(request("invisible staged memory", &session_id)),
            )
            .await
            .unwrap();
        let conflict = store
            .stage_compaction_batch(
                projection,
                MemoryIndexBatch::single(request("conflicting retry", &session_id)),
            )
            .await
            .unwrap_err();
        assert!(
            conflict
                .to_string()
                .contains("conflicting durable compaction stage")
        );
        assert_eq!(
            store
                .enumerate_scoped(&scope, enumeration(10, 0))
                .await
                .unwrap()
                .records
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn dropped_stage_await_retries_same_rewrite_with_new_timestamp_idempotently() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let session_id = SessionId::new();
        let first_projection =
            compaction_projection(&session_id, "parent-cancel", "revision-cancel");
        let retry_projection =
            compaction_projection(&session_id, "parent-cancel", "revision-cancel");
        assert_eq!(first_projection, retry_projection);
        let scope = MemorySearchScope::for_session(session_id.clone());
        let store = Arc::new(HnswMemoryStore::open(&memory_dir).unwrap());
        let first_timestamp = UNIX_EPOCH + Duration::from_secs(10);
        let retry_timestamp = UNIX_EPOCH + Duration::from_secs(20);
        let pause = store.arm_stage_commit_pause();
        let stage_store = Arc::clone(&store);
        let stage_session = session_id.clone();
        let stage = tokio::spawn(async move {
            stage_store
                .stage_compaction_batch(
                    first_projection,
                    MemoryIndexBatch::single(request_with(
                        "recover after cancellation",
                        &stage_session,
                        MessageRange::single(0),
                        first_timestamp,
                    )),
                )
                .await
        });
        wait_for_pause(&pause).await;
        stage.abort();
        assert!(stage.await.unwrap_err().is_cancelled());
        release_pause(&pause).await;

        assert!(
            store
                .enumerate_scoped(&scope, enumeration(10, 0))
                .await
                .unwrap()
                .records
                .is_empty()
        );

        // The retry carries a newly minted commit/index timestamp but the
        // same semantic rewrite and payload. It must match the durable first
        // stage rather than conflict or duplicate it.
        store
            .stage_compaction_batch(
                retry_projection.clone(),
                MemoryIndexBatch::single(request_with(
                    "recover after cancellation",
                    &session_id,
                    MessageRange::single(0),
                    retry_timestamp,
                )),
            )
            .await
            .unwrap();
        store
            .finalize_compaction_batch(&retry_projection)
            .await
            .unwrap();
        let page = store
            .enumerate_scoped(&scope, enumeration(10, 0))
            .await
            .unwrap();
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.records[0].content, "recover after cancellation");
        assert_eq!(page.records[0].metadata.indexed_at, first_timestamp);
    }

    #[tokio::test]
    async fn cancelled_finalize_cannot_overwrite_a_newer_live_index() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let session_id = SessionId::new();
        let projection = compaction_projection(&session_id, "parent-finalize", "revision-finalize");
        let scope = MemorySearchScope::for_session(session_id.clone());
        let store = Arc::new(HnswMemoryStore::open(&memory_dir).unwrap());
        store
            .stage_compaction_batch(
                projection.clone(),
                MemoryIndexBatch::single(request("compaction publication", &session_id)),
            )
            .await
            .unwrap();

        let pause = store.arm_finalize_publish_pause();
        let finalize_store = Arc::clone(&store);
        let finalize =
            tokio::spawn(
                async move { finalize_store.finalize_compaction_batch(&projection).await },
            );
        wait_for_pause(&pause).await;
        finalize.abort();
        assert!(finalize.await.unwrap_err().is_cancelled());

        let index_store = Arc::clone(&store);
        let index_session = session_id.clone();
        let mut concurrent_index = tokio::spawn(async move {
            index_store
                .index_scoped(request("newer concurrent memory", &index_session))
                .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut concurrent_index)
                .await
                .is_err(),
            "detached finalization must retain the publication lock"
        );
        release_pause(&pause).await;
        concurrent_index.await.unwrap().unwrap();

        let page = store
            .enumerate_scoped(&scope, enumeration(10, 0))
            .await
            .unwrap();
        assert_eq!(page.records.len(), 2);
        let results = store.search(&scope, "memory", 10).await.unwrap();
        let contents = results
            .iter()
            .map(|result| result.content.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert!(contents.contains("compaction publication"));
        assert!(contents.contains("newer concurrent memory"));
    }

    #[tokio::test]
    async fn scope_drop_serializes_with_finalize_and_retains_deletion_tombstone() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let session_id = SessionId::new();
        let projection = compaction_projection(&session_id, "parent-drop", "revision-drop");
        let owner = MemoryOwner::canonical_session(session_id.clone());
        let scope = MemorySearchScope::for_session(session_id.clone());
        let store = Arc::new(HnswMemoryStore::open(&memory_dir).unwrap());
        store
            .stage_compaction_batch(
                projection.clone(),
                MemoryIndexBatch::single(request("must not resurrect", &session_id)),
            )
            .await
            .unwrap();

        let pause = store.arm_finalize_publish_pause();
        let finalize_store = Arc::clone(&store);
        let finalize_projection = projection.clone();
        let finalize = tokio::spawn(async move {
            finalize_store
                .finalize_compaction_batch(&finalize_projection)
                .await
        });
        wait_for_pause(&pause).await;

        let drop_store = Arc::clone(&store);
        let mut drop_task = tokio::spawn(async move { drop_store.drop_scope(&owner).await });
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut drop_task)
                .await
                .is_err(),
            "scope drop must serialize behind the detached finalizer"
        );
        release_pause(&pause).await;
        finalize.await.unwrap().unwrap();
        let receipt = drop_task.await.unwrap().unwrap();
        assert_eq!(receipt.dropped_entries, 1);
        assert!(
            store
                .enumerate_scoped(&scope, enumeration(10, 0))
                .await
                .unwrap()
                .records
                .is_empty()
        );
        let finalized_after_drop = store.finalize_compaction_batch(&projection).await.unwrap();
        assert_eq!(finalized_after_drop.indexed_entries, 0);
        let restaged = store
            .stage_compaction_batch(
                projection.clone(),
                MemoryIndexBatch::single(request("must still not resurrect", &session_id)),
            )
            .await
            .unwrap();
        assert_eq!(restaged.staged_entries, 0);
        let reconciled = store
            .reconcile_compaction_stages(
                &MemoryOwner::canonical_session(session_id.clone()),
                std::slice::from_ref(&projection),
            )
            .await
            .unwrap();
        assert_eq!(reconciled, CompactionStageReconcileReceipt::default());
    }

    #[tokio::test]
    async fn dropped_scope_acks_pending_outbox_and_stale_stage_across_reopen() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let db_path = memory_dir.join("memory.sqlite3");
        let session_id = SessionId::new();
        let owner = MemoryOwner::canonical_session(session_id.clone());
        let scope = MemorySearchScope::for_session(session_id.clone());
        let projection = compaction_projection(&session_id, "parent-pending", "revision-pending");

        {
            let store = HnswMemoryStore::open(&memory_dir).unwrap();
            store
                .stage_compaction_batch(
                    projection.clone(),
                    MemoryIndexBatch::single(request("pending outbox payload", &session_id)),
                )
                .await
                .unwrap();
            let dropped = store.drop_scope(&owner).await.unwrap();
            assert_eq!(dropped.dropped_entries, 0);

            // `projection` models a RuntimeStore row that committed before the
            // session deletion. Reconcile + finalize must both acknowledge it
            // without publishing, so RuntimeStore can clear that pending row.
            let reconciled = store
                .reconcile_compaction_stages(&owner, std::slice::from_ref(&projection))
                .await
                .unwrap();
            assert_eq!(reconciled, CompactionStageReconcileReceipt::default());
            assert_eq!(
                store
                    .finalize_compaction_batch(&projection)
                    .await
                    .unwrap()
                    .indexed_entries,
                0
            );
            assert_eq!(
                store
                    .stage_compaction_batch(
                        projection.clone(),
                        MemoryIndexBatch::single(request("stale restage", &session_id)),
                    )
                    .await
                    .unwrap()
                    .staged_entries,
                0
            );
            assert_eq!(
                store
                    .index_scoped(request("stale direct publication", &session_id))
                    .await
                    .unwrap()
                    .indexed_entries,
                0
            );
        }

        let conn = Connection::open(&db_path).unwrap();
        let tombstones: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memory_scope_tombstone WHERE session_id = ?1",
                params![session_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tombstones, 1, "scope tombstone must survive close/reopen");
        drop(conn);

        let reopened = HnswMemoryStore::open(&memory_dir).unwrap();
        assert_eq!(
            reopened
                .reconcile_compaction_stages(&owner, std::slice::from_ref(&projection))
                .await
                .unwrap(),
            CompactionStageReconcileReceipt::default()
        );
        assert_eq!(
            reopened
                .finalize_compaction_batch(&projection)
                .await
                .unwrap()
                .indexed_entries,
            0
        );
        assert!(
            reopened
                .enumerate_scoped(&scope, enumeration(10, 0))
                .await
                .unwrap()
                .records
                .is_empty()
        );
        assert!(
            reopened
                .search(&scope, "payload", 10)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn cross_instance_drop_wins_after_finalize_commit_before_live_publish() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let session_id = SessionId::new();
        let owner = MemoryOwner::canonical_session(session_id.clone());
        let scope = MemorySearchScope::for_session(session_id.clone());
        let projection = compaction_projection(
            &session_id,
            "parent-cross-instance",
            "revision-cross-instance",
        );
        let finalizing = Arc::new(HnswMemoryStore::open(&memory_dir).unwrap());
        finalizing
            .stage_compaction_batch(
                projection.clone(),
                MemoryIndexBatch::single(request("cross-instance stale index", &session_id)),
            )
            .await
            .unwrap();

        let pause = finalizing.arm_finalize_publish_pause();
        let finalizing_task_store = Arc::clone(&finalizing);
        let finalize_projection = projection.clone();
        let finalize_task = tokio::spawn(async move {
            finalizing_task_store
                .finalize_compaction_batch(&finalize_projection)
                .await
        });
        wait_for_pause(&pause).await;

        // A distinct store instance has a distinct process-local publication
        // lock. Its SQLite tombstone/delete transaction must still win after
        // the first instance committed rows but before it publishes its cached
        // HNSW rebuild.
        let deleting = HnswMemoryStore::open(&memory_dir).unwrap();
        let deleted = tokio::time::timeout(Duration::from_secs(1), deleting.drop_scope(&owner))
            .await
            .expect("cross-instance drop must not wait on another instance's live-index lock")
            .unwrap();
        assert_eq!(deleted.dropped_entries, 1);
        release_pause(&pause).await;
        assert_eq!(
            finalize_task.await.unwrap().unwrap().indexed_entries,
            1,
            "the overlapping finalizer may acknowledge its pre-delete commit"
        );

        assert!(
            finalizing
                .search(&scope, "stale", 10)
                .await
                .unwrap()
                .is_empty(),
            "durable tombstone must fence a stale cached index published after deletion"
        );
        assert_eq!(
            finalizing
                .finalize_compaction_batch(&projection)
                .await
                .unwrap()
                .indexed_entries,
            0
        );
        assert_eq!(
            finalizing
                .stage_compaction_batch(
                    projection,
                    MemoryIndexBatch::single(request("stale replay", &session_id)),
                )
                .await
                .unwrap()
                .staged_entries,
            0
        );
    }

    #[tokio::test]
    async fn empty_authority_reconciliation_aborts_only_unfinalized_orphans() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let session_id = SessionId::new();
        let projection = compaction_projection(&session_id, "parent-orphan", "revision-orphan");
        let store = HnswMemoryStore::open(&memory_dir).unwrap();
        store
            .stage_compaction_batch(
                projection.clone(),
                MemoryIndexBatch::single(request("must stay invisible", &session_id)),
            )
            .await
            .unwrap();

        let receipt = store
            .reconcile_compaction_stages(&MemoryOwner::canonical_session(session_id.clone()), &[])
            .await
            .unwrap();
        assert_eq!(receipt.retained_committed, 0);
        assert_eq!(receipt.aborted_orphans, 1);
        let missing = store
            .finalize_compaction_batch(&projection)
            .await
            .unwrap_err();
        assert!(
            missing
                .to_string()
                .contains("missing durable compaction stage")
        );
        assert!(
            store
                .enumerate_scoped(
                    &MemorySearchScope::for_session(session_id),
                    enumeration(10, 0),
                )
                .await
                .unwrap()
                .records
                .is_empty()
        );
    }

    #[tokio::test]
    async fn reconciliation_distinguishes_semantic_commits_with_the_same_content_edge() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let session_id = SessionId::new();
        let committed = compaction_projection_with_actor(
            &session_id,
            "shared-parent",
            "shared-revision",
            "committed-actor",
        );
        let orphan = compaction_projection_with_actor(
            &session_id,
            "shared-parent",
            "shared-revision",
            "orphan-actor",
        );
        assert_ne!(committed, orphan);
        let store = HnswMemoryStore::open(&memory_dir).unwrap();
        store
            .stage_compaction_batch(
                committed.clone(),
                MemoryIndexBatch::single(request("committed projection", &session_id)),
            )
            .await
            .unwrap();
        store
            .stage_compaction_batch(
                orphan.clone(),
                MemoryIndexBatch::single(request("orphan projection", &session_id)),
            )
            .await
            .unwrap();

        let receipt = store
            .reconcile_compaction_stages(
                &MemoryOwner::canonical_session(session_id.clone()),
                std::slice::from_ref(&committed),
            )
            .await
            .unwrap();
        assert_eq!(receipt.retained_committed, 1);
        assert_eq!(receipt.aborted_orphans, 1);
        store.finalize_compaction_batch(&committed).await.unwrap();
        assert!(store.finalize_compaction_batch(&orphan).await.is_err());
        let page = store
            .enumerate_scoped(
                &MemorySearchScope::for_session(session_id),
                enumeration(10, 0),
            )
            .await
            .unwrap();
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.records[0].content, "committed projection");
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
                0,
                "open is lazy: no scoped indexes are built until first use"
            );

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

            assert_eq!(
                store.hnsw_index_count(),
                1,
                "only the searched scope is loaded"
            );
            assert!(
                store
                    .hnsw_index_hints()
                    .iter()
                    .all(|hint| *hint == MIN_INDEX_ELEMENTS_HINT),
                "lazily loaded one-entry scoped indexes keep bounded hints"
            );
            assert_eq!(store.hnsw_point_count(), 1);
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

    /// Overwrite a point's durable text bytes with invalid UTF-8, out-of-band.
    fn corrupt_text_bytes(db_path: &std::path::Path, bytes: &[u8]) {
        let conn = Connection::open(db_path).unwrap();
        let updated = conn
            .execute("UPDATE memory_text SET content = ?1", params![bytes])
            .unwrap();
        assert!(updated > 0, "corruption fixture must hit at least one row");
    }

    /// Gate: corrupt durable text bytes are a typed store-corruption fault on
    /// the first lazy scope load after reopen, never lossy-decoded into
    /// searchable content. (Open itself is migration + heal only and does not
    /// touch text rows.)
    #[tokio::test]
    async fn test_corrupt_text_bytes_fail_closed_on_first_scope_load() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let session_id = SessionId::new();
        {
            let store = HnswMemoryStore::open(&memory_dir).unwrap();
            store
                .index_scoped(request("clean entry before corruption", &session_id))
                .await
                .unwrap();
        }
        corrupt_text_bytes(&memory_dir.join("memory.sqlite3"), &[0xff, 0xfe, 0x41]);

        let store = HnswMemoryStore::open(&memory_dir).unwrap();
        let scope = MemorySearchScope::for_session(session_id);
        let err = store
            .search(&scope, "clean entry before corruption", 5)
            .await
            .unwrap_err();
        assert_eq!(err.error_code(), "memory_text_corruption");
    }

    /// Gate: corrupt durable text bytes surface as the typed fault on the
    /// search rehydration path instead of returning mojibake content.
    #[tokio::test]
    async fn test_corrupt_text_bytes_fail_closed_on_search() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let store = HnswMemoryStore::open(&memory_dir).unwrap();
        let session_id = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());
        store
            .index_scoped(request("entry destined for corruption", &session_id))
            .await
            .unwrap();
        corrupt_text_bytes(&memory_dir.join("memory.sqlite3"), &[0xc3, 0x28]);

        let err = store
            .search(&scope, "entry destined for corruption", 5)
            .await
            .unwrap_err();
        assert_eq!(err.error_code(), "memory_text_corruption");
    }

    /// Gate: a live neighbor slot whose durable row is gone is typed
    /// index/store divergence, not a silently skipped candidate.
    #[tokio::test]
    async fn test_missing_durable_row_is_typed_divergence_not_skip() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let store = HnswMemoryStore::open(&memory_dir).unwrap();
        let session_id = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());
        store
            .index_scoped(request("row deleted out of band", &session_id))
            .await
            .unwrap();
        {
            let conn = Connection::open(memory_dir.join("memory.sqlite3")).unwrap();
            conn.execute("DELETE FROM memory_text", []).unwrap();
        }

        let err = store
            .search(&scope, "row deleted out of band", 5)
            .await
            .unwrap_err();
        assert_eq!(err.error_code(), "memory_index_divergence");
    }

    /// Gate: when the post-rollback repair rebuild itself fails, the scope is
    /// poisoned (search fails closed with the typed fault, never phantom
    /// neighbors), and the next successful index attempt self-heals the scope
    /// by rebuilding it from durable rows.
    #[tokio::test]
    async fn test_repair_failure_poisons_scope_then_next_index_self_heals() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let db_path = memory_dir.join("memory.sqlite3");
        let store = HnswMemoryStore::open(&memory_dir).unwrap();
        let session_id = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());

        // A committed survivor whose text we corrupt so the repair rebuild
        // fails with a typed fault.
        store
            .index_scoped(request("survivor pending corruption", &session_id))
            .await
            .unwrap();
        corrupt_text_bytes(&db_path, &[0xff, 0x00, 0x41]);

        // Partial mid-batch in-memory failure: rollback succeeds, but the
        // repair rebuild hits the corrupt survivor and fails.
        store.arm_hnsw_insert_failure_after(1);
        let batch = MemoryIndexBatch::new(
            MemoryIndexScope::for_session(session_id.clone()),
            vec![
                request("doomed one", &session_id),
                request("doomed two", &session_id),
            ],
        )
        .unwrap();
        let err = store.index_scoped_batch(batch).await.unwrap_err();
        assert_eq!(err.error_code(), "memory_scope_repair_failed");
        store.arm_hnsw_insert_failure_after(-1);

        // The poisoned scope fails reads closed.
        let err = store.search(&scope, "survivor", 5).await.unwrap_err();
        assert_eq!(err.error_code(), "memory_scope_poisoned");

        // Restore valid durable bytes, then index again: the scope self-heals
        // by rebuilding from durable rows (survivor + the new entry).
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "UPDATE memory_text SET content = ?1",
                params![b"survivor restored text".as_slice()],
            )
            .unwrap();
        }
        store
            .index_scoped(request("fresh entry after heal", &session_id))
            .await
            .unwrap();

        let results = store
            .search(&scope, "fresh entry after heal", 5)
            .await
            .unwrap();
        assert!(
            results
                .iter()
                .any(|r| r.content.contains("fresh entry after heal")),
            "self-healed scope must serve durable-derived candidates"
        );
        let survivor = store
            .search(&scope, "survivor restored text", 5)
            .await
            .unwrap();
        assert!(
            survivor
                .iter()
                .any(|r| r.content.contains("survivor restored text")),
            "self-healed scope must include pre-existing durable rows"
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
        assert_eq!(
            MemoryStoreError::TextCorruption { point_id: 7 }.error_code(),
            "memory_text_corruption"
        );
        assert_eq!(
            MemoryStoreError::IndexDivergence { point_id: 7 }.error_code(),
            "memory_index_divergence"
        );
        assert_eq!(
            MemoryStoreError::ScopePoisoned.error_code(),
            "memory_scope_poisoned"
        );
        assert_eq!(
            MemoryStoreError::ScopeRepairFailed {
                original: Box::new(MemoryStoreError::LockPoisoned),
                repair: Box::new(MemoryStoreError::TextCorruption { point_id: 7 }),
            }
            .error_code(),
            "memory_scope_repair_failed"
        );
    }

    /// Gate: opening a pre-`session_id` durable file migrates it in place —
    /// the projection column is backfilled from typed metadata, the allocator
    /// is seeded from `MAX(point_id)`, and every old row stays searchable and
    /// enumerable.
    #[tokio::test]
    async fn test_migration_from_pre_session_id_schema_heals_and_serves() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        std::fs::create_dir_all(&memory_dir).unwrap();
        let db_path = memory_dir.join("memory.sqlite3");
        let session_a = SessionId::new();
        let session_b = SessionId::new();
        create_pre_migration_db(
            &db_path,
            &[
                (0, &session_a, "alpha entry from the old world"),
                (1, &session_a, "beta entry from the old world"),
                (2, &session_b, "gamma entry in another scope"),
            ],
        );

        let store = HnswMemoryStore::open(&memory_dir).unwrap();

        // Migration backfilled every row's projection column.
        assert_eq!(
            query_i64(
                &db_path,
                "SELECT COUNT(*) FROM memory_metadata WHERE session_id IS NULL",
            ),
            0
        );
        // Allocator seeded from MAX(point_id) + 1.
        assert_eq!(
            query_i64(
                &db_path,
                "SELECT high_water FROM memory_allocator WHERE id = 0"
            ),
            3
        );

        // Old rows are searchable per scope.
        let scope_a = MemorySearchScope::for_session(session_a.clone());
        let results = store
            .search(&scope_a, "alpha entry from the old world", 10)
            .await
            .unwrap();
        assert!(!results.is_empty());
        assert!(results.iter().all(|r| r.metadata.session_id == session_a));

        // And enumerable in durable-id order.
        let page = store
            .enumerate_scoped(&scope_a, enumeration(10, 0))
            .await
            .unwrap();
        assert_eq!(page.records.len(), 2);
        assert!(page.records[0].content.contains("alpha"));
        assert!(page.records[1].content.contains("beta"));
        assert_eq!(page.next_offset, None);

        // New inserts continue past the migrated rows' IDs.
        store
            .index_scoped(request("delta entry post migration", &session_a))
            .await
            .unwrap();
        assert_eq!(
            query_i64(&db_path, "SELECT MAX(point_id) FROM memory_metadata"),
            3
        );
    }

    /// Gate: the migration is idempotent across reopens — no duplicate
    /// columns, allocator preserved, store fully functional.
    #[tokio::test]
    async fn test_migration_is_idempotent_across_reopens() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let session_id = SessionId::new();

        {
            let store = HnswMemoryStore::open(&memory_dir).unwrap();
            store
                .index_scoped(request("entry surviving reopens", &session_id))
                .await
                .unwrap();
        }
        {
            let _store = HnswMemoryStore::open(&memory_dir).unwrap();
        }
        let store = HnswMemoryStore::open(&memory_dir).unwrap();

        let db_path = memory_dir.join("memory.sqlite3");
        let conn = Connection::open(&db_path).unwrap();
        let session_id_columns: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('memory_metadata') WHERE name = 'session_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            session_id_columns, 1,
            "repeated opens must not re-add the column"
        );
        assert_eq!(
            query_i64(&db_path, "SELECT COUNT(*) FROM memory_allocator"),
            1,
            "allocator stays a single row"
        );

        let scope = MemorySearchScope::for_session(session_id);
        let results = store
            .search(&scope, "entry surviving reopens", 5)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
    }

    /// Gate: a row written by an old binary after migration (column-list
    /// INSERT, NULL projection cell) is healed by the lazy scope load and is
    /// fully searchable — never permanently invisible.
    #[tokio::test]
    async fn test_null_session_id_row_healed_by_lazy_search_load() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let db_path = memory_dir.join("memory.sqlite3");
        let session_id = SessionId::new();
        let store = HnswMemoryStore::open(&memory_dir).unwrap();

        insert_old_binary_row(&db_path, 100, &session_id, "row from an old binary");

        let scope = MemorySearchScope::for_session(session_id.clone());
        let results = store
            .search(&scope, "row from an old binary", 5)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].metadata.session_id, session_id);
        assert_eq!(
            query_i64(
                &db_path,
                "SELECT COUNT(*) FROM memory_metadata WHERE session_id IS NULL",
            ),
            0,
            "the lazy load healed the projection cell"
        );
    }

    /// Gate: enumerate and drop heal NULL projection cells before their
    /// scoped SQL, so old-binary rows are enumerated and dropped, never
    /// silently skipped.
    #[tokio::test]
    async fn test_null_session_id_row_healed_before_enumerate_and_drop() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let db_path = memory_dir.join("memory.sqlite3");
        let session_id = SessionId::new();
        let store = HnswMemoryStore::open(&memory_dir).unwrap();

        insert_old_binary_row(&db_path, 100, &session_id, "old binary row to enumerate");

        let scope = MemorySearchScope::for_session(session_id.clone());
        let page = store
            .enumerate_scoped(&scope, enumeration(10, 0))
            .await
            .unwrap();
        assert_eq!(page.records.len(), 1);
        assert!(page.records[0].content.contains("old binary row"));

        // A fresh old-binary row is also visible to drop's heal.
        insert_old_binary_row(&db_path, 101, &session_id, "second old binary row");
        let receipt = store
            .drop_scope(&MemoryOwner::canonical_session(session_id.clone()))
            .await
            .unwrap();
        assert_eq!(receipt.dropped_entries, 2);
        assert_eq!(receipt.owner.session_id(), &session_id);
        assert_eq!(
            query_i64(&db_path, "SELECT COUNT(*) FROM memory_metadata"),
            0
        );
        assert_eq!(query_i64(&db_path, "SELECT COUNT(*) FROM memory_text"), 0);
    }

    /// Gate: an old-binary row whose point_id ran past the allocator's
    /// high-water mark must not wedge inserts on a permanent primary-key
    /// collision — allocation self-heals past MAX(point_id).
    #[tokio::test]
    async fn test_old_binary_row_does_not_wedge_point_allocation() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let db_path = memory_dir.join("memory.sqlite3");
        let session_id = SessionId::new();
        let store = HnswMemoryStore::open(&memory_dir).unwrap();

        // high_water is 0; the old-binary row lands far past it.
        insert_old_binary_row(&db_path, 100, &session_id, "old binary high row");

        store
            .index_scoped(request("new binary row after the gap", &session_id))
            .await
            .unwrap();
        assert_eq!(
            query_i64(&db_path, "SELECT MAX(point_id) FROM memory_metadata"),
            101,
            "allocation must jump past the old-binary row"
        );
        assert_eq!(
            query_i64(
                &db_path,
                "SELECT high_water FROM memory_allocator WHERE id = 0"
            ),
            102
        );
    }

    /// Danger-line regression: indexing into an existing-but-unloaded scope
    /// after a fresh open must build the live index from durable rows first —
    /// an empty live index would silently hide the scope's prior entries.
    #[tokio::test]
    async fn test_index_into_unloaded_existing_scope_serves_prior_entries() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let session_id = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());

        {
            let store = HnswMemoryStore::open(&memory_dir).unwrap();
            store
                .index_scoped(request("prior alpha entry", &session_id))
                .await
                .unwrap();
        }

        let store = HnswMemoryStore::open(&memory_dir).unwrap();
        assert_eq!(store.hnsw_index_count(), 0, "open must not load scopes");
        // Index into the unloaded scope WITHOUT searching first.
        store
            .index_scoped(request("fresh beta entry", &session_id))
            .await
            .unwrap();

        let prior = store.search(&scope, "prior alpha entry", 5).await.unwrap();
        assert!(
            prior
                .iter()
                .any(|r| r.content.contains("prior alpha entry")),
            "prior durable entries must stay searchable after an insert into an unloaded scope"
        );
        let fresh = store.search(&scope, "fresh beta entry", 5).await.unwrap();
        assert!(fresh.iter().any(|r| r.content.contains("fresh beta entry")));
        assert_eq!(store.hnsw_point_count(), 2);
    }

    /// Gate: drop_scope removes the scope's rows durably (both tables, one
    /// transaction), reports the metadata-row count, and leaves other scopes
    /// untouched.
    #[tokio::test]
    async fn test_drop_scope_removes_rows_durably_and_preserves_other_scopes() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let db_path = memory_dir.join("memory.sqlite3");
        let session_a = SessionId::new();
        let session_b = SessionId::new();
        let scope_a = MemorySearchScope::for_session(session_a.clone());
        let scope_b = MemorySearchScope::for_session(session_b.clone());

        {
            let store = HnswMemoryStore::open(&memory_dir).unwrap();
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

            let dropped = store
                .search(&scope_a, "doomed alpha entry", 10)
                .await
                .unwrap();
            assert!(dropped.is_empty(), "dropped scope must serve nothing");
            let page = store
                .enumerate_scoped(&scope_a, enumeration(10, 0))
                .await
                .unwrap();
            assert!(page.records.is_empty());
            assert_eq!(page.next_offset, None);

            let surviving = store
                .search(&scope_b, "surviving gamma entry", 10)
                .await
                .unwrap();
            assert_eq!(surviving.len(), 1);
        }

        // Durability: rows are gone across reopen, the other scope's remain.
        assert_eq!(
            query_i64(&db_path, "SELECT COUNT(*) FROM memory_metadata"),
            1
        );
        assert_eq!(query_i64(&db_path, "SELECT COUNT(*) FROM memory_text"), 1);
        let store = HnswMemoryStore::open(&memory_dir).unwrap();
        assert!(
            store
                .search(&scope_a, "doomed alpha entry", 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .search(&scope_b, "surviving gamma entry", 10)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    /// Dropping a scope with no durable rows is a successful no-op receipt.
    #[tokio::test]
    async fn test_drop_scope_unknown_scope_reports_zero() {
        let dir = TempDir::new().unwrap();
        let store = HnswMemoryStore::open(dir.path().join("memory")).unwrap();
        let receipt = store
            .drop_scope(&MemoryOwner::canonical_session(SessionId::new()))
            .await
            .unwrap();
        assert_eq!(receipt.dropped_entries, 0);
    }

    /// Gate: point IDs are never reused after drop_scope — the durable
    /// allocator's high-water mark survives the deletion.
    #[tokio::test]
    async fn test_point_ids_never_reused_after_drop_scope() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let db_path = memory_dir.join("memory.sqlite3");
        let session_id = SessionId::new();
        let store = HnswMemoryStore::open(&memory_dir).unwrap();

        store
            .index_scoped(request("first doomed entry", &session_id))
            .await
            .unwrap();
        store
            .index_scoped(request("second doomed entry", &session_id))
            .await
            .unwrap();
        assert_eq!(
            query_i64(&db_path, "SELECT MAX(point_id) FROM memory_metadata"),
            1
        );

        store
            .drop_scope(&MemoryOwner::canonical_session(session_id.clone()))
            .await
            .unwrap();

        let replacement_session_id = SessionId::new();
        store
            .index_scoped(request("entry after the drop", &replacement_session_id))
            .await
            .unwrap();
        assert_eq!(
            query_i64(&db_path, "SELECT MAX(point_id) FROM memory_metadata"),
            2,
            "IDs must strictly increase across a drop, never recycle"
        );
        assert_eq!(
            query_i64(
                &db_path,
                "SELECT high_water FROM memory_allocator WHERE id = 0"
            ),
            3
        );
    }

    /// Gate: enumeration pages raw scope rows deterministically in
    /// durable-id (insertion) order, scoped to the requested owner.
    #[tokio::test]
    async fn test_enumerate_scoped_pages_deterministically_in_insertion_order() {
        let dir = TempDir::new().unwrap();
        let store = HnswMemoryStore::open(dir.path().join("memory")).unwrap();
        let session_id = SessionId::new();
        let other_session = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());

        let texts = [
            "entry number zero",
            "entry number one",
            "entry number two",
            "entry number three",
            "entry number four",
        ];
        for (i, text) in texts.iter().enumerate() {
            store
                .index_scoped(request(*text, &session_id))
                .await
                .unwrap();
            // Interleave another scope's rows: they must not perturb this
            // scope's raw-offset accounting.
            store
                .index_scoped(request(format!("interloper {i}"), &other_session))
                .await
                .unwrap();
        }

        let first = store
            .enumerate_scoped(&scope, enumeration(2, 0))
            .await
            .unwrap();
        assert_eq!(first.records.len(), 2);
        assert_eq!(first.records[0].content, "entry number zero");
        assert_eq!(first.records[1].content, "entry number one");
        assert_eq!(first.next_offset, Some(2));

        let second = store
            .enumerate_scoped(&scope, enumeration(2, 2))
            .await
            .unwrap();
        assert_eq!(second.records.len(), 2);
        assert_eq!(second.records[0].content, "entry number two");
        assert_eq!(second.records[1].content, "entry number three");
        assert_eq!(second.next_offset, Some(4));

        let last = store
            .enumerate_scoped(&scope, enumeration(2, 4))
            .await
            .unwrap();
        assert_eq!(last.records.len(), 1);
        assert_eq!(last.records[0].content, "entry number four");
        assert_eq!(last.next_offset, None);

        // Determinism: an identical request returns the identical page.
        let replay = store
            .enumerate_scoped(&scope, enumeration(2, 0))
            .await
            .unwrap();
        assert_eq!(replay.records.len(), 2);
        assert_eq!(replay.records[0].content, "entry number zero");
        assert_eq!(replay.records[1].content, "entry number one");
        assert_eq!(replay.next_offset, Some(2));
    }

    /// Gate: `source_overlap` filters on typed metadata after the raw-row
    /// page is selected — a page may return fewer than `limit` records while
    /// `next_offset` still advances by raw rows scanned.
    #[tokio::test]
    async fn test_enumerate_scoped_source_overlap_filters_post_deserialize() {
        let dir = TempDir::new().unwrap();
        let store = HnswMemoryStore::open(dir.path().join("memory")).unwrap();
        let session_id = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());
        let indexed_at = UNIX_EPOCH + Duration::from_secs(1_000);

        store
            .index_scoped(request_with(
                "covers zero to five",
                &session_id,
                MessageRange::new(0, 5).unwrap(),
                indexed_at,
            ))
            .await
            .unwrap();
        store
            .index_scoped(request_with(
                "covers five to ten",
                &session_id,
                MessageRange::new(5, 10).unwrap(),
                indexed_at,
            ))
            .await
            .unwrap();
        store
            .index_scoped(request_with(
                "covers ten to fifteen",
                &session_id,
                MessageRange::new(10, 15).unwrap(),
                indexed_at,
            ))
            .await
            .unwrap();

        let page = store
            .enumerate_scoped(
                &scope,
                MemoryEnumerationRequest {
                    limit: 10,
                    offset: 0,
                    source_overlap: Some(MessageRange::new(4, 6).unwrap()),
                    indexed_after: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(page.records.len(), 2);
        assert_eq!(page.records[0].content, "covers zero to five");
        assert_eq!(page.records[1].content, "covers five to ten");
        assert_eq!(page.next_offset, None);

        // Fewer than limit: only the first raw row survives the filter, but
        // all three raw rows were scanned.
        let narrow = store
            .enumerate_scoped(
                &scope,
                MemoryEnumerationRequest {
                    limit: 10,
                    offset: 0,
                    source_overlap: Some(MessageRange::new(0, 1).unwrap()),
                    indexed_after: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(narrow.records.len(), 1);
        assert_eq!(narrow.records[0].content, "covers zero to five");
        assert_eq!(narrow.next_offset, None);
    }

    /// Gate: `indexed_after` admits strictly-later records only,
    /// disambiguating compaction generations whose source offsets restart.
    #[tokio::test]
    async fn test_enumerate_scoped_indexed_after_filter() {
        let dir = TempDir::new().unwrap();
        let store = HnswMemoryStore::open(dir.path().join("memory")).unwrap();
        let session_id = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());
        let generation_one = UNIX_EPOCH + Duration::from_secs(1_000);
        let generation_two = UNIX_EPOCH + Duration::from_secs(2_000);

        store
            .index_scoped(request_with(
                "first generation summary",
                &session_id,
                MessageRange::new(0, 3).unwrap(),
                generation_one,
            ))
            .await
            .unwrap();
        store
            .index_scoped(request_with(
                "second generation summary",
                &session_id,
                MessageRange::new(0, 3).unwrap(),
                generation_two,
            ))
            .await
            .unwrap();

        let page = store
            .enumerate_scoped(
                &scope,
                MemoryEnumerationRequest {
                    limit: 10,
                    offset: 0,
                    source_overlap: None,
                    indexed_after: Some(generation_one),
                },
            )
            .await
            .unwrap();
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.records[0].content, "second generation summary");

        // Strictly after: the boundary instant itself is excluded.
        let none_left = store
            .enumerate_scoped(
                &scope,
                MemoryEnumerationRequest {
                    limit: 10,
                    offset: 0,
                    source_overlap: None,
                    indexed_after: Some(generation_two),
                },
            )
            .await
            .unwrap();
        assert!(none_left.records.is_empty());
    }

    /// Gate: enumeration propagates typed corruption instead of skipping the
    /// undecodable row.
    #[tokio::test]
    async fn test_enumerate_scoped_corruption_fails_closed() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let store = HnswMemoryStore::open(&memory_dir).unwrap();
        let session_id = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());
        store
            .index_scoped(request("entry destined for corruption", &session_id))
            .await
            .unwrap();
        corrupt_text_bytes(&memory_dir.join("memory.sqlite3"), &[0xc3, 0x28]);

        let err = store
            .enumerate_scoped(&scope, enumeration(10, 0))
            .await
            .unwrap_err();
        assert_eq!(err.error_code(), "memory_text_corruption");
    }

    /// Gate: a metadata row whose text row is gone is typed index/store
    /// divergence on enumeration, not a silently skipped record.
    #[tokio::test]
    async fn test_enumerate_scoped_missing_text_row_is_typed_divergence() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        let store = HnswMemoryStore::open(&memory_dir).unwrap();
        let session_id = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());
        store
            .index_scoped(request("row deleted out of band", &session_id))
            .await
            .unwrap();
        {
            let conn = Connection::open(memory_dir.join("memory.sqlite3")).unwrap();
            conn.execute("DELETE FROM memory_text", []).unwrap();
        }

        let err = store
            .enumerate_scoped(&scope, enumeration(10, 0))
            .await
            .unwrap_err();
        assert_eq!(err.error_code(), "memory_index_divergence");
    }

    /// Enumerating an empty or unknown scope yields an empty terminal page.
    #[tokio::test]
    async fn test_enumerate_scoped_empty_scope() {
        let dir = TempDir::new().unwrap();
        let store = HnswMemoryStore::open(dir.path().join("memory")).unwrap();
        let scope = MemorySearchScope::for_session(SessionId::new());

        let page = store
            .enumerate_scoped(&scope, enumeration(10, 0))
            .await
            .unwrap();
        assert!(page.records.is_empty());
        assert_eq!(page.next_offset, None);
    }

    /// A zero-limit request scans nothing but still reports whether raw scope
    /// rows remain past the offset.
    #[tokio::test]
    async fn test_enumerate_scoped_limit_zero_is_typed_error() {
        // A zero limit cannot advance the raw-row cursor (`next_offset` would
        // equal the request offset), so a standard follow-`next_offset`
        // pagination loop would never terminate. Reject it fail-closed.
        let dir = TempDir::new().unwrap();
        let store = HnswMemoryStore::open(dir.path().join("memory")).unwrap();
        let session_id = SessionId::new();
        let scope = MemorySearchScope::for_session(session_id.clone());
        store
            .index_scoped(request("entry".to_string(), &session_id))
            .await
            .unwrap();

        let error = store
            .enumerate_scoped(&scope, enumeration(0, 1))
            .await
            .expect_err("limit zero must be rejected");
        assert!(matches!(error, MemoryStoreError::EnumerationLimitZero));
        assert_eq!(error.error_code(), "memory_enumeration_limit_zero");
    }
}
