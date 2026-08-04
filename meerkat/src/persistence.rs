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
use meerkat_runtime::{
    MeerkatMachine, RuntimeSessionPersistenceProfile, RuntimeStore, RuntimeStoreError,
};
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
    /// The explicit pre-floor importer is intentionally scoped to the
    /// co-tenanted SQLite realm layout it can authenticate end to end.
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    #[error(
        "the explicit pre-v0.8.10 bridge supports only SQLite realms; realm '{realm_id}' uses the '{backend}' backend"
    )]
    PreV0810BridgeBackend { realm_id: String, backend: String },
    /// The explicit bridge never follows realm-layout symlinks. A linked
    /// database could otherwise redirect maintenance writes outside the
    /// realm covered by the caller's fence.
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    #[error("the explicit pre-v0.8.10 bridge refuses symlinked realm path '{path}'")]
    PreV0810BridgeSymlink { path: PathBuf },
    /// A `Durable` storage slot resolved to a non-persistent store without
    /// the realm manifest declaring that domain ephemeral (fail-closed
    /// durability; see `storage_provider`).
    #[error(
        "durable storage domain '{domain}' resolved to a non-persistent store without an \
         ephemeral declaration in the realm manifest; refusing to start"
    )]
    DurabilityViolation { domain: String },
    /// A runtime profile that commits canonical session heads was paired with
    /// a SessionStore that cannot prepare or materialize that representation.
    #[error(
        "runtime session persistence profile '{profile}' is incompatible with the supplied session store: {detail}"
    )]
    SessionPersistenceProfileMismatch {
        profile: RuntimeSessionPersistenceProfile,
        detail: String,
    },
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
    #[cfg(feature = "session-store")]
    session_persistence_profile: RuntimeSessionPersistenceProfile,
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
        let session_persistence_profile = runtime_store.session_persistence_profile();
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
            session_persistence_profile,
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
    pub fn session_persistence_profile(&self) -> RuntimeSessionPersistenceProfile {
        self.session_persistence_profile
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

/// One schema domain considered by the explicit pre-0.8.10 storage bridge.
///
/// A `0 -> 0` result means the database exists but this domain owns no
/// objects, so the bridge left fresh-domain initialization to the ordinary
/// store constructor. For equal non-zero versions, `ledger_established` and
/// `prepared_rows` distinguish exact-current stamping or payload preparation
/// from a true idempotent no-op.
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreV0810DomainBridgeReport {
    /// Database file containing the domain.
    pub database: PathBuf,
    /// Stable ledger domain name.
    pub domain: String,
    /// Authenticated source schema version.
    pub from_version: i64,
    /// Schema version after the bridge.
    pub to_version: i64,
    /// Whether this call established the domain's previously missing ledger
    /// row. This is false for existing-ledger upgrades and idempotent re-runs.
    pub ledger_established: bool,
    /// Durable records rewritten by the domain's scoped preparation callback.
    pub prepared_rows: usize,
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
impl PreV0810DomainBridgeReport {
    /// True when this call established a ledger row, advanced a schema, or
    /// rewrote a durable payload.
    pub fn changed(&self) -> bool {
        self.ledger_established || self.from_version != self.to_version || self.prepared_rows != 0
    }
}

/// Result of the explicit pre-0.8.10 bridge across the SQLite files that
/// already exist in one realm.
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreV0810RealmBridgeReport {
    /// Domains considered, in dependency-safe bridge order.
    pub domains: Vec<PreV0810DomainBridgeReport>,
    /// Existing SQLite companions skipped because the realm manifest names a
    /// different durable authority.
    pub inactive_databases: Vec<PathBuf>,
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
fn record_pre_v0_8_10_domain(
    report: &mut PreV0810RealmBridgeReport,
    database: &Path,
    domain: &meerkat_sqlite::SchemaDomain,
    ledger_before: Option<i64>,
    result: meerkat_sqlite::MaintenanceBridgeReport,
) {
    report.domains.push(PreV0810DomainBridgeReport {
        database: database.to_path_buf(),
        domain: domain.name.to_string(),
        from_version: result.from_version,
        to_version: result.to_version,
        ledger_established: ledger_before.is_none() && result.to_version != 0,
        prepared_rows: result.prepared,
    });
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
fn census_pre_v0_8_10_domains(
    database: &Path,
    domains: &[&meerkat_sqlite::SchemaDomain],
) -> Result<(), PersistenceError> {
    if !database.is_file() {
        return Ok(());
    }
    let conn = meerkat_sqlite::open(database, meerkat_sqlite::ConnectionProfile::ReadOnly)
        .map_err(StoreError::from)?;
    for domain in domains {
        if let Some(found) =
            meerkat_sqlite::domain_version(&conn, domain.name).map_err(StoreError::from)?
            && found > domain.supported_version()
        {
            return Err(PersistenceError::Store(StoreError::SchemaFromTheFuture {
                domain: domain.name.to_string(),
                found,
                supported: domain.supported_version(),
            }));
        }
    }
    Ok(())
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
fn refuse_pre_v0_8_10_bridge_symlink(path: &Path) -> Result<(), PersistenceError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(PersistenceError::PreV0810BridgeSymlink {
                path: path.to_path_buf(),
            })
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(StoreError::Io(error).into()),
    }
}

/// Authenticate and migrate exact pre-0.8.10 SQLite schemas in one realm.
///
/// This is an explicit offline maintenance operation. `fence` must cover the
/// exact requested realm; its admission lock and fixed database inventory are
/// validated before the manifest or any database is read. Ordinary realm
/// opens remain strict and never invoke this bridge. Only existing database
/// files are opened, always with the maintenance-write profile, and every
/// domain bridge is transactionally authenticated by its owning migration
/// manifest before it is stamped.
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub fn bridge_pre_0_8_10_realm_storage_in(
    realms_root: &Path,
    realm_id: &str,
    fence: &meerkat_store::migrate::RealmMaintenanceFence,
) -> Result<PreV0810RealmBridgeReport, PersistenceError> {
    // Validate the public identity before deriving its sanitized directory.
    let requested_realm = meerkat_core::RealmId::parse(realm_id)
        .map_err(|_| StoreError::InvalidRealmSlug(realm_id.to_string()))?;

    let paths = realm_paths_in(realms_root, realm_id);
    refuse_pre_v0_8_10_bridge_symlink(&paths.root)?;
    refuse_pre_v0_8_10_bridge_symlink(&paths.manifest_path)?;
    refuse_pre_v0_8_10_bridge_symlink(&paths.root.join("memory"))?;
    let inventory = meerkat_store::migrate::enumerate_realm_sqlite_inventory(&paths.root);
    if let Some(path) = inventory.symlinks.first() {
        return Err(PersistenceError::PreV0810BridgeSymlink { path: path.clone() });
    }
    let expected_admission = meerkat_sqlite::fence_lock_path(
        &meerkat_store::migrate::realm_write_admission_target(&paths.root),
    );
    let covers_fixed_inventory = meerkat_store::migrate::REALM_SQLITE_FILES
        .iter()
        .map(|relative| paths.root.join(relative))
        .all(|database| fence.fenced_databases().contains(&database));
    let contains_foreign_database = fence
        .fenced_databases()
        .iter()
        .any(|database| !database.starts_with(&paths.root));
    if fence.admission_lock_path() != expected_admission
        || !covers_fixed_inventory
        || contains_foreign_database
    {
        return Err(PersistenceError::Store(StoreError::Internal(format!(
            "maintenance fence does not cover requested realm directory '{}'",
            paths.root.display()
        ))));
    }

    let manifest_pin = meerkat_store::read_realm_manifest_pin(&paths.manifest_path)?;
    if manifest_pin.realm() != &requested_realm {
        return Err(PersistenceError::Store(StoreError::RealmIdentityMismatch {
            requested: requested_realm.as_str().to_string(),
            existing: manifest_pin.realm().as_str().to_string(),
        }));
    }
    let manifest = match manifest_pin {
        meerkat_store::RealmManifestPin::Builtin(manifest) => manifest,
        meerkat_store::RealmManifestPin::External(manifest) => {
            return Err(PersistenceError::Store(StoreError::ExternalProviderRealm {
                realm_id: manifest.realm.as_str().to_string(),
                provider: manifest.provider,
            }));
        }
    };

    if !matches!(manifest.backend, RealmBackend::Sqlite) {
        return Err(PersistenceError::PreV0810BridgeBackend {
            realm_id: realm_id.to_string(),
            backend: manifest.backend.as_str().to_string(),
        });
    }

    // Cross-file census before the first write: malformed ledgers and any
    // future-version active domain abort the whole explicit bridge before an
    // earlier domain can be stamped or migrated.
    census_pre_v0_8_10_domains(
        &paths.sessions_sqlite_path,
        &[
            &meerkat_store::sqlite_store::SESSION_STORE_DOMAIN,
            &meerkat_runtime::store::sqlite::RUNTIME_STORE_DOMAIN,
            &meerkat_store::schedule_sqlite_store::SCHEDULE_STORE_DOMAIN,
        ],
    )?;
    census_pre_v0_8_10_domains(
        &paths.root.join("workgraph.sqlite3"),
        &[&meerkat_workgraph::WORKGRAPH_DOMAIN],
    )?;
    census_pre_v0_8_10_domains(&paths.jobs_sqlite_path, &[&meerkat_jobs::JOBS_DOMAIN])?;
    #[cfg(feature = "memory-store-session")]
    census_pre_v0_8_10_domains(
        &paths.root.join("memory").join("memory.sqlite3"),
        &[&meerkat_memory::MEMORY_DOMAIN],
    )?;
    census_pre_v0_8_10_domains(
        &paths.root.join("tasks.db"),
        &[&meerkat_tools::TOOLS_TASKS_DOMAIN],
    )?;

    let mut report = PreV0810RealmBridgeReport::default();

    if paths.runtime_sqlite_path.is_file() {
        report
            .inactive_databases
            .push(paths.runtime_sqlite_path.clone());
    }

    // The SQLite realm backend co-tenants these three domains in the session
    // database. Session migration runs first because runtime migration imports
    // session snapshots; scheduling follows both runtime authorities.
    if paths.sessions_sqlite_path.is_file() {
        let database = &paths.sessions_sqlite_path;
        let mut conn = meerkat_sqlite::open(
            database,
            meerkat_sqlite::ConnectionProfile::Maintenance { write: true },
        )
        .map_err(StoreError::from)?;

        let domain = &meerkat_store::sqlite_store::SESSION_STORE_DOMAIN;
        let ledger_before =
            meerkat_sqlite::domain_version(&conn, domain.name).map_err(StoreError::from)?;
        let result = meerkat_core::with_pre_floor_provider_image_metadata_import(|| {
            meerkat_sqlite::bridge_unledgered_domain(
                &mut conn,
                domain,
                domain.supported_version(),
                &[1],
                None,
            )
        })
        .map_err(StoreError::from)?;
        record_pre_v0_8_10_domain(&mut report, database, domain, ledger_before, result);

        let domain = &meerkat_runtime::store::sqlite::RUNTIME_STORE_DOMAIN;
        let ledger_before =
            meerkat_sqlite::domain_version(&conn, domain.name).map_err(StoreError::from)?;
        let result = meerkat_core::with_pre_floor_provider_image_metadata_import(|| {
            meerkat_sqlite::bridge_unledgered_domain(
                &mut conn,
                domain,
                domain.supported_version(),
                &[1],
                Some(meerkat_runtime::store::sqlite::prepare_pre_0_8_10_runtime_input_states),
            )
        })
        .map_err(StoreError::from)?;
        record_pre_v0_8_10_domain(&mut report, database, domain, ledger_before, result);

        let domain = &meerkat_store::schedule_sqlite_store::SCHEDULE_STORE_DOMAIN;
        let ledger_before =
            meerkat_sqlite::domain_version(&conn, domain.name).map_err(StoreError::from)?;
        let result = meerkat_sqlite::bridge_unledgered_domain(
            &mut conn,
            domain,
            domain.supported_version(),
            &[1],
            None,
        )
        .map_err(StoreError::from)?;
        record_pre_v0_8_10_domain(&mut report, database, domain, ledger_before, result);
    }

    let workgraph_db = paths.root.join("workgraph.sqlite3");
    if workgraph_db.is_file() {
        let mut conn = meerkat_sqlite::open(
            &workgraph_db,
            meerkat_sqlite::ConnectionProfile::Maintenance { write: true },
        )
        .map_err(StoreError::from)?;
        let domain = &meerkat_workgraph::WORKGRAPH_DOMAIN;
        let ledger_before =
            meerkat_sqlite::domain_version(&conn, domain.name).map_err(StoreError::from)?;
        let result = meerkat_sqlite::bridge_unledgered_domain(
            &mut conn,
            domain,
            domain.supported_version(),
            &[1, 2],
            Some(meerkat_workgraph::prepare_pre_0_8_10_workgraph_attention),
        )
        .map_err(StoreError::from)?;
        record_pre_v0_8_10_domain(&mut report, &workgraph_db, domain, ledger_before, result);
    }

    if paths.jobs_sqlite_path.is_file() {
        let database = &paths.jobs_sqlite_path;
        let mut conn = meerkat_sqlite::open(
            database,
            meerkat_sqlite::ConnectionProfile::Maintenance { write: true },
        )
        .map_err(StoreError::from)?;
        let domain = &meerkat_jobs::JOBS_DOMAIN;
        let ledger_before =
            meerkat_sqlite::domain_version(&conn, domain.name).map_err(StoreError::from)?;
        let result = meerkat_sqlite::bridge_unledgered_domain(
            &mut conn,
            domain,
            domain.supported_version(),
            &[1],
            None,
        )
        .map_err(StoreError::from)?;
        record_pre_v0_8_10_domain(&mut report, database, domain, ledger_before, result);
    }

    #[cfg(feature = "memory-store-session")]
    {
        let memory_db = paths.root.join("memory").join("memory.sqlite3");
        if memory_db.is_file() {
            let mut conn = meerkat_sqlite::open(
                &memory_db,
                meerkat_sqlite::ConnectionProfile::Maintenance { write: true },
            )
            .map_err(StoreError::from)?;
            let domain = &meerkat_memory::MEMORY_DOMAIN;
            let ledger_before =
                meerkat_sqlite::domain_version(&conn, domain.name).map_err(StoreError::from)?;
            let result = meerkat_sqlite::bridge_unledgered_domain(
                &mut conn,
                domain,
                domain.supported_version(),
                &[1],
                None,
            )
            .map_err(StoreError::from)?;
            record_pre_v0_8_10_domain(&mut report, &memory_db, domain, ledger_before, result);
        }
    }

    let tasks_db = paths.root.join("tasks.db");
    if tasks_db.is_file() {
        let mut conn = meerkat_sqlite::open(
            &tasks_db,
            meerkat_sqlite::ConnectionProfile::Maintenance { write: true },
        )
        .map_err(StoreError::from)?;
        let domain = &meerkat_tools::TOOLS_TASKS_DOMAIN;
        let ledger_before =
            meerkat_sqlite::domain_version(&conn, domain.name).map_err(StoreError::from)?;
        let result = meerkat_sqlite::bridge_unledgered_domain(
            &mut conn,
            domain,
            domain.supported_version(),
            &[1],
            None,
        )
        .map_err(StoreError::from)?;
        record_pre_v0_8_10_domain(&mut report, &tasks_db, domain, ledger_before, result);
    }

    Ok(report)
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
    let profile = set.runtime_store.session_persistence_profile();
    if profile == RuntimeSessionPersistenceProfile::HeadCanonicalV1
        && Arc::clone(&set.session_store).as_incremental().is_none()
    {
        return Err(PersistenceError::SessionPersistenceProfileMismatch {
            profile,
            detail: "HeadCanonical requires an IncrementalSessionStore pairing".to_string(),
        });
    }

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
            let runtime_store =
                Arc::new(meerkat_runtime::store::SqliteRuntimeStore::new_whole_blob(
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
            let runtime_store = Arc::new(
                meerkat_runtime::store::SqliteRuntimeStore::new_head_canonical(
                    sqlite_store.path().to_path_buf(),
                )?,
            ) as Arc<dyn RuntimeStore>;
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
    #[cfg(not(target_arch = "wasm32"))]
    use std::time::Duration;
    use tempfile::TempDir;

    #[cfg(not(target_arch = "wasm32"))]
    fn create_unledgered_prefix(
        database: &Path,
        domains: &[&meerkat_sqlite::SchemaDomain],
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = database.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut conn = meerkat_sqlite::open(
            database,
            meerkat_sqlite::ConnectionProfile::Primary { create: true },
        )?;
        let tx = conn.transaction()?;
        for domain in domains {
            (domain.migrations[0].apply)(&tx)?;
        }
        tx.commit()?;
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn write_builtin_manifest(
        paths: &meerkat_store::RealmPaths,
        realm_id: &str,
        backend: RealmBackend,
    ) -> Result<(), Box<dyn std::error::Error>> {
        std::fs::create_dir_all(&paths.root)?;
        let manifest = RealmManifest {
            realm: meerkat_core::RealmId::parse(realm_id).expect("test realm id is valid"),
            backend,
            origin: RealmOrigin::Explicit,
            created_at: "1970-01-01T00:00:00Z".to_string(),
            manifest_format: 1,
            provider: None,
            ephemeral_domains: Vec::new(),
        };
        std::fs::write(&paths.manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn explicit_pre_floor_bridge_orchestrates_existing_realm_databases_in_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let realm_id = "legacy-realm";
        let paths = realm_paths_in(temp.path(), realm_id);
        write_builtin_manifest(&paths, realm_id, RealmBackend::Sqlite)?;

        create_unledgered_prefix(
            &paths.sessions_sqlite_path,
            &[
                &meerkat_store::sqlite_store::SESSION_STORE_DOMAIN,
                &meerkat_runtime::store::sqlite::RUNTIME_STORE_DOMAIN,
                &meerkat_store::schedule_sqlite_store::SCHEDULE_STORE_DOMAIN,
            ],
        )?;
        create_unledgered_prefix(
            &paths.runtime_sqlite_path,
            &[&meerkat_runtime::store::sqlite::RUNTIME_STORE_DOMAIN],
        )?;
        let conn = meerkat_sqlite::open(
            &paths.runtime_sqlite_path,
            meerkat_sqlite::ConnectionProfile::Primary { create: false },
        )?;
        conn.execute_batch(
            "CREATE TABLE meerkat_schema (
                 domain TEXT PRIMARY KEY,
                 version INTEGER NOT NULL
             );
             INSERT INTO meerkat_schema (domain, version)
             VALUES ('runtime-store', 1);",
        )?;
        drop(conn);
        create_unledgered_prefix(
            &paths.root.join("workgraph.sqlite3"),
            &[&meerkat_workgraph::WORKGRAPH_DOMAIN],
        )?;
        create_unledgered_prefix(&paths.jobs_sqlite_path, &[&meerkat_jobs::JOBS_DOMAIN])?;
        #[cfg(feature = "memory-store-session")]
        create_unledgered_prefix(
            &paths.root.join("memory").join("memory.sqlite3"),
            &[&meerkat_memory::MEMORY_DOMAIN],
        )?;
        create_unledgered_prefix(
            &paths.root.join("tasks.db"),
            &[&meerkat_tools::TOOLS_TASKS_DOMAIN],
        )?;

        let fence = meerkat_store::migrate::RealmMaintenanceFence::acquire(
            &paths.root,
            Duration::from_secs(1),
        )?;
        let report = bridge_pre_0_8_10_realm_storage_in(temp.path(), realm_id, &fence)?;
        drop(fence);

        let mut expected = vec![
            ("session-store", 1, 3),
            ("runtime-store", 1, 2),
            ("schedule-store", 1, 2),
            ("workgraph", 1, 2),
            ("jobs", 1, 2),
        ];
        #[cfg(feature = "memory-store-session")]
        expected.push(("memory", 1, 2));
        expected.push(("tools-tasks", 1, 1));

        let actual = report
            .domains
            .iter()
            .map(|entry| (entry.domain.as_str(), entry.from_version, entry.to_version))
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
        assert!(report.domains.iter().all(|entry| entry.ledger_established));
        assert_eq!(
            report.inactive_databases,
            vec![paths.runtime_sqlite_path.clone()],
            "the standalone runtime is not authoritative for a SQLite realm"
        );

        for entry in &report.domains {
            let conn =
                meerkat_sqlite::open(&entry.database, meerkat_sqlite::ConnectionProfile::ReadOnly)?;
            assert_eq!(
                meerkat_sqlite::domain_version(&conn, &entry.domain)?,
                Some(entry.to_version),
                "{} must be stamped only after convergence",
                entry.domain
            );
        }

        let fence = meerkat_store::migrate::RealmMaintenanceFence::acquire(
            &paths.root,
            Duration::from_secs(1),
        )?;
        let rerun = bridge_pre_0_8_10_realm_storage_in(temp.path(), realm_id, &fence)?;
        drop(fence);
        assert_eq!(rerun.domains.len(), report.domains.len());
        assert!(
            rerun.domains.iter().all(|entry| {
                !entry.ledger_established && entry.from_version == entry.to_version
            }),
            "an already bridged realm must be an idempotent no-op"
        );
        assert_eq!(rerun.inactive_databases, report.inactive_databases);

        let conn = meerkat_sqlite::open(
            &paths.runtime_sqlite_path,
            meerkat_sqlite::ConnectionProfile::ReadOnly,
        )?;
        assert_eq!(
            meerkat_sqlite::domain_version(&conn, "runtime-store")?,
            Some(1),
            "the inactive standalone runtime must remain byte-authority untouched"
        );

        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn explicit_pre_floor_bridge_census_refuses_future_later_domain_before_session_mutation()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let realm_id = "future-runtime-realm";
        let paths = realm_paths_in(temp.path(), realm_id);
        write_builtin_manifest(&paths, realm_id, RealmBackend::Sqlite)?;
        create_unledgered_prefix(
            &paths.sessions_sqlite_path,
            &[
                &meerkat_store::sqlite_store::SESSION_STORE_DOMAIN,
                &meerkat_runtime::store::sqlite::RUNTIME_STORE_DOMAIN,
                &meerkat_store::schedule_sqlite_store::SCHEDULE_STORE_DOMAIN,
            ],
        )?;
        let conn = meerkat_sqlite::open(
            &paths.sessions_sqlite_path,
            meerkat_sqlite::ConnectionProfile::Primary { create: false },
        )?;
        conn.execute_batch(
            "CREATE TABLE meerkat_schema (
                 domain TEXT PRIMARY KEY,
                 version INTEGER NOT NULL
             );
             INSERT INTO meerkat_schema (domain, version)
             VALUES ('runtime-store', 99);",
        )?;
        drop(conn);

        let fence = meerkat_store::migrate::RealmMaintenanceFence::acquire(
            &paths.root,
            Duration::from_secs(1),
        )?;
        let error = bridge_pre_0_8_10_realm_storage_in(temp.path(), realm_id, &fence)
            .expect_err("future runtime domain must abort the pre-write census");
        drop(fence);
        assert!(matches!(
            error,
            PersistenceError::Store(StoreError::SchemaFromTheFuture {
                ref domain,
                found: 99,
                supported: 2,
            }) if domain == "runtime-store"
        ));

        let conn = meerkat_sqlite::open(
            &paths.sessions_sqlite_path,
            meerkat_sqlite::ConnectionProfile::ReadOnly,
        )?;
        assert_eq!(
            meerkat_sqlite::domain_version(&conn, "session-store")?,
            None,
            "the earlier session domain must not be stamped"
        );
        let session_v2_object: i64 = conn.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM sqlite_schema
                 WHERE type = 'table' AND name = 'session_strand_links'
             )",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(
            session_v2_object, 0,
            "the earlier session domain must not be migrated"
        );

        Ok(())
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "jsonl-store"))]
    #[test]
    fn explicit_pre_floor_bridge_refuses_jsonl_before_mutating_any_database()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let realm_id = "legacy-jsonl-realm";
        let paths = realm_paths_in(temp.path(), realm_id);
        write_builtin_manifest(&paths, realm_id, RealmBackend::Jsonl)?;
        create_unledgered_prefix(
            &paths.sessions_sqlite_path,
            &[&meerkat_store::sqlite_store::SESSION_STORE_DOMAIN],
        )?;
        create_unledgered_prefix(
            &paths.runtime_sqlite_path,
            &[&meerkat_runtime::store::sqlite::RUNTIME_STORE_DOMAIN],
        )?;

        let fence = meerkat_store::migrate::RealmMaintenanceFence::acquire(
            &paths.root,
            Duration::from_secs(1),
        )?;
        let error = bridge_pre_0_8_10_realm_storage_in(temp.path(), realm_id, &fence)
            .expect_err("JSONL realms are outside the explicit bridge authority");
        drop(fence);
        assert_eq!(
            error.to_string(),
            format!(
                "the explicit pre-v0.8.10 bridge supports only SQLite realms; realm '{realm_id}' uses the 'jsonl' backend"
            )
        );

        for (database, domain) in [
            (&paths.sessions_sqlite_path, "session-store"),
            (&paths.runtime_sqlite_path, "runtime-store"),
        ] {
            let conn = meerkat_sqlite::open(database, meerkat_sqlite::ConnectionProfile::ReadOnly)?;
            assert_eq!(meerkat_sqlite::domain_version(&conn, domain)?, None);
        }

        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn explicit_pre_floor_bridge_refuses_memory_before_mutating_any_database()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let realm_id = "legacy-memory-realm";
        let paths = realm_paths_in(temp.path(), realm_id);
        write_builtin_manifest(&paths, realm_id, RealmBackend::Memory)?;
        create_unledgered_prefix(
            &paths.sessions_sqlite_path,
            &[&meerkat_store::sqlite_store::SESSION_STORE_DOMAIN],
        )?;
        create_unledgered_prefix(
            &paths.runtime_sqlite_path,
            &[&meerkat_runtime::store::sqlite::RUNTIME_STORE_DOMAIN],
        )?;
        create_unledgered_prefix(
            &paths.root.join("workgraph.sqlite3"),
            &[&meerkat_workgraph::WORKGRAPH_DOMAIN],
        )?;
        create_unledgered_prefix(&paths.jobs_sqlite_path, &[&meerkat_jobs::JOBS_DOMAIN])?;
        #[cfg(feature = "memory-store-session")]
        create_unledgered_prefix(
            &paths.root.join("memory").join("memory.sqlite3"),
            &[&meerkat_memory::MEMORY_DOMAIN],
        )?;
        create_unledgered_prefix(
            &paths.root.join("tasks.db"),
            &[&meerkat_tools::TOOLS_TASKS_DOMAIN],
        )?;

        let fence = meerkat_store::migrate::RealmMaintenanceFence::acquire(
            &paths.root,
            Duration::from_secs(1),
        )?;
        let error = bridge_pre_0_8_10_realm_storage_in(temp.path(), realm_id, &fence)
            .expect_err("memory realms are outside the explicit bridge authority");
        drop(fence);
        assert_eq!(
            error.to_string(),
            format!(
                "the explicit pre-v0.8.10 bridge supports only SQLite realms; realm '{realm_id}' uses the 'memory' backend"
            )
        );

        let mut untouched = vec![
            (paths.sessions_sqlite_path.clone(), "session-store"),
            (paths.runtime_sqlite_path.clone(), "runtime-store"),
            (paths.root.join("workgraph.sqlite3"), "workgraph"),
            (paths.jobs_sqlite_path.clone(), "jobs"),
            (paths.root.join("tasks.db"), "tools-tasks"),
        ];
        #[cfg(feature = "memory-store-session")]
        untouched.push((paths.root.join("memory").join("memory.sqlite3"), "memory"));
        for (database, domain) in untouched {
            let conn =
                meerkat_sqlite::open(&database, meerkat_sqlite::ConnectionProfile::ReadOnly)?;
            assert_eq!(meerkat_sqlite::domain_version(&conn, domain)?, None);
        }

        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn explicit_pre_floor_bridge_refuses_a_fence_for_another_realm_without_mutation()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let realm_id = "target-realm";
        let paths = realm_paths_in(temp.path(), realm_id);
        write_builtin_manifest(&paths, realm_id, RealmBackend::Sqlite)?;
        create_unledgered_prefix(
            &paths.sessions_sqlite_path,
            &[&meerkat_store::sqlite_store::SESSION_STORE_DOMAIN],
        )?;

        let other_paths = realm_paths_in(temp.path(), "other-realm");
        std::fs::create_dir_all(&other_paths.root)?;
        let fence = meerkat_store::migrate::RealmMaintenanceFence::acquire(
            &other_paths.root,
            Duration::from_secs(1),
        )?;
        let error = bridge_pre_0_8_10_realm_storage_in(temp.path(), realm_id, &fence)
            .expect_err("a fence for another realm must refuse");
        drop(fence);
        assert!(error.to_string().contains("does not cover requested realm"));

        let conn = meerkat_sqlite::open(
            &paths.sessions_sqlite_path,
            meerkat_sqlite::ConnectionProfile::ReadOnly,
        )?;
        assert_eq!(
            meerkat_sqlite::domain_version(&conn, "session-store")?,
            None
        );

        Ok(())
    }

    #[cfg(all(unix, not(target_arch = "wasm32")))]
    #[test]
    fn explicit_pre_floor_bridge_refuses_symlinked_database_without_touching_target()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let realm_id = "linked-database-realm";
        let paths = realm_paths_in(temp.path(), realm_id);
        write_builtin_manifest(&paths, realm_id, RealmBackend::Sqlite)?;

        let external_dir = temp.path().join("external");
        let external_database = external_dir.join("sessions.sqlite3");
        create_unledgered_prefix(
            &external_database,
            &[&meerkat_store::sqlite_store::SESSION_STORE_DOMAIN],
        )?;
        let external_before = std::fs::read(&external_database)?;
        std::os::unix::fs::symlink(&external_database, &paths.sessions_sqlite_path)?;

        let fence = meerkat_store::migrate::RealmMaintenanceFence::acquire(
            &paths.root,
            Duration::from_secs(1),
        )?;
        let error = bridge_pre_0_8_10_realm_storage_in(temp.path(), realm_id, &fence)
            .expect_err("a symlinked database must refuse before any bridge write");
        drop(fence);
        assert!(matches!(
            error,
            PersistenceError::PreV0810BridgeSymlink { ref path }
                if path == &paths.sessions_sqlite_path
        ));
        assert_eq!(
            std::fs::read(&external_database)?,
            external_before,
            "the symlink target must remain byte-identical"
        );
        assert!(
            std::fs::symlink_metadata(&paths.sessions_sqlite_path)?
                .file_type()
                .is_symlink()
        );

        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn explicit_pre_floor_bridge_refuses_manifest_identity_alias_without_mutation()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let requested_realm = "alias.realm";
        let paths = realm_paths_in(temp.path(), requested_realm);
        write_builtin_manifest(&paths, "alias_realm", RealmBackend::Sqlite)?;
        create_unledgered_prefix(
            &paths.sessions_sqlite_path,
            &[&meerkat_store::sqlite_store::SESSION_STORE_DOMAIN],
        )?;

        let fence = meerkat_store::migrate::RealmMaintenanceFence::acquire(
            &paths.root,
            Duration::from_secs(1),
        )?;
        let error = bridge_pre_0_8_10_realm_storage_in(temp.path(), requested_realm, &fence)
            .expect_err("a path-aliasing manifest identity must refuse");
        drop(fence);
        assert!(matches!(
            error,
            PersistenceError::Store(StoreError::RealmIdentityMismatch { .. })
        ));

        let conn = meerkat_sqlite::open(
            &paths.sessions_sqlite_path,
            meerkat_sqlite::ConnectionProfile::ReadOnly,
        )?;
        assert_eq!(
            meerkat_sqlite::domain_version(&conn, "session-store")?,
            None
        );

        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn explicit_pre_floor_bridge_refuses_external_provider_pin_without_mutation()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let realm_id = "external-realm";
        let paths = realm_paths_in(temp.path(), realm_id);
        std::fs::create_dir_all(&paths.root)?;
        std::fs::write(
            &paths.manifest_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "realm_id": realm_id,
                "backend": "external:test-provider",
                "origin": "explicit",
                "created_at": "1970-01-01T00:00:00Z",
                "manifest_format": 2,
                "provider": "test-provider"
            }))?,
        )?;
        create_unledgered_prefix(
            &paths.sessions_sqlite_path,
            &[&meerkat_store::sqlite_store::SESSION_STORE_DOMAIN],
        )?;

        let fence = meerkat_store::migrate::RealmMaintenanceFence::acquire(
            &paths.root,
            Duration::from_secs(1),
        )?;
        let error = bridge_pre_0_8_10_realm_storage_in(temp.path(), realm_id, &fence)
            .expect_err("an external-provider pin must refuse disk bridge");
        drop(fence);
        assert!(matches!(
            error,
            PersistenceError::Store(StoreError::ExternalProviderRealm { .. })
        ));

        let conn = meerkat_sqlite::open(
            &paths.sessions_sqlite_path,
            meerkat_sqlite::ConnectionProfile::ReadOnly,
        )?;
        assert_eq!(
            meerkat_sqlite::domain_version(&conn, "session-store")?,
            None
        );

        Ok(())
    }

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
        assert_eq!(
            bundle.session_persistence_profile(),
            RuntimeSessionPersistenceProfile::HeadCanonicalV1
        );
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
        assert_eq!(
            bundle.session_persistence_profile(),
            RuntimeSessionPersistenceProfile::WholeBlobV1
        );
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
                meerkat_runtime::store::SerializedSessionSnapshot {
                    session_snapshot: serde_json::to_vec(&session)?.into(),
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
        assert_eq!(
            bundle.session_persistence_profile(),
            RuntimeSessionPersistenceProfile::WholeBlobV1
        );
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
                meerkat_runtime::store::SerializedSessionSnapshot {
                    session_snapshot: serde_json::to_vec(&session)?.into(),
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
