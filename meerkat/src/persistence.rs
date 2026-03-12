use std::sync::Arc;

use crate::SessionStore;

#[cfg(feature = "session-store")]
use std::any::Any;

#[cfg(feature = "session-store")]
use meerkat_runtime::{RuntimeSessionAdapter, RuntimeStore, RuntimeStoreError};
#[cfg(feature = "session-store")]
use meerkat_store::{RedbSessionStore, StoreError};

#[cfg(feature = "session-store")]
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Runtime(#[from] RuntimeStoreError),
}

/// Backend-owned pairing of a session store with its matching runtime companion.
#[derive(Clone)]
pub struct PersistenceBundle {
    session_store: Arc<dyn SessionStore>,
    #[cfg(feature = "session-store")]
    runtime_store: Option<Arc<dyn RuntimeStore>>,
}

impl PersistenceBundle {
    #[cfg(feature = "session-store")]
    pub fn from_session_store(store: Arc<dyn SessionStore>) -> Result<Self, PersistenceError> {
        let runtime_store = runtime_store_for_session_store(&store)?;
        Ok(Self {
            session_store: store,
            runtime_store,
        })
    }

    #[cfg(not(feature = "session-store"))]
    pub fn from_session_store(store: Arc<dyn SessionStore>) -> Self {
        Self {
            session_store: store,
        }
    }

    pub fn session_store(&self) -> Arc<dyn SessionStore> {
        self.session_store.clone()
    }

    #[cfg(feature = "session-store")]
    pub fn runtime_store(&self) -> Option<Arc<dyn RuntimeStore>> {
        self.runtime_store.clone()
    }

    #[cfg(feature = "session-store")]
    pub fn runtime_adapter(&self) -> Arc<RuntimeSessionAdapter> {
        match &self.runtime_store {
            Some(store) => Arc::new(RuntimeSessionAdapter::persistent(store.clone())),
            None => Arc::new(RuntimeSessionAdapter::ephemeral()),
        }
    }

    #[cfg(feature = "session-store")]
    pub fn into_parts(self) -> (Arc<dyn SessionStore>, Option<Arc<dyn RuntimeStore>>) {
        (self.session_store, self.runtime_store)
    }
}

#[cfg(feature = "session-store")]
fn runtime_store_for_session_store(
    store: &Arc<dyn SessionStore>,
) -> Result<Option<Arc<dyn RuntimeStore>>, PersistenceError> {
    let any_store = store.as_ref() as &dyn Any;
    if let Some(redb_store) = any_store.downcast_ref::<RedbSessionStore>() {
        let runtime_store = meerkat_runtime::store::RedbRuntimeStore::new(redb_store.database())?;
        return Ok(Some(Arc::new(runtime_store)));
    }
    Ok(None)
}

#[cfg(all(test, feature = "session-store"))]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_core::{Session, SessionId, SessionMeta};
    use meerkat_runtime::store::RuntimeStoreError;
    use meerkat_store::{JsonlStore, MemoryStore, SessionFilter};
    use tempfile::TempDir;

    struct UnknownStore;

    #[async_trait]
    impl SessionStore for UnknownStore {
        async fn save(&self, _session: &Session) -> Result<(), StoreError> {
            Ok(())
        }

        async fn load(&self, _id: &SessionId) -> Result<Option<Session>, StoreError> {
            Ok(None)
        }

        async fn list(&self, _filter: SessionFilter) -> Result<Vec<SessionMeta>, StoreError> {
            Ok(Vec::new())
        }

        async fn delete(&self, _id: &SessionId) -> Result<(), StoreError> {
            Ok(())
        }
    }

    #[test]
    fn redb_bundle_resolves_runtime_store_and_persistent_adapter()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let store: Arc<dyn SessionStore> =
            Arc::new(RedbSessionStore::open(temp.path().join("sessions.redb"))?);

        let bundle = PersistenceBundle::from_session_store(store)?;

        assert!(bundle.runtime_store().is_some());
        let _ = bundle.runtime_adapter();
        Ok(())
    }

    #[test]
    fn memory_bundle_keeps_existing_session_store_behavior_without_runtime_companion()
    -> Result<(), Box<dyn std::error::Error>> {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());

        let bundle = PersistenceBundle::from_session_store(store)?;

        assert!(bundle.runtime_store().is_none());
        let _ = bundle.runtime_adapter();
        Ok(())
    }

    #[cfg(feature = "jsonl-store")]
    #[test]
    fn jsonl_bundle_keeps_existing_session_store_behavior_without_runtime_companion()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let store: Arc<dyn SessionStore> = Arc::new(JsonlStore::new(temp.path().join("jsonl")));

        let bundle = PersistenceBundle::from_session_store(store)?;

        assert!(bundle.runtime_store().is_none());
        let _ = bundle.runtime_adapter();
        Ok(())
    }

    #[test]
    fn persistence_error_runtime_variant_wraps_runtime_store_error() {
        let err = PersistenceError::from(RuntimeStoreError::WriteFailed("boom".to_string()));

        assert!(matches!(err, PersistenceError::Runtime(_)));
    }

    #[test]
    fn unknown_store_has_no_runtime_companion() -> Result<(), Box<dyn std::error::Error>> {
        let store: Arc<dyn SessionStore> = Arc::new(UnknownStore);

        let bundle = PersistenceBundle::from_session_store(store)?;

        assert!(bundle.runtime_store().is_none());
        Ok(())
    }
}
