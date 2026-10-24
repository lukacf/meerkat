//! [`SpiSessionStore`]: the session slot of a host store realm, read from
//! the runtime store.
//!
//! A host store realm is WholeBlob: each session's committed document lives
//! in its runtime's scope ([`super::SpiRuntimeStore`]), and the session
//! service reads and writes sessions only through the runtime store. This
//! store answers the session slot's reads from there (the committed
//! document, and the runtime catalog for listings) and refuses every write
//! typed, so no second copy of a session can drift from its authority.

use std::sync::Arc;

use meerkat_core::session_store::{SessionStore, SessionStoreError};
use meerkat_core::types::SessionId;
use meerkat_core::{Session, SessionFilter, SessionMeta};

use super::SpiRuntimeStore;
use crate::identifiers::LogicalRuntimeId;
use crate::store::{RuntimeSessionCatalogEntry, RuntimeStore, RuntimeStoreError};

/// The session store of a host store realm. See the module documentation.
#[derive(Debug, Clone)]
pub struct SpiSessionStore {
    runtime: Arc<SpiRuntimeStore>,
}

impl SpiSessionStore {
    #[must_use]
    pub fn new(runtime: Arc<SpiRuntimeStore>) -> Self {
        Self { runtime }
    }
}

fn refused(operation: &str) -> SessionStoreError {
    SessionStoreError::Internal(format!(
        "{operation} is refused: a host store realm keeps every session in its runtime store, \
         which the session service writes"
    ))
}

fn read_failed(error: &RuntimeStoreError) -> SessionStoreError {
    SessionStoreError::Internal(format!("host store session read failed: {error}"))
}

fn meta_of(entry: &RuntimeSessionCatalogEntry) -> SessionMeta {
    let mut metadata = serde_json::Map::new();
    if !entry.labels().is_empty()
        && let Ok(labels) = serde_json::to_value(entry.labels())
    {
        metadata.insert("session_labels".to_string(), labels);
    }
    SessionMeta {
        id: entry.session_id().clone(),
        created_at: entry.created_at(),
        updated_at: entry.updated_at(),
        message_count: entry.message_count(),
        total_tokens: entry.total_tokens(),
        metadata,
    }
}

#[async_trait::async_trait]
impl SessionStore for SpiSessionStore {
    async fn save(&self, _session: &Session) -> Result<(), SessionStoreError> {
        Err(refused("save"))
    }

    async fn save_authoritative_projection(
        &self,
        _session: &Session,
    ) -> Result<(), SessionStoreError> {
        Err(refused("save_authoritative_projection"))
    }

    async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
        let runtime_id = LogicalRuntimeId::for_session(id);
        let snapshot =
            RuntimeStore::load_committed_whole_blob_snapshot(&*self.runtime, &runtime_id)
                .await
                .map_err(|error| read_failed(&error))?;
        Ok(snapshot.map(|snapshot| snapshot.session().clone()))
    }

    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, SessionStoreError> {
        let entries = RuntimeStore::list_runtime_session_catalog_entries(&*self.runtime, filter)
            .await
            .map_err(|error| read_failed(&error))?;
        Ok(entries.iter().map(meta_of).collect())
    }

    async fn load_meta(&self, id: &SessionId) -> Result<Option<SessionMeta>, SessionStoreError> {
        let runtime_id = LogicalRuntimeId::for_session(id);
        let entry = RuntimeStore::load_runtime_session_catalog_entry(&*self.runtime, &runtime_id)
            .await
            .map_err(|error| read_failed(&error))?;
        Ok(entry.as_ref().map(meta_of))
    }

    async fn delete(&self, _id: &SessionId) -> Result<(), SessionStoreError> {
        Err(refused("delete"))
    }

    async fn delete_if_current_revision(
        &self,
        _id: &SessionId,
        _expected_current_revision: &str,
    ) -> Result<bool, SessionStoreError> {
        Err(refused("delete_if_current_revision"))
    }
}
