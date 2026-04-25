use std::sync::Arc;

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
use std::path::{Path, PathBuf};

use crate::SessionStore;
use meerkat_core::{ArtifactStore, BlobStore};
use meerkat_schedule::{DisabledScheduleStore, ScheduleStore};
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
use meerkat_session::event_store::{EventStore, FileEventStore};
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
use meerkat_session::projector::SessionProjector;

#[cfg(feature = "session-store")]
use meerkat_runtime::{MeerkatMachine, RuntimeStore, RuntimeStoreError};
#[cfg(all(
    feature = "session-store",
    feature = "jsonl-store",
    not(target_arch = "wasm32")
))]
use meerkat_store::JsonlStore;
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
use meerkat_store::SqliteSessionStore;
#[cfg(all(feature = "session-store", target_arch = "wasm32"))]
use meerkat_store::StoreError;
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
use meerkat_store::{
    FsArtifactStore, FsBlobStore, RealmBackend, RealmManifest, RealmOrigin, SqliteScheduleStore,
    StoreError, ensure_realm_manifest_in, realm_paths_in,
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
    schedule_store: Arc<dyn ScheduleStore>,
    #[cfg(feature = "session-store")]
    runtime_store: Option<Arc<dyn RuntimeStore>>,
    blob_store: Arc<dyn BlobStore>,
    artifact_store: Arc<dyn ArtifactStore>,
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    event_store: Option<Arc<dyn EventStore>>,
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    projector: Option<Arc<SessionProjector>>,
    #[cfg(feature = "session-store")]
    runtime_adapter: Arc<MeerkatMachine>,
}

impl PersistenceBundle {
    #[cfg(feature = "session-store")]
    pub fn new(
        session_store: Arc<dyn SessionStore>,
        runtime_store: Option<Arc<dyn RuntimeStore>>,
        blob_store: Arc<dyn BlobStore>,
    ) -> Self {
        Self::new_with_schedule_store(
            session_store,
            runtime_store,
            blob_store,
            Arc::new(DisabledScheduleStore),
        )
    }

    #[cfg(feature = "session-store")]
    pub fn new_with_schedule_store(
        session_store: Arc<dyn SessionStore>,
        runtime_store: Option<Arc<dyn RuntimeStore>>,
        blob_store: Arc<dyn BlobStore>,
        schedule_store: Arc<dyn ScheduleStore>,
    ) -> Self {
        let runtime_adapter = match &runtime_store {
            Some(store) => Arc::new(MeerkatMachine::persistent(
                store.clone(),
                blob_store.clone(),
            )),
            None => Arc::new(MeerkatMachine::ephemeral()),
        };
        Self {
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            manifest: None,
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            store_path: None,
            session_store,
            schedule_store,
            runtime_store,
            blob_store,
            artifact_store: Arc::new(meerkat_store::MemoryArtifactStore::new()),
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            event_store: None,
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            projector: None,
            runtime_adapter,
        }
    }

    #[cfg(not(feature = "session-store"))]
    pub fn new(session_store: Arc<dyn SessionStore>, blob_store: Arc<dyn BlobStore>) -> Self {
        Self::new_with_schedule_store(session_store, blob_store, Arc::new(DisabledScheduleStore))
    }

