use std::sync::Arc;

use crate::SessionStore;

#[cfg(feature = "session-store")]
use meerkat_runtime::{RuntimeSessionAdapter, RuntimeStore, RuntimeStoreError};
#[cfg(all(
    feature = "session-store",
    feature = "jsonl-store",
    not(target_arch = "wasm32")
))]
use meerkat_store::JsonlStore;
#[cfg(all(feature = "session-store", target_arch = "wasm32"))]
use meerkat_store::StoreError;
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
use meerkat_store::{
    RealmBackend, RealmManifest, RealmOrigin, RedbSessionStore, StoreError,
    ensure_realm_manifest_in, realm_paths_in,
};

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
    #[cfg(feature = "session-store")]
    runtime_adapter: Arc<RuntimeSessionAdapter>,
}

impl PersistenceBundle {
    #[cfg(feature = "session-store")]
    pub fn new(
        session_store: Arc<dyn SessionStore>,
        runtime_store: Option<Arc<dyn RuntimeStore>>,
    ) -> Self {
        let runtime_adapter = match &runtime_store {
            Some(store) => Arc::new(RuntimeSessionAdapter::persistent(store.clone())),
            None => Arc::new(RuntimeSessionAdapter::ephemeral()),
        };
        Self {
            session_store,
            runtime_store,
            runtime_adapter,
        }
    }

    #[cfg(not(feature = "session-store"))]
    pub fn new(session_store: Arc<dyn SessionStore>) -> Self {
        Self { session_store }
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
        self.runtime_adapter.clone()
    }

    #[cfg(feature = "session-store")]
    pub fn into_parts(self) -> (Arc<dyn SessionStore>, Option<Arc<dyn RuntimeStore>>) {
        (self.session_store, self.runtime_store)
    }
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub async fn open_realm_persistence_in(
    realms_root: &std::path::Path,
    realm_id: &str,
    backend_hint: Option<RealmBackend>,
    origin_hint: Option<RealmOrigin>,
) -> Result<(RealmManifest, PersistenceBundle), PersistenceError> {
    let manifest =
        ensure_realm_manifest_in(realms_root, realm_id, backend_hint, origin_hint).await?;
    let paths = realm_paths_in(realms_root, realm_id);

    let bundle = match manifest.backend {
        #[cfg(feature = "jsonl-store")]
        RealmBackend::Jsonl => {
            let session_store: Arc<dyn SessionStore> =
                Arc::new(JsonlStore::new(paths.sessions_jsonl_dir));
            PersistenceBundle::new(session_store, None)
        }
        #[cfg(not(feature = "jsonl-store"))]
        RealmBackend::Jsonl => {
            return Err(StoreError::Internal(
                "jsonl realm opened without jsonl-store feature".to_string(),
            )
            .into());
        }
        RealmBackend::Redb => {
            if let Some(parent) = paths.sessions_redb_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(StoreError::Io)?;
            }
            let redb_path = paths.sessions_redb_path.clone();
            let redb_store = tokio::task::spawn_blocking(move || RedbSessionStore::open(redb_path))
                .await
                .map_err(StoreError::Join)??;
            let redb_store = Arc::new(redb_store);
            let runtime_store = Arc::new(meerkat_runtime::store::RedbRuntimeStore::new(
                redb_store.database(),
            )?) as Arc<dyn RuntimeStore>;
            PersistenceBundle::new(redb_store as Arc<dyn SessionStore>, Some(runtime_store))
        }
    };

    Ok((manifest, bundle))
}

#[cfg(all(test, feature = "session-store"))]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_core::{Session, SessionId, SessionMeta};
    use meerkat_runtime::store::RuntimeStoreError;
    use meerkat_store::{MemoryStore, SessionFilter};
    use tempfile::TempDir;

    struct WrappedStore {
        inner: Arc<dyn SessionStore>,
    }

    #[async_trait]
    impl SessionStore for WrappedStore {
        async fn save(&self, session: &Session) -> Result<(), StoreError> {
            self.inner.save(session).await
        }

        async fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
            self.inner.load(id).await
        }

        async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, StoreError> {
            self.inner.list(filter).await
        }

        async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
            self.inner.delete(id).await
        }
    }

    #[test]
    fn wrapped_redb_store_can_keep_runtime_companion() -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let redb_store = Arc::new(RedbSessionStore::open(temp.path().join("sessions.redb"))?);
        let wrapped: Arc<dyn SessionStore> = Arc::new(WrappedStore {
            inner: redb_store.clone(),
        });
        let runtime_store = Arc::new(meerkat_runtime::store::RedbRuntimeStore::new(
            redb_store.database(),
        )?) as Arc<dyn RuntimeStore>;

        let bundle = PersistenceBundle::new(wrapped, Some(runtime_store));

        assert!(bundle.runtime_store().is_some());
        let _ = bundle.runtime_adapter();
        Ok(())
    }

    #[tokio::test]
    async fn open_realm_persistence_redb_builds_runtime_companion()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;

        let (_manifest, bundle) = open_realm_persistence_in(
            temp.path(),
            "redb-realm",
            Some(RealmBackend::Redb),
            Some(RealmOrigin::Explicit),
        )
        .await?;

        assert!(bundle.runtime_store().is_some());
        Ok(())
    }

    #[cfg(feature = "jsonl-store")]
    #[tokio::test]
    async fn open_realm_persistence_jsonl_has_no_runtime_companion()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;

        let (_manifest, bundle) = open_realm_persistence_in(
            temp.path(),
            "jsonl-realm",
            Some(RealmBackend::Jsonl),
            Some(RealmOrigin::Explicit),
        )
        .await?;

        assert!(bundle.runtime_store().is_none());
        Ok(())
    }

    #[test]
    fn memory_bundle_keeps_existing_session_store_behavior_without_runtime_companion()
    -> Result<(), Box<dyn std::error::Error>> {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());

        let bundle = PersistenceBundle::new(store, None);

        assert!(bundle.runtime_store().is_none());
        let _ = bundle.runtime_adapter();
        Ok(())
    }

    #[test]
    fn persistence_error_runtime_variant_wraps_runtime_store_error() {
        let err = PersistenceError::from(RuntimeStoreError::WriteFailed("boom".to_string()));

        assert!(matches!(err, PersistenceError::Runtime(_)));
    }
}
