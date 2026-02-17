//! RedbSessionStore — redb-backed SessionStore implementation.
//!
//! Tables:
//! - `sessions_by_id`: UUID bytes → session JSON
//! - `sessions_by_updated`: inverted-timestamp + UUID → empty (for ordered listing)

use crate::{SessionFilter, SessionStore, StoreError};
use async_trait::async_trait;
use meerkat_core::{Session, SessionId, SessionMeta};
use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const SESSIONS_BY_ID: TableDefinition<&[u8], &[u8]> = TableDefinition::new("redb_sessions_by_id");
const SESSIONS_BY_UPDATED: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("redb_sessions_by_updated");
const EMPTY_VALUE: &[u8] = &[];

fn system_time_millis(time: SystemTime) -> u64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => match u64::try_from(duration.as_millis()) {
            Ok(millis) => millis,
            Err(_) => u64::MAX,
        },
        Err(_) => 0,
    }
}

fn session_id_key(id: &SessionId) -> [u8; 16] {
    *id.0.as_bytes()
}

fn updated_key(id: &SessionId, updated_at: SystemTime) -> [u8; 24] {
    let inverted = u64::MAX - system_time_millis(updated_at);
    let mut key = [0u8; 24];
    key[..8].copy_from_slice(&inverted.to_be_bytes());
    key[8..].copy_from_slice(id.0.as_bytes());
    key
}

/// redb-backed session store.
///
/// All session data is stored in a single redb database. Writes are
/// transactional (session data + index in same transaction).
pub struct RedbSessionStore {
    db: Arc<Database>,
}

impl RedbSessionStore {
    /// Open or create a session store at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let db = Database::create(path).map_err(|e| StoreError::Database(Box::new(e.into())))?;

        // Ensure required tables exist.
        let write_txn = db
            .begin_write()
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        {
            let _ = write_txn
                .open_table(SESSIONS_BY_ID)
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let _ = write_txn
                .open_table(SESSIONS_BY_UPDATED)
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        }
        write_txn
            .commit()
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;

        Ok(Self { db: Arc::new(db) })
    }
}

#[async_trait]
impl SessionStore for RedbSessionStore {
    async fn save(&self, session: &Session) -> Result<(), StoreError> {
        let id_key = session_id_key(session.id());
        let upd_key = updated_key(session.id(), session.updated_at());
        let session_id = session.id().clone();
        let json = serde_json::to_vec(session).map_err(StoreError::Serialization)?;
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || {
            let write_txn = db
                .begin_write()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            {
                let mut by_id_table = write_txn
                    .open_table(SESSIONS_BY_ID)
                    .map_err(|e| StoreError::Database(Box::new(e.into())))?;
                let mut by_updated_table = write_txn
                    .open_table(SESSIONS_BY_UPDATED)
                    .map_err(|e| StoreError::Database(Box::new(e.into())))?;

                // Remove old updated key if session already existed
                if let Some(old_data) = by_id_table
                    .get(id_key.as_slice())
                    .map_err(|e| StoreError::Database(Box::new(e.into())))?
                    && let Ok(old_session) = serde_json::from_slice::<Session>(old_data.value())
                {
                    let old_key = updated_key(&session_id, old_session.updated_at());
                    let _ = by_updated_table.remove(old_key.as_slice());
                }

                by_id_table
                    .insert(id_key.as_slice(), json.as_slice())
                    .map_err(|e| StoreError::Database(Box::new(e.into())))?;

                by_updated_table
                    .insert(upd_key.as_slice(), EMPTY_VALUE)
                    .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            }
            write_txn
                .commit()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;

            Ok(())
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
        let id_key = session_id_key(id);
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || {
            let read_txn = db
                .begin_read()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let table = read_txn
                .open_table(SESSIONS_BY_ID)
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;

            match table
                .get(id_key.as_slice())
                .map_err(|e| StoreError::Database(Box::new(e.into())))?
            {
                Some(data) => {
                    let session: Session =
                        serde_json::from_slice(data.value()).map_err(StoreError::Serialization)?;
                    Ok(Some(session))
                }
                None => Ok(None),
            }
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, StoreError> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || {
            let read_txn = db
                .begin_read()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let by_id = read_txn
                .open_table(SESSIONS_BY_ID)
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let by_updated = read_txn
                .open_table(SESSIONS_BY_UPDATED)
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;

            let count = by_updated
                .len()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let mut results = Vec::with_capacity(count as usize);

            let iter = by_updated
                .iter()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;

            for entry in iter {
                let (key_guard, _) = entry.map_err(|e| StoreError::Database(Box::new(e.into())))?;
                let key_bytes = key_guard.value();
                if key_bytes.len() < 24 {
                    continue;
                }

                // Extract session ID from key (bytes 8..24)
                let mut uuid_bytes = [0u8; 16];
                uuid_bytes.copy_from_slice(&key_bytes[8..24]);
                let session_id = SessionId(Uuid::from_bytes(uuid_bytes));

                // Load the full session to build meta
                let id_key = session_id_key(&session_id);
                if let Some(data) = by_id
                    .get(id_key.as_slice())
                    .map_err(|e| StoreError::Database(Box::new(e.into())))?
                    && let Ok(session) = serde_json::from_slice::<Session>(data.value())
                {
                    let meta = SessionMeta::from(&session);

                    // Apply filters
                    if let Some(after) = filter.created_after
                        && meta.created_at < after
                    {
                        continue;
                    }
                    if let Some(after) = filter.updated_after
                        && meta.updated_at < after
                    {
                        continue;
                    }

                    results.push(meta);

                    // Early exit: collect enough to satisfy offset + limit without overflow.
                    let effective_limit = filter
                        .offset
                        .unwrap_or(0)
                        .saturating_add(filter.limit.unwrap_or(usize::MAX));
                    if results.len() >= effective_limit {
                        break;
                    }
                }
            }

            // Apply offset
            if let Some(offset) = filter.offset {
                if offset < results.len() {
                    results = results.split_off(offset);
                } else {
                    results.clear();
                }
            }

            // Apply limit
            if let Some(limit) = filter.limit {
                results.truncate(limit);
            }

            Ok(results)
        })
        .await
        .map_err(StoreError::Join)?
    }

    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        let id_key = session_id_key(id);
        let id_owned = id.clone();
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || {
            let write_txn = db
                .begin_write()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            {
                let mut by_id_table = write_txn
                    .open_table(SESSIONS_BY_ID)
                    .map_err(|e| StoreError::Database(Box::new(e.into())))?;
                let mut by_updated_table = write_txn
                    .open_table(SESSIONS_BY_UPDATED)
                    .map_err(|e| StoreError::Database(Box::new(e.into())))?;

                if let Some(data) = by_id_table
                    .get(id_key.as_slice())
                    .map_err(|e| StoreError::Database(Box::new(e.into())))?
                    && let Ok(session) = serde_json::from_slice::<Session>(data.value())
                {
                    let old_key = updated_key(&id_owned, session.updated_at());
                    let _ = by_updated_table.remove(old_key.as_slice());
                }

                let _ = by_id_table.remove(id_key.as_slice());
            }
            write_txn
                .commit()
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;

            Ok(())
        })
        .await
        .map_err(StoreError::Join)?
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::types::UserMessage;
    use tempfile::TempDir;

