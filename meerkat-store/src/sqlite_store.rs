//! SQLite-backed session store.

use crate::{SessionFilter, SessionStore, StoreError};
use async_trait::async_trait;
use meerkat_core::time_compat::SystemTime;
use meerkat_core::{Session, SessionId, SessionMeta};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};
use uuid::Uuid;

const SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;

const CREATE_SESSIONS_TABLE_SQL: &str = r"
CREATE TABLE IF NOT EXISTS sessions (
    session_id TEXT PRIMARY KEY,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    message_count INTEGER NOT NULL,
    total_tokens INTEGER NOT NULL,
    metadata_json TEXT NOT NULL,
    session_json BLOB NOT NULL
)";

const CREATE_SESSIONS_UPDATED_INDEX_SQL: &str = r"
CREATE INDEX IF NOT EXISTS sessions_updated_idx
ON sessions(updated_at_ms DESC, session_id ASC)";

fn system_time_millis(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

fn millis_to_system_time(value: i64) -> SystemTime {
    let millis = u64::try_from(value).unwrap_or_default();
    UNIX_EPOCH + Duration::from_millis(millis)
}

fn parse_session_id(raw: String) -> Result<SessionId, StoreError> {
    let uuid = Uuid::parse_str(&raw)
        .map_err(|err| StoreError::Internal(format!("invalid session_id '{raw}': {err}")))?;
    Ok(SessionId(uuid))
}

pub fn open_connection(path: &Path) -> Result<Connection, StoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    ensure_schema(&conn)?;
    Ok(conn)
}

pub fn begin_immediate_transaction(conn: &mut Connection) -> Result<Transaction<'_>, StoreError> {
    conn.transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(StoreError::from)
}

pub fn ensure_schema(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(CREATE_SESSIONS_TABLE_SQL)?;
    conn.execute_batch(CREATE_SESSIONS_UPDATED_INDEX_SQL)?;
    Ok(())
}

