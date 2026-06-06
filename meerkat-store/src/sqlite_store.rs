//! SQLite-backed session store.

use crate::error::into_session_store_error;
use crate::{SessionFilter, SessionStore, SessionStoreError, StoreError};
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
    // Derived projection counters must round-trip through the durable i64
    // columns without loss. A count that exceeds i64::MAX is itself an
    // impossible state, so fail closed rather than silently clamping to a
    // fabricated maximum (terminal-truth store-metadata cluster).
    let message_count = i64::try_from(session.messages().len()).map_err(|_| {
        StoreError::Internal(format!(
            "session '{session_id}' message_count {} exceeds durable i64 range",
            session.messages().len()
        ))
    })?;
    let total_tokens = i64::try_from(session.total_tokens()).map_err(|_| {
        StoreError::Internal(format!(
            "session '{session_id}' total_tokens {} exceeds durable i64 range",
            session.total_tokens()
        ))
    })?;
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
            message_count,
            total_tokens,
            metadata_json,
            session_json,
        ],
    )?;
    Ok(())
}

fn load_session_snapshot_in_txn(
    tx: &Transaction<'_>,
    id: &SessionId,
) -> Result<Option<Session>, StoreError> {
    tx.query_row(
        "SELECT session_json FROM sessions WHERE session_id = ?1",
        params![id.to_string()],
        |row| row.get::<_, Vec<u8>>(0),
    )
    .optional()?
    .map(|bytes| {
        meerkat_core::session_migrations::deserialize_session_migrating(&bytes)
            .map_err(|err| StoreError::Internal(err.to_string()))
    })
    .transpose()
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

// Private methods return StoreError (preserves internal ? chains).
// Trait methods convert at the boundary via into_session_store_error().
impl SqliteSessionStore {
    async fn save_impl(&self, session: &Session) -> Result<(), StoreError> {
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

    async fn load_impl(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
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
            .map(|bytes| {
                meerkat_core::session_migrations::deserialize_session_migrating(&bytes)
                    .map_err(|err| StoreError::Internal(err.to_string()))
            })
            .transpose()
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn list_impl(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, StoreError> {
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
                    let id = parse_session_id(row.get(0)?).map_err(|err| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(err),
                        )
                    })?;
                    // Derived projection counters are stored as i64. A negative
                    // or out-of-range value is an impossible durable state, so
                    // fail closed with a typed Corrupted error rather than
                    // laundering it to usize::MAX/u64::MAX (terminal-truth
                    // store-metadata cluster). The canonical truth is in
                    // session_json; the caller can recover via load().
                    let message_count = usize::try_from(row.get::<_, i64>(3)?).map_err(|_| {
                        rusqlite::Error::FromSqlConversionFailure(
                            3,
                            rusqlite::types::Type::Integer,
                            Box::new(StoreError::Corrupted(id.clone())),
                        )
                    })?;
                    let total_tokens = u64::try_from(row.get::<_, i64>(4)?).map_err(|_| {
                        rusqlite::Error::FromSqlConversionFailure(
                            4,
                            rusqlite::types::Type::Integer,
                            Box::new(StoreError::Corrupted(id.clone())),
                        )
                    })?;
                    Ok(SessionMeta {
                        id,
                        created_at: millis_to_system_time(row.get(1)?),
                        updated_at: millis_to_system_time(row.get(2)?),
                        message_count,
                        total_tokens,
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

    async fn delete_impl(&self, id: &SessionId) -> Result<(), StoreError> {
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

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn save(&self, session: &Session) -> Result<(), SessionStoreError> {
        // F1 closure (wave-c C-H1): reject shrink-attempts at the trait
        // boundary before the row is overwritten on disk.
        let path = self.path.clone();
        let session = session.clone();
        tokio::task::spawn_blocking(move || -> Result<(), SessionStoreError> {
            let mut conn = open_connection(&path).map_err(into_session_store_error)?;
            let tx = begin_immediate_transaction(&mut conn).map_err(into_session_store_error)?;
            let previous = load_session_snapshot_in_txn(&tx, session.id())
                .map_err(into_session_store_error)?;
            meerkat_core::session_store::append_only_save_guard(&session, previous.as_ref())?;
            write_session_snapshot_in_txn(&tx, &session).map_err(into_session_store_error)?;
            tx.commit()
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            Ok(())
        })
        .await
        .map_err(StoreError::Join)
        .map_err(into_session_store_error)?
    }

    async fn save_transcript_rewrite(
        &self,
        session: &Session,
        commit: &meerkat_core::TranscriptRewriteCommit,
    ) -> Result<(), SessionStoreError> {
        let path = self.path.clone();
        let session = session.clone();
        let commit = commit.clone();
        tokio::task::spawn_blocking(move || -> Result<(), SessionStoreError> {
            let mut conn = open_connection(&path).map_err(into_session_store_error)?;
            let tx = begin_immediate_transaction(&mut conn).map_err(into_session_store_error)?;
            let previous = load_session_snapshot_in_txn(&tx, session.id())
                .map_err(into_session_store_error)?;
            meerkat_core::session_store::transcript_rewrite_save_guard(
                &session,
                previous.as_ref(),
                &commit,
            )?;
            write_session_snapshot_in_txn(&tx, &session).map_err(into_session_store_error)?;
            tx.commit()
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            Ok(())
        })
        .await
        .map_err(StoreError::Join)
        .map_err(into_session_store_error)?
    }

    async fn save_authoritative_projection(
        &self,
        session: &Session,
    ) -> Result<(), SessionStoreError> {
        self.save_impl(session)
            .await
            .map_err(into_session_store_error)
    }

    async fn save_authoritative_projection_if_current_revision(
        &self,
        session: &Session,
        expected_current_revision: Option<String>,
    ) -> Result<(), SessionStoreError> {
        let path = self.path.clone();
        let session = session.clone();
        tokio::task::spawn_blocking(move || -> Result<(), SessionStoreError> {
            let mut conn = open_connection(&path).map_err(into_session_store_error)?;
            let tx = begin_immediate_transaction(&mut conn).map_err(into_session_store_error)?;
            let previous = load_session_snapshot_in_txn(&tx, session.id())
                .map_err(into_session_store_error)?;
            meerkat_core::session_store::authoritative_projection_current_revision_guard(
                &session,
                previous.as_ref(),
                expected_current_revision.as_deref(),
            )?;
            write_session_snapshot_in_txn(&tx, &session).map_err(into_session_store_error)?;
            tx.commit()
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            Ok(())
        })
        .await
        .map_err(StoreError::Join)
        .map_err(into_session_store_error)?
    }

    async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
        self.load_impl(id).await.map_err(into_session_store_error)
    }

    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, SessionStoreError> {
        self.list_impl(filter)
            .await
            .map_err(into_session_store_error)
    }

    async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError> {
        self.delete_impl(id).await.map_err(into_session_store_error)
    }

    async fn delete_if_current_revision(
        &self,
        id: &SessionId,
        expected_current_revision: &str,
    ) -> Result<bool, SessionStoreError> {
        let path = self.path.clone();
        let session_id = id.clone();
        let expected_current_revision = expected_current_revision.to_string();
        tokio::task::spawn_blocking(move || -> Result<bool, SessionStoreError> {
            let mut conn = open_connection(&path).map_err(into_session_store_error)?;
            let tx = begin_immediate_transaction(&mut conn).map_err(into_session_store_error)?;
            let previous =
                load_session_snapshot_in_txn(&tx, &session_id).map_err(into_session_store_error)?;
            let Some(previous) = previous else {
                tx.commit()
                    .map_err(StoreError::from)
                    .map_err(into_session_store_error)?;
                return Ok(false);
            };
            let previous_token =
                meerkat_core::session_store::session_projection_cas_token(&previous)?;
            if previous_token != expected_current_revision {
                tx.commit()
                    .map_err(StoreError::from)
                    .map_err(into_session_store_error)?;
                return Ok(false);
            }
            tx.execute(
                "DELETE FROM sessions WHERE session_id = ?1",
                params![session_id.to_string()],
            )
            .map_err(StoreError::from)
            .map_err(into_session_store_error)?;
            tx.commit()
                .map_err(StoreError::from)
                .map_err(into_session_store_error)?;
            Ok(true)
        })
        .await
        .map_err(StoreError::Join)
        .map_err(into_session_store_error)?
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::types::{AssistantBlock, BlockAssistantMessage, Message, UserMessage};
    use meerkat_core::{StopReason, TranscriptRewriteReason, TranscriptRewriteSelection};
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

    #[tokio::test]
    async fn save_transcript_rewrite_rejects_stale_parent_after_intervening_save() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sessions.sqlite3");
        let first = SqliteSessionStore::open(&path).unwrap();
        let second = SqliteSessionStore::open(&path).unwrap();

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("hello".to_string())));
        session.push(Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::Text {
                text: "original".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
        )));
        first.save(&session).await.unwrap();

        let mut stale = first.load(session.id()).await.unwrap().unwrap();
        let mut newer = second.load(session.id()).await.unwrap().unwrap();
        newer.push(Message::User(UserMessage::text("intervening".to_string())));
        second.save(&newer).await.unwrap();

        let commit = stale
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::BlockAssistant(BlockAssistantMessage::new(
                    vec![AssistantBlock::Text {
                        text: "replacement".to_string(),
                        meta: None,
                    }],
                    StopReason::EndTurn,
                ))],
                TranscriptRewriteReason::new("compaction"),
                Some("test".to_string()),
                None,
            )
            .unwrap();

        let err = first
            .save_transcript_rewrite(&stale, &commit)
            .await
            .expect_err("stale rewrite must not overwrite newer session state");
        assert!(
            matches!(err, SessionStoreError::TranscriptRevisionConflict { .. }),
            "unexpected error: {err}"
        );

        let saved = first.load(session.id()).await.unwrap().unwrap();
        assert_eq!(saved.messages().len(), newer.messages().len());
    }

    #[tokio::test]
    async fn authoritative_projection_expected_revision_rejects_stale_writer() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sessions.sqlite3");
        let first = SqliteSessionStore::open(&path).unwrap();
        let second = SqliteSessionStore::open(&path).unwrap();

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("base".to_string())));
        first.save(&session).await.unwrap();
        let expected_revision = session.transcript_revision().unwrap();

        let mut newer = second.load(session.id()).await.unwrap().unwrap();
        newer.push(Message::User(UserMessage::text("newer".to_string())));
        second.save(&newer).await.unwrap();

        let mut stale_projection = session.clone();
        stale_projection.push(Message::User(UserMessage::text("stale".to_string())));
        let err = first
            .save_authoritative_projection_if_current_revision(
                &stale_projection,
                Some(expected_revision),
            )
            .await
            .expect_err("stale authoritative projection should be rejected");
        assert!(
            matches!(err, SessionStoreError::TranscriptContinuityViolation { .. }),
            "unexpected error: {err}"
        );

        let saved = first.load(session.id()).await.unwrap().unwrap();
        assert_eq!(saved.messages().len(), newer.messages().len());
        assert_eq!(
            saved.transcript_revision().unwrap(),
            newer.transcript_revision().unwrap()
        );
    }

    #[tokio::test]
    async fn delete_if_current_revision_only_deletes_matching_projection() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sessions.sqlite3");
        let first = SqliteSessionStore::open(&path).unwrap();
        let second = SqliteSessionStore::open(&path).unwrap();

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("base".to_string())));
        first.save(&session).await.unwrap();
        let stale_token =
            meerkat_core::session_store::session_projection_cas_token(&session).unwrap();

        let mut newer = second.load(session.id()).await.unwrap().unwrap();
        newer.push(Message::User(UserMessage::text("newer".to_string())));
        second.save(&newer).await.unwrap();

        assert!(
            !first
                .delete_if_current_revision(session.id(), &stale_token)
                .await
                .unwrap()
        );
        assert!(first.load(session.id()).await.unwrap().is_some());

        let current_token =
            meerkat_core::session_store::session_projection_cas_token(&newer).unwrap();
        assert!(
            first
                .delete_if_current_revision(session.id(), &current_token)
                .await
                .unwrap()
        );
        assert!(first.load(session.id()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_fails_closed_on_negative_durable_counter() {
        // Gate (row #238): a durable row carrying a negative message_count is
        // an impossible-state projection. list() must surface a typed error
        // rather than laundering it to usize::MAX. OLD behavior:
        // `usize::try_from(...).unwrap_or(usize::MAX)` returned a fabricated
        // count and list() succeeded.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sessions.sqlite3");
        let store = SqliteSessionStore::open(&path).unwrap();

        let session = Session::new();
        store.save(&session).await.unwrap();

        // Corrupt the derived counter column directly on disk.
        let conn = open_connection(&path).unwrap();
        conn.execute(
            "UPDATE sessions SET message_count = -1 WHERE session_id = ?1",
            params![session.id().to_string()],
        )
        .unwrap();
        drop(conn);

        let err = store
            .list(SessionFilter::default())
            .await
            .expect_err("list must fail closed on a negative durable counter");
        // Negative counters surface through the typed StoreError boundary,
        // not as usize::MAX.
        assert!(
            matches!(err, SessionStoreError::Internal(_)),
            "unexpected error: {err}"
        );

        // Canonical truth still recoverable from session_json via load().
        let loaded = store.load(session.id()).await.unwrap().unwrap();
        assert_eq!(loaded.id(), session.id());
    }
}
