//! Meta-side sidecar table for mob- and run-level labels.
//!
//! Member-level labels are owned by `meerkat-mob` (they flow through
//! `SpawnMemberSpec.with_labels()` and out via `MobMemberListEntry.labels`).
//! Mob-level and run-level labels — for associating an external context like
//! `repo`, `branch`, `customer`, `deployment`, or `environment` with a mob or
//! a flow run — have nowhere to live in the upstream model. This module owns
//! that side table.
//!
//! For v1 the table is in-memory only. Persistence behind `MobStorage` is a
//! future enhancement; restarts wipe the labels. The table is keyed by
//! [`MetadataScope`] so the same surface can serve mobs and runs uniformly.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rusqlite::Connection;
use serde_json::Value;
use tokio::sync::RwLock;

/// Scope of a label set.
///
/// Mob scope holds labels keyed by `mob_id`; run scope holds labels keyed by
/// `(mob_id, run_id)`. The mob id is part of the run scope so two mobs with
/// overlapping run identifiers stay isolated.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MetadataScope {
    Mob(String),
    Run(String, String),
}

impl MetadataScope {
    /// Return the mob id this scope belongs to.
    pub fn mob_id(&self) -> &str {
        match self {
            Self::Mob(mob) => mob,
            Self::Run(mob, _) => mob,
        }
    }

    /// Return the run id, if this scope is run-scoped.
    pub fn run_id(&self) -> Option<&str> {
        match self {
            Self::Mob(_) => None,
            Self::Run(_, run) => Some(run),
        }
    }
}

/// In-memory label table keyed by [`MetadataScope`].
///
/// Operations replace label sets wholesale (no merge). Callers wanting
/// merge semantics should read first, mutate the map, then write it back.
#[derive(Debug, Clone, Default)]
pub struct RuntimeMetadataTable {
    inner: Arc<RwLock<BTreeMap<MetadataScope, BTreeMap<String, String>>>>,
}

impl RuntimeMetadataTable {
    /// Create an empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the label set for `scope`. An empty `labels` map clears
    /// the entry.
    pub async fn set_labels(&self, scope: MetadataScope, labels: BTreeMap<String, String>) {
        let mut guard = self.inner.write().await;
        if labels.is_empty() {
            guard.remove(&scope);
        } else {
            guard.insert(scope, labels);
        }
    }

    /// Return the label set for `scope`, or an empty map if none is set.
    pub async fn get_labels(&self, scope: &MetadataScope) -> BTreeMap<String, String> {
        let guard = self.inner.read().await;
        guard.get(scope).cloned().unwrap_or_default()
    }

    /// Remove the label set for `scope`. Returns the previous value if any.
    pub async fn delete_labels(&self, scope: &MetadataScope) -> Option<BTreeMap<String, String>> {
        let mut guard = self.inner.write().await;
        guard.remove(scope)
    }

    /// Return all label sets associated with a mob — both the mob-scoped
    /// entry (if any) and every run-scoped entry whose mob id matches.
    pub async fn list_labels_for_mob(
        &self,
        mob_id: &str,
    ) -> Vec<(MetadataScope, BTreeMap<String, String>)> {
        let guard = self.inner.read().await;
        guard
            .iter()
            .filter(|(scope, _)| scope.mob_id() == mob_id)
            .map(|(scope, labels)| (scope.clone(), labels.clone()))
            .collect()
    }
}

/// Parse a JSON `labels` field as a string→string map.
///
/// Accepts a missing field, `null`, or an empty object — all yield an empty
/// map. Anything else must deserialize cleanly or returns a human-readable
/// error string suitable for a JSON-RPC `Invalid params` reply.
pub fn parse_labels_param(value: Option<&Value>) -> Result<BTreeMap<String, String>, String> {
    match value {
        None | Some(Value::Null) => Ok(BTreeMap::new()),
        Some(v) => serde_json::from_value::<BTreeMap<String, String>>(v.clone())
            .map_err(|err| format!("labels must be a map of string to string: {err}")),
    }
}

/// Render a label map as a JSON object suitable for the wire format.
pub fn labels_to_json_value(labels: &BTreeMap<String, String>) -> Value {
    let mut map = serde_json::Map::with_capacity(labels.len());
    for (k, v) in labels {
        map.insert(k.clone(), Value::String(v.clone()));
    }
    Value::Object(map)
}

/// Outcome of dispatching a label RPC against a [`RuntimeMetadataTable`].
///
/// Both transports (the unified-runtime JSON-RPC and the HTTP-console JSON-RPC)
/// project this into their own response envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelRpcResult {
    /// `set` / `delete`: returns `{"accepted": true}`.
    Accepted,
    /// `get`: returns `{"labels": {...}}`.
    Labels(BTreeMap<String, String>),
    /// Validation error — `Invalid params: <message>`.
    InvalidParams(String),
}