pub fn write_session_snapshot_in_txn(
    tx: &Transaction<'_>,
    session: &Session,
) -> Result<(), StoreError> {
    let session_id = session.id().to_string();
    let metadata_json = serde_json::to_string(session.metadata())?;
    let session_json = serde_json::to_vec(session)?;
    tx.execute(
        r"
        INSERT INTO sessions (
            session_id,
            created_at_ms,
            updated_at_ms,
            message_count,
            total_tokens,
            metadata_json,
            session_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ON CONFLICT(session_id) DO UPDATE SET
            created_at_ms = excluded.created_at_ms,
            updated_at_ms = excluded.updated_at_ms,
            message_count = excluded.message_count,
            total_tokens = excluded.total_tokens,
            metadata_json = excluded.metadata_json,
            session_json = excluded.session_json
        ",
        params![
            session_id,
            system_time_millis(session.created_at()),
            system_time_millis(session.updated_at()),
            i64::try_from(session.messages().len()).unwrap_or(i64::MAX),
            i64::try_from(session.total_tokens()).unwrap_or(i64::MAX),
            metadata_json,
            session_json,
        ],
    )?;
    Ok(())
}

/// SQLite-backed session store with one connection per operation.
pub struct SqliteSessionStore {
    path: PathBuf,
}

impl SqliteSessionStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        let conn = open_connection(&path)?;
        drop(conn);
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn save(&self, session: &Session) -> Result<(), StoreError> {
        let path = self.path.clone();
        let session = session.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate_transaction(&mut conn)?;
            write_session_snapshot_in_txn(&tx, &session)?;
            tx.commit()?;
            Ok(())
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
        let path = self.path.clone();
        let session_id = id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.query_row(
                "SELECT session_json FROM sessions WHERE session_id = ?1",
                params![session_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?
            .map(|bytes| serde_json::from_slice(&bytes).map_err(StoreError::Serialization))
            .transpose()
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, StoreError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let created_after = filter.created_after.map(system_time_millis);
            let updated_after = filter.updated_after.map(system_time_millis);
            let limit = i64::try_from(filter.limit.unwrap_or(usize::MAX)).unwrap_or(i64::MAX);
            let offset = i64::try_from(filter.offset.unwrap_or(0)).unwrap_or(i64::MAX);

            let mut stmt = conn.prepare(
                r"
                SELECT
                    session_id,
                    created_at_ms,
                    updated_at_ms,
                    message_count,
                    total_tokens,
                    metadata_json
                FROM sessions
                WHERE (?1 IS NULL OR created_at_ms >= ?1)
                  AND (?2 IS NULL OR updated_at_ms >= ?2)
                ORDER BY updated_at_ms DESC, session_id ASC
                LIMIT ?3 OFFSET ?4
                ",
            )?;
            let rows = stmt.query_map(
                params![created_after, updated_after, limit, offset],
                |row| {
                    let metadata_json: String = row.get(5)?;
                    let metadata = serde_json::from_str(&metadata_json).map_err(|err| {
                        rusqlite::Error::FromSqlConversionFailure(
                            5,
                            rusqlite::types::Type::Text,
                            Box::new(err),
                        )
                    })?;
                    Ok(SessionMeta {
                        id: parse_session_id(row.get(0)?).map_err(|err| {
                            rusqlite::Error::FromSqlConversionFailure(
                                0,
                                rusqlite::types::Type::Text,
                                Box::new(err),
                            )
                        })?,
                        created_at: millis_to_system_time(row.get(1)?),
                        updated_at: millis_to_system_time(row.get(2)?),
                        message_count: usize::try_from(row.get::<_, i64>(3)?).unwrap_or(usize::MAX),
                        total_tokens: u64::try_from(row.get::<_, i64>(4)?).unwrap_or(u64::MAX),
                        metadata,
                    })
                },
            )?;

            rows.collect::<Result<Vec<_>, _>>()
                .map_err(StoreError::from)
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        let path = self.path.clone();
        let session_id = id.to_string();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate_transaction(&mut conn)?;
            tx.execute(
                "DELETE FROM sessions WHERE session_id = ?1",
                params![session_id],
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
        .map_err(StoreError::Join)?
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::types::{Message, UserMessage};
    use tempfile::TempDir;

    fn temp_store() -> (TempDir, SqliteSessionStore) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sessions.sqlite3");
        let store = SqliteSessionStore::open(&path).unwrap();
        (dir, store)
    }

    #[tokio::test]
    async fn save_load_roundtrip() {
        let (_dir, store) = temp_store();
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("hello".to_string())));

        store.save(&session).await.unwrap();
        let loaded = store.load(session.id()).await.unwrap().unwrap();
        assert_eq!(loaded.id(), session.id());
        assert_eq!(loaded.messages().len(), 1);
    }

    #[tokio::test]
    async fn list_is_ordered_by_updated_desc() {
        let (_dir, store) = temp_store();
        let first = Session::new();
        store.save(&first).await.unwrap();
        std::thread::sleep(Duration::from_millis(10));

        let second = Session::new();
        store.save(&second).await.unwrap();

        let sessions = store.list(SessionFilter::default()).await.unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id, *second.id());
        assert_eq!(sessions[1].id, *first.id());
    }

    #[tokio::test]
    async fn delete_removes_session() {
        let (_dir, store) = temp_store();
        let session = Session::new();
        store.save(&session).await.unwrap();
        store.delete(session.id()).await.unwrap();
        assert!(store.load(session.id()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn reopen_reads_existing_data() {
        let (dir, store) = temp_store();
        let session = Session::new();
        store.save(&session).await.unwrap();

        let reopened = SqliteSessionStore::open(dir.path().join("sessions.sqlite3")).unwrap();
        assert!(reopened.load(session.id()).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn two_handles_share_same_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sessions.sqlite3");
        let first = SqliteSessionStore::open(&path).unwrap();
        let second = SqliteSessionStore::open(&path).unwrap();

        let session = Session::new();
        first.save(&session).await.unwrap();
        let loaded = second.load(session.id()).await.unwrap();
        assert!(loaded.is_some());
    }
}
