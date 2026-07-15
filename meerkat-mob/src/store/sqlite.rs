//! SQLite-backed store implementations.
//!
//! SQLite uses WAL mode with no exclusive file lock,
//! allowing the same database to be reopened after drop within the same process.

use super::realm_profile::{RealmProfileStore, StoredRealmProfile};
use super::{
    BeginPlacedSpawnResult, CommitPlacedSpawnResult, DeletePlacedSpawnResult,
    ExternalBindingOverlayRecord, MobEventStore, MobHostAuthorityDeletionAuthority,
    MobHostAuthorityPersistenceAuthority, MobHostAuthorityRecord, MobMemberEventCursorRecord,
    MobMemberLiveCleanupRecord, MobMemberOperatorPruneAuthority, MobMemberOperatorRequestBegin,
    MobMemberOperatorRequestKey, MobMemberOperatorRequestRecord, MobOperatorGrantDeletionAuthority,
    MobOperatorGrantPersistenceAuthority, MobOperatorGrantRecord,
    MobPlacedSpawnBindingPromotionAuthority, MobPlacedSpawnCarrierRecord,
    MobPlacedSpawnCleanupAuthority, MobPlacedSpawnCommitPersistenceAuthority,
    MobPlacedSpawnPendingPersistenceAuthority, MobRunStore, MobRuntimeMetadataStore, MobSpecStore,
    MobStoreError, PlacedSpawnCarrierPhase, PromotePlacedSpawnBindingResult,
    SupervisorAuthorityDeletionAuthority, SupervisorAuthorityPersistenceAuthority,
    SupervisorAuthorityRecord, private, step_failed_event_identity, terminal_event_identity,
    validate_mob_event_write_authority,
};
use crate::definition::MobDefinition;
use crate::error::MobError;
use crate::event::{MobEvent, NewMobEvent, decode_stored_mob_event, encode_stored_mob_event};
use crate::ids::{
    AgentIdentity, FlowId, FrameId, Generation, LoopId, LoopInstanceId, MobId, RunId, StepId,
};
use crate::machines::mob_machine as mob_dsl;
use crate::profile::Profile;
use crate::run::flow_run;
use crate::run::{
    FailureLedgerEntry, FrameSnapshot, LoopIterationLedgerEntry, LoopSnapshot, MobRun,
    MobRunProvenanceAuthority, MobRunRemoteTurnIntent, MobRunRemoteTurnReceipt, MobRunStatus,
    StepLedgerEntry, mob_machine_run_status_is_terminal,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use notify::{RecursiveMode, Watcher};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::thread;
use std::time::Duration;
use tokio::sync::broadcast;

const SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;
const EVENT_SUBSCRIPTION_CHANNEL_CAPACITY: usize = 4096;
const EVENT_WATCH_CATCH_UP_LIMIT: usize = 1024;
const EVENT_WATCH_POLL_FALLBACK_MS: u64 = 250;

const CREATE_SCHEMA_SQL: &str = r"
CREATE TABLE IF NOT EXISTS mob_events (
    cursor INTEGER PRIMARY KEY,
    event_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_event_meta (
    key TEXT PRIMARY KEY,
    value INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_runs (
    run_id TEXT PRIMARY KEY,
    run_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_run_remote_turn_intents (
    run_id TEXT NOT NULL,
    dispatch_sequence BLOB NOT NULL,
    intent_json BLOB NOT NULL,
    PRIMARY KEY (run_id, dispatch_sequence)
);
CREATE TABLE IF NOT EXISTS mob_run_remote_turn_receipts (
    run_id TEXT NOT NULL,
    dispatch_sequence BLOB NOT NULL,
    receipt_json BLOB NOT NULL,
    PRIMARY KEY (run_id, dispatch_sequence)
);
CREATE TABLE IF NOT EXISTS mob_specs (
    mob_id TEXT PRIMARY KEY,
    spec_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_runtime_supervisors (
    mob_id TEXT PRIMARY KEY,
    record_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_runtime_host_authorities (
    mob_id TEXT NOT NULL,
    host_id TEXT NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, host_id)
);
CREATE TABLE IF NOT EXISTS mob_runtime_host_binding_generation_highwaters (
    mob_id TEXT NOT NULL,
    host_id TEXT NOT NULL,
    binding_generation BLOB NOT NULL,
    PRIMARY KEY (mob_id, host_id)
);
CREATE TABLE IF NOT EXISTS mob_runtime_member_operator_requests (
    mob_id TEXT NOT NULL,
    agent_identity TEXT NOT NULL,
    generation BLOB NOT NULL,
    fence_token BLOB NOT NULL,
    host_id TEXT NOT NULL,
    binding_generation BLOB NOT NULL,
    member_session_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, agent_identity, generation, fence_token, host_id, binding_generation, member_session_id, request_id)
);
CREATE TABLE IF NOT EXISTS mob_runtime_placed_spawns (
    mob_id TEXT NOT NULL,
    agent_identity TEXT NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, agent_identity)
);
CREATE TABLE IF NOT EXISTS mob_runtime_operator_grants (
    mob_id TEXT NOT NULL,
    principal TEXT NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, principal)
);
CREATE TABLE IF NOT EXISTS mob_runtime_member_event_cursors (
    mob_id TEXT NOT NULL,
    agent_identity TEXT NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, agent_identity)
);
CREATE TABLE IF NOT EXISTS mob_runtime_member_live_cleanups (
    mob_id TEXT NOT NULL,
    cleanup_id TEXT NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, cleanup_id)
);
CREATE TABLE IF NOT EXISTS mob_runtime_binding_overlays (
    mob_id TEXT NOT NULL,
    agent_identity TEXT NOT NULL,
    generation INTEGER NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, agent_identity, generation)
);
CREATE TABLE IF NOT EXISTS realm_profiles (
    name TEXT PRIMARY KEY,
    profile_json BLOB NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
)";

fn se(error: impl std::fmt::Display) -> MobStoreError {
    MobStoreError::Internal(error.to_string())
}

fn encode_json<T: Serialize>(value: &T) -> Result<Vec<u8>, MobStoreError> {
    serde_json::to_vec(value).map_err(|e| MobStoreError::Serialization(e.to_string()))
}

fn decode_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, MobStoreError> {
    serde_json::from_slice(bytes).map_err(|e| MobStoreError::Serialization(e.to_string()))
}

fn validate_member_operator_request_row(
    record: &MobMemberOperatorRequestRecord,
    key: &MobMemberOperatorRequestKey,
) -> Result<(), MobStoreError> {
    record.validate()?;
    if record.key().eq(key) {
        return Ok(());
    }
    Err(MobStoreError::Internal(format!(
        "member operator request row key does not match record identity/generation/fence/host/binding_generation/session/request_id: row=({},{},{},{},{},{},{}) record=({},{},{},{},{},{},{})",
        key.agent_identity,
        key.generation,
        key.fence_token,
        key.host_id,
        key.host_binding_generation,
        key.member_session_id,
        key.request_id,
        record.agent_identity,
        record.generation,
        record.fence_token,
        record.host_id,
        record.host_binding_generation,
        record.member_session_id,
        record.request_id
    )))
}

fn cursor_to_i64(value: u64) -> Result<i64, MobStoreError> {
    i64::try_from(value)
        .map_err(|_| MobStoreError::Internal(format!("cursor value {value} exceeds i64::MAX")))
}

/// Full-domain, order-preserving SQLite key for newly introduced u64
/// identity/sequence columns. SQLite INTEGER is signed and would reject the
/// upper half of the public u64 domain; fixed-width big-endian blobs compare
/// lexicographically in the same order as the source integers.
fn u64_to_sql_key(value: u64) -> Vec<u8> {
    value.to_be_bytes().to_vec()
}

fn sql_key_to_u64(bytes: &[u8], field: &str) -> Result<u64, MobStoreError> {
    let encoded: [u8; std::mem::size_of::<u64>()] = bytes.try_into().map_err(|_| {
        MobStoreError::Internal(format!(
            "{field} SQLite key has {} bytes; expected {}",
            bytes.len(),
            std::mem::size_of::<u64>()
        ))
    })?;
    Ok(u64::from_be_bytes(encoded))
}

fn i64_to_cursor(value: i64) -> u64 {
    // SQLite INTEGER is signed; cursors start at 1 and are monotonic.
    // Negative values should never appear, but clamp to 0 defensively.
    u64::try_from(value).unwrap_or(0)
}

fn ensure_member_operator_execution_fence_schema(
    conn: &mut Connection,
) -> Result<(), MobStoreError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(se)?;
    let (has_host_id, has_binding_generation, has_member_session_id, primary_key) = {
        let mut stmt = tx
            .prepare("PRAGMA table_info(mob_runtime_member_operator_requests)")
            .map_err(se)?;
        let columns = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
            })
            .map_err(se)?;
        let mut found_host_id = false;
        let mut found_binding_generation = false;
        let mut found_member_session_id = false;
        let mut primary_key = Vec::new();
        for column in columns {
            let (name, key_position) = column.map_err(se)?;
            match name.as_str() {
                "host_id" => found_host_id = true,
                "binding_generation" => found_binding_generation = true,
                "member_session_id" => found_member_session_id = true,
                _ => {}
            }
            if key_position > 0 {
                primary_key.push((key_position, name));
            }
        }
        primary_key.sort_by_key(|(position, _)| *position);
        (
            found_host_id,
            found_binding_generation,
            found_member_session_id,
            primary_key
                .into_iter()
                .map(|(_, name)| name)
                .collect::<Vec<_>>(),
        )
    };
    let expected_primary_key = [
        "mob_id",
        "agent_identity",
        "generation",
        "fence_token",
        "host_id",
        "binding_generation",
        "member_session_id",
        "request_id",
    ];
    if !has_host_id
        || !has_binding_generation
        || !has_member_session_id
        || !primary_key
            .iter()
            .map(String::as_str)
            .eq(expected_primary_key)
    {
        // Pre-fence rows cannot be attributed to a host generation. The wire
        // now rejects every pre-fence request before ledger access, so keeping
        // those rows would only create ambiguous replay keys; rebuild this
        // bounded negative-memory table empty under the exact residency key.
        tx.execute_batch(
            "ALTER TABLE mob_runtime_member_operator_requests
                 RENAME TO mob_runtime_member_operator_requests_pre_execution_fence;
             CREATE TABLE mob_runtime_member_operator_requests (
                 mob_id TEXT NOT NULL,
                 agent_identity TEXT NOT NULL,
                 generation BLOB NOT NULL,
                 fence_token BLOB NOT NULL,
                 host_id TEXT NOT NULL,
                 binding_generation BLOB NOT NULL,
                 member_session_id TEXT NOT NULL,
                 request_id TEXT NOT NULL,
                 record_json BLOB NOT NULL,
                 PRIMARY KEY (
                     mob_id, agent_identity, generation, fence_token, host_id,
                     binding_generation, member_session_id, request_id
                 )
             );
             DROP TABLE mob_runtime_member_operator_requests_pre_execution_fence;",
        )
        .map_err(se)?;
    }
    tx.commit().map_err(se)
}

fn open_connection(path: &Path) -> Result<Connection, MobStoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(se)?;
    }
    let mut conn = Connection::open(path).map_err(se)?;
    conn.busy_timeout(Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))
        .map_err(se)?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(se)?;
    conn.pragma_update(None, "synchronous", "FULL")
        .map_err(se)?;
    conn.execute_batch(CREATE_SCHEMA_SQL).map_err(se)?;
    ensure_member_operator_execution_fence_schema(&mut conn)?;
    Ok(conn)
}

/// Open an existing mob database for passive observation without creating the
/// primary database or running migrations. Event-watch catch-up may race
/// terminal storage removal, so it must never use the create-capable writer
/// connection.
fn open_existing_read_connection(path: &Path) -> Result<Connection, MobStoreError> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(se)?;
    conn.busy_timeout(Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))
        .map_err(se)?;
    Ok(conn)
}

fn begin_immediate(conn: &mut Connection) -> Result<Transaction<'_>, MobStoreError> {
    conn.transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(se)
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn latest_event_cursor_sync(path: &Path) -> Result<u64, MobStoreError> {
    let conn = open_existing_read_connection(path)?;
    let cursor: Option<i64> = conn
        .query_row("SELECT MAX(cursor) FROM mob_events", [], |row| row.get(0))
        .optional()
        .map_err(se)?
        .flatten();
    Ok(cursor.map_or(0, i64_to_cursor))
}

fn poll_events_sync(
    path: &Path,
    after_cursor: u64,
    limit: usize,
) -> Result<Vec<MobEvent>, MobStoreError> {
    let conn = open_existing_read_connection(path)?;
    poll_events_from_connection(&conn, after_cursor, limit)
}

fn poll_events_from_connection(
    conn: &Connection,
    after_cursor: u64,
    limit: usize,
) -> Result<Vec<MobEvent>, MobStoreError> {
    let mut stmt = conn
        .prepare("SELECT event_json FROM mob_events WHERE cursor > ?1 ORDER BY cursor LIMIT ?2")
        .map_err(se)?;
    let rows = stmt
        .query_map(
            params![
                cursor_to_i64(after_cursor)?,
                i64::try_from(limit).map_err(|_| se("limit exceeds i64::MAX"))?
            ],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .map_err(se)?;
    let mut result = Vec::new();
    for row in rows {
        let bytes = row.map_err(se)?;
        result.push(
            decode_stored_mob_event(&bytes)
                .map_err(|e| MobStoreError::Serialization(e.to_string()))?,
        );
    }
    Ok(result)
}

fn load_run_bytes(tx: &Transaction<'_>, key: &str) -> Result<Option<Vec<u8>>, MobStoreError> {
    tx.query_row(
        "SELECT run_json FROM mob_runs WHERE run_id = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .map_err(se)
}

fn write_run_json(tx: &Transaction<'_>, key: &str, run: &MobRun) -> Result<(), MobStoreError> {
    let encoded = encode_json(run)?;
    tx.execute(
        "UPDATE mob_runs SET run_json = ?1 WHERE run_id = ?2",
        params![encoded, key],
    )
    .map_err(se)?;
    Ok(())
}

fn append_loop_iteration_ledger_if_absent(run: &mut MobRun, entry: LoopIterationLedgerEntry) {
    if !run.loop_iteration_ledger.iter().any(|existing| {
        existing.loop_instance_id == entry.loop_instance_id
            && existing.iteration == entry.iteration
            && existing.frame_id == entry.frame_id
    }) {
        run.loop_iteration_ledger.push(entry);
    }
}

async fn run_sqlite_task<T>(
    task: impl FnOnce() -> Result<T, MobStoreError> + Send + 'static,
) -> Result<T, MobStoreError>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|error| MobStoreError::Internal(format!("sqlite task join failed: {error}")))?
}

// ---------------------------------------------------------------------------
// SqliteMobStores — unified handle (stores only the path)
// ---------------------------------------------------------------------------

static SQLITE_EVENT_BUSES: OnceLock<Mutex<HashMap<PathBuf, Weak<SqliteMobEventBus>>>> =
    OnceLock::new();

fn sqlite_event_buses() -> &'static Mutex<HashMap<PathBuf, Weak<SqliteMobEventBus>>> {
    SQLITE_EVENT_BUSES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn sqlite_event_bus_for_path(path: PathBuf) -> Result<Arc<SqliteMobEventBus>, MobStoreError> {
    let mut buses = lock_unpoisoned(sqlite_event_buses());
    if let Some(bus) = buses.get(&path).and_then(Weak::upgrade) {
        return Ok(bus);
    }

    let bus = SqliteMobEventBus::new(path.clone())?;
    buses.insert(path, Arc::downgrade(&bus));
    Ok(bus)
}

struct SqliteMobEventBus {
    path: PathBuf,
    event_tx: broadcast::Sender<MobEvent>,
    latest_broadcast_cursor: Mutex<u64>,
    catch_up_lock: Mutex<()>,
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
}

impl std::fmt::Debug for SqliteMobEventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteMobEventBus")
            .field("path", &self.path)
            .finish()
    }
}

impl SqliteMobEventBus {
    fn new(path: PathBuf) -> Result<Arc<Self>, MobStoreError> {
        let latest_cursor = latest_event_cursor_sync(&path)?;
        let (event_tx, _event_rx) = broadcast::channel(EVENT_SUBSCRIPTION_CHANNEL_CAPACITY);
        let bus = Arc::new(Self {
            path,
            event_tx,
            latest_broadcast_cursor: Mutex::new(latest_cursor),
            catch_up_lock: Mutex::new(()),
            watcher: Mutex::new(None),
        });
        bus.start_external_watch();
        Ok(bus)
    }

    fn subscribe(&self) -> super::MobEventReceiver {
        self.event_tx.subscribe()
    }

    fn publish_committed(&self, event: MobEvent) {
        self.publish_committed_batch(std::slice::from_ref(&event));
    }

    fn publish_committed_batch(&self, events: &[MobEvent]) {
        let current_cursor = *lock_unpoisoned(&self.latest_broadcast_cursor);
        let mut expected_cursor = current_cursor.saturating_add(1);
        let mut has_gap = false;
        for event in events.iter().filter(|event| event.cursor > current_cursor) {
            if event.cursor > expected_cursor {
                has_gap = true;
                break;
            }
            expected_cursor = event.cursor.saturating_add(1);
        }
        if has_gap {
            if let Err(error) = self.publish_available_from_storage() {
                tracing::warn!(
                    error = %error,
                    path = %self.path.display(),
                    "sqlite mob event gap catch-up failed before direct publish",
                );
            }
        }
        self.publish_committed_batch_unchecked(events);
    }

    fn publish_committed_batch_unchecked(&self, events: &[MobEvent]) {
        let mut cursor = lock_unpoisoned(&self.latest_broadcast_cursor);
        for event in events {
            if event.cursor <= *cursor {
                continue;
            }
            *cursor = event.cursor;
            let _ = self.event_tx.send(event.clone());
        }
    }

    fn publish_available_from_storage(&self) -> Result<(), MobStoreError> {
        let _catch_up_guard = lock_unpoisoned(&self.catch_up_lock);
        if !self.path.try_exists().map_err(se)? {
            // A successful mob destroy removes the SQLite database while
            // existing handle clones may still keep this event bus alive. The
            // watcher can wake on that delete event; avoid reopening the path,
            // because opening SQLite would create a fresh empty database.
            return Ok(());
        }
        loop {
            let after_cursor = *lock_unpoisoned(&self.latest_broadcast_cursor);
            let batch = match poll_events_sync(&self.path, after_cursor, EVENT_WATCH_CATCH_UP_LIMIT)
            {
                Ok(batch) => batch,
                Err(error) => {
                    if !self.path.try_exists().map_err(se)? {
                        // The file disappeared after the preflight existence
                        // check. This is normal terminal destroy, and the
                        // no-create reader guarantees catch-up cannot resurrect
                        // the database in this window.
                        return Ok(());
                    }
                    return Err(error);
                }
            };
            if batch.is_empty() {
                return Ok(());
            }
            let is_complete = batch.len() < EVENT_WATCH_CATCH_UP_LIMIT;
            self.publish_committed_batch_unchecked(&batch);
            if is_complete {
                return Ok(());
            }
        }
    }