/// Dispatch one mob/run label method through the transport-neutral domain.
///
/// HTTP and stdio deliberately keep their own access checks, notification
/// behavior, error-message decoration, and JSON envelope serialization. This
/// function is the single authority for the six methods' scope selection,
/// parameter validation, and metadata mutation. `None` means `method` does
/// not belong to the labels domain and must be handled by the caller's normal
/// method-not-found path.
pub async fn dispatch_label_method(
    table: &RuntimeMetadataTable,
    mob_id: &str,
    method: &str,
    params: &Value,
) -> Option<LabelRpcResult> {
    let scope = match method {
        "meta/mob_labels/set" | "meta/mob_labels/get" | "meta/mob_labels/delete" => {
            MetadataScope::Mob(mob_id.to_string())
        }
        "meta/run_labels/set" | "meta/run_labels/get" | "meta/run_labels/delete" => {
            match parse_run_id_param(params) {
                Ok(run_id) => MetadataScope::Run(mob_id.to_string(), run_id.to_string()),
                Err(message) => return Some(LabelRpcResult::InvalidParams(message)),
            }
        }
        _ => return None,
    };

    let outcome = match method {
        "meta/mob_labels/set" | "meta/run_labels/set" => {
            dispatch_labels_set(table, scope, params).await
        }
        "meta/mob_labels/get" | "meta/run_labels/get" => dispatch_labels_get(table, scope).await,
        "meta/mob_labels/delete" | "meta/run_labels/delete" => {
            dispatch_labels_delete(table, scope).await
        }
        _ => return None,
    };
    Some(outcome)
}

/// Replace the label set for `scope`, parsing `labels` from RPC params.
pub async fn dispatch_labels_set(
    table: &RuntimeMetadataTable,
    scope: MetadataScope,
    params: &Value,
) -> LabelRpcResult {
    match parse_labels_param(params.get("labels")) {
        Ok(labels) => {
            table.set_labels(scope, labels).await;
            LabelRpcResult::Accepted
        }
        Err(message) => LabelRpcResult::InvalidParams(message),
    }
}

/// Read the label set for `scope`.
pub async fn dispatch_labels_get(
    table: &RuntimeMetadataTable,
    scope: MetadataScope,
) -> LabelRpcResult {
    LabelRpcResult::Labels(table.get_labels(&scope).await)
}

/// Remove the label set for `scope`.
pub async fn dispatch_labels_delete(
    table: &RuntimeMetadataTable,
    scope: MetadataScope,
) -> LabelRpcResult {
    let _ = table.delete_labels(&scope).await;
    LabelRpcResult::Accepted
}

/// Pull a non-empty `run_id` string from RPC params.
pub fn parse_run_id_param(params: &Value) -> Result<&str, String> {
    match params.get("run_id").and_then(Value::as_str) {
        Some(s) if !s.is_empty() => Ok(s),
        _ => Err("run_id required".to_string()),
    }
}

// ---------------------------------------------------------------------------
// Persistent metadata adapter
// ---------------------------------------------------------------------------
//
// Distinct from the in-memory `RuntimeMetadataTable` above. The label sidecar
// resets on restart (acceptable — labels are app-injected runtime metadata).
// The structural-events subscription cursor must survive restart so a
// restarted gateway resumes from where it left off rather than dropping
// events emitted between processes. This adapter owns that durable state.

/// Errors raised by [`PersistentMetadataStore`] implementations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataStoreError {
    /// Underlying I/O or storage failure (sqlite open, schema, query, ...).
    Io(String),
    /// A persisted value couldn't be parsed back into the typed shape — the
    /// store was probably written by a future meta version.
    Decode(String),
}

impl std::fmt::Display for MetadataStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "metadata store io: {msg}"),
            Self::Decode(msg) => write!(f, "metadata store decode: {msg}"),
        }
    }
}

impl std::error::Error for MetadataStoreError {}

/// One spawned member's durable idle-retirement opt-in.
///
/// A member seated in the primary mob is idle-retired only when its spawning
/// call opted it in (a `fork_off` child always is). The opt-in has to outlive
/// the process: after a restart the restored child is swept again only if its
/// opt-in is restored with it.
///
/// The opt-in belongs to one member instance, identified by the bridge
/// session it ran when the opt-in was recorded. A member id can be reused
/// after its member is retired; the sweep honours the record only while the
/// member seated under the id still runs `session_id`, and drops it
/// otherwise.
///
/// `recorded_at` orders the opt-in against mob events (a reset or destroy
/// releases only opt-ins recorded before it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberIdleRetireOverrideRecord {
    pub mob_id: String,
    pub member_id: String,
    pub session_id: meerkat_core::types::SessionId,
    pub policy: crate::mob_handle_runtime::DelegateIdleRetireOverride,
    pub recorded_at: chrono::DateTime<chrono::Utc>,
}

/// Stored value of a member idle-retirement row (the key carries the member
/// id, the row's `mob_id` column the mob). Rows written before `recorded_at`
/// existed decode as recorded at the epoch.
#[derive(serde::Serialize, serde::Deserialize)]
struct StoredMemberIdleRetireOverride {
    session_id: meerkat_core::types::SessionId,
    policy: crate::mob_handle_runtime::DelegateIdleRetireOverride,
    #[serde(default)]
    recorded_at: chrono::DateTime<chrono::Utc>,
}

/// The one field a delete matches on, decoded from any version's row.
#[derive(serde::Deserialize)]
struct StoredMemberIdleRetireSession {
    session_id: meerkat_core::types::SessionId,
}

impl StoredMemberIdleRetireOverride {
    fn encode(record: &MemberIdleRetireOverrideRecord) -> Result<String, MetadataStoreError> {
        serde_json::to_string(&Self {
            session_id: record.session_id.clone(),
            policy: record.policy,
            recorded_at: record.recorded_at,
        })
        .map_err(|err| MetadataStoreError::Io(format!("encode idle-retire opt-in: {err}")))
    }
}

