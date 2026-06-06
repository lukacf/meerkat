//! Session index interfaces.
//!
//! SQLite-backed implementation used by `JsonlStore` to avoid per-list
//! directory scans and per-session metadata file reads.

use crate::{SessionFilter, StoreError};
use meerkat_core::{SessionId, SessionMeta};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;

const CREATE_SESSION_INDEX_SQL: &str = r"
CREATE TABLE IF NOT EXISTS session_index (
    session_id TEXT PRIMARY KEY,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    meta_json BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS session_index_updated_idx
ON session_index(updated_at_ms DESC, session_id ASC)";

fn system_time_millis(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

fn open_connection(path: &Path) -> Result<Connection, StoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.execute_batch(CREATE_SESSION_INDEX_SQL)?;
    Ok(conn)
}

/// SQLite-backed session index implementation.
pub struct SqliteSessionIndex {
    path: PathBuf,
}

impl SqliteSessionIndex {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        let conn = open_connection(&path)?;
        drop(conn);
        Ok(Self { path })
    }

    pub fn is_empty(&self) -> Result<bool, StoreError> {
        let conn = open_connection(&self.path)?;
        let exists = conn
            .query_row("SELECT 1 FROM session_index LIMIT 1", [], |row| {
                row.get::<_, i64>(0)
            })
            .optional()?;
        Ok(exists.is_none())
    }

    pub fn entry_count(&self) -> Result<usize, StoreError> {
        let conn = open_connection(&self.path)?;
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM session_index", [], |row| row.get(0))?;
        Ok(usize::try_from(count).unwrap_or(usize::MAX))
    }

    pub fn lookup_meta(&self, id: &SessionId) -> Result<Option<SessionMeta>, StoreError> {
        let conn = open_connection(&self.path)?;
        conn.query_row(
            "SELECT meta_json FROM session_index WHERE session_id = ?1",
            params![id.to_string()],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(StoreError::Serialization))
        .transpose()
    }

    pub fn insert_meta(&self, meta: SessionMeta) -> Result<(), StoreError> {
        self.insert_many(vec![meta])
    }

    pub fn insert_many(&self, metas: Vec<SessionMeta>) -> Result<(), StoreError> {
        if metas.is_empty() {
            return Ok(());
        }

        let mut conn = open_connection(&self.path)?;
        let tx = conn.transaction()?;
        for meta in metas {
            let meta_json = serde_json::to_vec(&meta).map_err(StoreError::Serialization)?;
            tx.execute(
                r"
                INSERT INTO session_index (session_id, created_at_ms, updated_at_ms, meta_json)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(session_id) DO UPDATE SET
                    created_at_ms = excluded.created_at_ms,
                    updated_at_ms = excluded.updated_at_ms,
                    meta_json = excluded.meta_json
                ",
                params![
                    meta.id.to_string(),
                    system_time_millis(meta.created_at),
                    system_time_millis(meta.updated_at),
                    meta_json,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn remove(&self, id: &SessionId) -> Result<(), StoreError> {
        let conn = open_connection(&self.path)?;
        conn.execute(
            "DELETE FROM session_index WHERE session_id = ?1",
            params![id.to_string()],
        )?;
        Ok(())
    }

    pub fn list_meta(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, StoreError> {
        let conn = open_connection(&self.path)?;
        let created_after = filter.created_after.map(system_time_millis);
        let updated_after = filter.updated_after.map(system_time_millis);
        let limit = i64::try_from(filter.limit.unwrap_or(usize::MAX)).unwrap_or(i64::MAX);
        let offset = i64::try_from(filter.offset.unwrap_or(0)).unwrap_or(i64::MAX);

        let mut stmt = conn.prepare(
            r"
            SELECT meta_json
            FROM session_index
            WHERE (?1 IS NULL OR created_at_ms >= ?1)
              AND (?2 IS NULL OR updated_at_ms >= ?2)
            ORDER BY updated_at_ms DESC, session_id ASC
            LIMIT ?3 OFFSET ?4
            ",
        )?;
        let rows = stmt.query_map(
            params![created_after, updated_after, limit, offset],
            |row| row.get::<_, Vec<u8>>(0),
        )?;

        let mut metas = Vec::new();
        for row in rows {
            let bytes = row?;
            metas.push(serde_json::from_slice(&bytes).map_err(StoreError::Serialization)?);
        }
        Ok(metas)
    }
}