    fn start_external_watch(self: &Arc<Self>) {
        let Some(parent) = self.path.parent().map(Path::to_path_buf) else {
            return;
        };
        let watched_paths = sqlite_watch_paths(&self.path);
        let (wake_tx, wake_rx) = std::sync::mpsc::channel::<()>();
        let thread_bus = Arc::downgrade(self);
        let thread_builder = thread::Builder::new().name("sqlite-mob-event-watch".to_string());
        if let Err(error) = thread_builder.spawn(move || {
            loop {
                let received_wake = match wake_rx
                    .recv_timeout(Duration::from_millis(EVENT_WATCH_POLL_FALLBACK_MS))
                {
                    Ok(()) => true,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => false,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                };
                if received_wake {
                    thread::sleep(Duration::from_millis(10));
                }
                while wake_rx.try_recv().is_ok() {}
                let Some(bus) = thread_bus.upgrade() else {
                    break;
                };
                if !received_wake && bus.event_tx.receiver_count() == 0 {
                    continue;
                }
                if let Err(error) = bus.publish_available_from_storage() {
                    tracing::warn!(
                        error = %error,
                        path = %bus.path.display(),
                        "sqlite mob event watch catch-up failed",
                    );
                }
            }
        }) {
            tracing::warn!(
                error = %error,
                path = %self.path.display(),
                "failed to start sqlite mob event watch thread",
            );
            return;
        }

        let callback_wake_tx = wake_tx.clone();
        let callback_parent = parent.clone();
        let mut watcher =
            match notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                match result {
                    Ok(event)
                        if sqlite_watch_event_relevant(
                            &event,
                            &callback_parent,
                            &watched_paths,
                        ) =>
                    {
                        let _ = callback_wake_tx.send(());
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(
                            error = %error,
                            "sqlite mob event filesystem watch reported an error",
                        );
                    }
                }
            }) {
                Ok(watcher) => watcher,
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        path = %self.path.display(),
                        "failed to create sqlite mob event filesystem watcher",
                    );
                    return;
                }
            };

        if let Err(error) = watcher.watch(&parent, RecursiveMode::NonRecursive) {
            tracing::warn!(
                error = %error,
                path = %parent.display(),
                "failed to watch sqlite mob event directory",
            );
            return;
        }

        *lock_unpoisoned(&self.watcher) = Some(watcher);
    }
}

fn sqlite_sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn sqlite_watch_paths(path: &Path) -> Vec<PathBuf> {
    vec![
        path.to_path_buf(),
        sqlite_sidecar_path(path, "-wal"),
        sqlite_sidecar_path(path, "-shm"),
    ]
}

fn sqlite_watch_event_relevant(
    event: &notify::Event,
    parent: &Path,
    watched_paths: &[PathBuf],
) -> bool {
    if matches!(event.kind, notify::EventKind::Access(_)) {
        return false;
    }

    if event.paths.is_empty() {
        return true;
    }

    event.paths.iter().any(|path| {
        path == parent
            || watched_paths.iter().any(|watched| {
                path == watched
                    || (path.parent() == watched.parent()
                        && path.file_name() == watched.file_name())
            })
    })
}

/// Shared bundle that produces event/run/spec stores all pointing to the same db file.
#[derive(Debug, Clone)]
pub struct SqliteMobStores {
    path: PathBuf,
    event_bus: Arc<SqliteMobEventBus>,
}

impl SqliteMobStores {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MobError> {
        let path = path.as_ref().to_path_buf();
        // Validate the path works by opening and immediately closing.
        let _conn = open_connection(&path)?;
        let path = std::fs::canonicalize(&path).map_err(se)?;
        let event_bus = sqlite_event_bus_for_path(path.clone())?;
        Ok(Self { path, event_bus })
    }

    pub fn event_store(&self) -> SqliteMobEventStore {
        SqliteMobEventStore {
            path: self.path.clone(),
            event_bus: Arc::clone(&self.event_bus),
        }
    }

    pub fn run_store(&self) -> SqliteMobRunStore {
        SqliteMobRunStore {
            path: self.path.clone(),
        }
    }

    pub fn spec_store(&self) -> SqliteMobSpecStore {
        SqliteMobSpecStore {
            path: self.path.clone(),
        }
    }

    pub fn runtime_metadata_store(&self) -> SqliteMobRuntimeMetadataStore {
        SqliteMobRuntimeMetadataStore {
            path: self.path.clone(),
        }
    }