/// Persistent storage for meta runtime metadata that must survive a
/// gateway restart: the structural-events subscription cursor and spawned
/// members' idle-retirement opt-ins.
///
/// Two impls live in this module: [`InMemoryMetadataStore`] (no
/// persistence; used when no SQLite mob storage is configured) and
/// [`SqliteMetadataStore`] (writes a small `meta_metadata` table next
/// to the mob's own SQLite store). The `UnifiedRuntime` builder picks
/// the impl based on the configured `MobBootstrapSpec`.
#[async_trait]
pub trait PersistentMetadataStore: Send + Sync {
    /// Read the last-projected mob events cursor for `mob_id`. Returns
    /// `Ok(None)` when no cursor has been written yet (fresh deploy or
    /// in-memory deployment that just started).
    async fn get_subscription_cursor(
        &self,
        mob_id: &str,
    ) -> Result<Option<u64>, MetadataStoreError>;

    /// Persist the last-projected mob events cursor for `mob_id`.
    async fn set_subscription_cursor(
        &self,
        mob_id: &str,
        cursor: u64,
    ) -> Result<(), MetadataStoreError>;

    /// Every recorded member idle-retirement opt-in, across all mobs.
    ///
    /// The default records nothing: a store that predates this seam keeps
    /// opt-ins for the life of the process only, as before.
    async fn load_member_idle_retire_overrides(
        &self,
    ) -> Result<Vec<MemberIdleRetireOverrideRecord>, MetadataStoreError> {
        Ok(Vec::new())
    }

    /// Record (or replace) one member's idle-retirement opt-in.
    async fn set_member_idle_retire_override(
        &self,
        _record: &MemberIdleRetireOverrideRecord,
    ) -> Result<(), MetadataStoreError> {
        Ok(())
    }

    /// Forget this member instance's opt-in: the row is removed only while it
    /// is still bound to `record.session_id`, so an opt-in recorded for a
    /// newer member under the same id survives. Clearing an absent or
    /// different row is not an error.
    async fn clear_member_idle_retire_override(
        &self,
        _record: &MemberIdleRetireOverrideRecord,
    ) -> Result<(), MetadataStoreError> {
        Ok(())
    }
}

/// In-memory persistent metadata store.
///
/// "Persistent" is aspirational here — the values survive `Arc<...>` clones
/// but reset to empty on process restart. Used when no SQLite mob storage
/// is configured. The structural-events subscription falls back to "start
/// at latest" on restart in this case, which is the right behaviour:
/// in-memory deployments don't have a persistent ledger to replay against
/// either.
#[derive(Debug, Default)]
pub struct InMemoryMetadataStore {
    cursors: RwLock<BTreeMap<String, u64>>,
    idle_retire_overrides: RwLock<BTreeMap<(String, String), MemberIdleRetireOverrideRecord>>,
}

impl InMemoryMetadataStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl PersistentMetadataStore for InMemoryMetadataStore {
    async fn get_subscription_cursor(
        &self,
        mob_id: &str,
    ) -> Result<Option<u64>, MetadataStoreError> {
        Ok(self.cursors.read().await.get(mob_id).copied())
    }

    async fn set_subscription_cursor(
        &self,
        mob_id: &str,
        cursor: u64,
    ) -> Result<(), MetadataStoreError> {
        self.cursors
            .write()
            .await
            .insert(mob_id.to_string(), cursor);
        Ok(())
    }

    async fn load_member_idle_retire_overrides(
        &self,
    ) -> Result<Vec<MemberIdleRetireOverrideRecord>, MetadataStoreError> {
        Ok(self
            .idle_retire_overrides
            .read()
            .await
            .values()
            .cloned()
            .collect())
    }

    async fn set_member_idle_retire_override(
        &self,
        record: &MemberIdleRetireOverrideRecord,
    ) -> Result<(), MetadataStoreError> {
        self.idle_retire_overrides.write().await.insert(
            (record.mob_id.clone(), record.member_id.clone()),
            record.clone(),
        );
        Ok(())
    }

    async fn clear_member_idle_retire_override(
        &self,
        record: &MemberIdleRetireOverrideRecord,
    ) -> Result<(), MetadataStoreError> {
        let mut overrides = self.idle_retire_overrides.write().await;
        let key = (record.mob_id.clone(), record.member_id.clone());
        if overrides
            .get(&key)
            .is_some_and(|stored| stored.session_id == record.session_id)
        {
            overrides.remove(&key);
        }
        Ok(())
    }
}

/// SQLite-backed persistent metadata store.
///
/// Opens its own `rusqlite::Connection` to the supplied database path —
/// the same path the mob's `MobStorage` uses, but with a separate handle.
/// Cross-handle access is safe; meerkat #445's `notify`-based event-store
/// watcher already runs in this configuration. The `meta_metadata`
/// table is independent of meerkat-mob's own schema, so opening order
/// doesn't matter; in the shared file's migration ledger the table lives
/// under meta's own `meta-metadata` domain, co-tenanting meerkat-mob's
/// `mob` domain (the ledger keys strictly by domain name, so the two
/// crates stamp and migrate independently).
///
/// Schema:
/// ```text
/// CREATE TABLE meta_metadata (
///     mob_id  TEXT NOT NULL,
///     key     TEXT NOT NULL,
///     value   TEXT NOT NULL,
///     PRIMARY KEY (mob_id, key)
/// )
/// ```
///
/// The subscription cursor lives at `key = "subscription_cursor"`,
/// stored as a base-10 string for simple human inspection. Future
/// metadata fields land here under their own keys.
pub struct SqliteMetadataStore {
    conn: Mutex<Connection>,
    /// Database file path; `:memory:` for in-memory stores (where the
    /// per-operation fence guard degrades to a no-op).
    db_path: PathBuf,
}

const SUBSCRIPTION_CURSOR_KEY: &str = "subscription_cursor";