    #[cfg(not(feature = "session-store"))]
    pub fn new_with_schedule_store(
        session_store: Arc<dyn SessionStore>,
        blob_store: Arc<dyn BlobStore>,
        schedule_store: Arc<dyn ScheduleStore>,
    ) -> Self {
        Self {
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            manifest: None,
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            store_path: None,
            session_store,
            schedule_store,
            blob_store,
            artifact_store: Arc::new(meerkat_store::MemoryArtifactStore::new()),
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            event_store: None,
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            projector: None,
        }
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    pub fn with_realm_context(
        manifest: RealmManifest,
        store_path: PathBuf,
        projection_root: PathBuf,
        session_store: Arc<dyn SessionStore>,
        runtime_store: Option<Arc<dyn RuntimeStore>>,
        blob_store: Arc<dyn BlobStore>,
        schedule_store: Arc<dyn ScheduleStore>,
    ) -> Self {
        let mut bundle =
            Self::new_with_schedule_store(session_store, runtime_store, blob_store, schedule_store);
        let event_store: Arc<dyn EventStore> = Arc::new(FileEventStore::new(
            projection_root.join(".rkat").join("events"),
        ));
        bundle.event_store = Some(event_store);
        bundle.projector = Some(Arc::new(SessionProjector::new(
            projection_root.join(".rkat"),
        )));
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

    pub fn artifact_store(&self) -> Arc<dyn ArtifactStore> {
        self.artifact_store.clone()
    }

    pub fn schedule_store(&self) -> Arc<dyn ScheduleStore> {
        self.schedule_store.clone()
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
    pub fn runtime_adapter(&self) -> Arc<MeerkatMachine> {
        self.runtime_adapter.clone()
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    pub fn event_projection(&self) -> Option<(Arc<dyn EventStore>, Arc<SessionProjector>)> {
        Some((self.event_store.clone()?, self.projector.clone()?))
    }

    #[cfg(feature = "session-store")]
    #[allow(clippy::type_complexity)]
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
    };

    let bundle = match manifest.backend {
        #[cfg(feature = "jsonl-store")]
        RealmBackend::Jsonl => {
            let session_store: Arc<dyn SessionStore> =
                Arc::new(JsonlStore::new(paths.sessions_jsonl_dir));
            let blob_store: Arc<dyn BlobStore> =
                Arc::new(FsBlobStore::new(paths.root.join("blobs")));
            let artifact_store: Arc<dyn ArtifactStore> =
                Arc::new(FsArtifactStore::new(paths.root.join("artifacts")));
            let schedule_store: Arc<dyn ScheduleStore> = Arc::new(DisabledScheduleStore);
            let mut bundle = PersistenceBundle::with_realm_context(
                manifest.clone(),
                store_path,
                paths.root,
                session_store,
                None,
                blob_store,
                schedule_store,
            );
            bundle.artifact_store = artifact_store;
            bundle
        }
        RealmBackend::Sqlite => {
            let sqlite_store = Arc::new(SqliteSessionStore::open(
                paths.sessions_sqlite_path.clone(),
            )?);
            let schedule_store = Arc::new(SqliteScheduleStore::open(
                paths.sessions_sqlite_path.clone(),
            )?) as Arc<dyn ScheduleStore>;
            let runtime_store = Arc::new(meerkat_runtime::store::SqliteRuntimeStore::new(
                sqlite_store.path().to_path_buf(),
            )?) as Arc<dyn RuntimeStore>;
            let blob_store: Arc<dyn BlobStore> =
                Arc::new(FsBlobStore::new(paths.root.join("blobs")));
            let artifact_store: Arc<dyn ArtifactStore> =
                Arc::new(FsArtifactStore::new(paths.root.join("artifacts")));
            let mut bundle = PersistenceBundle::with_realm_context(
                manifest.clone(),
                store_path,
                paths.root,
                sqlite_store as Arc<dyn SessionStore>,
                Some(runtime_store),
                blob_store,
                schedule_store,
            );
            bundle.artifact_store = artifact_store;
            bundle
        }
    };

    Ok((manifest, bundle))
}

#[cfg(all(test, feature = "session-store"))]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_core::event::AgentEvent;
    use meerkat_core::{Session, SessionId, SessionMeta};
    use meerkat_runtime::store::RuntimeStoreError;
    #[cfg(feature = "memory-store")]
    use meerkat_store::MemoryStore;
    use meerkat_store::{MemoryBlobStore, SessionFilter, SessionStoreError};
    use tempfile::TempDir;

    struct WrappedStore {
        inner: Arc<dyn SessionStore>,
    }

    #[async_trait]
    impl SessionStore for WrappedStore {
        async fn save(&self, session: &Session) -> Result<(), SessionStoreError> {
            self.inner.save(session).await
        }

        async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
            self.inner.load(id).await
        }

        async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, SessionStoreError> {
            self.inner.list(filter).await
        }

        async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError> {
            self.inner.delete(id).await
        }
    }

    #[test]
    fn wrapped_sqlite_store_can_keep_runtime_companion() -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let sqlite_store = Arc::new(SqliteSessionStore::open(
            temp.path().join("sessions.sqlite3"),
        )?);
        let wrapped: Arc<dyn SessionStore> = Arc::new(WrappedStore {
            inner: sqlite_store.clone(),
        });
        let runtime_store = Arc::new(meerkat_runtime::store::SqliteRuntimeStore::new(
            sqlite_store.path().to_path_buf(),
        )?) as Arc<dyn RuntimeStore>;

        let bundle = PersistenceBundle::new(
            wrapped,
            Some(runtime_store),
            Arc::new(MemoryBlobStore::new()),
        );

        assert!(bundle.runtime_store().is_some());
        assert!(!bundle.blob_store().is_persistent());
        assert!(!bundle.artifact_store().is_persistent());
        let _ = bundle.runtime_adapter();
        Ok(())
    }

    #[tokio::test]
    async fn open_realm_persistence_sqlite_builds_runtime_companion()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;

        let (_manifest, bundle) = open_realm_persistence_in(
            temp.path(),
            "sqlite-realm",
            Some(RealmBackend::Sqlite),
            Some(RealmOrigin::Explicit),
        )
        .await?;

        assert!(bundle.runtime_store().is_some());
        assert!(bundle.blob_store().is_persistent());
        assert!(bundle.artifact_store().is_persistent());
        let (event_store, projector) = bundle
            .event_projection()
            .expect("realm persistence must wire event projection");
        let expected_paths = realm_paths_in(temp.path(), "sqlite-realm");
        assert_eq!(projector.output_dir(), expected_paths.root.join(".rkat"));

        let session_id = SessionId::new();
        event_store
            .append(&session_id, &[AgentEvent::TurnStarted { turn_number: 1 }])
            .await?;
        assert!(
            expected_paths
                .root
                .join(".rkat")
                .join("events")
                .join(format!("{session_id}.jsonl"))
                .exists(),
            "realm append log must live under the .rkat subtree"
        );
        projector
            .project(event_store.as_ref(), &session_id, 1)
            .await?;
        assert!(
            expected_paths
                .root
                .join(".rkat")
                .join("sessions")
                .join(session_id.to_string())
                .join("events.jsonl")
                .exists(),
            "realm event projection must materialize under the realm root"
        );
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
        assert!(
            bundle.event_projection().is_some(),
            "jsonl realms still need the append-only event projection bridge"
        );
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

        Ok(())
    }

    #[cfg(feature = "memory-store")]
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