    pub fn realm_profile_store(&self) -> SqliteRealmProfileStore {
        SqliteRealmProfileStore {
            path: self.path.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// SqliteMobRuntimeMetadataStore
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SqliteMobRuntimeMetadataStore {
    path: PathBuf,
}

impl std::fmt::Debug for SqliteMobRuntimeMetadataStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteMobRuntimeMetadataStore")
            .field("path", &self.path)
            .finish()
    }
}

#[async_trait]
impl MobRuntimeMetadataStore for SqliteMobRuntimeMetadataStore {
    async fn load_supervisor_authority(
        &self,
        mob_id: &MobId,
    ) -> Result<Option<SupervisorAuthorityRecord>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let row: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT record_json FROM mob_runtime_supervisors WHERE mob_id = ?1",
                    params![mob_id.as_str()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            row.map(|bytes| decode_json(&bytes)).transpose()
        })
        .await
    }

    async fn put_supervisor_authority(
        &self,
        mob_id: &MobId,
        record: &SupervisorAuthorityRecord,
        authority: &SupervisorAuthorityPersistenceAuthority,
    ) -> Result<(), MobStoreError> {
        authority.verify_record(record)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            tx.execute(
                "INSERT INTO mob_runtime_supervisors (mob_id, record_json) VALUES (?1, ?2)
                 ON CONFLICT(mob_id) DO UPDATE SET record_json = excluded.record_json",
                params![mob_id.as_str(), encode_json(&record)?],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn compare_and_put_supervisor_authority(
        &self,
        mob_id: &MobId,
        expected: &SupervisorAuthorityRecord,
        record: &SupervisorAuthorityRecord,
        authority: &SupervisorAuthorityPersistenceAuthority,
    ) -> Result<bool, MobStoreError> {
        authority.verify_record(record)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let expected = expected.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "UPDATE mob_runtime_supervisors
                     SET record_json = ?2
                     WHERE mob_id = ?1 AND record_json = ?3",
                    params![
                        mob_id.as_str(),
                        encode_json(&record)?,
                        encode_json(&expected)?
                    ],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn put_supervisor_authority_if_absent(
        &self,
        mob_id: &MobId,
        record: &SupervisorAuthorityRecord,
        authority: &SupervisorAuthorityPersistenceAuthority,
    ) -> Result<bool, MobStoreError> {
        authority.verify_record(record)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "INSERT OR IGNORE INTO mob_runtime_supervisors (mob_id, record_json) VALUES (?1, ?2)",
                    params![mob_id.as_str(), encode_json(&record)?],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn delete_supervisor_authority(
        &self,
        mob_id: &MobId,
        expected: &SupervisorAuthorityRecord,
        authority: &SupervisorAuthorityDeletionAuthority,
    ) -> Result<bool, MobStoreError> {
        authority.verify_record(expected)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let expected = expected.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "DELETE FROM mob_runtime_supervisors WHERE mob_id = ?1 AND record_json = ?2",
                    params![mob_id.as_str(), encode_json(&expected)?],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn load_mob_host_authority(
        &self,
        mob_id: &MobId,
        host_id: &str,
    ) -> Result<Option<MobHostAuthorityRecord>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let host_id = host_id.to_string();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let row: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT record_json FROM mob_runtime_host_authorities
                     WHERE mob_id = ?1 AND host_id = ?2",
                    params![mob_id.as_str(), host_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            row.map(|bytes| decode_json(&bytes)).transpose()
        })
        .await
    }

    async fn list_mob_host_authorities(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobHostAuthorityRecord>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT record_json FROM mob_runtime_host_authorities
                     WHERE mob_id = ?1
                     ORDER BY host_id",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(params![mob_id.as_str()], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            let mut records = Vec::new();
            for row in rows {
                let bytes = row.map_err(se)?;
                records.push(decode_json(&bytes)?);
            }
            Ok(records)
        })
        .await
    }

    async fn put_mob_host_authority(
        &self,
        mob_id: &MobId,
        record: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityPersistenceAuthority,
    ) -> Result<(), MobStoreError> {
        authority.verify_record(record)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            tx.execute(
                "INSERT INTO mob_runtime_host_authorities (mob_id, host_id, record_json)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(mob_id, host_id) DO UPDATE SET record_json = excluded.record_json",
                params![mob_id.as_str(), record.host_id, encode_json(&record)?],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn put_mob_host_authority_if_absent(
        &self,
        mob_id: &MobId,
        record: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityPersistenceAuthority,
    ) -> Result<bool, MobStoreError> {
        authority.verify_record(record)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "INSERT OR IGNORE INTO mob_runtime_host_authorities (mob_id, host_id, record_json)
                     VALUES (?1, ?2, ?3)",
                    params![mob_id.as_str(), record.host_id, encode_json(&record)?],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn compare_and_put_mob_host_authority(
        &self,
        mob_id: &MobId,
        expected: &MobHostAuthorityRecord,
        record: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityPersistenceAuthority,
    ) -> Result<bool, MobStoreError> {
        authority.verify_record(record)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let expected = expected.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "UPDATE mob_runtime_host_authorities
                     SET record_json = ?3
                     WHERE mob_id = ?1 AND host_id = ?2 AND record_json = ?4",
                    params![
                        mob_id.as_str(),
                        record.host_id,
                        encode_json(&record)?,
                        encode_json(&expected)?
                    ],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn delete_mob_host_authority(
        &self,
        mob_id: &MobId,
        expected: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityDeletionAuthority,
    ) -> Result<bool, MobStoreError> {
        authority.verify_record(expected)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let expected = expected.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "DELETE FROM mob_runtime_host_authorities
                     WHERE mob_id = ?1 AND host_id = ?2 AND record_json = ?3",
                    params![mob_id.as_str(), expected.host_id, encode_json(&expected)?],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn put_mob_host_binding_generation_highwater(
        &self,
        mob_id: &MobId,
        expected: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityDeletionAuthority,
    ) -> Result<(), MobStoreError> {
        authority.verify_record(expected)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let host_id = expected.host_id.clone();
        let binding_generation = u64_to_sql_key(expected.binding_generation);
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            tx.execute(
                "INSERT INTO mob_runtime_host_binding_generation_highwaters
                    (mob_id, host_id, binding_generation)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(mob_id, host_id) DO UPDATE SET binding_generation =
                    CASE
                        WHEN excluded.binding_generation > mob_runtime_host_binding_generation_highwaters.binding_generation
                        THEN excluded.binding_generation
                        ELSE mob_runtime_host_binding_generation_highwaters.binding_generation
                    END",
                params![mob_id.as_str(), host_id, binding_generation],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn list_mob_host_binding_generation_highwaters(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<(String, u64)>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT host_id, binding_generation
                     FROM mob_runtime_host_binding_generation_highwaters
                     WHERE mob_id = ?1
                     ORDER BY host_id",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(params![mob_id.as_str()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
                })
                .map_err(se)?;
            let mut highwaters = Vec::new();
            for row in rows {
                let (host_id, encoded) = row.map_err(se)?;
                highwaters.push((host_id, sql_key_to_u64(&encoded, "binding_generation")?));
            }
            Ok(highwaters)
        })
        .await
    }

    async fn begin_member_operator_request(
        &self,
        mob_id: &MobId,
        record: &MobMemberOperatorRequestRecord,
    ) -> Result<MobMemberOperatorRequestBegin, MobStoreError> {
        record.validate_pending()?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let generation = u64_to_sql_key(record.generation);
            let fence_token = u64_to_sql_key(record.fence_token);
            let binding_generation = u64_to_sql_key(record.host_binding_generation);
            let existing_bytes: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT record_json FROM mob_runtime_member_operator_requests
                     WHERE mob_id = ?1 AND agent_identity = ?2
                       AND generation = ?3 AND fence_token = ?4
                       AND host_id = ?5 AND binding_generation = ?6
                       AND member_session_id = ?7 AND request_id = ?8",
                    params![
                        mob_id.as_str(),
                        &record.agent_identity,
                        generation,
                        fence_token,
                        &record.host_id,
                        binding_generation,
                        &record.member_session_id,
                        &record.request_id,
                    ],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let result = if let Some(bytes) = existing_bytes {
                let existing: MobMemberOperatorRequestRecord = decode_json(&bytes)?;
                validate_member_operator_request_row(&existing, &record.key())?;
                MobMemberOperatorRequestBegin::Existing(existing)
            } else {
                let incarnation_rows: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM mob_runtime_member_operator_requests
                         WHERE mob_id = ?1 AND agent_identity = ?2
                           AND generation = ?3 AND fence_token = ?4
                           AND host_id = ?5 AND binding_generation = ?6
                           AND member_session_id = ?7",
                        params![
                            mob_id.as_str(),
                            &record.agent_identity,
                            generation,
                            fence_token,
                            &record.host_id,
                            binding_generation,
                            &record.member_session_id,
                        ],
                        |row| row.get(0),
                    )
                    .map_err(se)?;
                if incarnation_rows
                    >= i64::try_from(super::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION)
                        .unwrap_or(i64::MAX)
                {
                    return Err(MobStoreError::WriteFailed(format!(
                        "member operator request quota exhausted for '{}' generation {} fence {} host '{}' binding generation {} session '{}' (max {})",
                        record.agent_identity,
                        record.generation,
                        record.fence_token,
                        record.host_id,
                        record.host_binding_generation,
                        record.member_session_id,
                        super::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION,
                    )));
                }
                let mob_rows: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM mob_runtime_member_operator_requests
                         WHERE mob_id = ?1",
                        params![mob_id.as_str()],
                        |row| row.get(0),
                    )
                    .map_err(se)?;
                if mob_rows
                    >= i64::try_from(super::MEMBER_OPERATOR_REQUEST_MAX_PER_MOB)
                        .unwrap_or(i64::MAX)
                {
                    return Err(MobStoreError::WriteFailed(format!(
                        "member operator request quota exhausted for mob '{}' (max {})",
                        mob_id,
                        super::MEMBER_OPERATOR_REQUEST_MAX_PER_MOB,
                    )));
                }
                tx.execute(
                    "INSERT INTO mob_runtime_member_operator_requests
                     (mob_id, agent_identity, generation, fence_token, host_id, binding_generation, member_session_id, request_id, record_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        mob_id.as_str(),
                        &record.agent_identity,
                        generation,
                        fence_token,
                        &record.host_id,
                        binding_generation,
                        &record.member_session_id,
                        &record.request_id,
                        encode_json(&record)?,
                    ],
                )
                .map_err(se)?;
                MobMemberOperatorRequestBegin::Started
            };
            tx.commit().map_err(se)?;
            Ok(result)
        })
        .await
    }

    async fn load_member_operator_request(
        &self,
        mob_id: &MobId,
        key: &MobMemberOperatorRequestKey,
    ) -> Result<Option<MobMemberOperatorRequestRecord>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let key = key.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let row: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT record_json FROM mob_runtime_member_operator_requests
                     WHERE mob_id = ?1 AND agent_identity = ?2
                       AND generation = ?3 AND fence_token = ?4
                       AND host_id = ?5 AND binding_generation = ?6
                       AND member_session_id = ?7 AND request_id = ?8",
                    params![
                        mob_id.as_str(),
                        &key.agent_identity,
                        u64_to_sql_key(key.generation),
                        u64_to_sql_key(key.fence_token),
                        &key.host_id,
                        u64_to_sql_key(key.host_binding_generation),
                        &key.member_session_id,
                        &key.request_id,
                    ],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let record: Option<MobMemberOperatorRequestRecord> =
                row.map(|bytes| decode_json(&bytes)).transpose()?;
            if let Some(record) = &record {
                validate_member_operator_request_row(record, &key)?;
            }
            Ok(record)
        })
        .await
    }

    async fn compare_and_put_member_operator_request(
        &self,
        mob_id: &MobId,
        expected: &MobMemberOperatorRequestRecord,
        record: &MobMemberOperatorRequestRecord,
    ) -> Result<bool, MobStoreError> {
        expected.validate_terminal_transition(record)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let expected = expected.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "UPDATE mob_runtime_member_operator_requests
                     SET record_json = ?1
                     WHERE mob_id = ?2 AND agent_identity = ?3
                       AND generation = ?4 AND fence_token = ?5
                       AND host_id = ?6 AND binding_generation = ?7
                       AND member_session_id = ?8 AND request_id = ?9
                       AND record_json = ?10",
                    params![
                        encode_json(&record)?,
                        mob_id.as_str(),
                        expected.agent_identity,
                        u64_to_sql_key(expected.generation),
                        u64_to_sql_key(expected.fence_token),
                        expected.host_id,
                        u64_to_sql_key(expected.host_binding_generation),
                        expected.member_session_id,
                        expected.request_id,
                        encode_json(&expected)?,
                    ],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn list_member_operator_requests(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobMemberOperatorRequestRecord>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT agent_identity, generation, fence_token, host_id, binding_generation, member_session_id, request_id, record_json
                     FROM mob_runtime_member_operator_requests
                     WHERE mob_id = ?1
                     ORDER BY agent_identity, generation, fence_token, host_id, binding_generation, member_session_id, request_id",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(params![mob_id.as_str()], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, Vec<u8>>(7)?,
                    ))
                })
                .map_err(se)?;
            let mut records = Vec::new();
            for row in rows {
                let (
                    agent_identity,
                    generation,
                    fence_token,
                    host_id,
                    binding_generation,
                    member_session_id,
                    request_id,
                    bytes,
                ) =
                    row.map_err(se)?;
                let generation = sql_key_to_u64(
                    &generation,
                    &format!("member operator request '{request_id}' generation"),
                )?;
                let fence_token = sql_key_to_u64(
                    &fence_token,
                    &format!("member operator request '{request_id}' fence token"),
                )?;
                let binding_generation = sql_key_to_u64(
                    &binding_generation,
                    &format!(
                        "member operator request '{request_id}' binding generation"
                    ),
                )?;
                let record: MobMemberOperatorRequestRecord = decode_json(&bytes)?;
                let key = MobMemberOperatorRequestKey::new(
                    agent_identity,
                    generation,
                    fence_token,
                    host_id,
                    binding_generation,
                    member_session_id,
                    request_id,
                );
                validate_member_operator_request_row(&record, &key)?;
                records.push(record);
            }
            Ok(records)
        })
        .await
    }

    async fn prune_stale_member_operator_requests(
        &self,
        mob_id: &MobId,
        authority: &MobMemberOperatorPruneAuthority,
    ) -> Result<u64, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let authority = authority.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let stale = {
                let mut stmt = tx
                    .prepare(
                        "SELECT agent_identity, generation, fence_token, host_id,
                                binding_generation, member_session_id, request_id, record_json
                         FROM mob_runtime_member_operator_requests
                         WHERE mob_id = ?1",
                    )
                    .map_err(se)?;
                let rows = stmt
                    .query_map(params![mob_id.as_str()], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Vec<u8>>(1)?,
                            row.get::<_, Vec<u8>>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, Vec<u8>>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, Vec<u8>>(7)?,
                        ))
                    })
                    .map_err(se)?;
                let mut stale = Vec::new();
                for row in rows {
                    let (
                        agent_identity,
                        generation,
                        fence_token,
                        host_id,
                        binding_generation,
                        member_session_id,
                        request_id,
                        bytes,
                    ) = row.map_err(se)?;
                    let generation = sql_key_to_u64(
                        &generation,
                        &format!("member operator request '{request_id}' generation"),
                    )?;
                    let fence_token = sql_key_to_u64(
                        &fence_token,
                        &format!("member operator request '{request_id}' fence token"),
                    )?;
                    let binding_generation = sql_key_to_u64(
                        &binding_generation,
                        &format!("member operator request '{request_id}' binding generation"),
                    )?;
                    let record: MobMemberOperatorRequestRecord = decode_json(&bytes)?;
                    let key = MobMemberOperatorRequestKey::new(
                        agent_identity,
                        generation,
                        fence_token,
                        host_id,
                        binding_generation,
                        member_session_id,
                        request_id,
                    );
                    validate_member_operator_request_row(&record, &key)?;
                    if !authority.preserves(&record) {
                        stale.push(record);
                    }
                }
                stale
            };

            let mut deleted = 0usize;
            for record in stale {
                deleted += tx
                    .execute(
                        "DELETE FROM mob_runtime_member_operator_requests
                         WHERE mob_id = ?1 AND agent_identity = ?2
                           AND generation = ?3 AND fence_token = ?4
                           AND host_id = ?5 AND binding_generation = ?6
                           AND member_session_id = ?7 AND request_id = ?8",
                        params![
                            mob_id.as_str(),
                            record.agent_identity,
                            u64_to_sql_key(record.generation),
                            u64_to_sql_key(record.fence_token),
                            record.host_id,
                            u64_to_sql_key(record.host_binding_generation),
                            record.member_session_id,
                            record.request_id,
                        ],
                    )
                    .map_err(se)?;
            }
            tx.commit().map_err(se)?;
            u64::try_from(deleted).map_err(|_| {
                MobStoreError::Internal(
                    "member operator request SQLite prune count overflow".to_string(),
                )
            })
        })
        .await
    }

    async fn delete_member_operator_requests(&self, mob_id: &MobId) -> Result<u64, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let deleted = tx
                .execute(
                    "DELETE FROM mob_runtime_member_operator_requests WHERE mob_id = ?1",
                    params![mob_id.as_str()],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            u64::try_from(deleted).map_err(|_| {
                MobStoreError::Internal("member operator request delete count overflow".to_string())
            })
        })
        .await
    }

    async fn load_placed_spawn(
        &self,
        mob_id: &MobId,
        agent_identity: &str,
    ) -> Result<Option<MobPlacedSpawnCarrierRecord>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let agent_identity = agent_identity.to_string();
        run_sqlite_task(move || {
            let conn = open_existing_read_connection(&path)?;
            let row: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT record_json FROM mob_runtime_placed_spawns
                     WHERE mob_id = ?1 AND agent_identity = ?2",
                    params![mob_id.as_str(), agent_identity],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let record: Option<MobPlacedSpawnCarrierRecord> =
                row.map(|bytes| decode_json(&bytes)).transpose()?;
            if let Some(record) = &record {
                record.validate_for_store_key(&mob_id, &agent_identity)?;
            }
            Ok(record)
        })
        .await
    }

    async fn list_placed_spawns(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobPlacedSpawnCarrierRecord>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let conn = open_existing_read_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT agent_identity, record_json FROM mob_runtime_placed_spawns
                     WHERE mob_id = ?1 ORDER BY agent_identity",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(params![mob_id.as_str()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
                })
                .map_err(se)?;
            let mut records = Vec::new();
            for row in rows {
                let (agent_identity, bytes) = row.map_err(se)?;
                let record: MobPlacedSpawnCarrierRecord = decode_json(&bytes)?;
                record.validate_for_store_key(&mob_id, &agent_identity)?;
                records.push(record);
            }
            Ok(records)
        })
        .await
    }

    async fn begin_placed_spawn_if_absent(
        &self,
        mob_id: &MobId,
        record: &MobPlacedSpawnCarrierRecord,
        authority: &MobPlacedSpawnPendingPersistenceAuthority,
    ) -> Result<BeginPlacedSpawnResult, MobStoreError> {
        record.validate_for_mob(mob_id)?;
        authority.verify_record(record)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let row: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT record_json FROM mob_runtime_placed_spawns
                     WHERE mob_id = ?1 AND agent_identity = ?2",
                    params![mob_id.as_str(), record.agent_identity],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let result = if let Some(bytes) = row {
                let existing: MobPlacedSpawnCarrierRecord = decode_json(&bytes)?;
                existing.validate_for_store_key(&mob_id, &record.agent_identity)?;
                if !existing.same_attempt_as(&record) {
                    BeginPlacedSpawnResult::Conflict
                } else {
                    match existing.phase {
                        PlacedSpawnCarrierPhase::Pending => {
                            BeginPlacedSpawnResult::ExistingExactPending
                        }
                        PlacedSpawnCarrierPhase::Committed(_) => {
                            BeginPlacedSpawnResult::ExistingExactCommitted
                        }
                    }
                }
            } else {
                tx.execute(
                    "INSERT INTO mob_runtime_placed_spawns
                     (mob_id, agent_identity, record_json) VALUES (?1, ?2, ?3)",
                    params![
                        mob_id.as_str(),
                        record.agent_identity,
                        encode_json(&record)?
                    ],
                )
                .map_err(se)?;
                BeginPlacedSpawnResult::Inserted
            };
            tx.commit().map_err(se)?;
            Ok(result)
        })
        .await
    }

    async fn compare_and_commit_placed_spawn(
        &self,
        mob_id: &MobId,
        expected_pending: &MobPlacedSpawnCarrierRecord,
        committed: &MobPlacedSpawnCarrierRecord,
        authority: &MobPlacedSpawnCommitPersistenceAuthority,
    ) -> Result<CommitPlacedSpawnResult, MobStoreError> {
        expected_pending.validate_for_mob(mob_id)?;
        committed.validate_for_mob(mob_id)?;
        authority.verify_record(committed)?;
        if !matches!(expected_pending.phase, PlacedSpawnCarrierPhase::Pending)
            || !expected_pending.same_attempt_as(committed)
        {
            return Err(MobStoreError::Internal(
                "placed-spawn commit CAS changed its attempt tuple or expected non-pending state"
                    .to_string(),
            ));
        }
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let expected_pending = expected_pending.clone();
        let committed = committed.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let row: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT record_json FROM mob_runtime_placed_spawns
                     WHERE mob_id = ?1 AND agent_identity = ?2",
                    params![mob_id.as_str(), expected_pending.agent_identity],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let result = match row {
                None => CommitPlacedSpawnResult::Conflict,
                Some(bytes) => {
                    let existing: MobPlacedSpawnCarrierRecord = decode_json(&bytes)?;
                    existing.validate_for_store_key(&mob_id, &expected_pending.agent_identity)?;
                    if existing == committed {
                        CommitPlacedSpawnResult::AlreadyCommittedExact
                    } else if existing == expected_pending {
                        tx.execute(
                            "UPDATE mob_runtime_placed_spawns SET record_json = ?3
                             WHERE mob_id = ?1 AND agent_identity = ?2",
                            params![
                                mob_id.as_str(),
                                expected_pending.agent_identity,
                                encode_json(&committed)?
                            ],
                        )
                        .map_err(se)?;
                        CommitPlacedSpawnResult::Committed
                    } else if existing.same_attempt_as(&expected_pending)
                        && matches!(existing.phase, PlacedSpawnCarrierPhase::Pending)
                    {
                        CommitPlacedSpawnResult::StillPending
                    } else {
                        CommitPlacedSpawnResult::Conflict
                    }
                }
            };
            tx.commit().map_err(se)?;
            Ok(result)
        })
        .await
    }

    async fn compare_and_promote_placed_spawn_binding(
        &self,
        mob_id: &MobId,
        expected: &MobPlacedSpawnCarrierRecord,
        promoted: &MobPlacedSpawnCarrierRecord,
        authority: &MobPlacedSpawnBindingPromotionAuthority,
    ) -> Result<PromotePlacedSpawnBindingResult, MobStoreError> {
        expected.validate_for_mob(mob_id)?;
        promoted.validate_for_mob(mob_id)?;
        authority.verify_records(expected, promoted)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let expected = expected.clone();
        let promoted = promoted.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let row: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT record_json FROM mob_runtime_placed_spawns
                     WHERE mob_id = ?1 AND agent_identity = ?2",
                    params![mob_id.as_str(), expected.agent_identity],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let result = match row {
                None => PromotePlacedSpawnBindingResult::Conflict,
                Some(bytes) => {
                    let existing: MobPlacedSpawnCarrierRecord = decode_json(&bytes)?;
                    existing.validate_for_store_key(&mob_id, &expected.agent_identity)?;
                    if existing == promoted {
                        PromotePlacedSpawnBindingResult::AlreadyPromotedExact
                    } else if existing == expected {
                        tx.execute(
                            "UPDATE mob_runtime_placed_spawns SET record_json = ?3
                             WHERE mob_id = ?1 AND agent_identity = ?2",
                            params![
                                mob_id.as_str(),
                                expected.agent_identity,
                                encode_json(&promoted)?
                            ],
                        )
                        .map_err(se)?;
                        PromotePlacedSpawnBindingResult::Promoted
                    } else {
                        PromotePlacedSpawnBindingResult::Conflict
                    }
                }
            };
            tx.commit().map_err(se)?;
            Ok(result)
        })
        .await
    }

    async fn compare_and_delete_placed_spawn(
        &self,
        mob_id: &MobId,
        expected: &MobPlacedSpawnCarrierRecord,
        authority: &MobPlacedSpawnCleanupAuthority,
    ) -> Result<DeletePlacedSpawnResult, MobStoreError> {
        expected.validate_for_mob(mob_id)?;
        authority.verify_record(expected)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let expected = expected.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let row: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT record_json FROM mob_runtime_placed_spawns
                     WHERE mob_id = ?1 AND agent_identity = ?2",
                    params![mob_id.as_str(), expected.agent_identity],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let result = match row {
                None => DeletePlacedSpawnResult::AlreadyAbsent,
                Some(bytes) => {
                    let existing: MobPlacedSpawnCarrierRecord = decode_json(&bytes)?;
                    existing.validate_for_store_key(&mob_id, &expected.agent_identity)?;
                    if existing != expected {
                        DeletePlacedSpawnResult::Conflict
                    } else {
                        tx.execute(
                            "DELETE FROM mob_runtime_placed_spawns
                             WHERE mob_id = ?1 AND agent_identity = ?2",
                            params![mob_id.as_str(), expected.agent_identity],
                        )
                        .map_err(se)?;
                        DeletePlacedSpawnResult::Deleted
                    }
                }
            };
            tx.commit().map_err(se)?;
            Ok(result)
        })
        .await
    }

    async fn list_mob_operator_grants(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobOperatorGrantRecord>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT record_json FROM mob_runtime_operator_grants
                     WHERE mob_id = ?1
                     ORDER BY principal",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(params![mob_id.as_str()], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            let mut records = Vec::new();
            for row in rows {
                let bytes = row.map_err(se)?;
                records.push(decode_json(&bytes)?);
            }
            Ok(records)
        })
        .await
    }

    async fn put_mob_operator_grant(
        &self,
        mob_id: &MobId,
        record: &MobOperatorGrantRecord,
        authority: &MobOperatorGrantPersistenceAuthority,
    ) -> Result<(), MobStoreError> {
        authority.verify_record(record)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            tx.execute(
                "INSERT INTO mob_runtime_operator_grants (mob_id, principal, record_json)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(mob_id, principal) DO UPDATE SET record_json = excluded.record_json",
                params![mob_id.as_str(), record.principal, encode_json(&record)?],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn delete_mob_operator_grant(
        &self,
        mob_id: &MobId,
        principal: &str,
        authority: &MobOperatorGrantDeletionAuthority,
    ) -> Result<bool, MobStoreError> {
        authority.verify_principal(principal)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let principal = principal.to_string();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "DELETE FROM mob_runtime_operator_grants
                     WHERE mob_id = ?1 AND principal = ?2",
                    params![mob_id.as_str(), principal],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn delete_mob_operator_grants(&self, mob_id: &MobId) -> Result<u64, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "DELETE FROM mob_runtime_operator_grants WHERE mob_id = ?1",
                    params![mob_id.as_str()],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed as u64)
        })
        .await
    }

    async fn list_member_event_cursors(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobMemberEventCursorRecord>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT record_json FROM mob_runtime_member_event_cursors
                     WHERE mob_id = ?1
                     ORDER BY agent_identity",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(params![mob_id.as_str()], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            let mut records = Vec::new();
            for row in rows {
                let bytes = row.map_err(se)?;
                records.push(decode_json(&bytes)?);
            }
            Ok(records)
        })
        .await
    }

    async fn put_member_event_cursor(
        &self,
        mob_id: &MobId,
        record: &MobMemberEventCursorRecord,
    ) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            tx.execute(
                "INSERT INTO mob_runtime_member_event_cursors (mob_id, agent_identity, record_json)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(mob_id, agent_identity) DO UPDATE SET record_json = excluded.record_json",
                params![mob_id.as_str(), record.agent_identity, encode_json(&record)?],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn delete_member_event_cursor(
        &self,
        mob_id: &MobId,
        agent_identity: &str,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let agent_identity = agent_identity.to_string();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "DELETE FROM mob_runtime_member_event_cursors
                     WHERE mob_id = ?1 AND agent_identity = ?2",
                    params![mob_id.as_str(), agent_identity],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn delete_member_event_cursors(&self, mob_id: &MobId) -> Result<u64, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "DELETE FROM mob_runtime_member_event_cursors WHERE mob_id = ?1",
                    params![mob_id.as_str()],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed as u64)
        })
        .await
    }

    async fn list_member_live_cleanup_records(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobMemberLiveCleanupRecord>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT record_json FROM mob_runtime_member_live_cleanups
                     WHERE mob_id = ?1 ORDER BY cleanup_id",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(params![mob_id.as_str()], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            let mut records = Vec::new();
            for row in rows {
                records.push(decode_json(&row.map_err(se)?)?);
            }
            Ok(records)
        })
        .await
    }

    async fn put_member_live_cleanup_record_if_absent(
        &self,
        mob_id: &MobId,
        record: &MobMemberLiveCleanupRecord,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let encoded = encode_json(&record)?;
            let changed = tx
                .execute(
                    "INSERT OR IGNORE INTO mob_runtime_member_live_cleanups
                     (mob_id, cleanup_id, record_json) VALUES (?1, ?2, ?3)",
                    params![mob_id.as_str(), record.cleanup_id.as_str(), encoded],
                )
                .map_err(se)?;
            if changed == 0 {
                let existing: Vec<u8> = tx
                    .query_row(
                        "SELECT record_json FROM mob_runtime_member_live_cleanups
                         WHERE mob_id = ?1 AND cleanup_id = ?2",
                        params![mob_id.as_str(), record.cleanup_id.as_str()],
                        |row| row.get(0),
                    )
                    .map_err(se)?;
                let existing: MobMemberLiveCleanupRecord = decode_json(&existing)?;
                if existing != record {
                    return Err(MobStoreError::Internal(format!(
                        "member-live cleanup id '{}' was reused for a conflicting record",
                        record.cleanup_id
                    )));
                }
            }
            tx.commit().map_err(se)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn delete_member_live_cleanup_record(
        &self,
        mob_id: &MobId,
        expected: &MobMemberLiveCleanupRecord,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let expected = expected.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let expected_encoded = encode_json(&expected)?;
            let changed = tx
                .execute(
                    "DELETE FROM mob_runtime_member_live_cleanups
                     WHERE mob_id = ?1 AND cleanup_id = ?2 AND record_json = ?3",
                    params![
                        mob_id.as_str(),
                        expected.cleanup_id.as_str(),
                        expected_encoded.as_slice()
                    ],
                )
                .map_err(se)?;
            if changed == 0 {
                let existing: Option<Vec<u8>> = tx
                    .query_row(
                        "SELECT record_json FROM mob_runtime_member_live_cleanups
                         WHERE mob_id = ?1 AND cleanup_id = ?2",
                        params![mob_id.as_str(), expected.cleanup_id.as_str()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(se)?;
                if existing.is_some_and(|bytes| bytes != expected_encoded) {
                    return Err(MobStoreError::Internal(format!(
                        "member-live cleanup id '{}' no longer matches its expected immutable record",
                        expected.cleanup_id
                    )));
                }
            }
            tx.commit().map_err(se)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn list_external_binding_overlays(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<ExternalBindingOverlayRecord>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT record_json
                     FROM mob_runtime_binding_overlays
                     WHERE mob_id = ?1
                     ORDER BY agent_identity, generation",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(params![mob_id.as_str()], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            let mut records = Vec::new();
            for row in rows {
                let bytes = row.map_err(se)?;
                records.push(decode_json(&bytes)?);
            }
            Ok(records)
        })
        .await
    }

    async fn put_external_binding_overlay_if_absent(
        &self,
        mob_id: &MobId,
        record: &ExternalBindingOverlayRecord,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "INSERT OR IGNORE INTO mob_runtime_binding_overlays
                     (mob_id, agent_identity, generation, record_json)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        mob_id.as_str(),
                        record.agent_identity.as_str(),
                        i64::try_from(record.generation.get()).map_err(|_| {
                            MobStoreError::Internal(format!(
                                "generation {} exceeds i64::MAX",
                                record.generation.get()
                            ))
                        })?,
                        encode_json(&record)?,
                    ],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn upsert_external_binding_overlay(
        &self,
        mob_id: &MobId,
        record: &ExternalBindingOverlayRecord,
    ) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            tx.execute(
                "INSERT INTO mob_runtime_binding_overlays
                 (mob_id, agent_identity, generation, record_json)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(mob_id, agent_identity, generation)
                 DO UPDATE SET record_json = excluded.record_json",
                params![
                    mob_id.as_str(),
                    record.agent_identity.as_str(),
                    i64::try_from(record.generation.get()).map_err(|_| {
                        MobStoreError::Internal(format!(
                            "generation {} exceeds i64::MAX",
                            record.generation.get()
                        ))
                    })?,
                    encode_json(&record)?,
                ],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn delete_external_binding_overlay(
        &self,
        mob_id: &MobId,
        agent_identity: &AgentIdentity,
        generation: Generation,
    ) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let agent_identity = agent_identity.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            tx.execute(
                "DELETE FROM mob_runtime_binding_overlays
                 WHERE mob_id = ?1 AND agent_identity = ?2 AND generation = ?3",
                params![
                    mob_id.as_str(),
                    agent_identity.as_str(),
                    i64::try_from(generation.get()).map_err(|_| {
                        MobStoreError::Internal(format!(
                            "generation {} exceeds i64::MAX",
                            generation.get()
                        ))
                    })?,
                ],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn delete_external_binding_overlays(&self, mob_id: &MobId) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            tx.execute(
                "DELETE FROM mob_runtime_binding_overlays WHERE mob_id = ?1",
                params![mob_id.as_str()],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// SqliteMobEventStore
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SqliteMobEventStore {
    path: PathBuf,
    event_bus: Arc<SqliteMobEventBus>,
}

impl std::fmt::Debug for SqliteMobEventStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteMobEventStore")
            .field("path", &self.path)
            .finish()
    }
}

const EVENT_CURSOR_KEY: &str = "next_cursor";

impl private::MobEventStoreSealed for SqliteMobEventStore {}

#[async_trait]
impl MobEventStore for SqliteMobEventStore {
    async fn append(&self, event: NewMobEvent) -> Result<MobEvent, MobStoreError> {
        validate_mob_event_write_authority(&event.kind)?;

        let path = self.path.clone();
        let stored = run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let cursor = next_event_cursor(&tx)?;
            let stored = MobEvent {
                cursor,
                timestamp: event.timestamp.unwrap_or_else(Utc::now),
                mob_id: event.mob_id,
                kind: event.kind,
            };
            let encoded = encode_stored_mob_event(&stored)
                .map_err(|e| MobStoreError::Serialization(e.to_string()))?;
            tx.execute(
                "INSERT INTO mob_events (cursor, event_json) VALUES (?1, ?2)",
                params![cursor_to_i64(cursor)?, encoded],
            )
            .map_err(se)?;
            set_next_cursor(&tx, cursor.saturating_add(1))?;
            tx.commit().map_err(se)?;
            Ok(stored)
        })
        .await?;
        self.event_bus.publish_committed(stored.clone());
        Ok(stored)
    }

    async fn append_terminal_event_if_absent(
        &self,
        event: NewMobEvent,
    ) -> Result<Option<MobEvent>, MobStoreError> {
        let Some((run_id, flow_id)) = terminal_event_identity(&event.kind) else {
            return Err(MobStoreError::Internal(
                "append_terminal_event_if_absent requires a terminal flow event".to_string(),
            ));
        };
        let run_id = run_id.clone();
        let flow_id = flow_id.clone();
        let mob_id = event.mob_id.clone();

        let path = self.path.clone();
        let stored = run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let mut stmt = tx
                .prepare("SELECT event_json FROM mob_events ORDER BY cursor")
                .map_err(se)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            for row in rows {
                let bytes = row.map_err(se)?;
                let existing = decode_stored_mob_event(&bytes)
                    .map_err(|e| MobStoreError::Serialization(e.to_string()))?;
                if existing.mob_id == mob_id
                    && terminal_event_identity(&existing.kind).is_some_and(
                        |(existing_run_id, existing_flow_id)| {
                            existing_run_id == &run_id && existing_flow_id == &flow_id
                        },
                    )
                {
                    return Ok(None);
                }
            }
            drop(stmt);

            let cursor = next_event_cursor(&tx)?;
            let stored = MobEvent {
                cursor,
                timestamp: event.timestamp.unwrap_or_else(Utc::now),
                mob_id: event.mob_id,
                kind: event.kind,
            };
            let encoded = encode_stored_mob_event(&stored)
                .map_err(|e| MobStoreError::Serialization(e.to_string()))?;
            tx.execute(
                "INSERT INTO mob_events (cursor, event_json) VALUES (?1, ?2)",
                params![cursor_to_i64(cursor)?, encoded],
            )
            .map_err(se)?;
            set_next_cursor(&tx, cursor.saturating_add(1))?;
            tx.commit().map_err(se)?;
            Ok(Some(stored))
        })
        .await?;
        if let Some(stored) = stored.as_ref() {
            self.event_bus.publish_committed(stored.clone());
        }
        Ok(stored)
    }

    async fn append_step_failed_event_if_absent(
        &self,
        event: NewMobEvent,
    ) -> Result<Option<MobEvent>, MobStoreError> {
        validate_mob_event_write_authority(&event.kind)?;
        let Some((run_id, step_id, _)) = step_failed_event_identity(&event.kind) else {
            return Err(MobStoreError::Internal(
                "append_step_failed_event_if_absent requires a StepFailed event".to_string(),
            ));
        };
        let run_id = run_id.clone();
        let step_id = step_id.clone();
        let mob_id = event.mob_id.clone();

        let path = self.path.clone();
        let stored = run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let mut stmt = tx
                .prepare("SELECT event_json FROM mob_events ORDER BY cursor")
                .map_err(se)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            let mut exact_replay = false;
            for row in rows {
                let bytes = row.map_err(se)?;
                let existing = decode_stored_mob_event(&bytes)
                    .map_err(|e| MobStoreError::Serialization(e.to_string()))?;
                if existing.mob_id != mob_id {
                    continue;
                }
                let Some((existing_run_id, existing_step_id, _)) =
                    step_failed_event_identity(&existing.kind)
                else {
                    continue;
                };
                if existing_run_id == &run_id && existing_step_id == &step_id {
                    if existing.kind != event.kind {
                        return Err(MobStoreError::Internal(format!(
                            "StepFailed event conflict for run '{run_id}' step '{step_id}'"
                        )));
                    }
                    exact_replay = true;
                }
            }
            drop(stmt);
            if exact_replay {
                return Ok(None);
            }

            let cursor = next_event_cursor(&tx)?;
            let stored = MobEvent {
                cursor,
                timestamp: event.timestamp.unwrap_or_else(Utc::now),
                mob_id: event.mob_id,
                kind: event.kind,
            };
            let encoded = encode_stored_mob_event(&stored)
                .map_err(|e| MobStoreError::Serialization(e.to_string()))?;
            tx.execute(
                "INSERT INTO mob_events (cursor, event_json) VALUES (?1, ?2)",
                params![cursor_to_i64(cursor)?, encoded],
            )
            .map_err(se)?;
            set_next_cursor(&tx, cursor.saturating_add(1))?;
            tx.commit().map_err(se)?;
            Ok(Some(stored))
        })
        .await?;
        if let Some(stored) = stored.as_ref() {
            self.event_bus.publish_committed(stored.clone());
        }
        Ok(stored)
    }

    async fn append_batch(&self, batch: Vec<NewMobEvent>) -> Result<Vec<MobEvent>, MobStoreError> {
        for event in &batch {
            validate_mob_event_write_authority(&event.kind)?;
        }

        let path = self.path.clone();
        let stored = run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let mut cursor = get_next_cursor(&tx)?;
            let mut results = Vec::with_capacity(batch.len());
            for event in batch {
                let stored = MobEvent {
                    cursor,
                    timestamp: event.timestamp.unwrap_or_else(Utc::now),
                    mob_id: event.mob_id,
                    kind: event.kind,
                };
                let encoded = encode_stored_mob_event(&stored)
                    .map_err(|e| MobStoreError::Serialization(e.to_string()))?;
                tx.execute(
                    "INSERT INTO mob_events (cursor, event_json) VALUES (?1, ?2)",
                    params![cursor_to_i64(cursor)?, encoded],
                )
                .map_err(se)?;
                results.push(stored);
                cursor = cursor.saturating_add(1);
            }
            set_next_cursor(&tx, cursor)?;
            tx.commit().map_err(se)?;
            Ok(results)
        })
        .await?;
        self.event_bus.publish_committed_batch(&stored);
        Ok(stored)
    }

    async fn poll(&self, after_cursor: u64, limit: usize) -> Result<Vec<MobEvent>, MobStoreError> {
        let path = self.path.clone();
        run_sqlite_task(move || poll_events_sync(&path, after_cursor, limit)).await
    }

    async fn replay_all(&self) -> Result<Vec<MobEvent>, MobStoreError> {
        let path = self.path.clone();
        run_sqlite_task(move || {
            let conn = open_existing_read_connection(&path)?;
            let mut stmt = conn
                .prepare("SELECT event_json FROM mob_events ORDER BY cursor")
                .map_err(se)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            let mut result = Vec::new();
            for row in rows {
                let bytes = row.map_err(se)?;
                result.push(
                    decode_stored_mob_event(&bytes)
                        .map_err(|e| MobStoreError::Serialization(e.to_string()))?,
                );
            }
            Ok(result)
        })
        .await
    }

    async fn latest_cursor(&self) -> Result<u64, MobStoreError> {
        let path = self.path.clone();
        run_sqlite_task(move || latest_event_cursor_sync(&path)).await
    }

    fn subscribe(&self) -> Result<super::MobEventReceiver, MobStoreError> {
        Ok(self.event_bus.subscribe())
    }

    async fn clear(&self) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            tx.execute("DELETE FROM mob_events", []).map_err(se)?;
            tx.execute(
                "INSERT OR REPLACE INTO mob_event_meta (key, value) VALUES (?1, ?2)",
                params![EVENT_CURSOR_KEY, 1i64],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn prune(&self, older_than: DateTime<Utc>) -> Result<u64, MobStoreError> {
        let path = self.path.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;

            // Read all events, delete those older than the threshold.
            // Events store timestamp inside JSON, so we must deserialize to check.
            let mut stmt = tx
                .prepare("SELECT cursor, event_json FROM mob_events ORDER BY cursor")
                .map_err(se)?;
            let rows: Vec<(i64, Vec<u8>)> = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(se)?
                .collect::<Result<_, _>>()
                .map_err(se)?;
            drop(stmt);

            let mut removed = 0u64;
            for (cursor_val, bytes) in rows {
                let event: MobEvent = decode_stored_mob_event(&bytes)
                    .map_err(|e| MobStoreError::Serialization(e.to_string()))?;
                if event.timestamp < older_than {
                    tx.execute(
                        "DELETE FROM mob_events WHERE cursor = ?1",
                        params![cursor_val],
                    )
                    .map_err(se)?;
                    removed = removed.saturating_add(1);
                }
            }
            tx.commit().map_err(se)?;
            Ok(removed)
        })
        .await
    }
}

