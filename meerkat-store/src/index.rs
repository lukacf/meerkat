//! Session index interfaces.
//!
//! Phase 0 contract: define the SessionIndex API surface; implementations (e.g. redb-backed)
//! arrive in later phases.

use crate::{SessionFilter, StoreError};
use meerkat_core::{SessionId, SessionMeta};
use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const SESSIONS_BY_ID: TableDefinition<&[u8], &[u8]> = TableDefinition::new("sessions_by_id");
const SESSIONS_BY_UPDATED: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("sessions_by_updated");
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

/// redb-backed [`SessionIndex`] implementation.
///
/// This is a write-through index used by JsonlStore to avoid per-list directory scans and
/// per-session metadata file reads.
pub struct RedbSessionIndex {
    db: Database,
}

impl RedbSessionIndex {
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

        Ok(Self { db })
    }

    pub fn is_empty(&self) -> Result<bool, StoreError> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        let table = read_txn
            .open_table(SESSIONS_BY_ID)
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        let mut iter = table
            .iter()
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        Ok(iter.next().is_none())
    }

    /// Count the number of sessions in the index
    pub fn entry_count(&self) -> Result<usize, StoreError> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        let table = read_txn
            .open_table(SESSIONS_BY_ID)
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        let count = table
            .len()
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        Ok(count as usize)
    }

    pub fn lookup_meta(&self, id: &SessionId) -> Result<Option<SessionMeta>, StoreError> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        let table = read_txn
            .open_table(SESSIONS_BY_ID)
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        let key = session_id_key(id);
        let Some(meta_bytes) = table
            .get(key.as_ref())
            .map_err(|e| StoreError::Database(Box::new(e.into())))?
        else {
            return Ok(None);
        };
        let meta: SessionMeta =
            serde_json::from_slice(meta_bytes.value()).map_err(StoreError::Serialization)?;
        Ok(Some(meta))
    }

    pub fn insert_meta(&self, meta: SessionMeta) -> Result<(), StoreError> {
        let meta_bytes = serde_json::to_vec(&meta).map_err(StoreError::Serialization)?;

        let write_txn = self
            .db
            .begin_write()
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        {
            let mut by_id = write_txn
                .open_table(SESSIONS_BY_ID)
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let mut by_updated = write_txn
                .open_table(SESSIONS_BY_UPDATED)
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;

            let id_key = session_id_key(&meta.id);

            // If this session already exists, remove the old updated_at index entry.
            if let Some(existing) = by_id
                .get(id_key.as_ref())
                .map_err(|e| StoreError::Database(Box::new(e.into())))?
            {
                let existing_meta: SessionMeta =
                    serde_json::from_slice(existing.value()).map_err(StoreError::Serialization)?;
                let old_key = updated_key(&existing_meta.id, existing_meta.updated_at);
                let _ = by_updated
                    .remove(old_key.as_ref())
                    .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            }

            by_id
                .insert(id_key.as_ref(), meta_bytes.as_slice())
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;

            let new_key = updated_key(&meta.id, meta.updated_at);
            by_updated
                .insert(new_key.as_ref(), EMPTY_VALUE)
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        }
        write_txn
            .commit()
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        Ok(())
    }

    pub fn insert_many(&self, metas: Vec<SessionMeta>) -> Result<(), StoreError> {
        if metas.is_empty() {
            return Ok(());
        }

        let write_txn = self
            .db
            .begin_write()
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        {
            let mut by_id = write_txn
                .open_table(SESSIONS_BY_ID)
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let mut by_updated = write_txn
                .open_table(SESSIONS_BY_UPDATED)
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;

            for meta in metas {
                let meta_bytes = serde_json::to_vec(&meta).map_err(StoreError::Serialization)?;
                let id_key = session_id_key(&meta.id);

                // Remove stale updated_at entry if session already exists with different timestamp
                if let Some(existing) = by_id
                    .get(id_key.as_ref())
                    .map_err(|e| StoreError::Database(Box::new(e.into())))?
                {
                    let existing_meta: SessionMeta = serde_json::from_slice(existing.value())
                        .map_err(StoreError::Serialization)?;
                    // Only remove if timestamp changed to avoid duplicate entries
                    if existing_meta.updated_at != meta.updated_at {
                        let old_key = updated_key(&existing_meta.id, existing_meta.updated_at);
                        let _ = by_updated
                            .remove(old_key.as_ref())
                            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
                    }
                }

                by_id
                    .insert(id_key.as_ref(), meta_bytes.as_slice())
                    .map_err(|e| StoreError::Database(Box::new(e.into())))?;

                let new_key = updated_key(&meta.id, meta.updated_at);
                by_updated
                    .insert(new_key.as_ref(), EMPTY_VALUE)
                    .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            }
        }
        write_txn
            .commit()
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        Ok(())
    }

    pub fn remove(&self, id: &SessionId) -> Result<(), StoreError> {
        let write_txn = self
            .db
            .begin_write()
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        {
            let mut by_id = write_txn
                .open_table(SESSIONS_BY_ID)
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let mut by_updated = write_txn
                .open_table(SESSIONS_BY_UPDATED)
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;

            let id_key = session_id_key(id);
            if let Some(existing) = by_id
                .get(id_key.as_ref())
                .map_err(|e| StoreError::Database(Box::new(e.into())))?
            {
                let existing_meta: SessionMeta =
                    serde_json::from_slice(existing.value()).map_err(StoreError::Serialization)?;
                let old_key = updated_key(&existing_meta.id, existing_meta.updated_at);
                let _ = by_updated
                    .remove(old_key.as_ref())
                    .map_err(|e| StoreError::Database(Box::new(e.into())))?;
            }

            let _ = by_id
                .remove(id_key.as_ref())
                .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        }
        write_txn
            .commit()
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        Ok(())
    }

    pub fn list_meta(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, StoreError> {
        let limit = filter.limit.unwrap_or(usize::MAX);
        let mut offset = filter.offset.unwrap_or(0);

        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        let by_updated = read_txn
            .open_table(SESSIONS_BY_UPDATED)
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;
        let by_id = read_txn
            .open_table(SESSIONS_BY_ID)
            .map_err(|e| StoreError::Database(Box::new(e.into())))?;

        let mut results = Vec::new();

        for row in by_updated
            .iter()
            .map_err(|e| StoreError::Database(Box::new(e.into())))?
        {
            let (key, _value) = row.map_err(|e| StoreError::Database(Box::new(e.into())))?;
            let key_bytes = key.value();
            if key_bytes.len() != 24 {
                continue;
            }

            let mut id_bytes = [0u8; 16];
            id_bytes.copy_from_slice(&key_bytes[8..]);
            let Some(meta_bytes) = by_id
                .get(id_bytes.as_ref())
                .map_err(|e| StoreError::Database(Box::new(e.into())))?
            else {
                continue;
            };
            let meta: SessionMeta =
                serde_json::from_slice(meta_bytes.value()).map_err(StoreError::Serialization)?;

            if let Some(updated_after) = filter.updated_after {
                // Because the index is ordered by descending updated_at, we can stop early.
                if meta.updated_at < updated_after {
                    break;
                }
            }
            if let Some(created_after) = filter.created_after {
                if meta.created_at < created_after {
                    continue;
                }
            }

            if offset > 0 {
                offset -= 1;
                continue;
            }

            results.push(meta);
            if results.len() >= limit {
                break;
            }
        }

        Ok(results)
    }
}

pub trait SessionIndex: Send + Sync {
    fn lookup(&self, id: &SessionId) -> Option<SessionMeta>;

    fn insert(&self, meta: SessionMeta);

    fn list(&self, filter: SessionFilter) -> Vec<SessionMeta>;
}

impl SessionIndex for RedbSessionIndex {
    fn lookup(&self, id: &SessionId) -> Option<SessionMeta> {
        match self.lookup_meta(id) {
            Ok(meta) => meta,
            Err(err) => {
                tracing::warn!("session index lookup failed: {err}");
                None
            }
        }
    }

    fn insert(&self, meta: SessionMeta) {
        if let Err(err) = self.insert_meta(meta) {
            tracing::warn!("session index insert failed: {err}");
        }
    }

    fn list(&self, filter: SessionFilter) -> Vec<SessionMeta> {
        match self.list_meta(filter) {
            Ok(metas) => metas,
            Err(err) => {
                tracing::warn!("session index list failed: {err}");
                Vec::new()
            }
        }
    }
}