/// Key prefix of a member idle-retirement opt-in row: the member id follows
/// the prefix and the value is the JSON-serialized bound session and policy
/// (`StoredMemberIdleRetireOverride`).
const MEMBER_IDLE_RETIRE_KEY_PREFIX: &str = "member_idle_retire/";

fn member_idle_retire_key(member_id: &str) -> String {
    format!("{MEMBER_IDLE_RETIRE_KEY_PREFIX}{member_id}")
}

/// The runtime-metadata store's schema domain in the per-file migration
/// ledger. Migration 0001 is the historical one-table DDL.
const META_METADATA_DOMAIN: meerkat_sqlite::SchemaDomain = meerkat_sqlite::SchemaDomain {
    name: "meta-metadata",
    migrations: &[meerkat_sqlite::Migration {
        version: 1,
        name: "base-schema",
        apply: migration_0001_metadata_schema,
    }],
    initialize_current: migration_0001_metadata_schema,
    allowed_existing_versions: &[1],
    // Unledgered meta files are refused at open (below the 0.8.8 ledger
    // floor) and meta never runs the offline bridge, so no source
    // version is inferable.
    bridge_recoverable_versions: &[],
    released_predecessors: &[],
    owned_objects: &[meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: METADATA_TABLE,
    }],
    // The predecessor (pre-0.9) table name stays reserved: a file that
    // still carries it has not been adopted, and opening it must refuse
    // rather than start an empty table beside it.
    retired_objects: &[meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: crate::storage_predecessor::PREDECESSOR_METADATA_TABLE,
    }],
};

/// The runtime-metadata table name.
pub(crate) const METADATA_TABLE: &str = "meta_metadata";

/// Create the runtime-metadata table with the current DDL (shared with the
/// predecessor adoption so an adopted catalog matches a fresh one).
pub(crate) fn create_metadata_table(tx: &rusqlite::Transaction<'_>) -> Result<(), rusqlite::Error> {
    migration_0001_metadata_schema(tx)
}

fn migration_0001_metadata_schema(tx: &rusqlite::Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta_metadata (
            mob_id TEXT NOT NULL,
            key    TEXT NOT NULL,
            value  TEXT NOT NULL,
            PRIMARY KEY (mob_id, key)
        );",
    )
}

impl SqliteMetadataStore {
    /// Open (or create) a SQLite metadata store at `path`.
    ///
    /// `path` should typically be the same database the mob's `MobStorage`
    /// uses; the table is `meta_metadata` and won't collide with
    /// meerkat-mob's own tables.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MetadataStoreError> {
        let path = path.as_ref().to_path_buf();
        let mut conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::PRIMARY)
            .map_err(|err| MetadataStoreError::Io(format!("open: {err}")))?;
        meerkat_sqlite::apply_domain_migrations(&mut conn, &META_METADATA_DOMAIN)
            .map_err(|err| MetadataStoreError::Io(format!("schema: {err}")))?;
        Ok(Self {
            conn: Mutex::new(conn),
            db_path: path,
        })
    }

    /// Open an in-memory SQLite store (for tests).
    pub fn in_memory() -> Result<Self, MetadataStoreError> {
        let mut conn = Connection::open_in_memory()
            .map_err(|err| MetadataStoreError::Io(format!("in-memory open: {err}")))?;
        meerkat_sqlite::apply_domain_migrations(&mut conn, &META_METADATA_DOMAIN)
            .map_err(|err| MetadataStoreError::Io(format!("schema: {err}")))?;
        Ok(Self {
            conn: Mutex::new(conn),
            db_path: PathBuf::from(":memory:"),
        })
    }

    /// Per-operation maintenance-fence guard: the connection is held for
    /// the store's lifetime, so the fence cannot ride the open — every
    /// operation takes its own shared guard.
    fn operation_fence(&self) -> Result<meerkat_sqlite::OperationGuard, MetadataStoreError> {
        meerkat_sqlite::OperationGuard::for_database(&self.db_path)
            .map_err(|err| MetadataStoreError::Io(format!("operation fence: {err}")))
    }

    fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, MetadataStoreError> {
        self.conn
            .lock()
            .map_err(|err| MetadataStoreError::Io(format!("connection mutex poisoned: {err}")))
    }
}