    fn temp_store() -> (TempDir, RedbSessionStore) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sessions.redb");
        let store = RedbSessionStore::open(&path).unwrap();
        (dir, store)
    }

    #[tokio::test]
    async fn test_save_and_load() {
        let (_dir, store) = temp_store();
        let mut session = Session::new();
        session.push(meerkat_core::types::Message::User(UserMessage {
            content: "Hello".to_string(),
        }));

        store.save(&session).await.unwrap();

        let loaded = store.load(session.id()).await.unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.id(), session.id());
        assert_eq!(loaded.messages().len(), 1);
    }

    #[tokio::test]
    async fn test_load_nonexistent() {
        let (_dir, store) = temp_store();
        let id = SessionId::new();
        let loaded = store.load(&id).await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_list_ordered_by_updated() {
        let (_dir, store) = temp_store();

        let s1 = Session::new();
        store.save(&s1).await.unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));

        let s2 = Session::new();
        store.save(&s2).await.unwrap();

        let metas = store.list(SessionFilter::default()).await.unwrap();
        assert_eq!(metas.len(), 2);
        // Most recently updated should be first (inverted timestamp key)
        assert_eq!(metas[0].id, *s2.id());
        assert_eq!(metas[1].id, *s1.id());
    }

    #[tokio::test]
    async fn test_delete() {
        let (_dir, store) = temp_store();
        let session = Session::new();
        store.save(&session).await.unwrap();

        store.delete(session.id()).await.unwrap();

        let loaded = store.load(session.id()).await.unwrap();
        assert!(loaded.is_none());

        let metas = store.list(SessionFilter::default()).await.unwrap();
        assert!(metas.is_empty());
    }

    #[tokio::test]
    async fn test_save_update_moves_index_key() {
        let (_dir, store) = temp_store();
        let mut session = Session::new();
        store.save(&session).await.unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));

        // Update the session
        session.push(meerkat_core::types::Message::User(UserMessage {
            content: "Update".to_string(),
        }));
        store.save(&session).await.unwrap();

        // Should still have exactly one entry in list
        let metas = store.list(SessionFilter::default()).await.unwrap();
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].message_count, 1);
    }

    #[tokio::test]
    async fn test_list_with_limit() {
        let (_dir, store) = temp_store();

        for _ in 0..5 {
            store.save(&Session::new()).await.unwrap();
        }

        let metas = store
            .list(SessionFilter {
                limit: Some(3),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(metas.len(), 3);
    }

    #[tokio::test]
    async fn test_session_survives_reopen() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sessions.redb");

        let session_id;
        {
            let store = RedbSessionStore::open(&path).unwrap();
            let mut session = Session::new();
            session_id = session.id().clone();
            session.push(meerkat_core::types::Message::User(UserMessage {
                content: "Persistent".to_string(),
            }));
            store.save(&session).await.unwrap();
        }

        // Reopen
        {
            let store = RedbSessionStore::open(&path).unwrap();
            let loaded = store.load(&session_id).await.unwrap();
            assert!(loaded.is_some());
            let loaded = loaded.unwrap();
            assert_eq!(loaded.messages().len(), 1);
        }
    }
}
