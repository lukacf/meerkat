use std::sync::Arc;

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
use std::path::{Path, PathBuf};

use crate::SessionStore;
use meerkat_core::BlobStore;

#[cfg(feature = "session-store")]
use meerkat_runtime::{RuntimeSessionAdapter, RuntimeStore, RuntimeStoreError};
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
use meerkat_store::SqliteSessionStore;
#[cfg(all(feature = "session-store", target_arch = "wasm32"))]
use meerkat_store::StoreError;
#[cfg(all(
    feature = "session-store",
    feature = "jsonl-store",
    not(target_arch = "wasm32")
))]
use meerkat_store::{FsBlobStore, JsonlStore};
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
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    manifest: Option<RealmManifest>,
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    store_path: Option<PathBuf>,
    session_store: Arc<dyn SessionStore>,
    #[cfg(feature = "session-store")]
    runtime_store: Option<Arc<dyn RuntimeStore>>,
    blob_store: Arc<dyn BlobStore>,
    #[cfg(feature = "session-store")]
    runtime_adapter: Arc<RuntimeSessionAdapter>,
}

impl PersistenceBundle {
    #[cfg(feature = "session-store")]
    pub fn new(
        session_store: Arc<dyn SessionStore>,
        runtime_store: Option<Arc<dyn RuntimeStore>>,
        blob_store: Arc<dyn BlobStore>,
    ) -> Self {
        let runtime_adapter = match &runtime_store {
            Some(store) => Arc::new(RuntimeSessionAdapter::persistent(
                store.clone(),
                blob_store.clone(),
            )),
            None => Arc::new(RuntimeSessionAdapter::ephemeral()),
        };
        Self {
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            manifest: None,
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            store_path: None,
            session_store,
            runtime_store,
            blob_store,
            runtime_adapter,
        }
    }

    #[cfg(not(feature = "session-store"))]
    pub fn new(session_store: Arc<dyn SessionStore>, blob_store: Arc<dyn BlobStore>) -> Self {
        Self {
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            manifest: None,
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            store_path: None,
            session_store,
            blob_store,
        }
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    pub fn with_realm_context(
        manifest: RealmManifest,
        store_path: PathBuf,
        session_store: Arc<dyn SessionStore>,
        runtime_store: Option<Arc<dyn RuntimeStore>>,
        blob_store: Arc<dyn BlobStore>,
    ) -> Self {
        let mut bundle = Self::new(session_store, runtime_store, blob_store);
        bundle.manifest = Some(manifest);
        bundle.store_path = Some(store_path);
        bundle
    }

    pub fn session_store(&self) -> Arc<dyn SessionStore> {
        self.session_store.clone()
    }

    pub fn blob_store(&self) -> Arc<dyn BlobStore> {
        self.blob_store.clone()
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    pub fn manifest(&self) -> Option<&RealmManifest> {
        self.manifest.as_ref()
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    pub fn store_path(&self) -> Option<&Path> {
        self.store_path.as_deref()
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
    pub fn into_parts(
        self,
    ) -> (
        Arc<dyn SessionStore>,
        Option<Arc<dyn RuntimeStore>>,
        Arc<dyn BlobStore>,
    ) {
        (self.session_store, self.runtime_store, self.blob_store)
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
    let store_path = match manifest.backend {
        #[cfg(feature = "jsonl-store")]
        RealmBackend::Jsonl => paths.sessions_jsonl_dir.clone(),
        RealmBackend::Sqlite => paths.root.clone(),
        RealmBackend::Redb => paths.root.clone(),
    };

    let bundle = match manifest.backend {
        #[cfg(feature = "jsonl-store")]
        RealmBackend::Jsonl => {
            let session_store: Arc<dyn SessionStore> =
                Arc::new(JsonlStore::new(paths.sessions_jsonl_dir));
            let blob_store: Arc<dyn BlobStore> =
                Arc::new(FsBlobStore::new(paths.root.join("blobs")));
            PersistenceBundle::with_realm_context(
                manifest.clone(),
                store_path.clone(),
                session_store,
                None,
                blob_store,
            )
        }
        #[cfg(not(feature = "jsonl-store"))]
        RealmBackend::Jsonl => {
            return Err(StoreError::Internal(
                "jsonl realm opened without jsonl-store feature".to_string(),
            )
            .into());
        }
        RealmBackend::Sqlite => {
            let sqlite_store = Arc::new(SqliteSessionStore::open(paths.sessions_sqlite_path)?);
            let runtime_store = Arc::new(meerkat_runtime::store::SqliteRuntimeStore::new(
                sqlite_store.path().to_path_buf(),
            )?) as Arc<dyn RuntimeStore>;
            let blob_store: Arc<dyn BlobStore> =
                Arc::new(FsBlobStore::new(paths.root.join("blobs")));
            PersistenceBundle::with_realm_context(
                manifest.clone(),
                store_path.clone(),
                sqlite_store as Arc<dyn SessionStore>,
                Some(runtime_store),
                blob_store,
            )
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
            let blob_store: Arc<dyn BlobStore> =
                Arc::new(FsBlobStore::new(paths.root.join("blobs")));
            PersistenceBundle::with_realm_context(
                manifest.clone(),
                store_path.clone(),
                redb_store as Arc<dyn SessionStore>,
                Some(runtime_store),
                blob_store,
            )
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
    use meerkat_store::{MemoryBlobStore, MemoryStore, SessionFilter};
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

        let bundle = PersistenceBundle::new(
            wrapped,
            Some(runtime_store),
            Arc::new(MemoryBlobStore::new()),
        );

        assert!(bundle.runtime_store().is_some());
        assert!(!bundle.blob_store().is_persistent());
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
        assert!(bundle.blob_store().is_persistent());
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
        assert!(bundle.blob_store().is_persistent());
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn built_in_persistent_realms_construct_with_persistent_blob_stores()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;

        let (_sqlite_manifest, sqlite_bundle) = open_realm_persistence_in(
            temp.path(),
            "sqlite-realm",
            Some(RealmBackend::Sqlite),
            Some(RealmOrigin::Explicit),
        )
        .await?;
        assert!(
            sqlite_bundle.blob_store().is_persistent(),
            "sqlite realms must not pair durable stores with an in-memory blob store"
        );

        let (_redb_manifest, redb_bundle) = open_realm_persistence_in(
            temp.path(),
            "redb-realm-2",
            Some(RealmBackend::Redb),
            Some(RealmOrigin::Explicit),
        )
        .await?;
        assert!(
            redb_bundle.blob_store().is_persistent(),
            "redb realms must not pair durable stores with an in-memory blob store"
        );

        Ok(())
    }

    #[test]
    fn memory_bundle_keeps_existing_session_store_behavior_without_runtime_companion()
    -> Result<(), Box<dyn std::error::Error>> {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());

        let bundle = PersistenceBundle::new(store, None, Arc::new(MemoryBlobStore::new()));

        assert!(bundle.runtime_store().is_none());
        assert!(!bundle.blob_store().is_persistent());
        let _ = bundle.runtime_adapter();
        Ok(())
    }

    #[test]
    fn persistence_error_runtime_variant_wraps_runtime_store_error() {
        let err = PersistenceError::from(RuntimeStoreError::WriteFailed("boom".to_string()));

        assert!(matches!(err, PersistenceError::Runtime(_)));
    }
}