fn get_next_cursor(conn: &Connection) -> Result<u64, MobStoreError> {
    let result: Option<i64> = conn
        .query_row(
            "SELECT value FROM mob_event_meta WHERE key = ?1",
            params![EVENT_CURSOR_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(se)?;
    Ok(result.map_or(1, i64_to_cursor))
}

fn next_event_cursor(tx: &Transaction<'_>) -> Result<u64, MobStoreError> {
    get_next_cursor(tx)
}

fn set_next_cursor(conn: &Connection, value: u64) -> Result<(), MobStoreError> {
    conn.execute(
        "INSERT OR REPLACE INTO mob_event_meta (key, value) VALUES (?1, ?2)",
        params![EVENT_CURSOR_KEY, cursor_to_i64(value)?],
    )
    .map_err(se)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// SqliteMobRunStore
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SqliteMobRunStore {
    path: PathBuf,
}

#[derive(Debug, Clone, Copy)]
enum MissingRunCasBehavior {
    ReturnFalse,
    NotFound,
}

impl std::fmt::Debug for SqliteMobRunStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteMobRunStore")
            .field("path", &self.path)
            .finish()
    }
}

impl SqliteMobRunStore {
    async fn update_run_with_authority_if<F>(
        &self,
        run_id: &RunId,
        missing_behavior: MissingRunCasBehavior,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
        update: F,
    ) -> Result<bool, MobStoreError>
    where
        F: FnOnce(&mut MobRun) -> Result<bool, MobStoreError> + Send + 'static,
    {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes = load_run_bytes(&tx, &key)?;
            let Some(bytes) = bytes else {
                return match missing_behavior {
                    MissingRunCasBehavior::ReturnFalse => Ok(false),
                    MissingRunCasBehavior::NotFound => {
                        Err(MobStoreError::NotFound(format!("run not found: {run_id}")))
                    }
                };
            };
            let mut run: MobRun = decode_json(&bytes)?;
            if !update(&mut run)? {
                return Ok(false);
            }
            run.append_flow_authority_inputs(authority_inputs)
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            run.validate_flow_authority_projection()
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            write_run_json(&tx, &key, &run)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }
}

#[async_trait]
impl MobRunStore for SqliteMobRunStore {
    async fn create_run(&self, run: MobRun) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        let key = run.run_id.to_string();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;

            let exists: bool = tx
                .query_row(
                    "SELECT 1 FROM mob_runs WHERE run_id = ?1",
                    params![key],
                    |_| Ok(true),
                )
                .optional()
                .map_err(se)?
                .unwrap_or(false);
            if exists {
                return Err(MobStoreError::Internal(format!(
                    "run already exists: {}",
                    run.run_id
                )));
            }
            run.validate_flow_authority_projection()
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;

            let encoded = encode_json(&run)?;
            tx.execute(
                "INSERT INTO mob_runs (run_id, run_json) VALUES (?1, ?2)",
                params![key, encoded],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn get_run(&self, run_id: &RunId) -> Result<Option<MobRun>, MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        run_sqlite_task(move || {
            let conn = open_existing_read_connection(&path)?;
            let bytes: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT run_json FROM mob_runs WHERE run_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            match bytes {
                Some(b) => Ok(Some(decode_json(&b)?)),
                None => Ok(None),
            }
        })
        .await
    }

    async fn list_runs(
        &self,
        mob_id: &MobId,
        flow_id: Option<&FlowId>,
    ) -> Result<Vec<MobRun>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let flow_id = flow_id.cloned();
        run_sqlite_task(move || {
            let conn = open_existing_read_connection(&path)?;
            let mut stmt = conn.prepare("SELECT run_json FROM mob_runs").map_err(se)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            let mut runs = Vec::new();
            for row in rows {
                let bytes = row.map_err(se)?;
                let run: MobRun = decode_json(&bytes)?;
                if run.mob_id == mob_id && flow_id.as_ref().is_none_or(|fid| run.flow_id == *fid) {
                    runs.push(run);
                }
            }
            Ok(runs)
        })
        .await
    }

    async fn put_remote_turn_intent(
        &self,
        run_id: &RunId,
        intent: &MobRunRemoteTurnIntent,
    ) -> Result<bool, MobStoreError> {
        if &intent.obligation.run_id != run_id || intent.obligation.dispatch_sequence == 0 {
            return Err(MobStoreError::Internal(format!(
                "remote-turn intent does not match run '{run_id}' or has sequence zero"
            )));
        }
        let path = self.path.clone();
        let run_key = run_id.to_string();
        let sequence = u64_to_sql_key(intent.obligation.dispatch_sequence);
        let intent = intent.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let run_json: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT run_json FROM mob_runs WHERE run_id = ?1",
                    params![run_key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let Some(run_json) = run_json else {
                return Err(MobStoreError::NotFound(format!(
                    "run not found: {}",
                    intent.obligation.run_id
                )));
            };
            let run: MobRun = decode_json(&run_json)?;
            intent
                .validate_for(&run.run_id, &run.mob_id)
                .map_err(MobStoreError::Internal)?;
            let existing: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT intent_json FROM mob_run_remote_turn_intents \
                     WHERE run_id = ?1 AND dispatch_sequence = ?2",
                    params![run_key, sequence],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            if let Some(bytes) = existing {
                let existing: MobRunRemoteTurnIntent = decode_json(&bytes)?;
                if existing != intent {
                    return Err(MobStoreError::Internal(format!(
                        "remote-turn intent sequence {} conflicts for run '{}'",
                        intent.obligation.dispatch_sequence, intent.obligation.run_id
                    )));
                }
                return Ok(false);
            }
            tx.execute(
                "INSERT INTO mob_run_remote_turn_intents \
                 (run_id, dispatch_sequence, intent_json) VALUES (?1, ?2, ?3)",
                params![run_key, sequence, encode_json(&intent)?],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }

    async fn delete_remote_turn_intent(
        &self,
        run_id: &RunId,
        dispatch_sequence: u64,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let run_key = run_id.to_string();
        let sequence = u64_to_sql_key(dispatch_sequence);
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            Ok(conn
                .execute(
                    "DELETE FROM mob_run_remote_turn_intents \
                     WHERE run_id = ?1 AND dispatch_sequence = ?2",
                    params![run_key, sequence],
                )
                .map_err(se)?
                > 0)
        })
        .await
    }

    async fn list_remote_turn_intents(
        &self,
        run_id: &RunId,
    ) -> Result<Vec<MobRunRemoteTurnIntent>, MobStoreError> {
        let path = self.path.clone();
        let run_key = run_id.to_string();
        run_sqlite_task(move || {
            let conn = open_existing_read_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT intent_json FROM mob_run_remote_turn_intents \
                     WHERE run_id = ?1 ORDER BY dispatch_sequence ASC",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(params![run_key], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            let mut intents = Vec::new();
            for row in rows {
                intents.push(decode_json(&row.map_err(se)?)?);
            }
            Ok(intents)
        })
        .await
    }

    async fn put_remote_turn_receipt(
        &self,
        run_id: &RunId,
        receipt: &MobRunRemoteTurnReceipt,
    ) -> Result<bool, MobStoreError> {
        if &receipt.obligation.run_id != run_id || receipt.obligation.dispatch_sequence == 0 {
            return Err(MobStoreError::Internal(format!(
                "remote-turn receipt does not match run '{run_id}' or has sequence zero"
            )));
        }
        let path = self.path.clone();
        let run_key = run_id.to_string();
        let sequence = u64_to_sql_key(receipt.obligation.dispatch_sequence);
        let receipt = receipt.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let run_json: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT run_json FROM mob_runs WHERE run_id = ?1",
                    params![run_key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let Some(run_json) = run_json else {
                return Err(MobStoreError::NotFound(format!(
                    "run not found: {}",
                    receipt.obligation.run_id
                )));
            };
            let run: MobRun = decode_json(&run_json)?;
            let intent_json: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT intent_json FROM mob_run_remote_turn_intents \
                     WHERE run_id = ?1 AND dispatch_sequence = ?2",
                    params![run_key, sequence],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let intent: MobRunRemoteTurnIntent = intent_json
                .map(|bytes| decode_json(&bytes))
                .transpose()?
                .ok_or_else(|| {
                    MobStoreError::Internal(
                        "remote-turn receipt has no exact durable intent".to_string(),
                    )
                })?;
            receipt
                .validate_for(&run.run_id, &run.mob_id, &intent)
                .map_err(MobStoreError::Internal)?;
            let existing: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT receipt_json FROM mob_run_remote_turn_receipts \
                     WHERE run_id = ?1 AND dispatch_sequence = ?2",
                    params![run_key, sequence],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            if let Some(bytes) = existing {
                let existing: MobRunRemoteTurnReceipt = decode_json(&bytes)?;
                if existing != receipt {
                    return Err(MobStoreError::Internal(format!(
                        "remote-turn receipt sequence {} conflicts for run '{}'",
                        receipt.obligation.dispatch_sequence, receipt.obligation.run_id
                    )));
                }
                return Ok(false);
            }
            let bytes = encode_json(&receipt)?;
            tx.execute(
                "INSERT INTO mob_run_remote_turn_receipts \
                 (run_id, dispatch_sequence, receipt_json) VALUES (?1, ?2, ?3)",
                params![run_key, sequence, bytes],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }

    async fn list_remote_turn_receipts(
        &self,
        run_id: &RunId,
    ) -> Result<Vec<MobRunRemoteTurnReceipt>, MobStoreError> {
        let path = self.path.clone();
        let run_key = run_id.to_string();
        run_sqlite_task(move || {
            let conn = open_existing_read_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT receipt_json FROM mob_run_remote_turn_receipts \
                     WHERE run_id = ?1 ORDER BY dispatch_sequence ASC",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(params![run_key], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            let mut receipts = Vec::new();
            for row in rows {
                receipts.push(decode_json(&row.map_err(se)?)?);
            }
            Ok(receipts)
        })
        .await
    }

    async fn delete_remote_turn_receipt(
        &self,
        run_id: &RunId,
        dispatch_sequence: u64,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let run_key = run_id.to_string();
        let sequence = u64_to_sql_key(dispatch_sequence);
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            Ok(conn
                .execute(
                    "DELETE FROM mob_run_remote_turn_receipts \
                     WHERE run_id = ?1 AND dispatch_sequence = ?2",
                    params![run_key, sequence],
                )
                .map_err(se)?
                > 0)
        })
        .await
    }

    async fn cas_flow_state_with_authority(
        &self,
        run_id: &RunId,
        expected: &flow_run::State,
        next: &flow_run::State,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        let expected = expected.clone();
        let next = next.clone();
        self.update_run_with_authority_if(
            run_id,
            MissingRunCasBehavior::ReturnFalse,
            authority_inputs,
            move |run| {
                if run.flow_state != expected {
                    return Ok(false);
                }
                run.flow_state = next;
                Ok(true)
            },
        )
        .await
    }

    async fn cas_run_snapshot_with_authority(
        &self,
        run_id: &RunId,
        expected_status: MobRunStatus,
        expected_flow_state: &flow_run::State,
        next_status: MobRunStatus,
        next_flow_state: &flow_run::State,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        let expected_flow_state = expected_flow_state.clone();
        let next_flow_state = next_flow_state.clone();
        let terminality_run_id = run_id.clone();
        self.update_run_with_authority_if(
            run_id,
            MissingRunCasBehavior::ReturnFalse,
            authority_inputs,
            move |run| {
                let current_terminal =
                    mob_machine_run_status_is_terminal(&terminality_run_id, &run.status)
                        .map_err(|error| MobStoreError::Internal(error.to_string()))?;
                if run.status != expected_status
                    || current_terminal
                    || run.flow_state != expected_flow_state
                {
                    return Ok(false);
                }
                let terminal =
                    mob_machine_run_status_is_terminal(&terminality_run_id, &next_status)
                        .map_err(|error| MobStoreError::Internal(error.to_string()))?;
                run.status = next_status;
                run.flow_state = next_flow_state;
                if terminal && run.completed_at.is_none() {
                    run.completed_at = Some(Utc::now());
                }
                Ok(true)
            },
        )
        .await
    }

    async fn cas_run_snapshot_and_append_terminal_event_with_authority(
        &self,
        run_id: &RunId,
        expected_status: MobRunStatus,
        expected_flow_state: &flow_run::State,
        next_status: MobRunStatus,
        next_flow_state: &flow_run::State,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
        _events: &dyn MobEventStore,
        terminal_event: NewMobEvent,
    ) -> Result<Option<MobEvent>, MobStoreError> {
        // The mob run table and the mob event log live in the same SQLite
        // database file, so the terminal snapshot and the terminal event are
        // committed in ONE transaction — no divergence window between terminal
        // run-status truth and terminal-event truth. The `_events` handle is
        // unused: any event store for this storage bundle shares this database
        // path, and the committed event is broadcast on the per-path bus.
        validate_mob_event_write_authority(&terminal_event.kind)?;
        if terminal_event_identity(&terminal_event.kind).is_none() {
            return Err(MobStoreError::Internal(
                "cas_run_snapshot_and_append_terminal_event_with_authority requires a terminal flow event".to_string(),
            ));
        }
        let path = self.path.clone();
        let key = run_id.to_string();
        let terminality_run_id = run_id.clone();
        let expected_flow_state = expected_flow_state.clone();
        let next_flow_state = next_flow_state.clone();
        let event_bus = sqlite_event_bus_for_path(self.path.clone())?;
        let stored = run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes = load_run_bytes(&tx, &key)?;
            let Some(bytes) = bytes else {
                return Ok(None);
            };
            let mut run: MobRun = decode_json(&bytes)?;
            let current_terminal =
                mob_machine_run_status_is_terminal(&terminality_run_id, &run.status)
                    .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            if run.status != expected_status
                || current_terminal
                || run.flow_state != expected_flow_state
            {
                return Ok(None);
            }
            let terminal = mob_machine_run_status_is_terminal(&terminality_run_id, &next_status)
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            run.status = next_status;
            run.flow_state = next_flow_state;
            if terminal && run.completed_at.is_none() {
                run.completed_at = Some(Utc::now());
            }
            run.append_flow_authority_inputs(authority_inputs)
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            run.validate_flow_authority_projection()
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            write_run_json(&tx, &key, &run)?;

            let cursor = next_event_cursor(&tx)?;
            let stored = MobEvent {
                cursor,
                timestamp: terminal_event.timestamp.unwrap_or_else(Utc::now),
                mob_id: terminal_event.mob_id,
                kind: terminal_event.kind,
            };
            let encoded = encode_stored_mob_event(&stored)
                .map_err(|e| MobStoreError::Serialization(e.to_string()))?;
            tx.execute(
                "INSERT INTO mob_events (cursor, event_json) VALUES (?1, ?2)",
                params![cursor_to_i64(cursor)?, encoded],
            )
            .map_err(se)?;
            set_next_cursor(&tx, cursor.saturating_add(1))?;
            tx.commit().map_err(se)?;
            Ok(Some(stored))
        })
        .await?;
        if let Some(stored) = stored.as_ref() {
            event_bus.publish_committed(stored.clone());
        }
        Ok(stored)
    }

    async fn append_step_entry_with_authority(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT run_json FROM mob_runs WHERE run_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let Some(bytes) = bytes else {
                return Err(MobStoreError::NotFound(format!("run not found: {run_id}")));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            authority
                .validate_step_entry(&run, &entry)
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            run.step_ledger.push(entry);
            run.validate_flow_authority_projection()
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            let encoded = encode_json(&run)?;
            tx.execute(
                "UPDATE mob_runs SET run_json = ?1 WHERE run_id = ?2",
                params![encoded, key],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn append_step_entry_if_absent_with_authority(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT run_json FROM mob_runs WHERE run_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let Some(bytes) = bytes else {
                return Err(MobStoreError::NotFound(format!("run not found: {run_id}")));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            authority
                .validate_step_entry(&run, &entry)
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            let is_duplicate = run.step_ledger.iter().any(|existing| {
                existing.step_id == entry.step_id
                    && existing.agent_identity == entry.agent_identity
                    && existing.status == entry.status
            });
            if is_duplicate {
                return Ok(false);
            }
            run.step_ledger.push(entry);
            run.validate_flow_authority_projection()
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            let encoded = encode_json(&run)?;
            tx.execute(
                "UPDATE mob_runs SET run_json = ?1 WHERE run_id = ?2",
                params![encoded, key],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }

    async fn append_failure_entry_with_authority(
        &self,
        run_id: &RunId,
        entry: FailureLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT run_json FROM mob_runs WHERE run_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let Some(bytes) = bytes else {
                return Err(MobStoreError::NotFound(format!("run not found: {run_id}")));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            authority
                .validate_failure_entry(&run, &entry)
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            run.failure_ledger.push(entry);
            run.validate_flow_authority_projection()
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            let encoded = encode_json(&run)?;
            tx.execute(
                "UPDATE mob_runs SET run_json = ?1 WHERE run_id = ?2",
                params![encoded, key],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn append_failure_entry_if_absent_with_authority(
        &self,
        run_id: &RunId,
        entry: FailureLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT run_json FROM mob_runs WHERE run_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let Some(bytes) = bytes else {
                return Err(MobStoreError::NotFound(format!("run not found: {run_id}")));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            authority
                .validate_failure_entry(&run, &entry)
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            if let Some(existing) = run
                .failure_ledger
                .iter()
                .find(|existing| existing.step_id == entry.step_id)
            {
                if existing.reason == entry.reason
                    && existing.error_report == entry.error_report
                    && existing.error == entry.error
                {
                    return Ok(false);
                }
                return Err(MobStoreError::Internal(format!(
                    "failure ledger conflict for run '{run_id}' step '{}'",
                    entry.step_id
                )));
            }
            run.failure_ledger.push(entry);
            run.validate_flow_authority_projection()
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            let encoded = encode_json(&run)?;
            tx.execute(
                "UPDATE mob_runs SET run_json = ?1 WHERE run_id = ?2",
                params![encoded, key],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }

    async fn cas_frame_state_with_authority(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        expected: Option<&FrameSnapshot>,
        next: FrameSnapshot,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        let frame_id = frame_id.clone();
        let expected = expected.cloned();
        self.update_run_with_authority_if(
            run_id,
            MissingRunCasBehavior::NotFound,
            authority_inputs,
            move |run| {
                let current = run.frames.get(&frame_id);
                let matches = match (expected.as_ref(), current) {
                    (None, None) => true,
                    (Some(exp), Some(cur)) => exp == cur,
                    _ => false,
                };
                if !matches {
                    return Ok(false);
                }
                run.frames.insert(frame_id, next);
                Ok(true)
            },
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_grant_node_slot_with_authority(
        &self,
        run_id: &RunId,
        expected_run_state: &flow_run::State,
        next_run_state: flow_run::State,
        frame_id: &FrameId,
        expected_frame: &FrameSnapshot,
        next_frame: FrameSnapshot,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        let expected_run_state = expected_run_state.clone();
        let frame_id = frame_id.clone();
        let expected_frame = expected_frame.clone();
        self.update_run_with_authority_if(
            run_id,
            MissingRunCasBehavior::NotFound,
            authority_inputs,
            move |run| {
                if run.flow_state != expected_run_state {
                    return Ok(false);
                }
                if run.frames.get(&frame_id) != Some(&expected_frame) {
                    return Ok(false);
                }
                run.flow_state = next_run_state;
                run.frames.insert(frame_id, next_frame);
                Ok(true)
            },
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_complete_step_and_record_output_with_authority(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        expected_frame: &FrameSnapshot,
        next_frame: FrameSnapshot,
        step_output_key: String,
        step_output: serde_json::Value,
        loop_context: Option<(&LoopId, u64)>,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        let frame_id = frame_id.clone();
        let expected_frame = expected_frame.clone();
        let loop_context = loop_context.map(|(loop_id, iteration)| (loop_id.clone(), iteration));
        self.update_run_with_authority_if(
            run_id,
            MissingRunCasBehavior::NotFound,
            authority_inputs,
            move |run| {
                if run.frames.get(&frame_id) != Some(&expected_frame) {
                    return Ok(false);
                }
                run.frames.insert(frame_id, next_frame);
                match loop_context {
                    None => {
                        run.root_step_outputs
                            .insert(StepId::from(step_output_key.as_str()), step_output);
                    }
                    Some((loop_id, iteration)) => {
                        let iteration_index = usize::try_from(iteration).map_err(|_| {
                            MobStoreError::Internal(format!(
                                "loop iteration index {iteration} exceeds usize::MAX on this target"
                            ))
                        })?;
                        let outputs = run.loop_iteration_outputs.entry(loop_id).or_default();
                        while outputs.len() <= iteration_index {
                            outputs.push(indexmap::IndexMap::new());
                        }
                        outputs[iteration_index]
                            .insert(StepId::from(step_output_key.as_str()), step_output);
                    }
                }
                Ok(true)
            },
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_start_loop_with_authority(
        &self,
        run_id: &RunId,
        loop_instance_id: &LoopInstanceId,
        expected_run_state: &flow_run::State,
        next_run_state: flow_run::State,
        frame_id: &FrameId,
        expected_frame: &FrameSnapshot,
        next_frame: FrameSnapshot,
        initial_loop: LoopSnapshot,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        let loop_instance_id = loop_instance_id.clone();
        let expected_run_state = expected_run_state.clone();
        let frame_id = frame_id.clone();
        let expected_frame = expected_frame.clone();
        self.update_run_with_authority_if(
            run_id,
            MissingRunCasBehavior::NotFound,
            authority_inputs,
            move |run| {
                if run.flow_state != expected_run_state {
                    return Ok(false);
                }
                if run.frames.get(&frame_id) != Some(&expected_frame) {
                    return Ok(false);
                }
                if run.loops.contains_key(&loop_instance_id) {
                    return Ok(false);
                }
                run.flow_state = next_run_state;
                run.frames.insert(frame_id, next_frame);
                run.loops.insert(loop_instance_id, initial_loop);
                Ok(true)
            },
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_loop_request_body_frame_with_authority(
        &self,
        run_id: &RunId,
        loop_instance_id: &LoopInstanceId,
        expected_loop: &LoopSnapshot,
        next_loop: LoopSnapshot,
        expected_run_state: &flow_run::State,
        next_run_state: flow_run::State,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        let loop_instance_id = loop_instance_id.clone();
        let expected_loop = expected_loop.clone();
        let expected_run_state = expected_run_state.clone();
        self.update_run_with_authority_if(
            run_id,
            MissingRunCasBehavior::NotFound,
            authority_inputs,
            move |run| {
                if run.flow_state != expected_run_state {
                    return Ok(false);
                }
                if run.loops.get(&loop_instance_id) != Some(&expected_loop) {
                    return Ok(false);
                }
                run.flow_state = next_run_state;
                run.loops.insert(loop_instance_id, next_loop);
                Ok(true)
            },
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_grant_body_frame_start_with_authority(
        &self,
        run_id: &RunId,
        loop_instance_id: &LoopInstanceId,
        expected_loop: &LoopSnapshot,
        next_loop: LoopSnapshot,
        frame_id: &FrameId,
        initial_frame: FrameSnapshot,
        ledger_entry: LoopIterationLedgerEntry,
        expected_run_state: &flow_run::State,
        next_run_state: flow_run::State,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        let loop_instance_id = loop_instance_id.clone();
        let expected_loop = expected_loop.clone();
        let frame_id = frame_id.clone();
        let expected_run_state = expected_run_state.clone();
        self.update_run_with_authority_if(
            run_id,
            MissingRunCasBehavior::NotFound,
            authority_inputs,
            move |run| {
                if run.flow_state != expected_run_state {
                    return Ok(false);
                }
                if run.loops.get(&loop_instance_id) != Some(&expected_loop) {
                    return Ok(false);
                }
                if run.frames.contains_key(&frame_id) {
                    return Ok(false);
                }
                run.flow_state = next_run_state;
                run.loops.insert(loop_instance_id, next_loop);
                run.frames.insert(frame_id, initial_frame);
                append_loop_iteration_ledger_if_absent(run, ledger_entry);
                Ok(true)
            },
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_complete_body_frame_with_authority(
        &self,
        run_id: &RunId,
        loop_instance_id: &LoopInstanceId,
        expected_loop: &LoopSnapshot,
        next_loop: LoopSnapshot,
        frame_id: &FrameId,
        expected_frame: &FrameSnapshot,
        next_frame: FrameSnapshot,
        expected_run_state: &flow_run::State,
        next_run_state: flow_run::State,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        let loop_instance_id = loop_instance_id.clone();
        let expected_loop = expected_loop.clone();
        let frame_id = frame_id.clone();
        let expected_frame = expected_frame.clone();
        let expected_run_state = expected_run_state.clone();
        self.update_run_with_authority_if(
            run_id,
            MissingRunCasBehavior::NotFound,
            authority_inputs,
            move |run| {
                if run.flow_state != expected_run_state {
                    return Ok(false);
                }
                if run.loops.get(&loop_instance_id) != Some(&expected_loop) {
                    return Ok(false);
                }
                if run.frames.get(&frame_id) != Some(&expected_frame) {
                    return Ok(false);
                }
                run.flow_state = next_run_state;
                run.loops.insert(loop_instance_id, next_loop);
                run.frames.insert(frame_id, next_frame);
                Ok(true)
            },
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_complete_loop_with_authority(
        &self,
        run_id: &RunId,
        loop_instance_id: &LoopInstanceId,
        expected_loop: &LoopSnapshot,
        next_loop: LoopSnapshot,
        frame_id: &FrameId,
        expected_frame: &FrameSnapshot,
        next_frame: FrameSnapshot,
        expected_run_state: &flow_run::State,
        next_run_state: flow_run::State,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        let loop_instance_id = loop_instance_id.clone();
        let expected_loop = expected_loop.clone();
        let frame_id = frame_id.clone();
        let expected_frame = expected_frame.clone();
        let expected_run_state = expected_run_state.clone();
        self.update_run_with_authority_if(
            run_id,
            MissingRunCasBehavior::NotFound,
            authority_inputs,
            move |run| {
                if run.flow_state != expected_run_state {
                    return Ok(false);
                }
                if run.loops.get(&loop_instance_id) != Some(&expected_loop) {
                    return Ok(false);
                }
                if run.frames.get(&frame_id) != Some(&expected_frame) {
                    return Ok(false);
                }
                run.flow_state = next_run_state;
                run.loops.insert(loop_instance_id, next_loop);
                run.frames.insert(frame_id, next_frame);
                Ok(true)
            },
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// SqliteMobSpecStore
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SqliteMobSpecStore {
    path: PathBuf,
}

impl std::fmt::Debug for SqliteMobSpecStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteMobSpecStore")
            .field("path", &self.path)
            .finish()
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredSpec {
    definition: MobDefinition,
    revision: u64,
}

#[async_trait]
impl MobSpecStore for SqliteMobSpecStore {
    async fn put_spec(
        &self,
        mob_id: &MobId,
        definition: &MobDefinition,
        revision: Option<u64>,
    ) -> Result<u64, MobStoreError> {
        let path = self.path.clone();
        let key = mob_id.to_string();
        let mob_id = mob_id.clone();
        let definition = definition.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;

            let current: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT spec_json FROM mob_specs WHERE mob_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let current_revision = match current {
                Some(bytes) => decode_json::<StoredSpec>(&bytes)?.revision,
                None => 0,
            };

            if let Some(expected) = revision
                && expected != current_revision
            {
                return Err(MobStoreError::SpecRevisionConflict {
                    mob_id,
                    expected: revision,
                    actual: current_revision,
                });
            }

            let next_revision = current_revision + 1;
            let payload = StoredSpec {
                definition,
                revision: next_revision,
            };
            let encoded = encode_json(&payload)?;
            tx.execute(
                "INSERT OR REPLACE INTO mob_specs (mob_id, spec_json) VALUES (?1, ?2)",
                params![key, encoded],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(next_revision)
        })
        .await
    }

    async fn get_spec(
        &self,
        mob_id: &MobId,
    ) -> Result<Option<(MobDefinition, u64)>, MobStoreError> {
        let path = self.path.clone();
        let key = mob_id.to_string();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let bytes: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT spec_json FROM mob_specs WHERE mob_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            match bytes {
                Some(b) => {
                    let stored: StoredSpec = decode_json(&b)?;
                    Ok(Some((stored.definition, stored.revision)))
                }
                None => Ok(None),
            }
        })
        .await
    }

    async fn list_specs(&self) -> Result<Vec<MobId>, MobStoreError> {
        let path = self.path.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn.prepare("SELECT mob_id FROM mob_specs").map_err(se)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(se)?;
            let mut result = Vec::new();
            for row in rows {
                let id = row.map_err(se)?;
                result.push(MobId::from(id));
            }
            Ok(result)
        })
        .await
    }

    async fn delete_spec(
        &self,
        mob_id: &MobId,
        revision: Option<u64>,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let key = mob_id.to_string();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;

            let bytes: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT spec_json FROM mob_specs WHERE mob_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let Some(bytes) = bytes else {
                return Ok(false);
            };
            let stored: StoredSpec = decode_json(&bytes)?;
            if let Some(expected) = revision
                && expected != stored.revision
            {
                return Ok(false);
            }

            tx.execute("DELETE FROM mob_specs WHERE mob_id = ?1", params![key])
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// SqliteRealmProfileStore
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SqliteRealmProfileStore {
    path: PathBuf,
}

impl SqliteRealmProfileStore {
    /// Open a standalone realm profile store at the given database path.
    ///
    /// Creates the parent directory and initializes the schema if needed.
    pub fn open(db_path: &std::path::Path) -> Result<Self, MobStoreError> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                MobStoreError::Internal(format!(
                    "failed to create realm profile store directory: {e}"
                ))
            })?;
        }
        let conn = rusqlite::Connection::open(db_path).map_err(|e| {
            MobStoreError::Internal(format!("failed to open realm profile store: {e}"))
        })?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000; PRAGMA synchronous=FULL;",
        )
        .map_err(se)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS realm_profiles (
                name TEXT PRIMARY KEY,
                profile_json BLOB NOT NULL,
                revision INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
        )
        .map_err(se)?;
        Ok(Self {
            path: db_path.to_path_buf(),
        })
    }
}

impl std::fmt::Debug for SqliteRealmProfileStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteRealmProfileStore")
            .field("path", &self.path)
            .finish()
    }
}

#[async_trait]
impl RealmProfileStore for SqliteRealmProfileStore {
    async fn create(
        &self,
        name: &str,
        profile: &Profile,
    ) -> Result<StoredRealmProfile, MobStoreError> {
        let path = self.path.clone();
        let name = name.to_string();
        let profile = profile.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;

            let exists: bool = tx
                .query_row(
                    "SELECT 1 FROM realm_profiles WHERE name = ?1",
                    params![name],
                    |_| Ok(true),
                )
                .optional()
                .map_err(se)?
                .unwrap_or(false);

            if exists {
                return Err(MobStoreError::CasConflict(format!(
                    "realm profile already exists: {name}"
                )));
            }

            let now = Utc::now();
            let now_str = now.to_rfc3339();
            let profile_json = encode_json(&profile)?;

            tx.execute(
                "INSERT INTO realm_profiles (name, profile_json, revision, created_at, updated_at) VALUES (?1, ?2, 1, ?3, ?4)",
                params![name, profile_json, now_str, now_str],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;

            Ok(StoredRealmProfile {
                name,
                profile,
                revision: 1,
                created_at: now,
                updated_at: now,
            })
        })
        .await
    }

    async fn get(&self, name: &str) -> Result<Option<StoredRealmProfile>, MobStoreError> {
        let path = self.path.clone();
        let name = name.to_string();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let row: Option<(Vec<u8>, i64, String, String)> = conn
                .query_row(
                    "SELECT profile_json, revision, created_at, updated_at FROM realm_profiles WHERE name = ?1",
                    params![name],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(se)?;

            match row {
                Some((bytes, revision, created_at_str, updated_at_str)) => {
                    let profile: Profile = decode_json(&bytes)?;
                    let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
                        .map_err(|e| MobStoreError::Serialization(e.to_string()))?
                        .with_timezone(&Utc);
                    let updated_at = chrono::DateTime::parse_from_rfc3339(&updated_at_str)
                        .map_err(|e| MobStoreError::Serialization(e.to_string()))?
                        .with_timezone(&Utc);
                    Ok(Some(StoredRealmProfile {
                        name,
                        profile,
                        revision: revision as u64,
                        created_at,
                        updated_at,
                    }))
                }
                None => Ok(None),
            }
        })
        .await
    }

    async fn list(&self) -> Result<Vec<StoredRealmProfile>, MobStoreError> {
        let path = self.path.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare("SELECT name, profile_json, revision, created_at, updated_at FROM realm_profiles ORDER BY name")
                .map_err(se)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })
                .map_err(se)?;

            let mut result = Vec::new();
            for row in rows {
                let (name, bytes, revision, created_at_str, updated_at_str) = row.map_err(se)?;
                let profile: Profile = decode_json(&bytes)?;
                let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
                    .map_err(|e| MobStoreError::Serialization(e.to_string()))?
                    .with_timezone(&Utc);
                let updated_at = chrono::DateTime::parse_from_rfc3339(&updated_at_str)
                    .map_err(|e| MobStoreError::Serialization(e.to_string()))?
                    .with_timezone(&Utc);
                result.push(StoredRealmProfile {
                    name,
                    profile,
                    revision: revision as u64,
                    created_at,
                    updated_at,
                });
            }
            Ok(result)
        })
        .await
    }

    async fn update(
        &self,
        name: &str,
        profile: &Profile,
        expected_revision: u64,
    ) -> Result<StoredRealmProfile, MobStoreError> {
        let path = self.path.clone();
        let name = name.to_string();
        let profile = profile.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;

            let row: Option<(i64, String)> = tx
                .query_row(
                    "SELECT revision, created_at FROM realm_profiles WHERE name = ?1",
                    params![name],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(se)?;

            let (current_revision, created_at_str) = row.ok_or_else(|| {
                MobStoreError::NotFound(format!("realm profile not found: {name}"))
            })?;

            if current_revision as u64 != expected_revision {
                return Err(MobStoreError::CasConflict(format!(
                    "realm profile '{name}' revision conflict: expected {expected_revision}, actual {current_revision}"
                )));
            }

            let next_revision = expected_revision + 1;
            let now = Utc::now();
            let now_str = now.to_rfc3339();
            let profile_json = encode_json(&profile)?;

            tx.execute(
                "UPDATE realm_profiles SET profile_json = ?1, revision = ?2, updated_at = ?3 WHERE name = ?4",
                params![profile_json, next_revision as i64, now_str, name],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;

            let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
                .map_err(|e| MobStoreError::Serialization(e.to_string()))?
                .with_timezone(&Utc);

            Ok(StoredRealmProfile {
                name,
                profile,
                revision: next_revision,
                created_at,
                updated_at: now,
            })
        })
        .await
    }

    async fn delete(
        &self,
        name: &str,
        expected_revision: u64,
    ) -> Result<StoredRealmProfile, MobStoreError> {
        let path = self.path.clone();
        let name = name.to_string();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;

            let row: Option<(Vec<u8>, i64, String, String)> = tx
                .query_row(
                    "SELECT profile_json, revision, created_at, updated_at FROM realm_profiles WHERE name = ?1",
                    params![name],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(se)?;

            let (bytes, current_revision, created_at_str, updated_at_str) = row.ok_or_else(|| {
                MobStoreError::NotFound(format!("realm profile not found: {name}"))
            })?;

            if current_revision as u64 != expected_revision {
                return Err(MobStoreError::CasConflict(format!(
                    "realm profile '{name}' revision conflict: expected {expected_revision}, actual {current_revision}"
                )));
            }

            let profile: Profile = decode_json(&bytes)?;
            tx.execute(
                "DELETE FROM realm_profiles WHERE name = ?1",
                params![name],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;

            let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
                .map_err(|e| MobStoreError::Serialization(e.to_string()))?
                .with_timezone(&Utc);
            let updated_at = chrono::DateTime::parse_from_rfc3339(&updated_at_str)
                .map_err(|e| MobStoreError::Serialization(e.to_string()))?
                .with_timezone(&Utc);

            Ok(StoredRealmProfile {
                name,
                profile,
                revision: expected_revision,
                created_at,
                updated_at,
            })
        })
        .await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::definition::{BackendConfig, FlowSpec, WiringRules};
    use crate::event::{MemberRef, MobEventKind};
    use crate::ids::{AgentIdentity, Generation, ProfileName};
    use crate::profile::{Profile, ProfileBinding, ToolConfig};
    use crate::run::StepRunStatus;
    use crate::store::ExternalBindingOverlayStatus;
    use futures::future::join_all;
    use indexmap::IndexMap;

    fn default_bridge_protocol_version()
    -> meerkat_contracts::wire::supervisor_bridge::BridgeProtocolVersion {
        meerkat_contracts::wire::supervisor_bridge::supervisor_bridge_default_protocol_version()
    }

    fn temp_db_path() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mob.db");
        (dir, path)
    }

    fn operator_request(
        identity: &str,
        generation: u64,
        sequence: usize,
    ) -> MobMemberOperatorRequestRecord {
        MobMemberOperatorRequestRecord::pending(
            MobMemberOperatorRequestKey::new(
                identity,
                generation,
                1,
                "host-a",
                1,
                "member-session-a",
                format!("request-{sequence}"),
            ),
            "0".repeat(64),
        )
    }

    fn seed_operator_requests(
        path: &Path,
        mob_id: &MobId,
        records: &[MobMemberOperatorRequestRecord],
    ) {
        let mut conn = open_connection(path).expect("open quota seed connection");
        let tx = begin_immediate(&mut conn).expect("begin quota seed transaction");
        {
            let mut insert = tx
                .prepare(
                    "INSERT INTO mob_runtime_member_operator_requests
                     (mob_id, agent_identity, generation, fence_token, host_id, binding_generation, member_session_id, request_id, record_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                )
                .expect("prepare quota seed insert");
            for record in records {
                record.validate_pending().expect("valid quota seed row");
                insert
                    .execute(params![
                        mob_id.as_str(),
                        &record.agent_identity,
                        u64_to_sql_key(record.generation),
                        u64_to_sql_key(record.fence_token),
                        &record.host_id,
                        u64_to_sql_key(record.host_binding_generation),
                        &record.member_session_id,
                        &record.request_id,
                        encode_json(record).expect("encode quota seed row"),
                    ])
                    .expect("insert quota seed row");
            }
        }
        tx.commit().expect("commit quota seed transaction");
    }

    fn operator_request_count(path: &Path, mob_id: &MobId) -> usize {
        let conn = open_connection(path).expect("open quota count connection");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM mob_runtime_member_operator_requests WHERE mob_id = ?1",
                params![mob_id.as_str()],
                |row| row.get(0),
            )
            .expect("count operator request rows");
        usize::try_from(count).expect("non-negative operator request count")
    }

    fn sample_definition() -> MobDefinition {
        let mut profiles = std::collections::BTreeMap::new();
        profiles.insert(
            ProfileName::from("worker"),
            ProfileBinding::Inline(Box::new(Profile {
                model: "model".to_string(),
                provider: None,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
                skills: Vec::new(),
                tools: ToolConfig::default(),
                peer_description: "worker".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: crate::MobRuntimeMode::AutonomousHost,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            })),
        );
        let mut definition = MobDefinition::explicit("mob");
        definition.profiles = profiles;
        definition.flows = {
            let mut flows = std::collections::BTreeMap::new();
            flows.insert(
                FlowId::from("flow-a"),
                FlowSpec::new(None, IndexMap::new(), None),
            );
            flows
        };
        definition
    }

    fn sample_run(status: MobRunStatus) -> MobRun {
        MobRun::authority_backed_for_steps(
            RunId::new(),
            MobId::from("mob"),
            FlowId::from("flow-a"),
            [crate::ids::StepId::from("step-1")],
            status,
            serde_json::json!({"a":1}),
        )
        .expect("authority-backed sample run")
    }

    #[tokio::test]
    async fn test_sqlite_event_store_append_poll_replay_prune() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path).unwrap().event_store();
        let now = Utc::now();

        store
            .append(NewMobEvent {
                mob_id: MobId::from("mob"),
                timestamp: Some(now - chrono::Duration::minutes(10)),
                kind: MobEventKind::MobCompleted,
            })
            .await
            .unwrap();
        store
            .append(NewMobEvent {
                mob_id: MobId::from("mob"),
                timestamp: Some(now),
                kind: MobEventKind::MobCompleted,
            })
            .await
            .unwrap();

        let polled = store.poll(0, 10).await.unwrap();
        assert_eq!(polled.len(), 2);

        let removed = store
            .prune(now - chrono::Duration::minutes(1))
            .await
            .unwrap();
        assert_eq!(removed, 1);

        let replayed = store.replay_all().await.unwrap();
        assert_eq!(replayed.len(), 1);
    }

    #[test]
    fn test_sqlite_event_bus_catch_up_does_not_recreate_deleted_database() {
        let (_dir, path) = temp_db_path();
        let stores = SqliteMobStores::open(&path).unwrap();
        assert!(
            path.exists(),
            "opening the store should create the database"
        );

        std::fs::remove_file(&path).unwrap();
        assert!(
            !path.exists(),
            "test setup should remove the database before watcher catch-up"
        );

        stores.event_bus.publish_available_from_storage().unwrap();
        assert!(
            !path.exists(),
            "watcher catch-up must not recreate intentionally deleted mob storage"
        );
    }

    #[test]
    fn test_sqlite_event_bus_catch_up_open_does_not_recreate_database_deleted_after_precheck() {
        let (_dir, path) = temp_db_path();
        let _stores = SqliteMobStores::open(&path).unwrap();
        assert!(
            path.try_exists().unwrap(),
            "watcher preflight should observe the database before the simulated race"
        );

        std::fs::remove_file(&path).unwrap();
        let error = poll_events_sync(&path, 0, EVENT_WATCH_CATCH_UP_LIMIT)
            .expect_err("open-existing catch-up must reject a concurrently deleted database");

        assert!(
            !path.exists(),
            "passive catch-up must not recreate a database deleted after its preflight check: {error}"
        );
    }

    #[tokio::test]
    async fn test_sqlite_terminal_reads_do_not_recreate_deleted_database() {
        let (_dir, path) = temp_db_path();
        let stores = SqliteMobStores::open(&path).unwrap();
        let event_store = stores.event_store();
        let run_store = stores.run_store();
        let runtime_store = stores.runtime_metadata_store();
        std::fs::remove_file(&path).unwrap();

        assert!(event_store.poll(0, 1).await.is_err());
        assert!(event_store.replay_all().await.is_err());
        assert!(event_store.latest_cursor().await.is_err());
        let run_id = RunId::new();
        assert!(run_store.get_run(&run_id).await.is_err());
        assert!(
            run_store
                .list_runs(&MobId::from("deleted-mob"), None)
                .await
                .is_err()
        );
        assert!(run_store.list_remote_turn_intents(&run_id).await.is_err());
        assert!(run_store.list_remote_turn_receipts(&run_id).await.is_err());
        assert!(
            runtime_store
                .load_placed_spawn(&MobId::from("deleted-mob"), "deleted-agent")
                .await
                .is_err()
        );
        assert!(
            runtime_store
                .list_placed_spawns(&MobId::from("deleted-mob"))
                .await
                .is_err()
        );
        assert!(
            !path.exists(),
            "terminal storage reads must not recreate removed mob storage"
        );
    }

    #[tokio::test]
    async fn test_sqlite_event_store_subscribe_receives_appended_events() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path).unwrap().event_store();
        let mut rx = store.subscribe().expect("subscribe");

        let stored = store
            .append(NewMobEvent {
                mob_id: MobId::from("mob"),
                timestamp: None,
                kind: MobEventKind::MobCompleted,
            })
            .await
            .unwrap();

        let observed = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("subscription should receive appended event")
            .expect("subscription should stay open");

        assert_eq!(observed.cursor, stored.cursor);
        assert!(matches!(observed.kind, MobEventKind::MobCompleted));
    }

    #[tokio::test]
    async fn test_sqlite_event_store_subscribe_receives_appends_from_separate_open() {
        let (_dir, path) = temp_db_path();
        let subscriber_store = SqliteMobStores::open(&path).unwrap().event_store();
        let writer_store = SqliteMobStores::open(&path).unwrap().event_store();
        let mut rx = subscriber_store.subscribe().expect("subscribe");

        let stored = writer_store
            .append(NewMobEvent {
                mob_id: MobId::from("mob"),
                timestamp: None,
                kind: MobEventKind::MobCompleted,
            })
            .await
            .unwrap();

        let observed = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("subscription should receive event from separately opened writer")
            .expect("subscription should stay open");

        assert_eq!(observed.cursor, stored.cursor);
        assert!(matches!(observed.kind, MobEventKind::MobCompleted));
    }

    #[tokio::test]
    async fn test_sqlite_event_store_subscribe_receives_raw_sqlite_append() {
        let (_dir, path) = temp_db_path();
        let subscriber_store = SqliteMobStores::open(&path).unwrap().event_store();
        let mut rx = subscriber_store.subscribe().expect("subscribe");
        let stored = MobEvent {
            cursor: 1,
            timestamp: Utc::now(),
            mob_id: MobId::from("mob"),
            kind: MobEventKind::MobCompleted,
        };
        let encoded = encode_stored_mob_event(&stored).unwrap();
        let writer_path = path.clone();

        run_sqlite_task(move || {
            let mut conn = open_connection(&writer_path)?;
            let tx = begin_immediate(&mut conn)?;
            tx.execute(
                "INSERT INTO mob_events (cursor, event_json) VALUES (?1, ?2)",
                params![cursor_to_i64(stored.cursor)?, encoded],
            )
            .map_err(se)?;
            set_next_cursor(&tx, stored.cursor.saturating_add(1))?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
        .unwrap();

        let observed = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("subscription should receive raw sqlite append")
            .expect("subscription should stay open");

        assert_eq!(observed.cursor, 1);
        assert!(matches!(observed.kind, MobEventKind::MobCompleted));
    }

    #[tokio::test]
    async fn test_sqlite_event_store_rejects_pre_0_6_unversioned_history() {
        let (_dir, path) = temp_db_path();
        let raw_event = serde_json::to_vec(&MobEvent {
            cursor: 1,
            timestamp: Utc::now(),
            mob_id: MobId::from("mob"),
            kind: MobEventKind::MobCompleted,
        })
        .unwrap();

        {
            let conn = open_connection(&path).unwrap();
            conn.execute(
                "INSERT INTO mob_events (cursor, event_json) VALUES (?1, ?2)",
                params![1i64, raw_event],
            )
            .unwrap();
        }

        let store = SqliteMobStores::open(&path).unwrap().event_store();
        let error = store
            .replay_all()
            .await
            .expect_err("pre-0.6 unversioned history must be rejected");
        match error {
            MobStoreError::Serialization(message) => {
                assert!(message.contains("pre-0.6 mob event history is unsupported"));
            }
            other => panic!("expected serialization error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_sqlite_run_store_cas_and_dedup() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path).unwrap().run_store();
        let run = sample_run(MobRunStatus::Running);
        let run_id = run.run_id.clone();
        let expected_flow_state = run.flow_state.clone();
        let (completed_flow_state, completed_authority_input) = run
            .flow_run_command_projection_for_test(
                crate::run::MobMachineFlowRunCommand::TerminalizeCompleted(
                    crate::run::flow_run::inputs::TerminalizeCompleted {},
                ),
            )
            .expect("project completed run state");
        let (failed_flow_state, failed_authority_input) = run
            .flow_run_command_projection_for_test(
                crate::run::MobMachineFlowRunCommand::TerminalizeFailed(
                    crate::run::flow_run::inputs::TerminalizeFailed {},
                ),
            )
            .expect("project failed run state");

        store.create_run(run).await.unwrap();

        let s1 = store.clone();
        let rid1 = run_id.clone();
        let state1 = expected_flow_state.clone();
        let completed_state = completed_flow_state.clone();
        let completed_input = completed_authority_input.clone();
        let s2 = store.clone();
        let rid2 = run_id.clone();
        let state2 = expected_flow_state.clone();
        let failed_state = failed_flow_state.clone();
        let failed_input = failed_authority_input.clone();
        let outcomes = join_all(vec![
            tokio::spawn(async move {
                s1.cas_run_snapshot_with_authority(
                    &rid1,
                    MobRunStatus::Running,
                    &state1,
                    MobRunStatus::Completed,
                    &completed_state,
                    vec![completed_input],
                )
                .await
                .unwrap()
            }),
            tokio::spawn(async move {
                s2.cas_run_snapshot_with_authority(
                    &rid2,
                    MobRunStatus::Running,
                    &state2,
                    MobRunStatus::Failed,
                    &failed_state,
                    vec![failed_input],
                )
                .await
                .unwrap()
            }),
        ])
        .await;

        let winners = outcomes
            .into_iter()
            .map(|join| join.unwrap())
            .filter(|value| *value)
            .count();
        assert_eq!(winners, 1);

        let ledger_run = sample_run(MobRunStatus::Running);
        let ledger_run_id = ledger_run.run_id.clone();
        let step_id = StepId::from("step-1");
        let ledger_expected_flow_state = ledger_run.flow_state.clone();
        let (dispatched_flow_state, dispatched_authority_input) = ledger_run
            .flow_run_command_projection_for_test(
                crate::run::MobMachineFlowRunCommand::DispatchStep(
                    crate::run::flow_run::inputs::DispatchStep {
                        step_id: step_id.clone(),
                    },
                ),
            )
            .expect("project dispatched step state");
        store.create_run(ledger_run).await.unwrap();
        assert!(
            store
                .cas_flow_state_with_authority(
                    &ledger_run_id,
                    &ledger_expected_flow_state,
                    &dispatched_flow_state,
                    vec![dispatched_authority_input.clone()],
                )
                .await
                .unwrap()
        );
        let dispatched_authority =
            MobRunProvenanceAuthority::from_flow_authority_input(dispatched_authority_input)
                .expect("dispatch input is provenance authority");

        let entry = StepLedgerEntry {
            step_id,
            agent_identity: AgentIdentity::flow_system_provenance(),
            status: StepRunStatus::Dispatched,
            output: None,
            timestamp: Utc::now(),
        };

        assert!(
            store
                .append_step_entry_if_absent_with_authority(
                    &ledger_run_id,
                    entry.clone(),
                    dispatched_authority.clone(),
                )
                .await
                .unwrap()
        );
        assert!(
            !store
                .append_step_entry_if_absent_with_authority(
                    &ledger_run_id,
                    entry,
                    dispatched_authority.clone(),
                )
                .await
                .unwrap()
        );
        store
            .append_step_entry_with_authority(
                &ledger_run_id,
                StepLedgerEntry {
                    step_id: StepId::from("step-1"),
                    agent_identity: AgentIdentity::flow_system_provenance(),
                    status: StepRunStatus::Dispatched,
                    output: None,
                    timestamp: Utc::now(),
                },
                dispatched_authority,
            )
            .await
            .expect_err("one dispatch authority must not authorize duplicate step ledger rows");
    }

    #[tokio::test]
    async fn test_sqlite_run_store_create_rejects_preset_provenance_ledgers_without_authority() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path).unwrap().run_store();
        let mut run = sample_run(MobRunStatus::Running);
        run.step_ledger.push(StepLedgerEntry {
            step_id: StepId::from("step-1"),
            agent_identity: AgentIdentity::from("worker-1"),
            status: StepRunStatus::Dispatched,
            output: None,
            timestamp: Utc::now(),
        });

        let error = store
            .create_run(run)
            .await
            .expect_err("raw step provenance must be rejected");
        assert!(error.to_string().contains("step ledger entry"));

        let mut run = sample_run(MobRunStatus::Running);
        run.failure_ledger.push(FailureLedgerEntry {
            step_id: StepId::from("step-1"),
            reason: "caller-injected failure".to_string(),
            error_report: None,
            error: None,
            timestamp: Utc::now(),
        });

        let error = store
            .create_run(run)
            .await
            .expect_err("raw failure provenance must be rejected");
        assert!(error.to_string().contains("failure ledger entry"));

        let mut run = sample_run(MobRunStatus::Running);
        run.schema_version = crate::run::mob_run_schema_version() - 1;
        let error = store
            .create_run(run)
            .await
            .expect_err("caller-controlled schema version must be rejected");
        assert!(error.to_string().contains("schema_version"));
    }

    #[tokio::test]
    async fn test_sqlite_run_store_snapshot_rejects_missing_authority_without_mutation() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path).unwrap().run_store();
        let run = sample_run(MobRunStatus::Running);
        let run_id = run.run_id.clone();
        let expected_flow_state = run.flow_state.clone();
        let authority_input_count = run.flow_authority_inputs.len();
        store.create_run(run).await.unwrap();

        let error = store
            .cas_run_snapshot_with_authority(
                &run_id,
                MobRunStatus::Running,
                &expected_flow_state,
                MobRunStatus::Completed,
                &expected_flow_state,
                Vec::new(),
            )
            .await
            .expect_err("missing machine authority must reject snapshot CAS");
        assert!(
            error
                .to_string()
                .contains("store mutation missing MobMachine authority input")
        );

        let stored = store.get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(stored.status, MobRunStatus::Running);
        assert!(stored.completed_at.is_none());
        assert_eq!(stored.flow_authority_inputs.len(), authority_input_count);
    }

    #[tokio::test]
    async fn test_sqlite_terminal_snapshot_and_event_commit_atomically() {
        let (_dir, path) = temp_db_path();
        let stores = SqliteMobStores::open(&path).unwrap();
        let store = stores.run_store();
        let events = stores.event_store();

        let run = sample_run(MobRunStatus::Running);
        let run_id = run.run_id.clone();
        let expected_flow_state = run.flow_state.clone();
        let (completed_flow_state, completed_authority_input) = run
            .flow_run_command_projection_for_test(
                crate::run::MobMachineFlowRunCommand::TerminalizeCompleted(
                    crate::run::flow_run::inputs::TerminalizeCompleted {},
                ),
            )
            .expect("project completed run state");
        store.create_run(run).await.unwrap();

        let stored = store
            .cas_run_snapshot_and_append_terminal_event_with_authority(
                &run_id,
                MobRunStatus::Running,
                &expected_flow_state,
                MobRunStatus::Completed,
                &completed_flow_state,
                vec![completed_authority_input.clone()],
                &events,
                crate::event::NewMobEvent {
                    mob_id: MobId::from("mob"),
                    timestamp: None,
                    kind: crate::event::MobEventKind::FlowCompleted {
                        run_id: run_id.clone(),
                        flow_id: FlowId::from("flow-a"),
                        structured_output: None,
                    },
                },
            )
            .await
            .expect("atomic terminal commit");
        assert!(stored.is_some(), "winning CAS must commit the event");

        // Both sides committed: run is terminal AND the terminal event exists.
        let run = store.get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(run.status, MobRunStatus::Completed);
        assert!(run.completed_at.is_some());
        let replayed = events.replay_all().await.unwrap();
        assert!(replayed.iter().any(|event| matches!(
            &event.kind,
            crate::event::MobEventKind::FlowCompleted { run_id: event_run, .. } if event_run == &run_id
        )));

        // A second (stale) attempt loses the CAS and appends NOTHING — the
        // terminal event cannot exist twice or without its snapshot.
        let lost = store
            .cas_run_snapshot_and_append_terminal_event_with_authority(
                &run_id,
                MobRunStatus::Running,
                &expected_flow_state,
                MobRunStatus::Completed,
                &completed_flow_state,
                vec![completed_authority_input],
                &events,
                crate::event::NewMobEvent {
                    mob_id: MobId::from("mob"),
                    timestamp: None,
                    kind: crate::event::MobEventKind::FlowCompleted {
                        run_id: run_id.clone(),
                        flow_id: FlowId::from("flow-a"),
                        structured_output: None,
                    },
                },
            )
            .await
            .expect("lost CAS is not an error");
        assert!(lost.is_none());
        assert_eq!(
            events.replay_all().await.unwrap().len(),
            1,
            "a lost CAS must not append a terminal event"
        );
    }

    #[tokio::test]
    async fn test_sqlite_spec_store_revision_conflict() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path).unwrap().spec_store();
        let definition = sample_definition();

        let revision = store
            .put_spec(&MobId::from("mob"), &definition, None)
            .await
            .unwrap();
        assert_eq!(revision, 1);

        let conflict = store
            .put_spec(&MobId::from("mob"), &definition, Some(0))
            .await
            .expect_err("revision conflict expected");
        assert!(matches!(
            conflict,
            MobStoreError::SpecRevisionConflict { .. }
        ));

        let loaded = store.get_spec(&MobId::from("mob")).await.unwrap();
        assert!(loaded.is_some());
    }

    #[tokio::test]
    async fn test_sqlite_member_operator_request_ledger_survives_reopen_and_replays_terminal() {
        use meerkat_contracts::wire::supervisor_bridge::{
            MemberOperatorOp, MemberOperatorOutcome, MemberOperatorReply, WireOpaqueJson,
        };

        let (_dir, path) = temp_db_path();
        let mob_id = MobId::from("mob-upcall-ledger");
        let op = MemberOperatorOp::MobRunFlow {
            flow_id: "release".to_string(),
            params: None,
        };
        let pending = MobMemberOperatorRequestRecord::pending(
            MobMemberOperatorRequestKey::new(
                "worker-1",
                u64::MAX - 1,
                u64::MAX,
                "host-a",
                u64::MAX - 2,
                "member-session-a",
                "req-reopen-1",
            ),
            crate::store::member_operator_op_digest(&op).expect("operation digest"),
        );

        {
            let store = SqliteMobStores::open(&path)
                .expect("open initial store")
                .runtime_metadata_store();
            assert_eq!(
                store
                    .begin_member_operator_request(&mob_id, &pending)
                    .await
                    .expect("persist Pending"),
                MobMemberOperatorRequestBegin::Started
            );
        }

        let terminal_reply = MemberOperatorReply {
            request_id: pending.request_id.clone(),
            outcome: MemberOperatorOutcome::Completed {
                result: WireOpaqueJson::from_value(&serde_json::json!({"ok": true})),
            },
        };
        let terminal = pending
            .terminal(terminal_reply.clone())
            .expect("legal terminal transition");
        {
            let reopened = SqliteMobStores::open(&path)
                .expect("reopen after Pending")
                .runtime_metadata_store();
            assert_eq!(
                reopened
                    .load_member_operator_request(&mob_id, &pending.key())
                    .await
                    .expect("load reopened Pending"),
                Some(pending.clone())
            );
            assert!(
                reopened
                    .compare_and_put_member_operator_request(&mob_id, &pending, &terminal)
                    .await
                    .expect("terminal CAS")
            );
        }

        let reopened = SqliteMobStores::open(&path)
            .expect("reopen after Terminal")
            .runtime_metadata_store();
        let loaded = reopened
            .load_member_operator_request(&mob_id, &pending.key())
            .await
            .expect("load reopened Terminal")
            .expect("terminal row survives");
        assert_eq!(loaded, terminal);
        assert_eq!(loaded.terminal_reply(), Some(&terminal_reply));
        assert_eq!(
            reopened
                .list_member_operator_requests(&mob_id)
                .await
                .expect("list full-domain terminal row"),
            vec![terminal.clone()]
        );
        assert_eq!(
            reopened
                .begin_member_operator_request(&mob_id, &pending)
                .await
                .expect("duplicate begin reads terminal"),
            MobMemberOperatorRequestBegin::Existing(loaded)
        );
    }

    #[tokio::test]
    async fn sqlite_member_operator_request_key_separates_host_generation_and_member_session() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path)
            .expect("open execution-fence key store")
            .runtime_metadata_store();
        let mob_id = MobId::from("sqlite-operator-execution-fence-key");
        let base = MobMemberOperatorRequestRecord::pending(
            MobMemberOperatorRequestKey::new(
                "member-a",
                7,
                11,
                "host-a",
                1,
                "member-session-a",
                "same-request-id",
            ),
            "0".repeat(64),
        );
        let mut host_generation_two = base.clone();
        host_generation_two.host_binding_generation = 2;
        let mut replacement_session = host_generation_two.clone();
        replacement_session.member_session_id = "member-session-b".to_string();

        for record in [&base, &host_generation_two, &replacement_session] {
            assert_eq!(
                store
                    .begin_member_operator_request(&mob_id, record)
                    .await
                    .expect("insert exact SQLite execution-fence key"),
                MobMemberOperatorRequestBegin::Started,
            );
        }
        assert!(matches!(
            store
                .begin_member_operator_request(&mob_id, &base)
                .await
                .expect("exact SQLite duplicate replays"),
            MobMemberOperatorRequestBegin::Existing(existing) if existing == base
        ));

        let reopened = SqliteMobStores::open(&path)
            .expect("reopen execution-fence key store")
            .runtime_metadata_store();
        for record in [&base, &host_generation_two, &replacement_session] {
            assert_eq!(
                reopened
                    .load_member_operator_request(&mob_id, &record.key())
                    .await
                    .expect("load exact SQLite execution-fence key"),
                Some(record.clone()),
            );
        }
        assert_eq!(
            reopened
                .list_member_operator_requests(&mob_id)
                .await
                .expect("list distinct SQLite execution-fence rows")
                .len(),
            3,
        );
    }

    #[tokio::test]
    async fn sqlite_migrates_pre_execution_fence_ledger_by_dropping_ambiguous_rows() {
        let (_dir, path) = temp_db_path();
        {
            let conn = Connection::open(&path).expect("open legacy member-operator database");
            conn.execute_batch(
                "CREATE TABLE mob_runtime_member_operator_requests (
                     mob_id TEXT NOT NULL,
                     agent_identity TEXT NOT NULL,
                     generation BLOB NOT NULL,
                     fence_token BLOB NOT NULL,
                     request_id TEXT NOT NULL,
                     record_json BLOB NOT NULL,
                     PRIMARY KEY (
                         mob_id, agent_identity, generation, fence_token, request_id
                     )
                 );",
            )
            .expect("create pre-execution-fence ledger");
            conn.execute(
                "INSERT INTO mob_runtime_member_operator_requests
                 (mob_id, agent_identity, generation, fence_token, request_id, record_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    "legacy-mob",
                    "member-a",
                    u64_to_sql_key(7),
                    u64_to_sql_key(11),
                    "ambiguous-request",
                    b"{}".as_slice(),
                ],
            )
            .expect("seed ambiguous pre-fence row");
        }

        let stores = SqliteMobStores::open(&path).expect("open and migrate legacy ledger");
        assert!(
            stores
                .runtime_metadata_store()
                .list_member_operator_requests(&MobId::from("legacy-mob"))
                .await
                .expect("list migrated ledger")
                .is_empty(),
            "a pre-fence row cannot be safely attributed and must not replay"
        );

        let conn = Connection::open(&path).expect("inspect migrated ledger schema");
        let mut statement = conn
            .prepare("PRAGMA table_info(mob_runtime_member_operator_requests)")
            .expect("prepare table-info inspection");
        let columns = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
            })
            .expect("read migrated table info")
            .map(|row| row.expect("read migrated column"))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(columns.get("host_id"), Some(&5));
        assert_eq!(columns.get("binding_generation"), Some(&6));
        assert_eq!(columns.get("member_session_id"), Some(&7));
        assert_eq!(columns.get("request_id"), Some(&8));
    }

    #[tokio::test]
    async fn sqlite_rebuilds_execution_fence_columns_when_they_are_not_in_the_primary_key() {
        let (_dir, path) = temp_db_path();
        {
            let conn = Connection::open(&path).expect("open malformed member-operator database");
            conn.execute_batch(
                "CREATE TABLE mob_runtime_member_operator_requests (
                     mob_id TEXT NOT NULL,
                     agent_identity TEXT NOT NULL,
                     generation BLOB NOT NULL,
                     fence_token BLOB NOT NULL,
                     host_id TEXT NOT NULL,
                     binding_generation BLOB NOT NULL,
                     member_session_id TEXT NOT NULL,
                     request_id TEXT NOT NULL,
                     record_json BLOB NOT NULL,
                     PRIMARY KEY (
                         mob_id, agent_identity, generation, fence_token, request_id
                     )
                 );",
            )
            .expect("create ledger whose fence columns are not key columns");
            conn.execute(
                "INSERT INTO mob_runtime_member_operator_requests
                 (mob_id, agent_identity, generation, fence_token, host_id,
                  binding_generation, member_session_id, request_id, record_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    "malformed-key-mob",
                    "member-a",
                    u64_to_sql_key(7),
                    u64_to_sql_key(11),
                    "host-a",
                    u64_to_sql_key(3),
                    "member-session-a",
                    "ambiguous-request",
                    b"{}".as_slice(),
                ],
            )
            .expect("seed row under malformed execution-fence key");
        }

        let stores = SqliteMobStores::open(&path).expect("open and rebuild malformed ledger");
        assert!(
            stores
                .runtime_metadata_store()
                .list_member_operator_requests(&MobId::from("malformed-key-mob"))
                .await
                .expect("list rebuilt malformed ledger")
                .is_empty(),
            "rows written without the full tuple in the primary key are ambiguous and must drop"
        );

        let conn = Connection::open(&path).expect("inspect rebuilt malformed ledger schema");
        let mut statement = conn
            .prepare("PRAGMA table_info(mob_runtime_member_operator_requests)")
            .expect("prepare rebuilt table-info inspection");
        let primary_key = statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(5)?, row.get::<_, String>(1)?))
            })
            .expect("read rebuilt table info")
            .map(|row| row.expect("read rebuilt column"))
            .filter(|(position, _)| *position > 0)
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            primary_key.into_values().collect::<Vec<_>>(),
            vec![
                "mob_id",
                "agent_identity",
                "generation",
                "fence_token",
                "host_id",
                "binding_generation",
                "member_session_id",
                "request_id",
            ],
        );
    }

    #[test]
    fn sqlite_u64_blob_keys_round_trip_and_sort_the_full_domain() {
        let (_dir, path) = temp_db_path();
        let conn = open_connection(&path).expect("open full-domain key store");
        let expected = [1, i64::MAX as u64, i64::MAX as u64 + 1, u64::MAX];
        for value in expected {
            conn.execute(
                "INSERT INTO mob_run_remote_turn_intents \
                 (run_id, dispatch_sequence, intent_json) VALUES (?1, ?2, ?3)",
                params!["full-domain-run", u64_to_sql_key(value), Vec::<u8>::new()],
            )
            .expect("insert full-domain dispatch key");
        }
        let mut statement = conn
            .prepare(
                "SELECT dispatch_sequence FROM mob_run_remote_turn_intents \
                 WHERE run_id = ?1 ORDER BY dispatch_sequence ASC",
            )
            .expect("prepare ordered full-domain key query");
        let rows = statement
            .query_map(params!["full-domain-run"], |row| row.get::<_, Vec<u8>>(0))
            .expect("query full-domain keys");
        let actual = rows
            .map(|row| {
                let key = row.expect("read full-domain key");
                sql_key_to_u64(&key, "test dispatch sequence").expect("decode full-domain key")
            })
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_sqlite_member_operator_incarnation_quota_is_atomic_and_replays_at_ceiling() {
        let (_dir, path) = temp_db_path();
        let stores = SqliteMobStores::open(&path).expect("open SQLite quota store");
        let mob_id = MobId::from("operator-incarnation-quota");
        let oversized = MobMemberOperatorRequestRecord::pending(
            MobMemberOperatorRequestKey::new(
                "member-a",
                1,
                1,
                "host-a",
                1,
                "member-session-a",
                "x".repeat(crate::store::MEMBER_OPERATOR_REQUEST_ID_MAX_BYTES + 1),
            ),
            "0".repeat(64),
        );
        assert!(
            stores
                .runtime_metadata_store()
                .begin_member_operator_request(&mob_id, &oversized)
                .await
                .is_err(),
            "SQLite must reject oversized untrusted idempotency keys before persistence"
        );
        assert_eq!(operator_request_count(&path, &mob_id), 0);
        let seed = (0..crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION - 1)
            .map(|sequence| operator_request("member-a", 1, sequence))
            .collect::<Vec<_>>();
        seed_operator_requests(&path, &mob_id, &seed);

        let candidate_a = operator_request(
            "member-a",
            1,
            crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION - 1,
        );
        let candidate_b = operator_request(
            "member-a",
            1,
            crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION,
        );
        let store_a = stores.runtime_metadata_store();
        let store_b = stores.runtime_metadata_store();
        let (result_a, result_b) = tokio::join!(
            store_a.begin_member_operator_request(&mob_id, &candidate_a),
            store_b.begin_member_operator_request(&mob_id, &candidate_b),
        );

        assert_eq!(
            [&result_a, &result_b]
                .into_iter()
                .filter(|result| matches!(result, Ok(MobMemberOperatorRequestBegin::Started)))
                .count(),
            1,
            "BEGIN IMMEDIATE must serialize the count+insert so only one contender owns execution"
        );
        assert_eq!(
            [&result_a, &result_b]
                .into_iter()
                .filter(|result| matches!(result, Err(MobStoreError::WriteFailed(_))))
                .count(),
            1,
            "the serialized loser must observe the committed ceiling and fail closed"
        );
        assert_eq!(
            operator_request_count(&path, &mob_id),
            crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION,
            "concurrent begins must not oversubscribe the incarnation ceiling"
        );

        let winner = if matches!(result_a, Ok(MobMemberOperatorRequestBegin::Started)) {
            &candidate_a
        } else {
            &candidate_b
        };
        assert_eq!(
            stores
                .runtime_metadata_store()
                .begin_member_operator_request(&mob_id, winner)
                .await
                .expect("the winning key must replay at the ceiling"),
            MobMemberOperatorRequestBegin::Existing(winner.clone()),
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_sqlite_member_operator_global_quota_is_atomic_and_replays_at_ceiling() {
        let (_dir, path) = temp_db_path();
        let stores = SqliteMobStores::open(&path).expect("open SQLite quota store");
        let mob_id = MobId::from("operator-global-quota");
        let seed = (0..crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_MOB - 1)
            .map(|sequence| {
                let incarnation =
                    sequence / crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION;
                let request = sequence % crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION;
                operator_request(&format!("member-{incarnation}"), 1, request)
            })
            .collect::<Vec<_>>();
        seed_operator_requests(&path, &mob_id, &seed);

        let candidate_a = operator_request("member-overflow-a", 1, 0);
        let candidate_b = operator_request("member-overflow-b", 1, 0);
        let store_a = stores.runtime_metadata_store();
        let store_b = stores.runtime_metadata_store();
        let (result_a, result_b) = tokio::join!(
            store_a.begin_member_operator_request(&mob_id, &candidate_a),
            store_b.begin_member_operator_request(&mob_id, &candidate_b),
        );

        assert_eq!(
            [&result_a, &result_b]
                .into_iter()
                .filter(|result| matches!(result, Ok(MobMemberOperatorRequestBegin::Started)))
                .count(),
            1,
            "only one concurrent contender may claim the final per-mob slot"
        );
        assert_eq!(
            [&result_a, &result_b]
                .into_iter()
                .filter(|result| matches!(result, Err(MobStoreError::WriteFailed(_))))
                .count(),
            1,
            "the other contender must observe the committed global ceiling"
        );
        assert_eq!(
            operator_request_count(&path, &mob_id),
            crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_MOB,
            "concurrent begins must not oversubscribe the per-mob ceiling"
        );

        let winner = if matches!(result_a, Ok(MobMemberOperatorRequestBegin::Started)) {
            &candidate_a
        } else {
            &candidate_b
        };
        assert_eq!(
            stores
                .runtime_metadata_store()
                .begin_member_operator_request(&mob_id, winner)
                .await
                .expect("the globally winning key must replay at the ceiling"),
            MobMemberOperatorRequestBegin::Existing(winner.clone()),
        );
    }

    #[tokio::test]
    async fn sqlite_recovery_prune_frees_stale_global_capacity_and_preserves_current_rows() {
        use meerkat_contracts::wire::supervisor_bridge::{
            MemberOperatorOutcome, MemberOperatorReply, WireOpaqueJson,
        };

        let (_dir, path) = temp_db_path();
        let mob_id = MobId::from("sqlite-operator-recovery-prune");
        let current = (0..crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION)
            .map(|sequence| operator_request("member-current", 1, sequence));
        let stale = (0..3).flat_map(|incarnation| {
            (0..crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION).map(move |sequence| {
                operator_request(&format!("member-stale-{incarnation}"), 1, sequence)
            })
        });
        let rows = current.chain(stale).collect::<Vec<_>>();
        seed_operator_requests(&path, &mob_id, &rows);

        let current_pending = operator_request("member-current", 1, 0);
        let current_terminal = current_pending
            .terminal(MemberOperatorReply {
                request_id: current_pending.request_id.clone(),
                outcome: MemberOperatorOutcome::Completed {
                    result: WireOpaqueJson::from_value(&serde_json::json!({"ok": true})),
                },
            })
            .expect("terminalize current SQLite replay row");
        {
            let store = SqliteMobStores::open(&path)
                .expect("open before simulated crash")
                .runtime_metadata_store();
            assert!(
                store
                    .compare_and_put_member_operator_request(
                        &mob_id,
                        &current_pending,
                        &current_terminal,
                    )
                    .await
                    .expect("persist current SQLite terminal")
            );
            assert!(
                store
                    .begin_member_operator_request(&mob_id, &operator_request("member-new", 1, 0),)
                    .await
                    .is_err(),
                "the recovered store starts at the global fail-closed ceiling"
            );
        }

        let recovered = SqliteMobStores::open(&path)
            .expect("reopen after simulated crash")
            .runtime_metadata_store();
        let authority = MobMemberOperatorPruneAuthority::from_actor_current_residencies(
            std::collections::BTreeSet::from([
                crate::store::MobMemberOperatorResidency {
                    agent_identity: "member-current".to_string(),
                    generation: 1,
                    fence_token: 1,
                    host_id: "host-a".to_string(),
                    host_binding_generation: 1,
                    member_session_id: "member-session-a".to_string(),
                },
                crate::store::MobMemberOperatorResidency {
                    agent_identity: "member-new".to_string(),
                    generation: 1,
                    fence_token: 1,
                    host_id: "host-a".to_string(),
                    host_binding_generation: 1,
                    member_session_id: "member-session-a".to_string(),
                },
            ]),
        );
        assert_eq!(
            recovered
                .prune_stale_member_operator_requests(&mob_id, &authority)
                .await
                .expect("recovered actor-authorized SQLite prune"),
            3 * crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION as u64,
        );
        assert_eq!(
            recovered
                .begin_member_operator_request(&mob_id, &current_pending)
                .await
                .expect("current SQLite terminal still replays"),
            MobMemberOperatorRequestBegin::Existing(current_terminal),
        );
        assert!(
            recovered
                .begin_member_operator_request(
                    &mob_id,
                    &operator_request(
                        "member-current",
                        1,
                        crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION,
                    ),
                )
                .await
                .is_err(),
            "recovery pruning must preserve the current incarnation ceiling"
        );
        assert_eq!(
            recovered
                .begin_member_operator_request(&mob_id, &operator_request("member-new", 1, 0))
                .await
                .expect("recovery pruning frees global SQLite capacity"),
            MobMemberOperatorRequestBegin::Started,
        );
    }

    #[tokio::test]
    async fn test_sqlite_runtime_metadata_store_roundtrips_supervisor_and_overlay_records() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path)
            .unwrap()
            .runtime_metadata_store();
        let mob_id = MobId::from("mob");
        let supervisor = SupervisorAuthorityRecord::generate(default_bridge_protocol_version());
        let supervisor_authority =
            crate::store::supervisor_authority_persistence_authority_for_record(&supervisor)
                .unwrap();
        let overlay = ExternalBindingOverlayRecord {
            agent_identity: AgentIdentity::from("worker-1"),
            generation: Generation::new(2),
            normalized_member_ref: Some(MemberRef::BackendPeer {
                peer_id: "peer-worker-1".to_string(),
                address: "tcp://worker-1".to_string(),
                bootstrap_token: None,
                session_id: None,
                pubkey: [7u8; 32],
            }),
            bootstrap_token: None,
            status: ExternalBindingOverlayStatus::Normalized,
            updated_at: Utc::now(),
        };

        store
            .put_supervisor_authority(&mob_id, &supervisor, &supervisor_authority)
            .await
            .unwrap();
        let loaded_supervisor = store
            .load_supervisor_authority(&mob_id)
            .await
            .unwrap()
            .expect("supervisor should persist");
        assert_eq!(loaded_supervisor, supervisor);

        assert!(
            store
                .put_external_binding_overlay_if_absent(&mob_id, &overlay)
                .await
                .unwrap(),
            "first overlay insert should win"
        );
        assert!(
            !store
                .put_external_binding_overlay_if_absent(&mob_id, &overlay)
                .await
                .unwrap(),
            "duplicate overlay insert should be ignored"
        );

        let overlays = store.list_external_binding_overlays(&mob_id).await.unwrap();
        assert_eq!(overlays, vec![overlay.clone()]);

        let failed_overlay = ExternalBindingOverlayRecord {
            status: ExternalBindingOverlayStatus::Failed {
                reason: "normalization failed".to_string(),
            },
            normalized_member_ref: None,
            updated_at: Utc::now(),
            ..overlay
        };
        store
            .upsert_external_binding_overlay(&mob_id, &failed_overlay)
            .await
            .unwrap();
        let overlays = store.list_external_binding_overlays(&mob_id).await.unwrap();
        assert_eq!(overlays, vec![failed_overlay]);

        store
            .delete_external_binding_overlays(&mob_id)
            .await
            .unwrap();
        assert!(
            store
                .list_external_binding_overlays(&mob_id)
                .await
                .unwrap()
                .is_empty(),
            "overlay delete should clear all records for the mob"
        );

        let delete_authority =
            crate::store::supervisor_authority_deletion_authority_for_record(&supervisor).unwrap();
        assert!(
            store
                .delete_supervisor_authority(&mob_id, &supervisor, &delete_authority)
                .await
                .unwrap()
        );
        assert!(
            store
                .load_supervisor_authority(&mob_id)
                .await
                .unwrap()
                .is_none(),
            "supervisor delete should remove the stored record"
        );
    }

    #[tokio::test]
    async fn test_sqlite_runtime_metadata_store_put_supervisor_if_absent_preserves_existing_record()
    {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path)
            .unwrap()
            .runtime_metadata_store();
        let mob_id = MobId::from("mob");
        let first = SupervisorAuthorityRecord::generate(default_bridge_protocol_version());
        let second = SupervisorAuthorityRecord::generate(default_bridge_protocol_version());
        let first_authority =
            crate::store::supervisor_authority_persistence_authority_for_record(&first).unwrap();
        let second_authority =
            crate::store::supervisor_authority_persistence_authority_for_record(&second).unwrap();

        assert!(
            store
                .put_supervisor_authority_if_absent(&mob_id, &first, &first_authority)
                .await
                .unwrap()
        );
        assert!(
            !store
                .put_supervisor_authority_if_absent(&mob_id, &second, &second_authority)
                .await
                .unwrap()
        );
        assert_eq!(
            store.load_supervisor_authority(&mob_id).await.unwrap(),
            Some(first)
        );
    }

    #[tokio::test]
    async fn test_sqlite_runtime_metadata_store_compare_and_put_supervisor_authority() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path)
            .unwrap()
            .runtime_metadata_store();
        let mob_id = MobId::from("mob-cas");
        let first = SupervisorAuthorityRecord::generate(default_bridge_protocol_version());
        let second = SupervisorAuthorityRecord::generate(default_bridge_protocol_version());
        let third = SupervisorAuthorityRecord::generate(default_bridge_protocol_version());
        let first_authority =
            crate::store::supervisor_authority_persistence_authority_for_record(&first).unwrap();
        let second_authority =
            crate::store::supervisor_authority_persistence_authority_for_record(&second).unwrap();
        let third_authority =
            crate::store::supervisor_authority_persistence_authority_for_record(&third).unwrap();

        store
            .put_supervisor_authority(&mob_id, &first, &first_authority)
            .await
            .unwrap();
        assert!(
            !store
                .compare_and_put_supervisor_authority(&mob_id, &second, &third, &third_authority)
                .await
                .unwrap(),
            "mismatched expected authority must not update"
        );
        assert_eq!(
            store.load_supervisor_authority(&mob_id).await.unwrap(),
            Some(first.clone())
        );
        assert!(
            store
                .compare_and_put_supervisor_authority(&mob_id, &first, &second, &second_authority)
                .await
                .unwrap(),
            "matching expected authority should update"
        );
        assert_eq!(
            store.load_supervisor_authority(&mob_id).await.unwrap(),
            Some(second)
        );
    }

    #[tokio::test]
    async fn test_sqlite_runtime_metadata_store_roundtrips_mob_host_authority_records() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path)
            .unwrap()
            .runtime_metadata_store();
        let mob_id = MobId::from("mob-hosts");
        let other_mob = MobId::from("mob-other");
        let host_b = crate::store::sample_mob_host_authority_record("host-peer-b", 1);
        let host_c = crate::store::sample_mob_host_authority_record("host-peer-c", 2);
        let host_b_authority =
            crate::store::mob_host_authority_persistence_authority_for_record(&host_b).unwrap();
        let host_c_authority =
            crate::store::mob_host_authority_persistence_authority_for_record(&host_c).unwrap();

        assert!(
            store
                .put_mob_host_authority_if_absent(&mob_id, &host_b, &host_b_authority)
                .await
                .unwrap()
        );
        assert!(
            !store
                .put_mob_host_authority_if_absent(&mob_id, &host_b, &host_b_authority)
                .await
                .unwrap(),
            "duplicate (mob, host) insert must be ignored"
        );
        store
            .put_mob_host_authority(&mob_id, &host_c, &host_c_authority)
            .await
            .unwrap();
        store
            .put_mob_host_authority(&other_mob, &host_b, &host_b_authority)
            .await
            .unwrap();

        assert_eq!(
            store
                .load_mob_host_authority(&mob_id, "host-peer-b")
                .await
                .unwrap(),
            Some(host_b.clone())
        );
        assert_eq!(
            store.list_mob_host_authorities(&mob_id).await.unwrap(),
            vec![host_b.clone(), host_c.clone()],
            "listing is mob-scoped and host-id ordered"
        );

        // Rebind CAS to the next epoch under a rebind-witnessed authority.
        let host_b_rebound = MobHostAuthorityRecord {
            authority_epoch: 2,
            live_endpoint: None,
            ..host_b.clone()
        };
        let rebound_authority =
            crate::store::mob_host_authority_persistence_authority_for_record(&host_b_rebound)
                .unwrap();
        assert!(
            !store
                .compare_and_put_mob_host_authority(
                    &mob_id,
                    &host_b_rebound,
                    &host_b_rebound,
                    &rebound_authority
                )
                .await
                .unwrap(),
            "mismatched expected record must not update"
        );
        assert!(
            store
                .compare_and_put_mob_host_authority(
                    &mob_id,
                    &host_b,
                    &host_b_rebound,
                    &rebound_authority
                )
                .await
                .unwrap()
        );

        // Durable across reopen (the controlling-restart fact FLAG-3 exists
        // for).
        drop(store);
        let reopened = SqliteMobStores::open(&path)
            .unwrap()
            .runtime_metadata_store();
        assert_eq!(
            reopened.list_mob_host_authorities(&mob_id).await.unwrap(),
            vec![host_b_rebound.clone(), host_c],
            "host bindings must survive a store reopen"
        );

        // Revoke: delete requires the exact expected record + revoke
        // witness; the other mob's row survives (A14 isolation).
        let deletion =
            crate::store::mob_host_authority_deletion_authority_for_record(&host_b_rebound)
                .unwrap();
        reopened
            .put_mob_host_binding_generation_highwater(&mob_id, &host_b_rebound, &deletion)
            .await
            .unwrap();
        assert!(
            !reopened
                .delete_mob_host_authority(&mob_id, &host_b, &deletion)
                .await
                .unwrap(),
            "stale expected record must not delete"
        );
        assert!(
            reopened
                .delete_mob_host_authority(&mob_id, &host_b_rebound, &deletion)
                .await
                .unwrap()
        );
        assert!(
            reopened
                .load_mob_host_authority(&mob_id, "host-peer-b")
                .await
                .unwrap()
                .is_none()
        );
        drop(reopened);
        let reopened = SqliteMobStores::open(&path)
            .unwrap()
            .runtime_metadata_store();
        assert_eq!(
            reopened
                .list_mob_host_binding_generation_highwaters(&mob_id)
                .await
                .unwrap(),
            vec![("host-peer-b".to_string(), host_b_rebound.binding_generation)],
            "the generation tombstone survives both active-row deletion and reopen",
        );
        assert_eq!(
            reopened
                .list_mob_host_authorities(&other_mob)
                .await
                .unwrap(),
            vec![host_b],
            "another mob's binding for the same host must survive"
        );
    }

    #[tokio::test]
    async fn test_sqlite_stores_share_single_database_path() {
        let (_dir, path) = temp_db_path();
        let shared = SqliteMobStores::open(&path).unwrap();
        let events = shared.event_store();
        let runs = shared.run_store();
        let specs = shared.spec_store();

        events
            .append(NewMobEvent {
                mob_id: MobId::from("mob"),
                timestamp: None,
                kind: MobEventKind::MobCompleted,
            })
            .await
            .unwrap();

        let run = sample_run(MobRunStatus::Pending);
        let run_id = run.run_id.clone();
        runs.create_run(run).await.unwrap();
        let fetched_run = runs.get_run(&run_id).await.unwrap();
        assert!(fetched_run.is_some(), "run store should share same db path");

        let definition = sample_definition();
        let revision = specs
            .put_spec(&MobId::from("mob"), &definition, None)
            .await
            .unwrap();
        assert_eq!(revision, 1);
        assert_eq!(events.replay_all().await.unwrap().len(), 1);
    }

    #[tokio::test]
    #[ignore] // integration_real: large keyset stress test
    async fn integration_real_sqlite_event_store_clear_and_prune_large_keyset() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path).unwrap().event_store();
        let now = Utc::now();

        for i in 0..2_000 {
            store
                .append(NewMobEvent {
                    mob_id: MobId::from("mob"),
                    timestamp: Some(now - chrono::Duration::minutes((i % 10) as i64)),
                    kind: MobEventKind::MobCompleted,
                })
                .await
                .unwrap();
        }

        let removed = store
            .prune(now - chrono::Duration::minutes(5))
            .await
            .unwrap();
        assert!(removed > 0, "expected stale events to be pruned");

        store.clear().await.unwrap();
        let replayed = store.replay_all().await.unwrap();
        assert!(
            replayed.is_empty(),
            "clear should remove all persisted events"
        );
    }

    /// Regression test: durable persistence must allow
    /// reopening the same database path after drop within the same process.
    /// SQLite WAL mode does not have this limitation.
    #[tokio::test]
    async fn test_sqlite_reopen_after_drop_same_path() {
        let (_dir, path) = temp_db_path();

        // First open: write data
        {
            let stores = SqliteMobStores::open(&path).unwrap();
            let events = stores.event_store();
            events
                .append(NewMobEvent {
                    mob_id: MobId::from("mob"),
                    timestamp: None,
                    kind: MobEventKind::MobCompleted,
                })
                .await
                .unwrap();
            // stores + events dropped here
        }

        // Second open: must succeed and see prior data
        {
            let stores = SqliteMobStores::open(&path).unwrap();
            let events = stores.event_store();
            let all = events.replay_all().await.unwrap();
            assert_eq!(all.len(), 1, "data from first open must survive reopen");
        }
    }

    // -----------------------------------------------------------------------
    // RealmProfileStore contract tests (SQLite)
    // -----------------------------------------------------------------------

    use crate::store::realm_profile::contract_tests;

    fn sqlite_realm_profile_store() -> (tempfile::TempDir, SqliteRealmProfileStore) {
        let (dir, path) = temp_db_path();
        let stores = SqliteMobStores::open(&path).unwrap();
        (dir, stores.realm_profile_store())
    }

    #[tokio::test]
    async fn realm_profile_create_and_get() {
        let (_dir, store) = sqlite_realm_profile_store();
        contract_tests::test_create_and_get(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_get_nonexistent() {
        let (_dir, store) = sqlite_realm_profile_store();
        contract_tests::test_get_nonexistent(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_create_duplicate_fails() {
        let (_dir, store) = sqlite_realm_profile_store();
        contract_tests::test_create_duplicate_fails(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_update_correct_revision() {
        let (_dir, store) = sqlite_realm_profile_store();
        contract_tests::test_update_with_correct_revision(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_update_wrong_revision() {
        let (_dir, store) = sqlite_realm_profile_store();
        contract_tests::test_update_with_wrong_revision(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_update_nonexistent() {
        let (_dir, store) = sqlite_realm_profile_store();
        contract_tests::test_update_nonexistent(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_delete_correct_revision() {
        let (_dir, store) = sqlite_realm_profile_store();
        contract_tests::test_delete_with_correct_revision(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_delete_wrong_revision() {
        let (_dir, store) = sqlite_realm_profile_store();
        contract_tests::test_delete_with_wrong_revision(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_delete_nonexistent() {
        let (_dir, store) = sqlite_realm_profile_store();
        contract_tests::test_delete_nonexistent(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_list() {
        let (_dir, store) = sqlite_realm_profile_store();
        contract_tests::test_list(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_list_empty() {
        let (_dir, store) = sqlite_realm_profile_store();
        contract_tests::test_list_empty(&store).await;
    }

    // -----------------------------------------------------------------------
    // SqliteRealmProfileStore::open() standalone constructor tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn sqlite_realm_profile_store_open_creates_directory() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b").join("realm_profiles.db");
        assert!(!nested.parent().unwrap().exists());
        let _store = SqliteRealmProfileStore::open(&nested).unwrap();
        assert!(nested.parent().unwrap().exists());
    }

    #[tokio::test]
    async fn sqlite_realm_profile_store_open_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("realm_profiles.db");

        // First open: create a profile.
        {
            let store = SqliteRealmProfileStore::open(&db_path).unwrap();
            let profile = Profile {
                model: "claude-sonnet-4-5".into(),
                provider: None,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
                skills: Vec::new(),
                tools: ToolConfig::default(),
                peer_description: String::new(),
                external_addressable: false,
                backend: None,
                runtime_mode: crate::MobRuntimeMode::AutonomousHost,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            };
            store.create("test-profile", &profile).await.unwrap();
        }

        // Second open: must see the profile from the first session.
        {
            let store = SqliteRealmProfileStore::open(&db_path).unwrap();
            let got = store.get("test-profile").await.unwrap();
            assert!(got.is_some(), "profile must survive reopen");
            assert_eq!(got.unwrap().profile.model, "claude-sonnet-4-5");
        }
    }

    #[tokio::test]
    async fn sqlite_realm_profile_store_open_contract_create_and_get() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("realm_profiles.db");
        let store = SqliteRealmProfileStore::open(&db_path).unwrap();
        contract_tests::test_create_and_get(&store).await;
    }

    #[tokio::test]
    async fn sqlite_realm_profile_store_open_contract_get_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("realm_profiles.db");
        let store = SqliteRealmProfileStore::open(&db_path).unwrap();
        contract_tests::test_get_nonexistent(&store).await;
    }

    #[tokio::test]
    async fn sqlite_realm_profile_store_open_contract_list() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("realm_profiles.db");
        let store = SqliteRealmProfileStore::open(&db_path).unwrap();
        contract_tests::test_list(&store).await;
    }
}
