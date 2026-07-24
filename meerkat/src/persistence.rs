use std::sync::Arc;

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
use std::path::{Path, PathBuf};

use crate::SessionStore;
use meerkat_core::{ArtifactStore, BlobStore};
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
use meerkat_schedule::MemoryScheduleStore;
use meerkat_schedule::{DisabledScheduleStore, ScheduleStore};
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
use meerkat_session::event_store::{EventStore, FileEventStore};
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
use meerkat_session::projector::SessionProjector;
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
use meerkat_workgraph::MemoryWorkGraphStore;
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
use meerkat_workgraph::SqliteWorkGraphStore;
use meerkat_workgraph::{DisabledWorkGraphStore, WorkGraphStore};

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
    StoreError, realm_paths_in,
};
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
use meerkat_store::{MemoryBlobStore, MemoryStore};

#[cfg(feature = "session-store")]
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Runtime(#[from] RuntimeStoreError),
    #[error(transparent)]
    WorkGraph(#[from] meerkat_workgraph::WorkGraphError),
    #[error(transparent)]
    Jobs(#[from] meerkat_jobs::DetachedJobError),
    /// Resolving the storage layout for an open failed (invalid realm id,
    /// undeterminable root probe, identity-colliding realm directory, ...).
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    #[error(transparent)]
    Bootstrap(#[from] meerkat_core::RuntimeBootstrapError),
    /// Cross-candidate first-start refusal: the realm was concurrently
    /// materialized under a different candidate root, or the reservation
    /// stayed contended past the bounded wait. (Plain store errors from the
    /// same protocol surface as [`PersistenceError::Store`].)
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    #[error(transparent)]
    FirstStart(meerkat_store::realm::RealmFirstStartError),
    /// A `Durable` storage slot resolved to a non-persistent store without
    /// the realm manifest declaring that domain ephemeral (fail-closed
    /// durability; see `storage_provider`).
    #[error(
        "durable storage domain '{domain}' resolved to a non-persistent store without an \
         ephemeral declaration in the realm manifest; refusing to start"
    )]
    DurabilityViolation { domain: String },
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
impl From<meerkat_store::realm::RealmFirstStartError> for PersistenceError {
    fn from(err: meerkat_store::realm::RealmFirstStartError) -> Self {
        match err {
            // Unwrap plain store failures so existing `Store(_)` matching
            // keeps seeing them; only the reservation refusals are new.
            meerkat_store::realm::RealmFirstStartError::Store(store) => Self::Store(store),
            other => Self::FirstStart(other),
        }
    }
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
    workgraph_store: Arc<dyn WorkGraphStore>,
    job_store: Arc<dyn meerkat_jobs::DetachedJobStore>,
    #[cfg(feature = "session-store")]
    runtime_store: Arc<dyn RuntimeStore>,
    blob_store: Arc<dyn BlobStore>,
    artifact_store: Arc<dyn ArtifactStore>,
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    event_store: Option<Arc<dyn EventStore>>,
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    projector: Option<Arc<SessionProjector>>,
    #[cfg(feature = "session-store")]
    runtime_adapter: Arc<MeerkatMachine>,
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
struct RealmSubsystemStores {
    session_store: Arc<dyn SessionStore>,
    runtime_store: Arc<dyn RuntimeStore>,
    blob_store: Arc<dyn BlobStore>,
    schedule_store: Arc<dyn ScheduleStore>,
    workgraph_store: Arc<dyn WorkGraphStore>,
    job_store: Arc<dyn meerkat_jobs::DetachedJobStore>,
}

impl PersistenceBundle {
    #[cfg(feature = "session-store")]
    pub fn new(
        session_store: Arc<dyn SessionStore>,
        runtime_store: Arc<dyn RuntimeStore>,
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
        runtime_store: Arc<dyn RuntimeStore>,
        blob_store: Arc<dyn BlobStore>,
        schedule_store: Arc<dyn ScheduleStore>,
    ) -> Self {
        Self::new_with_subsystem_stores(
            session_store,
            runtime_store,
            blob_store,
            schedule_store,
            Arc::new(DisabledWorkGraphStore),
        )
    }

    #[cfg(feature = "session-store")]
    pub fn new_with_subsystem_stores(
        session_store: Arc<dyn SessionStore>,
        runtime_store: Arc<dyn RuntimeStore>,
        blob_store: Arc<dyn BlobStore>,
        schedule_store: Arc<dyn ScheduleStore>,
        workgraph_store: Arc<dyn WorkGraphStore>,
    ) -> Self {
        let runtime_adapter = Arc::new(MeerkatMachine::persistent(
            runtime_store.clone(),
            blob_store.clone(),
        ));
        Self {
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            manifest: None,
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            store_path: None,
            session_store,
            schedule_store,
            workgraph_store,
            job_store: Arc::new(meerkat_jobs::MemoryDetachedJobStore::new()),
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
        Self::new_with_subsystem_stores(
            session_store,
            blob_store,
            schedule_store,
            Arc::new(DisabledWorkGraphStore),
        )
    }

    #[cfg(not(feature = "session-store"))]
    pub fn new_with_subsystem_stores(
        session_store: Arc<dyn SessionStore>,
        blob_store: Arc<dyn BlobStore>,
        schedule_store: Arc<dyn ScheduleStore>,
        workgraph_store: Arc<dyn WorkGraphStore>,
    ) -> Self {
        Self {
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            manifest: None,
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            store_path: None,
            session_store,
            schedule_store,
            workgraph_store,
            job_store: Arc::new(meerkat_jobs::MemoryDetachedJobStore::new()),
            blob_store,
            artifact_store: Arc::new(meerkat_store::MemoryArtifactStore::new()),
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            event_store: None,
            #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
            projector: None,
        }
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    fn with_realm_context(
        manifest: RealmManifest,
        store_path: PathBuf,
        projection_root: PathBuf,
        stores: RealmSubsystemStores,
    ) -> Self {
        let mut bundle = Self::new_with_subsystem_stores(
            stores.session_store,
            stores.runtime_store,
            stores.blob_store,
            stores.schedule_store,
            stores.workgraph_store,
        );
        bundle.job_store = stores.job_store;
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

    pub fn workgraph_store(&self) -> Arc<dyn WorkGraphStore> {
        self.workgraph_store.clone()
    }

    pub fn job_store(&self) -> Arc<dyn meerkat_jobs::DetachedJobStore> {
        self.job_store.clone()
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
    pub fn runtime_store(&self) -> Arc<dyn RuntimeStore> {
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
        Arc<dyn RuntimeStore>,
        Arc<dyn BlobStore>,
    ) {
        (self.session_store, self.runtime_store, self.blob_store)
    }
}

/// Build the [`meerkat_core::StorageLayout`] for an open whose state root
/// the caller already resolved. The root is threaded as the explicit state
/// root (no dual-root probing — the caller's resolution already happened),
/// while the ambient user/project slots resolve through the same bootstrap
/// machinery the surfaces use, so the provider seam always receives ONE
/// layout authority instead of composing roots independently.
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub(crate) fn layout_for_explicit_state_root(
    realms_root: &std::path::Path,
    realm_id: &str,
) -> Result<meerkat_core::StorageLayout, PersistenceError> {
    use meerkat_core::{RealmConfig, RealmSelection, StorageLayoutInputs};
    let realm_config = RealmConfig {
        selection: RealmSelection::Explicit {
            realm_id: realm_id.to_string(),
        },
        state_root: Some(realms_root.to_path_buf()),
        ..RealmConfig::default()
    };
    let inputs = StorageLayoutInputs {
        invocation_context: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        ..StorageLayoutInputs::default()
    };
    let resolved = meerkat_core::StorageLayout::resolve(inputs, &realm_config)?;
    Ok(resolved.layout)
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub async fn open_realm_persistence_in(
    realms_root: &std::path::Path,
    realm_id: &str,
    backend_hint: Option<RealmBackend>,
    origin_hint: Option<RealmOrigin>,
) -> Result<(RealmManifest, PersistenceBundle), PersistenceError> {
    let layout = layout_for_explicit_state_root(realms_root, realm_id)?;
    open_realm_persistence_builtin_with_layout(layout, realm_id, backend_hint, origin_hint).await
}

/// Built-in disk open through an externally resolved
/// [`meerkat_core::StorageLayout`]: the layout's state root is the realm
/// root and the layout (with its realm-root candidates, arming the
/// cross-candidate first-start reservation) threads into the provider
/// context. Surfaces that already resolved a layout call this (via
/// `storage_provider::open_realm_persistence_with_layout`) instead of
/// resolving twice.
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub(crate) async fn open_realm_persistence_builtin_with_layout(
    layout: meerkat_core::StorageLayout,
    realm_id: &str,
    backend_hint: Option<RealmBackend>,
    origin_hint: Option<RealmOrigin>,
) -> Result<(RealmManifest, PersistenceBundle), PersistenceError> {
    let realms_root = layout.state_root().to_path_buf();
    let (pin, bundle) = open_realm_persistence_with_provider(
        &crate::storage_provider::DiskStorageProvider,
        &realms_root,
        realm_id,
        backend_hint,
        origin_hint,
        Some(layout),
    )
    .await?;
    match pin {
        meerkat_store::RealmManifestPin::Builtin(manifest) => Ok((manifest, bundle)),
        meerkat_store::RealmManifestPin::External(manifest) => {
            // Unreachable through the disk provider (its ensure refuses
            // external pins), kept typed rather than panicking.
            Err(PersistenceError::Store(StoreError::ExternalProviderRealm {
                realm_id: manifest.realm.as_str().to_string(),
                provider: manifest.provider,
            }))
        }
    }
}

/// Bootstrap convergence: ensure the manifest, open the realm's stores
/// through the provider seam, enforce fail-closed durability, and compose
/// the bundle (event projection included when the provider names a
/// projection root).
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub async fn open_realm_persistence_with_provider(
    provider: &dyn crate::storage_provider::RealmStorageProvider,
    realms_root: &std::path::Path,
    realm_id: &str,
    backend_hint: Option<RealmBackend>,
    origin_hint: Option<RealmOrigin>,
    layout: Option<meerkat_core::StorageLayout>,
) -> Result<(meerkat_store::RealmManifestPin, PersistenceBundle), PersistenceError> {
    // Provider-aware ensure: the disk provider keeps the historical
    // builtin-only semantics; a named external provider accepts (and
    // creates) exactly its own pins, so external realms are openable
    // through the seam they were pinned for. When the layout carries
    // dual-root candidates, first materialization runs under the
    // cross-candidate reservation so a concurrent first start with a
    // different default root cannot manufacture a split brain.
    let provider_pin_name = (provider.name() != "disk").then(|| provider.name());
    let candidate_roots: Vec<std::path::PathBuf> = layout
        .as_ref()
        .map(|layout| layout.realm_root_candidates().to_vec())
        .unwrap_or_default();
    let manifest = meerkat_store::realm::ensure_realm_manifest_pin_with_candidates(
        realms_root,
        &candidate_roots,
        realm_id,
        provider_pin_name,
        backend_hint,
        origin_hint,
    )
    .await?;
    let paths = realm_paths_in(realms_root, realm_id);
    let realm = meerkat_core::RealmId::parse(realm_id)
        .map_err(|_| StoreError::InvalidRealmSlug(realm_id.to_string()))?;
    let ctx = crate::storage_provider::RealmOpenContext {
        locator: meerkat_core::RealmLocator {
            state_root: realms_root.to_path_buf(),
            realm,
        },
        manifest: manifest.clone(),
        paths,
        layout,
    };
    let set = provider.open(&ctx).await?;
    crate::storage_provider::enforce_fail_closed_durability(&set, manifest.ephemeral_domains())?;

    let builtin_manifest = manifest.as_builtin().cloned();
    let mut bundle = if let (Some(projection_root), Some(builtin)) =
        (set.projection_root.clone(), builtin_manifest.clone())
    {
        PersistenceBundle::with_realm_context(
            builtin,
            set.store_path.clone(),
            projection_root,
            RealmSubsystemStores {
                session_store: set.session_store.clone(),
                runtime_store: set.runtime_store.clone(),
                blob_store: set.blob_store.clone(),
                schedule_store: set.schedule_store.clone(),
                workgraph_store: set.workgraph_store.clone(),
                job_store: set.job_store.clone(),
            },
        )
    } else {
        let mut bundle = PersistenceBundle::new_with_subsystem_stores(
            set.session_store.clone(),
            set.runtime_store.clone(),
            set.blob_store.clone(),
            set.schedule_store.clone(),
            set.workgraph_store.clone(),
        );
        bundle.manifest = builtin_manifest;
        bundle.store_path = Some(set.store_path.clone());
        bundle.job_store = set.job_store.clone();
        bundle
    };
    bundle.artifact_store = set.artifact_store.clone();

    Ok((manifest, bundle))
}

/// The built-in disk composition (sqlite / jsonl / memory), unchanged in
/// behavior from before the provider seam existed. Crate-visible so the
/// `DiskStorageProvider` stays a thin adapter.
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub(crate) fn open_disk_store_set(
    ctx: &crate::storage_provider::RealmOpenContext,
) -> Result<crate::storage_provider::RealmStoreSet, PersistenceError> {
    use crate::storage_provider::RealmStoreSet;
    use meerkat_core::{DurabilityDeclaration, DurabilityResolution};
    let paths = &ctx.paths;
    // The disk provider only ever receives builtin pins (its ensure path
    // refuses external manifests); keep the refusal typed regardless.
    let Some(manifest) = ctx.manifest.as_builtin() else {
        return Err(PersistenceError::Store(StoreError::ExternalProviderRealm {
            realm_id: ctx.locator.realm.as_str().to_string(),
            provider: ctx
                .manifest
                .provider_name()
                .unwrap_or("unknown")
                .to_string(),
        }));
    };
    let durable_disk =
        |domain: &str| DurabilityDeclaration::durable(domain, DurabilityResolution::Persistent);
    let declared_ephemeral = |domain: &str| {
        DurabilityDeclaration::durable(domain, DurabilityResolution::DeclaredEphemeral)
    };

    match manifest.backend {
        #[cfg(feature = "jsonl-store")]
        RealmBackend::Jsonl => {
            let session_store: Arc<dyn SessionStore> =
                Arc::new(JsonlStore::new(paths.sessions_jsonl_dir.clone()));
            let blob_store: Arc<dyn BlobStore> =
                Arc::new(FsBlobStore::new(paths.root.join("blobs")));
            let artifact_store: Arc<dyn ArtifactStore> =
                Arc::new(FsArtifactStore::new(paths.root.join("artifacts")));
            let schedule_store: Arc<dyn ScheduleStore> = Arc::new(DisabledScheduleStore);
            let workgraph_store: Arc<dyn WorkGraphStore> = Arc::new(SqliteWorkGraphStore::open(
                paths.root.join("workgraph.sqlite3"),
            )?);
            let runtime_store = Arc::new(meerkat_runtime::store::SqliteRuntimeStore::new(
                paths.runtime_sqlite_path.clone(),
            )?) as Arc<dyn RuntimeStore>;
            let job_store = Arc::new(meerkat_jobs::SqliteDetachedJobStore::open(
                paths.jobs_sqlite_path.clone(),
            )?) as Arc<dyn meerkat_jobs::DetachedJobStore>;
            Ok(RealmStoreSet {
                session_store,
                runtime_store,
                schedule_store,
                workgraph_store,
                job_store,
                blob_store,
                artifact_store,
                store_path: paths.sessions_jsonl_dir.clone(),
                projection_root: Some(paths.root.clone()),
                durability: vec![
                    durable_disk("sessions"),
                    durable_disk("runtime"),
                    durable_disk("workgraph"),
                    durable_disk("jobs"),
                    durable_disk("blobs"),
                    durable_disk("artifacts"),
                    // Scheduling is disabled on the jsonl backend by design.
                    DurabilityDeclaration::durable(
                        "schedule",
                        DurabilityResolution::DeclaredEphemeral,
                    ),
                ],
            })
        }
        RealmBackend::Memory => {
            // The memory backend IS the ephemeral declaration: every slot
            // resolves declared-ephemeral rather than silently
            // non-persistent.
            let session_store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
            let blob_store: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
            let artifact_store: Arc<dyn ArtifactStore> =
                Arc::new(meerkat_store::MemoryArtifactStore::new());
            let schedule_store: Arc<dyn ScheduleStore> = Arc::new(MemoryScheduleStore::new());
            let workgraph_store: Arc<dyn WorkGraphStore> = Arc::new(MemoryWorkGraphStore::new());
            let runtime_store = Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new())
                as Arc<dyn RuntimeStore>;
            let job_store = Arc::new(meerkat_jobs::MemoryDetachedJobStore::new())
                as Arc<dyn meerkat_jobs::DetachedJobStore>;
            Ok(RealmStoreSet {
                session_store,
                runtime_store,
                schedule_store,
                workgraph_store,
                job_store,
                blob_store,
                artifact_store,
                store_path: paths.root.clone(),
                projection_root: None,
                durability: [
                    "sessions",
                    "runtime",
                    "schedule",
                    "workgraph",
                    "jobs",
                    "blobs",
                    "artifacts",
                ]
                .iter()
                .map(|domain| declared_ephemeral(domain))
                .collect(),
            })
        }
        RealmBackend::Sqlite => {
            let sqlite_store = Arc::new(SqliteSessionStore::open(
                paths.sessions_sqlite_path.clone(),
            )?);
            let schedule_store = Arc::new(SqliteScheduleStore::open(
                paths.sessions_sqlite_path.clone(),
            )?) as Arc<dyn ScheduleStore>;
            let workgraph_store = Arc::new(SqliteWorkGraphStore::open(
                paths.root.join("workgraph.sqlite3"),
            )?) as Arc<dyn WorkGraphStore>;
            let runtime_store = Arc::new(meerkat_runtime::store::SqliteRuntimeStore::new(
                sqlite_store.path().to_path_buf(),
            )?) as Arc<dyn RuntimeStore>;
            let job_store = Arc::new(meerkat_jobs::SqliteDetachedJobStore::open(
                paths.jobs_sqlite_path.clone(),
            )?) as Arc<dyn meerkat_jobs::DetachedJobStore>;
            let blob_store: Arc<dyn BlobStore> =
                Arc::new(FsBlobStore::new(paths.root.join("blobs")));
            let artifact_store: Arc<dyn ArtifactStore> =
                Arc::new(FsArtifactStore::new(paths.root.join("artifacts")));
            Ok(RealmStoreSet {
                session_store: sqlite_store as Arc<dyn SessionStore>,
                runtime_store,
                schedule_store,
                workgraph_store,
                job_store,
                blob_store,
                artifact_store,
                store_path: paths.root.clone(),
                projection_root: Some(paths.root.clone()),
                durability: [
                    "sessions",
                    "runtime",
                    "schedule",
                    "workgraph",
                    "jobs",
                    "blobs",
                    "artifacts",
                ]
                .iter()
                .map(|domain| durable_disk(domain))
                .collect(),
            })
        }
    }
}

#[cfg(all(test, feature = "session-store"))]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_core::event::AgentEvent;
    use meerkat_core::{Session, SessionId, SessionMeta};
    use meerkat_runtime::store::RuntimeStoreError;
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

        async fn save_authoritative_projection(
            &self,
            session: &Session,
        ) -> Result<(), SessionStoreError> {
            self.inner.save_authoritative_projection(session).await
        }

        async fn save_authoritative_projection_if_current_revision(
            &self,
            session: &Session,
            expected_current_revision: Option<String>,
        ) -> Result<(), SessionStoreError> {
            self.inner
                .save_authoritative_projection_if_current_revision(
                    session,
                    expected_current_revision,
                )
                .await
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

        async fn delete_if_current_revision(
            &self,
            id: &SessionId,
            expected_current_revision: &str,
        ) -> Result<bool, SessionStoreError> {
            self.inner
                .delete_if_current_revision(id, expected_current_revision)
                .await
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

        let bundle =
            PersistenceBundle::new(wrapped, runtime_store, Arc::new(MemoryBlobStore::new()));

        assert!(!bundle.blob_store().is_persistent());
        assert!(!bundle.artifact_store().is_persistent());
        let _ = bundle.runtime_store();
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
    async fn open_realm_persistence_jsonl_builds_durable_runtime_companion()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;

        let (_manifest, bundle) = open_realm_persistence_in(
            temp.path(),
            "jsonl-realm",
            Some(RealmBackend::Jsonl),
            Some(RealmOrigin::Explicit),
        )
        .await?;

        assert!(bundle.blob_store().is_persistent());
        assert!(
            bundle.event_projection().is_some(),
            "jsonl realms still need the append-only event projection bridge"
        );

        let expected_paths = realm_paths_in(temp.path(), "jsonl-realm");
        assert!(
            expected_paths.runtime_sqlite_path.exists(),
            "jsonl realms must mount the sqlite runtime companion at the realm root"
        );

        let session = meerkat_core::Session::new();
        let session_id = session.id().clone();
        let runtime_id = meerkat_runtime::identifiers::LogicalRuntimeId::for_session(&session_id);
        bundle
            .runtime_store()
            .commit_session_snapshot(
                &runtime_id,
                meerkat_runtime::store::SessionDelta {
                    session_snapshot: serde_json::to_vec(&session)?,
                },
            )
            .await?;
        drop(bundle);

        let (_manifest, reopened) = open_realm_persistence_in(
            temp.path(),
            "jsonl-realm",
            Some(RealmBackend::Jsonl),
            Some(RealmOrigin::Explicit),
        )
        .await?;
        let recovered = reopened
            .runtime_store()
            .load_session_snapshot(&runtime_id)
            .await?
            .expect("jsonl runtime companion must recover runtime authority across reopen");
        let recovered_session: meerkat_core::Session = serde_json::from_slice(&recovered)?;
        assert_eq!(
            recovered_session.id(),
            &session_id,
            "jsonl runtime companion must recover the committed session snapshot"
        );
        Ok(())
    }

    #[tokio::test]
    async fn open_realm_persistence_memory_has_no_durable_companions()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;

        let (manifest, bundle) = open_realm_persistence_in(
            temp.path(),
            "memory-realm",
            Some(RealmBackend::Memory),
            Some(RealmOrigin::Explicit),
        )
        .await?;

        assert_eq!(manifest.backend, RealmBackend::Memory);
        assert!(!bundle.blob_store().is_persistent());
        assert!(!bundle.artifact_store().is_persistent());
        assert_eq!(
            bundle.schedule_store().kind(),
            meerkat_schedule::ScheduleStoreKind::Memory
        );
        assert_eq!(
            bundle.workgraph_store().kind(),
            meerkat_workgraph::WorkGraphStoreKind::Memory
        );
        assert!(
            bundle.event_projection().is_none(),
            "memory realms must not persist conversation events through the file projection bridge"
        );

        let session = Session::new();
        let session_id = session.id().clone();
        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);
        bundle.session_store().save(&session).await?;
        bundle
            .runtime_store()
            .commit_session_snapshot(
                &runtime_id,
                meerkat_runtime::store::SessionDelta {
                    session_snapshot: serde_json::to_vec(&session)?,
                },
            )
            .await?;
        assert!(bundle.session_store().load(&session_id).await?.is_some());
        assert!(
            bundle
                .runtime_store()
                .load_session_snapshot(&runtime_id)
                .await?
                .is_some()
        );

        drop(bundle);
        let (reopened_manifest, reopened) = open_realm_persistence_in(
            temp.path(),
            "memory-realm",
            Some(RealmBackend::Memory),
            Some(RealmOrigin::Explicit),
        )
        .await?;
        assert_eq!(reopened_manifest.backend, RealmBackend::Memory);
        assert!(
            reopened.session_store().load(&session_id).await?.is_none(),
            "a new memory-realm bundle must not recover prior process-local sessions"
        );
        assert!(
            reopened
                .runtime_store()
                .load_session_snapshot(&runtime_id)
                .await?
                .is_none(),
            "a new memory-realm bundle must not recover prior process-local runtime authority"
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

    #[test]
    fn memory_bundle_keeps_existing_session_store_behavior_with_in_memory_runtime_companion()
    -> Result<(), Box<dyn std::error::Error>> {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store: Arc<dyn RuntimeStore> =
            Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new());

        let bundle = PersistenceBundle::new(store, runtime_store, Arc::new(MemoryBlobStore::new()));

        assert!(!bundle.blob_store().is_persistent());
        let _ = bundle.runtime_store();
        let _ = bundle.runtime_adapter();
        Ok(())
    }

    #[test]
    fn persistence_error_runtime_variant_wraps_runtime_store_error() {
        let err = PersistenceError::from(RuntimeStoreError::WriteFailed("boom".to_string()));

        assert!(matches!(err, PersistenceError::Runtime(_)));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn explicit_root_layout_is_single_candidate_at_the_given_root() {
        let temp = TempDir::new().expect("tempdir");
        let layout = layout_for_explicit_state_root(temp.path(), "team").expect("layout resolves");
        assert_eq!(layout.state_root(), temp.path());
        // A caller-resolved root never probes: single-candidate layout, so
        // the store's first-start reservation degenerates to the unchanged
        // single-root path.
        assert_eq!(layout.realm_root_candidates(), &[temp.path().to_path_buf()]);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn explicit_root_layout_rejects_invalid_realm_ids_typed() {
        let temp = TempDir::new().expect("tempdir");
        let err = match layout_for_explicit_state_root(temp.path(), "not a realm id") {
            Err(err) => err,
            Ok(_) => panic!("invalid realm id must refuse"),
        };
        assert!(matches!(err, PersistenceError::Bootstrap(_)));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn first_start_store_errors_still_surface_as_store_variant() {
        // The From unwrap keeps plain store failures on the historical
        // `Store(_)` arm; only reservation refusals ride `FirstStart`.
        let err = PersistenceError::from(meerkat_store::realm::RealmFirstStartError::Store(
            StoreError::Internal("boom".to_string()),
        ));
        assert!(matches!(err, PersistenceError::Store(_)));
        let refusal =
            PersistenceError::from(meerkat_store::realm::RealmFirstStartError::Contention {
                realm_id: "team".to_string(),
            });
        assert!(matches!(refusal, PersistenceError::FirstStart(_)));
    }
}