#[async_trait]
impl PersistentMetadataStore for SqliteMetadataStore {
    async fn get_subscription_cursor(
        &self,
        mob_id: &str,
    ) -> Result<Option<u64>, MetadataStoreError> {
        let _fence = self.operation_fence()?;
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare_cached(
                "SELECT value FROM meta_metadata WHERE mob_id = ?1 AND key = ?2 LIMIT 1",
            )
            .map_err(|err| MetadataStoreError::Io(format!("prepare: {err}")))?;
        let value: Option<String> = stmt
            .query_row(rusqlite::params![mob_id, SUBSCRIPTION_CURSOR_KEY], |row| {
                row.get::<_, String>(0)
            })
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(MetadataStoreError::Io(format!("query: {other}"))),
            })?;
        match value {
            Some(s) => s
                .parse::<u64>()
                .map(Some)
                .map_err(|err| MetadataStoreError::Decode(format!("cursor parse: {err}"))),
            None => Ok(None),
        }
    }

    async fn set_subscription_cursor(
        &self,
        mob_id: &str,
        cursor: u64,
    ) -> Result<(), MetadataStoreError> {
        let _fence = self.operation_fence()?;
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO meta_metadata (mob_id, key, value) VALUES (?1, ?2, ?3) \
             ON CONFLICT(mob_id, key) DO UPDATE SET value = excluded.value",
            rusqlite::params![mob_id, SUBSCRIPTION_CURSOR_KEY, cursor.to_string()],
        )
        .map_err(|err| MetadataStoreError::Io(format!("upsert: {err}")))?;
        Ok(())
    }

    async fn load_member_idle_retire_overrides(
        &self,
    ) -> Result<Vec<MemberIdleRetireOverrideRecord>, MetadataStoreError> {
        let _fence = self.operation_fence()?;
        let conn = self.lock_conn()?;
        // A prefix compare, not LIKE: member ids may carry `_` and `%`.
        let mut stmt = conn
            .prepare_cached(
                "SELECT mob_id, key, value FROM meta_metadata \
                 WHERE substr(key, 1, ?1) = ?2 ORDER BY mob_id, key",
            )
            .map_err(|err| MetadataStoreError::Io(format!("prepare: {err}")))?;
        let rows = stmt
            .query_map(
                rusqlite::params![
                    MEMBER_IDLE_RETIRE_KEY_PREFIX.len(),
                    MEMBER_IDLE_RETIRE_KEY_PREFIX
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(|err| MetadataStoreError::Io(format!("query: {err}")))?;
        let mut records = Vec::new();
        for row in rows {
            let (mob_id, key, value) =
                row.map_err(|err| MetadataStoreError::Io(format!("row: {err}")))?;
            let Some(member_id) = key.strip_prefix(MEMBER_IDLE_RETIRE_KEY_PREFIX) else {
                continue;
            };
            // One undecodable row (for instance a policy variant written by a
            // newer Meta before a rollback) costs only that member's
            // opt-in, never the whole restore.
            match serde_json::from_str::<StoredMemberIdleRetireOverride>(&value) {
                Ok(stored) => records.push(MemberIdleRetireOverrideRecord {
                    mob_id,
                    member_id: member_id.to_string(),
                    session_id: stored.session_id,
                    policy: stored.policy,
                    recorded_at: stored.recorded_at,
                }),
                Err(error) => tracing::warn!(
                    mob_id,
                    member_id,
                    error = %error,
                    "skipping an idle-retire opt-in row that cannot be decoded"
                ),
            }
        }
        Ok(records)
    }

    async fn set_member_idle_retire_override(
        &self,
        record: &MemberIdleRetireOverrideRecord,
    ) -> Result<(), MetadataStoreError> {
        let value = StoredMemberIdleRetireOverride::encode(record)?;
        let _fence = self.operation_fence()?;
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO meta_metadata (mob_id, key, value) VALUES (?1, ?2, ?3) \
             ON CONFLICT(mob_id, key) DO UPDATE SET value = excluded.value",
            rusqlite::params![
                record.mob_id,
                member_idle_retire_key(&record.member_id),
                value
            ],
        )
        .map_err(|err| MetadataStoreError::Io(format!("upsert: {err}")))?;
        Ok(())
    }

    async fn clear_member_idle_retire_override(
        &self,
        record: &MemberIdleRetireOverrideRecord,
    ) -> Result<(), MetadataStoreError> {
        // Match on the key and the DECODED session id, not the stored bytes:
        // rows written by other versions (another `recorded_at` shape) must
        // stay deletable. The DELETE then pins the exact value read, so a row
        // rewritten for a newer member in between is kept.
        let _fence = self.operation_fence()?;
        let conn = self.lock_conn()?;
        let key = member_idle_retire_key(&record.member_id);
        let stored: Option<String> = conn
            .query_row(
                "SELECT value FROM meta_metadata WHERE mob_id = ?1 AND key = ?2",
                rusqlite::params![record.mob_id, key],
                |row| row.get::<_, String>(0),
            )
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(MetadataStoreError::Io(format!("query: {other}"))),
            })?;
        let Some(stored) = stored else {
            return Ok(());
        };
        let bound_to_record = serde_json::from_str::<StoredMemberIdleRetireSession>(&stored)
            .is_ok_and(|row| row.session_id == record.session_id);
        if !bound_to_record {
            return Ok(());
        }
        conn.execute(
            "DELETE FROM meta_metadata WHERE mob_id = ?1 AND key = ?2 AND value = ?3",
            rusqlite::params![record.mob_id, key, stored],
        )
        .map_err(|err| MetadataStoreError::Io(format!("delete: {err}")))?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[tokio::test]
    async fn set_and_get_mob_labels() {
        let table = RuntimeMetadataTable::new();
        let scope = MetadataScope::Mob("mob-a".to_string());
        table
            .set_labels(scope.clone(), labels(&[("repo", "agents"), ("env", "dev")]))
            .await;
        let got = table.get_labels(&scope).await;
        assert_eq!(got.get("repo").map(String::as_str), Some("agents"));
        assert_eq!(got.get("env").map(String::as_str), Some("dev"));
    }

    #[tokio::test]
    async fn set_replaces_rather_than_merges() {
        let table = RuntimeMetadataTable::new();
        let scope = MetadataScope::Mob("mob-a".to_string());
        table
            .set_labels(scope.clone(), labels(&[("a", "1"), ("b", "2")]))
            .await;
        table.set_labels(scope.clone(), labels(&[("a", "9")])).await;
        let got = table.get_labels(&scope).await;
        assert_eq!(got.len(), 1);
        assert_eq!(got.get("a").map(String::as_str), Some("9"));
        assert!(!got.contains_key("b"));
    }

    #[tokio::test]
    async fn delete_clears_entry() {
        let table = RuntimeMetadataTable::new();
        let scope = MetadataScope::Run("mob-a".to_string(), "run-1".to_string());
        table.set_labels(scope.clone(), labels(&[("k", "v")])).await;
        let prev = table.delete_labels(&scope).await;
        assert_eq!(prev.unwrap().get("k").map(String::as_str), Some("v"));
        let after = table.get_labels(&scope).await;
        assert!(after.is_empty());
    }

    #[tokio::test]
    async fn empty_set_clears_entry() {
        let table = RuntimeMetadataTable::new();
        let scope = MetadataScope::Mob("mob-a".to_string());
        table.set_labels(scope.clone(), labels(&[("k", "v")])).await;
        table.set_labels(scope.clone(), BTreeMap::new()).await;
        assert!(table.get_labels(&scope).await.is_empty());
    }

    #[tokio::test]
    async fn list_returns_mob_and_run_entries() {
        let table = RuntimeMetadataTable::new();
        let mob_scope = MetadataScope::Mob("mob-a".to_string());
        let run_scope = MetadataScope::Run("mob-a".to_string(), "run-1".to_string());
        let other_run = MetadataScope::Run("mob-b".to_string(), "run-1".to_string());
        table
            .set_labels(mob_scope.clone(), labels(&[("env", "dev")]))
            .await;
        table
            .set_labels(run_scope.clone(), labels(&[("trace", "abc")]))
            .await;
        table
            .set_labels(other_run, labels(&[("trace", "xyz")]))
            .await;

        let entries = table.list_labels_for_mob("mob-a").await;
        assert_eq!(entries.len(), 2);
        let scopes: Vec<&MetadataScope> = entries.iter().map(|(s, _)| s).collect();
        assert!(scopes.contains(&&mob_scope));
        assert!(scopes.contains(&&run_scope));
    }

    // ----- PersistentMetadataStore tests --------------------------------

    #[tokio::test]
    async fn in_memory_persistent_store_round_trip() {
        let store = InMemoryMetadataStore::new();
        assert_eq!(
            store.get_subscription_cursor("mob-a").await.unwrap(),
            None,
            "fresh store should have no cursor",
        );
        store.set_subscription_cursor("mob-a", 42).await.unwrap();
        assert_eq!(
            store.get_subscription_cursor("mob-a").await.unwrap(),
            Some(42),
        );
        // Per-mob isolation.
        assert_eq!(store.get_subscription_cursor("mob-b").await.unwrap(), None,);
    }

    #[tokio::test]
    async fn in_memory_persistent_store_overwrite() {
        let store = InMemoryMetadataStore::new();
        store.set_subscription_cursor("m", 1).await.unwrap();
        store.set_subscription_cursor("m", 2).await.unwrap();
        assert_eq!(store.get_subscription_cursor("m").await.unwrap(), Some(2),);
    }

    #[tokio::test]
    async fn sqlite_persistent_store_round_trip() {
        let store = SqliteMetadataStore::in_memory().unwrap();
        assert_eq!(store.get_subscription_cursor("mob-a").await.unwrap(), None,);
        store.set_subscription_cursor("mob-a", 1234).await.unwrap();
        assert_eq!(
            store.get_subscription_cursor("mob-a").await.unwrap(),
            Some(1234),
        );
        // Overwrite via UPSERT.
        store.set_subscription_cursor("mob-a", 9999).await.unwrap();
        assert_eq!(
            store.get_subscription_cursor("mob-a").await.unwrap(),
            Some(9999),
        );
        // Per-mob isolation.
        store.set_subscription_cursor("mob-b", 5).await.unwrap();
        assert_eq!(
            store.get_subscription_cursor("mob-a").await.unwrap(),
            Some(9999),
        );
        assert_eq!(
            store.get_subscription_cursor("mob-b").await.unwrap(),
            Some(5),
        );
    }

    #[tokio::test]
    async fn sqlite_store_persists_across_handles() {
        // The whole point of SQLite-backed persistence: a fresh handle to
        // the same DB sees writes from the previous handle. We can't drop
        // and reopen an in-memory DB (it disappears with the connection),
        // so write to a tempfile, drop, reopen.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta-metadata.sqlite");
        {
            let store = SqliteMetadataStore::open(&path).unwrap();
            store.set_subscription_cursor("mob-x", 7777).await.unwrap();
        }
        // Reopen.
        let store = SqliteMetadataStore::open(&path).unwrap();
        assert_eq!(
            store.get_subscription_cursor("mob-x").await.unwrap(),
            Some(7777),
            "cursor should survive handle drop",
        );
    }

    #[tokio::test]
    async fn fresh_store_stamps_meta_metadata_domain() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta-metadata.sqlite");
        let store = SqliteMetadataStore::open(&path).unwrap();
        store.set_subscription_cursor("mob-a", 1).await.unwrap();
        let probe = Connection::open(&path).unwrap();
        assert_eq!(
            meerkat_sqlite::domain_version(&probe, "meta-metadata").unwrap(),
            Some(1)
        );
    }

    /// A pre-ledger file (bare meta_metadata table, no meerkat_schema row)
    /// is refused typed at open with its rows left untouched and no ledger
    /// stamped: pre-ledger corpora are below the mobkit 0.8.8 floor, and the
    /// 0.8.11 reset retired silent pre-floor convergence (this test pinned
    /// that convergence until then).
    #[tokio::test]
    async fn legacy_metadata_file_is_refused_with_rows_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta-metadata.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE meta_metadata (
                    mob_id TEXT NOT NULL,
                    key    TEXT NOT NULL,
                    value  TEXT NOT NULL,
                    PRIMARY KEY (mob_id, key)
                );
                INSERT INTO meta_metadata (mob_id, key, value)
                    VALUES ('mob-legacy', 'subscription_cursor', '314');",
            )
            .unwrap();
        }
        assert!(
            SqliteMetadataStore::open(&path).is_err(),
            "opening a pre-ledger metadata database must refuse typed: unledgered owned \
             tables are below the mobkit 0.8.8 floor and must never be silently converged"
        );
        let probe = Connection::open(&path).unwrap();
        let preserved: String = probe
            .query_row(
                "SELECT value FROM meta_metadata \
                 WHERE mob_id = 'mob-legacy' AND key = 'subscription_cursor'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            preserved, "314",
            "the refusal must leave legacy rows untouched"
        );
        assert_eq!(
            meerkat_sqlite::domain_version(&probe, "meta-metadata").unwrap(),
            None,
            "a refused open must not stamp the ledger"
        );
    }

    /// The metadata table co-tenants the same database file as meerkat-mob's
    /// MobStorage. The per-file ledger keys strictly by domain, so the two
    /// crates' domains (`mob` and `meta-metadata`) must coexist in one
    /// `meerkat_schema` table without clobbering each other.
    #[tokio::test]
    async fn metadata_and_mob_domains_cotenant_one_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mob.sqlite3");
        let _mob = meerkat_mob::MobStorage::persistent(&path).expect("mob storage");
        let store = SqliteMetadataStore::open(&path).expect("metadata store");
        store.set_subscription_cursor("mob-a", 42).await.unwrap();
        assert_eq!(
            store.get_subscription_cursor("mob-a").await.unwrap(),
            Some(42)
        );
        let probe = Connection::open(&path).unwrap();
        let mob_version = meerkat_sqlite::domain_version(&probe, "mob").unwrap();
        assert!(
            mob_version.is_some_and(|version| version >= 1),
            "meerkat-mob's own domain row must be present: {mob_version:?}"
        );
        assert_eq!(
            meerkat_sqlite::domain_version(&probe, "meta-metadata").unwrap(),
            Some(1),
            "meta's domain row must coexist with meerkat-mob's"
        );
    }

    #[tokio::test]
    async fn run_scope_distinguishes_mobs() {
        let table = RuntimeMetadataTable::new();
        let scope_a = MetadataScope::Run("mob-a".to_string(), "run-1".to_string());
        let scope_b = MetadataScope::Run("mob-b".to_string(), "run-1".to_string());
        table
            .set_labels(scope_a.clone(), labels(&[("k", "a")]))
            .await;
        table
            .set_labels(scope_b.clone(), labels(&[("k", "b")]))
            .await;
        assert_eq!(
            table
                .get_labels(&scope_a)
                .await
                .get("k")
                .map(String::as_str),
            Some("a")
        );
        assert_eq!(
            table
                .get_labels(&scope_b)
                .await
                .get("k")
                .map(String::as_str),
            Some("b")
        );
    }

    fn idle_record(
        mob_id: &str,
        member_id: &str,
        session_id: &meerkat_core::types::SessionId,
        policy: crate::mob_handle_runtime::DelegateIdleRetireOverride,
    ) -> MemberIdleRetireOverrideRecord {
        MemberIdleRetireOverrideRecord {
            mob_id: mob_id.to_string(),
            member_id: member_id.to_string(),
            session_id: session_id.clone(),
            policy,
            recorded_at: chrono::DateTime::from_timestamp(1_790_000_000, 0)
                .expect("fixed timestamp"),
        }
    }

    /// Opt-ins written by one process are read back by the next one against
    /// the same file, bound sessions included, member ids with SQL wildcard
    /// characters included, and other metadata rows (the subscription
    /// cursor) never leak in.
    #[tokio::test]
    async fn sqlite_member_idle_retire_overrides_survive_reopen() {
        use crate::mob_handle_runtime::DelegateIdleRetireOverride;
        use meerkat_core::types::SessionId;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("metadata.sqlite3");
        let (s1, s2, s3) = (SessionId::new(), SessionId::new(), SessionId::new());
        let fork_child = idle_record(
            "mob-a",
            "fork_child%1",
            &s1,
            DelegateIdleRetireOverride::Seconds(300),
        );
        let helper = idle_record("mob-b", "helper", &s2, DelegateIdleRetireOverride::Disabled);
        let newer_fork_2 = idle_record(
            "mob-a",
            "fork-2",
            &s3,
            DelegateIdleRetireOverride::RuntimeDefault,
        );
        {
            let store = SqliteMetadataStore::open(&path).expect("open metadata store");
            store
                .set_subscription_cursor("mob-a", 7)
                .await
                .expect("cursor");
            store
                .set_member_idle_retire_override(&fork_child)
                .await
                .expect("set fork child");
            store
                .set_member_idle_retire_override(&helper)
                .await
                .expect("set helper");
            let older_fork_2 = idle_record(
                "mob-a",
                "fork-2",
                &s1,
                DelegateIdleRetireOverride::Seconds(5),
            );
            store
                .set_member_idle_retire_override(&older_fork_2)
                .await
                .expect("set fork-2");
            // A later member under the same id replaces the row...
            store
                .set_member_idle_retire_override(&newer_fork_2)
                .await
                .expect("replace fork-2");
            // ...and clearing the OLDER opt-in must not remove the newer one.
            store
                .clear_member_idle_retire_override(&older_fork_2)
                .await
                .expect("clearing a superseded opt-in is not an error");
            let gone = idle_record("mob-b", "gone", &s2, DelegateIdleRetireOverride::Seconds(1));
            store
                .set_member_idle_retire_override(&gone)
                .await
                .expect("set gone");
            store
                .clear_member_idle_retire_override(&gone)
                .await
                .expect("clear gone");
            store
                .clear_member_idle_retire_override(&gone)
                .await
                .expect("clearing an absent opt-in is not an error");
        }

        let reopened = SqliteMetadataStore::open(&path).expect("reopen metadata store");
        assert_eq!(
            reopened
                .load_member_idle_retire_overrides()
                .await
                .expect("load"),
            vec![newer_fork_2, fork_child, helper]
        );
        assert_eq!(
            reopened
                .get_subscription_cursor("mob-a")
                .await
                .expect("cursor"),
            Some(7)
        );
    }

    /// One undecodable row (a policy variant from a newer Meta, say) is
    /// skipped; the rest of the restore still loads.
    #[tokio::test]
    async fn sqlite_member_idle_retire_load_skips_undecodable_rows() {
        use crate::mob_handle_runtime::DelegateIdleRetireOverride;
        use meerkat_core::types::SessionId;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("metadata.sqlite3");
        let store = SqliteMetadataStore::open(&path).expect("open metadata store");
        let good = idle_record(
            "mob-a",
            "fork-1",
            &SessionId::new(),
            DelegateIdleRetireOverride::Seconds(300),
        );
        store
            .set_member_idle_retire_override(&good)
            .await
            .expect("set good row");
        {
            let conn = store.lock_conn().expect("connection");
            conn.execute(
                "INSERT INTO meta_metadata (mob_id, key, value) VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    "mob-a",
                    member_idle_retire_key("from-the-future"),
                    r#"{"session_id":"not-a-session","policy":"hibernate"}"#
                ],
            )
            .expect("insert undecodable row");
        }
        assert_eq!(
            store
                .load_member_idle_retire_overrides()
                .await
                .expect("load"),
            vec![good]
        );
    }

    #[tokio::test]
    async fn in_memory_member_idle_retire_overrides_set_replace_and_clear() {
        use crate::mob_handle_runtime::DelegateIdleRetireOverride;
        use meerkat_core::types::SessionId;

        let store = InMemoryMetadataStore::new();
        let (s1, s2) = (SessionId::new(), SessionId::new());
        let kept = idle_record(
            "mob-a",
            "fork-1",
            &s1,
            DelegateIdleRetireOverride::Seconds(300),
        );
        store
            .set_member_idle_retire_override(&kept)
            .await
            .expect("set");
        let older = idle_record(
            "mob-a",
            "fork-2",
            &s1,
            DelegateIdleRetireOverride::Seconds(5),
        );
        let newer = idle_record(
            "mob-a",
            "fork-2",
            &s2,
            DelegateIdleRetireOverride::RuntimeDefault,
        );
        store
            .set_member_idle_retire_override(&older)
            .await
            .expect("set");
        store
            .set_member_idle_retire_override(&newer)
            .await
            .expect("replace");
        store
            .clear_member_idle_retire_override(&older)
            .await
            .expect("stale clear");
        let mut loaded = store
            .load_member_idle_retire_overrides()
            .await
            .expect("load");
        loaded.sort_by(|a, b| a.member_id.cmp(&b.member_id));
        assert_eq!(loaded, vec![kept.clone(), newer.clone()]);
        store
            .clear_member_idle_retire_override(&newer)
            .await
            .expect("clear");
        assert_eq!(
            store
                .load_member_idle_retire_overrides()
                .await
                .expect("load"),
            vec![kept]
        );
    }

    /// Review of #447: a release deletes a row by its key and the session it
    /// is bound to, whatever version wrote it. A row written by #442 (no
    /// `recorded_at`) or by a build that stored extra fields is deletable; a
    /// row bound to another session (a newer member) is kept.
    #[tokio::test]
    async fn sqlite_member_idle_retire_rows_from_other_versions_are_deletable() {
        use crate::mob_handle_runtime::DelegateIdleRetireOverride;
        use meerkat_core::types::SessionId;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("metadata.sqlite3");
        let store = SqliteMetadataStore::open(&path).expect("open metadata store");
        let (old_format, new_format, kept) = (SessionId::new(), SessionId::new(), SessionId::new());
        {
            let conn = store.lock_conn().expect("connection");
            for (member, value) in [
                (
                    "written-by-442",
                    format!(r#"{{"session_id":"{old_format}","policy":{{"seconds":300}}}}"#),
                ),
                (
                    "written-with-extras",
                    format!(
                        r#"{{"session_id":"{new_format}","policy":"runtime_default","recorded_at":"2026-09-25T20:00:00Z","carrying":true}}"#
                    ),
                ),
                (
                    "newer-member",
                    format!(r#"{{"session_id":"{kept}","policy":"disabled"}}"#),
                ),
            ] {
                conn.execute(
                    "INSERT INTO meta_metadata (mob_id, key, value) VALUES (?1, ?2, ?3)",
                    rusqlite::params!["mob-a", member_idle_retire_key(member), value],
                )
                .expect("insert row");
            }
        }
        let loaded = store
            .load_member_idle_retire_overrides()
            .await
            .expect("load");
        assert_eq!(loaded.len(), 3, "every version's row decodes: {loaded:?}");
        for record in loaded
            .iter()
            .filter(|record| record.member_id != "newer-member")
        {
            store
                .clear_member_idle_retire_override(record)
                .await
                .expect("clear");
        }
        // A release for a gone instance of `newer-member` keeps the row.
        store
            .clear_member_idle_retire_override(&idle_record(
                "mob-a",
                "newer-member",
                &SessionId::new(),
                DelegateIdleRetireOverride::Disabled,
            ))
            .await
            .expect("stale clear");
        let remaining = store
            .load_member_idle_retire_overrides()
            .await
            .expect("load");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].member_id, "newer-member");
        assert_eq!(remaining[0].session_id, kept);
    }
}
