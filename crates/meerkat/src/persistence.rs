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
    Session(#[from] meerkat_core::SessionStoreError),
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

/// Activate one externally injected HeadCanonical store before its stores are
/// allowed to enter a [`PersistenceBundle`].
///
/// The provider boundary is the only asynchronous construction seam before a
/// runtime-backed session service is built. Consume the backend's one bulk
/// activation result here so no service can observe a pre-current physical
/// head. Each store-issued proof is then consumed exactly once by the paired
/// RuntimeStore. A crash between those commits is retry-safe because the
/// physical store reissues `AlreadyCurrent` and the runtime consumer accepts
/// either the exact installed token or the same semantic pre-activation
/// boundary.
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
async fn activate_external_head_canonical_store(
    session_store: &Arc<dyn SessionStore>,
    runtime_store: &Arc<dyn RuntimeStore>,
    profile: RuntimeSessionPersistenceProfile,
) -> Result<(), PersistenceError> {
    if profile != RuntimeSessionPersistenceProfile::HeadCanonicalV1 {
        return Ok(());
    }
    let incremental = Arc::clone(session_store).as_incremental().ok_or_else(|| {
        PersistenceError::SessionPersistenceProfileMismatch {
            profile,
            detail: "HeadCanonical requires an IncrementalSessionStore pairing".to_string(),
        }
    })?;
    let crossings = match incremental.activate_head_canonical_store().await? {
        meerkat_core::HeadCanonicalStoreActivation::NotApplicable => {
            return Err(PersistenceError::SessionPersistenceProfileMismatch {
                profile,
                detail: "HeadCanonical activation returned NotApplicable".to_string(),
            });
        }
        meerkat_core::HeadCanonicalStoreActivation::Activated(crossings) => crossings,
    };
    let mut seen = std::collections::HashSet::with_capacity(crossings.len());
    for crossing in crossings {
        let authority = match crossing {
            meerkat_core::HeadCanonicalAuthorityCrossing::Converted(authority)
            | meerkat_core::HeadCanonicalAuthorityCrossing::AlreadyCurrent(authority) => authority,
            meerkat_core::HeadCanonicalAuthorityCrossing::NotApplicable => {
                return Err(PersistenceError::SessionPersistenceProfileMismatch {
                    profile,
                    detail: "HeadCanonical bulk activation contained NotApplicable".to_string(),
                });
            }
        };
        let session_id = authority.head().id.clone();
        let head_token = authority.head_token().to_string();
        if !seen.insert(session_id.clone()) {
            return Err(PersistenceError::SessionPersistenceProfileMismatch {
                profile,
                detail: format!(
                    "HeadCanonical bulk activation returned session {session_id} more than once"
                ),
            });
        }
        let aligned = runtime_store
            .activate_head_canonical_runtime_authority(authority)
            .await?;
        if aligned.authority().session_id() != &session_id
            || aligned.authority().committed_head_token() != head_token
        {
            return Err(PersistenceError::SessionPersistenceProfileMismatch {
                profile,
                detail: format!(
                    "HeadCanonical runtime activation returned authority different from verified physical session {session_id}"
                ),
            });
        }
    }
    Ok(())
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
    /// Durable records the preparation callback could not carry forward, each
    /// named with its reason. The domain still landed and these records' bytes
    /// were left exactly as found: nothing was deleted or blanked. A non-empty
    /// list is not a domain failure, but it is state this binary cannot read,
    /// so every caller must surface it rather than counting only what landed.
    pub refused_records: Vec<meerkat_sqlite::MaintenanceRecordRefusal>,
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
impl PreV0810DomainBridgeReport {
    /// True when this call established a ledger row, advanced a schema, or
    /// rewrote a durable payload.
    pub fn changed(&self) -> bool {
        self.ledger_established || self.from_version != self.to_version || self.prepared_rows != 0
    }

    /// True when some durable record in this domain was left behind.
    pub fn left_records_behind(&self) -> bool {
        !self.refused_records.is_empty()
    }
}

/// Why one domain did not land during the explicit pre-0.8.10 bridge.
///
/// Every variant is fail-closed for the domain it names: nothing was stamped
/// and no schema advanced. It exists so a refusal that only one domain earned
/// cannot silently condemn its co-tenants, and so the operator sees which
/// refusal they actually hit.
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PreV0810DomainRefusal {
    /// No frozen released catalog authenticates this domain's on-disk shape,
    /// so the binary cannot prove which release wrote it. The realm predates
    /// every source catalog this binary can bridge.
    UnauthenticatedSourceCatalog,
    /// The catalog authenticated but the migration chain refused to advance
    /// it. The domain is left at its prior version.
    MigrationRefused,
    /// The database file holding this domain could not be opened for
    /// maintenance, so the domain was never inspected.
    DatabaseUnopenable,
    /// Not attempted, because a domain this one reads from did not land.
    SkippedUnsatisfiedPrerequisite,
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
impl PreV0810DomainRefusal {
    /// Stable operator-facing label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnauthenticatedSourceCatalog => "unauthenticated-source-catalog",
            Self::MigrationRefused => "migration-refused",
            Self::DatabaseUnopenable => "database-unopenable",
            Self::SkippedUnsatisfiedPrerequisite => "skipped-unsatisfied-prerequisite",
        }
    }
}

/// One domain the explicit pre-0.8.10 bridge refused, with the reason.
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreV0810DomainBridgeFailure {
    /// Database file containing the domain.
    pub database: PathBuf,
    /// Stable ledger domain name.
    pub domain: String,
    /// Typed classification of the refusal.
    pub refusal: PreV0810DomainRefusal,
    /// Full text of the owning migration manifest's refusal.
    pub detail: String,
}

/// Result of the explicit pre-0.8.10 bridge across the SQLite files that
/// already exist in one realm.
///
/// Each domain owns its own transaction, so a domain that committed stays
/// committed when a later domain refuses. `domains` therefore records durable
/// progress and `failures` records refusals; both must be surfaced. A report
/// with a non-empty `failures` is not a success, and callers decide policy
/// through [`PreV0810RealmBridgeReport::is_complete`].
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreV0810RealmBridgeReport {
    /// Domains considered, in dependency-safe bridge order.
    pub domains: Vec<PreV0810DomainBridgeReport>,
    /// Domains this bridge refused, in the same order.
    pub failures: Vec<PreV0810DomainBridgeFailure>,
    /// Existing SQLite companions skipped because the realm manifest names a
    /// different durable authority.
    pub inactive_databases: Vec<PathBuf>,
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
impl PreV0810RealmBridgeReport {
    /// True when every domain the bridge considered landed.
    ///
    /// Deliberately says nothing about individual records. A domain that
    /// landed while leaving one record behind is complete *as a domain*: the
    /// schema advanced, the ledger is stamped, and the realm opens. Whether
    /// records stayed behind is a separate question with a separate answer,
    /// [`Self::records_left_behind`], because conflating them is what made
    /// one unrepresentable row cost an operator their whole realm.
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty()
    }

    /// Every durable record the bridge could not carry forward, across all
    /// domains that landed. Their bytes were left exactly as found.
    pub fn records_left_behind(
        &self,
    ) -> impl Iterator<Item = (&str, &meerkat_sqlite::MaintenanceRecordRefusal)> {
        self.domains.iter().flat_map(|domain| {
            domain
                .refused_records
                .iter()
                .map(|refusal| (domain.domain.as_str(), refusal))
        })
    }

    /// True when some domain durably advanced, stamped, or rewrote payloads.
    pub fn changed_anything(&self) -> bool {
        self.domains.iter().any(PreV0810DomainBridgeReport::changed)
    }
}

/// Classify a domain refusal without discarding its text.
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
fn classify_pre_v0_8_10_refusal(error: &meerkat_sqlite::SqliteStoreError) -> PreV0810DomainRefusal {
    match error {
        meerkat_sqlite::SqliteStoreError::UnledgeredSchemaNoMatch { .. }
        | meerkat_sqlite::SqliteStoreError::UnledgeredSchemaAmbiguous { .. }
        | meerkat_sqlite::SqliteStoreError::UnledgeredDomainObjects { .. }
        | meerkat_sqlite::SqliteStoreError::UnsupportedSchemaPredecessor { .. } => {
            PreV0810DomainRefusal::UnauthenticatedSourceCatalog
        }
        _ => PreV0810DomainRefusal::MigrationRefused,
    }
}

/// Record one domain's bridge outcome. Returns true when the domain landed,
/// so a dependent domain can be skipped rather than run against a source its
/// prerequisite did not establish.
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
fn record_pre_v0_8_10_outcome(
    report: &mut PreV0810RealmBridgeReport,
    database: &Path,
    domain: &meerkat_sqlite::SchemaDomain,
    ledger_before: Option<i64>,
    outcome: Result<meerkat_sqlite::MaintenanceBridgeReport, meerkat_sqlite::SqliteStoreError>,
) -> bool {
    match outcome {
        Ok(result) => {
            record_pre_v0_8_10_domain(report, database, domain, ledger_before, result);
            true
        }
        Err(error) => {
            report.failures.push(PreV0810DomainBridgeFailure {
                database: database.to_path_buf(),
                domain: domain.name.to_string(),
                refusal: classify_pre_v0_8_10_refusal(&error),
                detail: StoreError::from(error).to_string(),
            });
            false
        }
    }
}

/// Record every domain in a database the bridge could not open at all.
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
fn record_pre_v0_8_10_unopenable(
    report: &mut PreV0810RealmBridgeReport,
    database: &Path,
    domains: &[&meerkat_sqlite::SchemaDomain],
    error: &meerkat_sqlite::SqliteStoreError,
) {
    let detail = error.to_string();
    for domain in domains {
        report.failures.push(PreV0810DomainBridgeFailure {
            database: database.to_path_buf(),
            domain: domain.name.to_string(),
            refusal: PreV0810DomainRefusal::DatabaseUnopenable,
            detail: detail.clone(),
        });
    }
}

/// Bridge one domain that owns its database file outright.
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
fn bridge_pre_v0_8_10_single_domain_file(
    report: &mut PreV0810RealmBridgeReport,
    database: &Path,
    domain: &meerkat_sqlite::SchemaDomain,
    prepare: Option<meerkat_sqlite::MaintenancePrepareFn>,
) {
    if !database.is_file() {
        return;
    }
    let mut conn = match meerkat_sqlite::open(
        database,
        meerkat_sqlite::ConnectionProfile::Maintenance { write: true },
    ) {
        Ok(conn) => conn,
        Err(error) => {
            record_pre_v0_8_10_unopenable(report, database, &[domain], &error);
            return;
        }
    };
    let ledger_before = match meerkat_sqlite::domain_version(&conn, domain.name) {
        Ok(found) => found,
        Err(error) => {
            record_pre_v0_8_10_outcome(report, database, domain, None, Err(error));
            return;
        }
    };
    let outcome = meerkat_sqlite::bridge_unledgered_domain(
        &mut conn,
        domain,
        domain.supported_version(),
        domain.bridge_recoverable_versions,
        prepare,
    );
    record_pre_v0_8_10_outcome(report, database, domain, ledger_before, outcome);
}

/// Record a domain the bridge deliberately did not attempt.
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
fn record_pre_v0_8_10_skip(
    report: &mut PreV0810RealmBridgeReport,
    database: &Path,
    domain: &meerkat_sqlite::SchemaDomain,
    prerequisite: &str,
) {
    report.failures.push(PreV0810DomainBridgeFailure {
        database: database.to_path_buf(),
        domain: domain.name.to_string(),
        refusal: PreV0810DomainRefusal::SkippedUnsatisfiedPrerequisite,
        detail: format!(
            "not attempted because prerequisite domain '{prerequisite}' did not land in the \
             same database; this domain's migration reads state that domain owns"
        ),
    });
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
        refused_records: result.refused,
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
    //
    // Each domain owns its own transaction, so one domain's refusal is not
    // evidence about its co-tenants: it is recorded and the next domain is
    // still attempted. Only the true prerequisite edge is honoured - the
    // runtime bridge imports session snapshots, so a failed session-store
    // skips it. Schedule migrations read only schedule-owned objects, so they
    // proceed regardless. The cross-file census above stays an all-or-nothing
    // preflight on purpose: it runs before the first write, and a malformed or
    // future-version ledger anywhere means this binary must not begin.
    if paths.sessions_sqlite_path.is_file() {
        let database = &paths.sessions_sqlite_path;
        let sessions_file_domains: &[&meerkat_sqlite::SchemaDomain] = &[
            &meerkat_store::sqlite_store::SESSION_STORE_DOMAIN,
            &meerkat_runtime::store::sqlite::RUNTIME_STORE_DOMAIN,
            &meerkat_store::schedule_sqlite_store::SCHEDULE_STORE_DOMAIN,
        ];
        match meerkat_sqlite::open(
            database,
            meerkat_sqlite::ConnectionProfile::Maintenance { write: true },
        ) {
            Ok(mut conn) => {
                bridge_pre_v0_8_10_sessions_file(&mut report, database, &mut conn);
            }
            Err(error) => {
                record_pre_v0_8_10_unopenable(&mut report, database, sessions_file_domains, &error);
            }
        }
    }

    bridge_pre_v0_8_10_single_domain_file(
        &mut report,
        &paths.root.join("workgraph.sqlite3"),
        &meerkat_workgraph::WORKGRAPH_DOMAIN,
        Some(meerkat_workgraph::prepare_pre_0_8_10_workgraph_attention),
    );

    bridge_pre_v0_8_10_single_domain_file(
        &mut report,
        &paths.jobs_sqlite_path,
        &meerkat_jobs::JOBS_DOMAIN,
        None,
    );

    #[cfg(feature = "memory-store-session")]
    bridge_pre_v0_8_10_single_domain_file(
        &mut report,
        &paths.root.join("memory").join("memory.sqlite3"),
        &meerkat_memory::MEMORY_DOMAIN,
        None,
    );

    bridge_pre_v0_8_10_single_domain_file(
        &mut report,
        &paths.root.join("tasks.db"),
        &meerkat_tools::TOOLS_TASKS_DOMAIN,
        None,
    );

    Ok(report)
}

/// Bridge the three domains that co-tenant one realm's sessions database.
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
fn bridge_pre_v0_8_10_sessions_file(
    report: &mut PreV0810RealmBridgeReport,
    database: &Path,
    conn: &mut meerkat_sqlite::Connection,
) {
    let session_domain = &meerkat_store::sqlite_store::SESSION_STORE_DOMAIN;
    let session_landed = match meerkat_sqlite::domain_version(conn, session_domain.name) {
        Ok(ledger_before) => {
            let outcome = meerkat_core::with_pre_floor_provider_image_metadata_import(|| {
                meerkat_sqlite::bridge_unledgered_domain(
                    conn,
                    session_domain,
                    session_domain.supported_version(),
                    session_domain.bridge_recoverable_versions,
                    Some(meerkat_store::sqlite_store::prepare_pre_0_8_10_session_base_schema),
                )
            });
            record_pre_v0_8_10_outcome(report, database, session_domain, ledger_before, outcome)
        }
        Err(error) => {
            record_pre_v0_8_10_outcome(report, database, session_domain, None, Err(error))
        }
    };

    let runtime_domain = &meerkat_runtime::store::sqlite::RUNTIME_STORE_DOMAIN;
    if session_landed {
        match meerkat_sqlite::domain_version(conn, runtime_domain.name) {
            Ok(ledger_before) => {
                let outcome = meerkat_core::with_pre_floor_provider_image_metadata_import(|| {
                    meerkat_sqlite::bridge_unledgered_domain(
                        conn,
                        runtime_domain,
                        runtime_domain.supported_version(),
                        runtime_domain.bridge_recoverable_versions,
                        Some(
                            meerkat_runtime::store::sqlite::prepare_pre_0_8_10_runtime_input_states,
                        ),
                    )
                });
                record_pre_v0_8_10_outcome(
                    report,
                    database,
                    runtime_domain,
                    ledger_before,
                    outcome,
                );
            }
            Err(error) => {
                record_pre_v0_8_10_outcome(report, database, runtime_domain, None, Err(error));
            }
        }
    } else {
        record_pre_v0_8_10_skip(report, database, runtime_domain, session_domain.name);
    }

    let schedule_domain = &meerkat_store::schedule_sqlite_store::SCHEDULE_STORE_DOMAIN;
    match meerkat_sqlite::domain_version(conn, schedule_domain.name) {
        Ok(ledger_before) => {
            let outcome = meerkat_sqlite::bridge_unledgered_domain(
                conn,
                schedule_domain,
                schedule_domain.supported_version(),
                schedule_domain.bridge_recoverable_versions,
                None,
            );
            record_pre_v0_8_10_outcome(report, database, schedule_domain, ledger_before, outcome);
        }
        Err(error) => {
            record_pre_v0_8_10_outcome(report, database, schedule_domain, None, Err(error));
        }
    }
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
    if manifest.provider_name().is_some() {
        activate_external_head_canonical_store(&set.session_store, &set.runtime_store, profile)
            .await?;
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

    /// Copy a whole realm tree, including the derived `.rkat` projection the
    /// published writer left beside its databases.
    #[cfg(not(target_arch = "wasm32"))]
    fn copy_realm_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(destination)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            let target = destination.join(entry.file_name());
            if entry.path().is_dir() {
                copy_realm_tree(&entry.path(), &target)?;
            } else {
                std::fs::copy(entry.path(), target)?;
            }
        }
        Ok(())
    }

    /// Stage a realm written by a published pre-0.8.10 `rkat` into `state`.
    ///
    /// The corpus is owned by `meerkat-runtime` and minted from the release
    /// assets; see `tests/fixtures/v0_7_x_pre_ledger_realm`. Copying the whole
    /// directory keeps every byte the released writer left, including its own
    /// realm manifest.
    ///
    /// `capture` selects which run of that release is staged.
    /// `bootstrap-only` died before admitting any input and carries no
    /// `runtime_input_states` rows; `attempted-turn` admitted the operator's
    /// prompt and persisted it before the provider call failed, so it is the
    /// only shape that reaches the runtime bridge's row-preparation callback.
    #[cfg(not(target_arch = "wasm32"))]
    fn stage_published_pre_ledger_realm(
        state_root: &Path,
        realm_id: &str,
        version: &str,
        capture: &str,
    ) -> Result<meerkat_store::RealmPaths, Box<dyn std::error::Error>> {
        let corpus = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../meerkat-runtime/tests/fixtures/v0_7_x_pre_ledger_realm/corpus/realms")
            .join(version)
            .join(capture);
        assert!(
            corpus.is_dir(),
            "RELEASE BLOCKER: published pre-ledger realm corpus is missing at {}; re-mint it \
             with meerkat-runtime/tests/fixtures/v0_7_x_pre_ledger_realm/mint_pre_ledger_fixture.py",
            corpus.display()
        );
        let paths = realm_paths_in(state_root, realm_id);
        copy_realm_tree(&corpus, &paths.root)?;
        // The corpus realm is named `legacy-realm`; rewrite only the identity
        // so one corpus can stage under any test realm id.
        write_builtin_manifest(&paths, realm_id, RealmBackend::Sqlite)?;
        Ok(paths)
    }

    /// Diverge one domain's catalog so no frozen released verifier can
    /// authenticate it, without touching its co-tenants.
    #[cfg(not(target_arch = "wasm32"))]
    fn diverge_catalog(database: &Path, statement: &str) -> Result<(), Box<dyn std::error::Error>> {
        let conn = meerkat_sqlite::open(
            database,
            meerkat_sqlite::ConnectionProfile::Maintenance { write: true },
        )?;
        conn.execute_batch(statement)?;
        Ok(())
    }

    /// A domain the bridge cannot authenticate must not condemn the domains
    /// that share its file.
    ///
    /// This is the 0.8.23 shape: runtime-store refused, and because the loop
    /// returned on the first failure, schedule-store was never attempted even
    /// though it would have migrated cleanly. Two of three recoverable
    /// domains landed zero.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn unbridgeable_domain_does_not_condemn_its_co_tenants()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let realm_id = "legacy-realm";
        let paths =
            stage_published_pre_ledger_realm(temp.path(), realm_id, "0.7.5", "bootstrap-only")?;
        diverge_catalog(
            &paths.sessions_sqlite_path,
            "ALTER TABLE runtime_states ADD COLUMN unreleased_column TEXT",
        )?;

        let fence = meerkat_store::migrate::RealmMaintenanceFence::acquire(
            &paths.root,
            Duration::from_secs(1),
        )?;
        let report = bridge_pre_0_8_10_realm_storage_in(temp.path(), realm_id, &fence)?;
        drop(fence);

        assert!(
            !report.is_complete(),
            "a refused domain must leave the realm bridge incomplete"
        );
        let landed = report
            .domains
            .iter()
            .filter(|domain| domain.changed())
            .map(|domain| {
                (
                    domain.domain.as_str(),
                    domain.from_version,
                    domain.to_version,
                )
            })
            .collect::<Vec<_>>();
        assert!(
            landed.contains(&("session-store", 1, 4)),
            "session-store committed before the refusal and must be reported: {landed:?}"
        );
        assert!(
            landed.contains(&("schedule-store", 1, 3)),
            "schedule-store shares no state with runtime-store and must still land: {landed:?}"
        );

        let failures = report
            .failures
            .iter()
            .map(|failure| (failure.domain.as_str(), failure.refusal))
            .collect::<Vec<_>>();
        assert_eq!(
            failures,
            vec![(
                "runtime-store",
                PreV0810DomainRefusal::UnauthenticatedSourceCatalog
            )],
            "exactly the diverged domain must be refused"
        );

        // Fail-closed for the refused domain: no ledger row was stamped.
        let conn = meerkat_sqlite::open(
            &paths.sessions_sqlite_path,
            meerkat_sqlite::ConnectionProfile::ReadOnly,
        )?;
        assert_eq!(
            meerkat_sqlite::domain_version(
                &conn,
                meerkat_runtime::store::sqlite::RUNTIME_STORE_DOMAIN.name
            )?,
            None,
            "a refused domain must not be stamped"
        );
        assert_eq!(
            meerkat_sqlite::domain_version(
                &conn,
                meerkat_store::schedule_sqlite_store::SCHEDULE_STORE_DOMAIN.name
            )?,
            Some(meerkat_store::schedule_sqlite_store::SCHEDULE_STORE_DOMAIN.supported_version()),
            "schedule-store must be durably stamped"
        );
        Ok(())
    }

    /// The one real dependency edge is still honoured: the runtime bridge
    /// imports session snapshots, so a failed session-store must skip it
    /// rather than run it against a source that was never established.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn runtime_bridge_is_skipped_when_its_session_prerequisite_fails()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let realm_id = "legacy-realm";
        let paths =
            stage_published_pre_ledger_realm(temp.path(), realm_id, "0.7.5", "bootstrap-only")?;
        diverge_catalog(
            &paths.sessions_sqlite_path,
            "ALTER TABLE sessions ADD COLUMN unreleased_column TEXT",
        )?;

        let fence = meerkat_store::migrate::RealmMaintenanceFence::acquire(
            &paths.root,
            Duration::from_secs(1),
        )?;
        let report = bridge_pre_0_8_10_realm_storage_in(temp.path(), realm_id, &fence)?;
        drop(fence);

        assert!(!report.is_complete());
        let failures = report
            .failures
            .iter()
            .map(|failure| (failure.domain.as_str(), failure.refusal))
            .collect::<Vec<_>>();
        assert_eq!(
            failures,
            vec![
                (
                    "session-store",
                    PreV0810DomainRefusal::UnauthenticatedSourceCatalog
                ),
                (
                    "runtime-store",
                    PreV0810DomainRefusal::SkippedUnsatisfiedPrerequisite
                ),
            ],
            "runtime-store must be skipped, not attempted, when session-store fails"
        );
        assert!(
            report
                .domains
                .iter()
                .any(|domain| domain.domain == "schedule-store" && domain.to_version == 3),
            "schedule-store depends on neither and must still land"
        );
        Ok(())
    }

    /// A realm written by a published pre-0.8.10 binary must bridge end to end
    /// through the same facade entry point the CLI calls.
    ///
    /// Both captures of each release are staged. The `attempted-turn` capture
    /// is the one an operator actually owns: it carries the durable rows a
    /// real run left behind, and it is the only one that reaches the runtime
    /// bridge's row-preparation callback.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn published_pre_ledger_realm_bridges_through_the_facade()
    -> Result<(), Box<dyn std::error::Error>> {
        for (version, capture) in [
            ("0.7.5", "bootstrap-only"),
            ("0.7.5", "attempted-turn"),
            ("0.7.28", "bootstrap-only"),
            ("0.7.28", "attempted-turn"),
        ] {
            let temp = TempDir::new()?;
            let realm_id = "legacy-realm";
            let paths = stage_published_pre_ledger_realm(temp.path(), realm_id, version, capture)?;

            // What an ordinary open would tell this operator, for every domain
            // the bridge below then lands - including the one that lives in its
            // own file. The message and the command read the same authenticator
            // now, and a domain checked in neither place is a domain where that
            // could silently stop being true.
            for (domain, database) in [
                (
                    &meerkat_store::sqlite_store::SESSION_STORE_DOMAIN,
                    &paths.sessions_sqlite_path,
                ),
                (
                    &meerkat_runtime::store::sqlite::RUNTIME_STORE_DOMAIN,
                    &paths.sessions_sqlite_path,
                ),
                (
                    &meerkat_store::schedule_sqlite_store::SCHEDULE_STORE_DOMAIN,
                    &paths.sessions_sqlite_path,
                ),
                (
                    &meerkat_workgraph::WORKGRAPH_DOMAIN,
                    &paths.root.join("workgraph.sqlite3"),
                ),
            ] {
                let conn =
                    meerkat_sqlite::open(database, meerkat_sqlite::ConnectionProfile::ReadOnly)?;
                assert_eq!(
                    domain.bridge_eligibility(&conn),
                    meerkat_sqlite::BridgeEligibility::CatalogAuthenticated,
                    "rkat {version} ({capture}) domain {} is bridgeable and must be reported so",
                    domain.name
                );
            }

            let ingress_payloads_before = read_ingress_payloads(&paths.sessions_sqlite_path)?;
            assert_eq!(
                ingress_payloads_before.is_empty(),
                capture == "bootstrap-only",
                "rkat {version} ({capture}) does not carry the ingress payloads this capture \
                 exists to witness"
            );

            let fence = meerkat_store::migrate::RealmMaintenanceFence::acquire(
                &paths.root,
                Duration::from_secs(1),
            )?;
            let report = bridge_pre_0_8_10_realm_storage_in(temp.path(), realm_id, &fence)?;
            drop(fence);

            assert!(
                report.is_complete(),
                "realm written by published rkat {version} ({capture}) was refused: {:?}",
                report.failures
            );
            let landed = report
                .domains
                .iter()
                .map(|domain| {
                    (
                        domain.domain.as_str(),
                        domain.from_version,
                        domain.to_version,
                    )
                })
                .collect::<Vec<_>>();
            // The three sessions-file co-tenants are pinned exactly: their
            // source version is the fact this fix turns on.
            for expected in [
                ("session-store", 1, 4),
                ("runtime-store", 1, 3),
                ("schedule-store", 1, 3),
            ] {
                assert!(
                    landed.contains(&expected),
                    "rkat {version} ({capture}) realm missing {expected:?}: {landed:?}"
                );
            }
            // workgraph's own schema advanced inside the 0.7.x line (0.7.5
            // writes version 1, 0.7.28 writes version 2), so only its
            // authenticated source range and landing version are contracted.
            let workgraph = landed
                .iter()
                .find(|(domain, _, _)| *domain == "workgraph")
                .unwrap_or_else(|| {
                    panic!("rkat {version} ({capture}) realm has no workgraph domain")
                });
            assert!(
                (1..=2).contains(&workgraph.1),
                "rkat {version} ({capture}) workgraph source version {} is outside the authorized range",
                workgraph.1
            );
            assert_eq!(
                workgraph.2,
                meerkat_workgraph::WORKGRAPH_DOMAIN.supported_version(),
                "rkat {version} ({capture}) workgraph did not reach the current version"
            );

            // THE OPERATOR'S OWN PROMPT, through the entry point their command
            // calls. A bridge that reports success while deleting the ingress
            // payload it was run to rescue is worse than the refusal it
            // replaced, and this is the assertion that would have caught it:
            // the payload was read before, and it must read back after.
            for (input_id, before) in &ingress_payloads_before {
                let after = read_ingress_payload(&paths.sessions_sqlite_path, input_id)?;
                assert_eq!(
                    after.as_deref(),
                    Some(before.as_str()),
                    "rkat {version} ({capture}) input {input_id}: the operator's ingress payload \
                     did not survive a bridge that reported success"
                );
            }
        }
        Ok(())
    }

    /// Every input row's ingress payload text, keyed by input id.
    ///
    /// Reading this BEFORE the bridge is the point: an assertion built from
    /// what the bridge left behind can only prove self-consistency.
    #[cfg(not(target_arch = "wasm32"))]
    fn read_ingress_payloads(
        database: &Path,
    ) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
        let conn = meerkat_sqlite::open(database, meerkat_sqlite::ConnectionProfile::ReadOnly)?;
        let mut statement = conn.prepare(
            "SELECT input_id, json_extract(CAST(state_json AS TEXT), '$.persisted_input.content')
             FROM runtime_input_states
             ORDER BY runtime_id, input_id",
        )?;
        let rows = statement
            .query_map((), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|(input_id, content)| content.map(|content| (input_id, content)))
            .collect())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn read_ingress_payload(
        database: &Path,
        input_id: &str,
    ) -> Result<Option<String>, Box<dyn std::error::Error>> {
        let conn = meerkat_sqlite::open(database, meerkat_sqlite::ConnectionProfile::ReadOnly)?;
        Ok(conn.query_row(
            "SELECT json_extract(CAST(state_json AS TEXT), '$.persisted_input.content')
             FROM runtime_input_states WHERE input_id = ?1",
            (input_id,),
            |row| row.get::<_, Option<String>>(0),
        )?)
    }

    /// THE PER-RECORD CONTRACT REACHES THE WHOLE BRIDGE, not just its first
    /// step.
    ///
    /// Its predecessor pinned the opposite and called it a residual: the
    /// row-preparation callback refused an undecodable terminal row per
    /// record, and the runtime-store v1 -> v2 migration then re-decoded the
    /// same row with a hard `?` and refused the entire domain. So an operator
    /// who was told the schema was recognized and to run the bridge got
    /// session-store, schedule-store and workgraph committed and runtime-store
    /// refused - the promise they acted on was false, and one bad row still
    /// cost them the domain.
    ///
    /// The released-row importers no longer run under the rescue at all (see
    /// `pre_0_8_10_rescue_in_progress`), so the callback is the sole authority
    /// over row content there and the ordinary ledgered v1 -> v2 upgrade keeps
    /// failing closed on an undecodable durable row exactly as before. This
    /// test contracts the operator-visible half: the domain lands, the record
    /// is named, and its bytes are untouched.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn an_undecodable_terminal_row_costs_its_record_and_not_the_domain()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let realm_id = "legacy-realm";
        let paths =
            stage_published_pre_ledger_realm(temp.path(), realm_id, "0.7.28", "attempted-turn")?;
        let injected = "01a00703-278e-78b1-9207-ddd7a914abfd";
        let injected_bytes = {
            let conn = meerkat_sqlite::open(
                &paths.sessions_sqlite_path,
                meerkat_sqlite::ConnectionProfile::Maintenance { write: true },
            )?;
            let (runtime_id, source): (String, Vec<u8>) = conn.query_row(
                "SELECT runtime_id, state_json FROM runtime_input_states LIMIT 1",
                (),
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let mut row: serde_json::Value = serde_json::from_slice(&source)?;
            row["input_id"] = serde_json::json!(injected);
            row["persisted_input"]["header"]["id"] = serde_json::json!(injected);
            // Terminal (`abandoned`, inherited from the source row) and valid
            // JSON, but not decodable by the current type.
            row["attempt_count"] = serde_json::json!("three");
            let bytes = serde_json::to_vec(&row)?;
            conn.execute(
                "INSERT INTO runtime_input_states (runtime_id, input_id, state_json)
                 VALUES (?1, ?2, ?3)",
                (&runtime_id, injected, &bytes),
            )?;
            bytes
        };

        let fence = meerkat_store::migrate::RealmMaintenanceFence::acquire(
            &paths.root,
            Duration::from_secs(1),
        )?;
        let report = bridge_pre_0_8_10_realm_storage_in(temp.path(), realm_id, &fence)?;
        drop(fence);

        assert!(
            report.is_complete(),
            "one undecodable record must not refuse the domain the operator was told to \
             bridge: {:?}",
            report.failures
        );
        let left_behind = report.records_left_behind().collect::<Vec<_>>();
        assert!(
            left_behind
                .iter()
                .any(|(domain, refusal)| *domain == "runtime-store"
                    && refusal.record.contains(injected)),
            "the undecodable record must be named to the operator: {left_behind:?}"
        );

        let conn = meerkat_sqlite::open(
            &paths.sessions_sqlite_path,
            meerkat_sqlite::ConnectionProfile::ReadOnly,
        )?;
        assert_eq!(
            meerkat_sqlite::domain_version(
                &conn,
                meerkat_runtime::store::sqlite::RUNTIME_STORE_DOMAIN.name
            )?,
            Some(meerkat_runtime::store::sqlite::RUNTIME_STORE_DOMAIN.supported_version()),
            "the domain named in the remedy must land"
        );
        let stored: Vec<u8> = conn.query_row(
            "SELECT state_json FROM runtime_input_states WHERE input_id = ?1",
            (injected,),
            |row| row.get(0),
        )?;
        assert_eq!(stored, injected_bytes, "the refusal must not rewrite bytes");
        Ok(())
    }

    /// A record the bridge cannot carry must not cost the operator the realm.
    ///
    /// THE OTHER HALF OF THE FIX, END TO END. Every other assertion in this
    /// tree checks that nothing was refused; this one injects a row that
    /// WILL be refused - a row carrying a field this binary cannot
    /// represent, which is what a realm touched by a newer or unknown writer
    /// looks like - and then requires all of: the domain lands, the ledger is
    /// stamped, the refused row's bytes are untouched, the healthy sibling is
    /// carried, the refusal is named through the accessor the CLI prints
    /// from, and the realm afterwards OPENS.
    ///
    /// Asserting "refusal is per record" without ever producing a refusal
    /// would leave the operator-facing promise - that the bridge inspects
    /// records and prints the ones it cannot carry - backed by nothing, which
    /// is the exact shape of unverified promise this P0 exists to remove.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn an_uncarriable_record_is_named_and_the_realm_still_opens()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let realm_id = "legacy-realm";
        let paths =
            stage_published_pre_ledger_realm(temp.path(), realm_id, "0.7.28", "attempted-turn")?;

        // Derive the injected row from the realm's own row so it belongs to a
        // real runtime with real owning fragments. It is left NONTERMINAL on
        // purpose: the runtime-store v1 -> v2 migration only touches terminal
        // rows, so this row's bytes must survive the whole bridge untouched
        // and the test can assert that literally.
        let injected_input_id = "01a00703-278e-78b1-9207-ddd7a914abfe";
        let (runtime_id, injected_bytes) = {
            let conn = meerkat_sqlite::open(
                &paths.sessions_sqlite_path,
                meerkat_sqlite::ConnectionProfile::Maintenance { write: true },
            )?;
            let (runtime_id, source): (String, Vec<u8>) = conn.query_row(
                "SELECT runtime_id, state_json FROM runtime_input_states LIMIT 1",
                (),
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let mut row: serde_json::Value = serde_json::from_slice(&source)?;
            let first_history = row["history"][0].clone();
            row["input_id"] = serde_json::json!(injected_input_id);
            row["persisted_input"]["header"]["id"] = serde_json::json!(injected_input_id);
            row["current_state"] = serde_json::json!("queued");
            row["history"] = serde_json::json!([first_history]);
            row["updated_at"] = row["history"][0]["timestamp"].clone();
            row.as_object_mut()
                .expect("row object")
                .remove("terminal_outcome");
            // The fact this binary cannot represent. serde would drop it
            // silently on decode; the carry-forward oracle refuses instead.
            row["unknown_future_field"] = serde_json::json!({"written_by": "a newer rkat"});
            let bytes = serde_json::to_vec(&row)?;
            conn.execute(
                "INSERT INTO runtime_input_states (runtime_id, input_id, state_json)
                 VALUES (?1, ?2, ?3)",
                (&runtime_id, injected_input_id, &bytes),
            )?;
            (runtime_id, bytes)
        };

        let fence = meerkat_store::migrate::RealmMaintenanceFence::acquire(
            &paths.root,
            Duration::from_secs(1),
        )?;
        let report = bridge_pre_0_8_10_realm_storage_in(temp.path(), realm_id, &fence)?;
        drop(fence);

        // 1. One unrepresentable row does not abort the domain.
        assert!(
            report.is_complete(),
            "one uncarriable row refused the whole realm: {:?}",
            report.failures
        );
        let runtime = report
            .domains
            .iter()
            .find(|domain| domain.domain == "runtime-store")
            .expect("runtime-store domain report");
        assert_eq!(
            runtime.to_version,
            meerkat_runtime::store::sqlite::RUNTIME_STORE_DOMAIN.supported_version(),
            "the domain must still land at the current version"
        );
        assert_eq!(
            runtime.prepared_rows, 1,
            "the healthy sibling must still be carried"
        );

        // 2. The refusal is named, through the accessor `storage migrate`
        //    prints from.
        let left_behind = report.records_left_behind().collect::<Vec<_>>();
        assert_eq!(
            left_behind.len(),
            1,
            "expected exactly one named refusal, got {left_behind:?}"
        );
        let (domain, refusal) = left_behind[0];
        assert_eq!(domain, "runtime-store");
        assert!(
            refusal.record.contains(injected_input_id),
            "the refusal must name the record: {refusal:?}"
        );
        assert!(
            refusal.reason.contains("unknown_future_field"),
            "the refusal must name what could not be carried: {refusal:?}"
        );
        assert!(runtime.left_records_behind());

        // 3. The refused row's bytes are exactly as found. Nothing deleted,
        //    nothing blanked.
        {
            let conn = meerkat_sqlite::open(
                &paths.sessions_sqlite_path,
                meerkat_sqlite::ConnectionProfile::ReadOnly,
            )?;
            let stored: Vec<u8> = conn.query_row(
                "SELECT state_json FROM runtime_input_states
                 WHERE runtime_id = ?1 AND input_id = ?2",
                (&runtime_id, injected_input_id),
                |row| row.get(0),
            )?;
            assert_eq!(
                stored, injected_bytes,
                "a refused record must be left exactly as found"
            );
        }

        // 4. And the realm opens anyway - which is the whole point.
        let (_manifest, bundle) = open_realm_persistence_in(temp.path(), realm_id, None, None)
            .await
            .unwrap_or_else(|error| {
                panic!("realm did not open after a per-record refusal: {error}")
            });
        let sessions = bundle
            .session_store()
            .list(meerkat_core::SessionFilter::default())
            .await?;
        assert!(
            !sessions.is_empty(),
            "the realm's own session must still be readable"
        );
        Ok(())
    }

    /// THE OWNER'S QUESTION, END TO END: after the documented bridge, does
    /// the realm actually OPEN?
    ///
    /// Bridging exit 0 is not the deliverable. The 0.8.23 report ends with an
    /// operator whose realm is half-bridged and still unopenable, so the
    /// evidence that the P0 is fixed has to be the ordinary open path
    /// succeeding on the realm a published binary wrote - including the
    /// `attempted-turn` captures, whose durable rows are the ones the bridge
    /// used to refuse.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn bridged_pre_ledger_realm_opens_through_the_ordinary_path()
    -> Result<(), Box<dyn std::error::Error>> {
        for (version, capture) in [
            ("0.7.5", "attempted-turn"),
            ("0.7.28", "attempted-turn"),
            ("0.7.28", "bootstrap-only"),
        ] {
            let temp = TempDir::new()?;
            let realm_id = "legacy-realm";
            let paths = stage_published_pre_ledger_realm(temp.path(), realm_id, version, capture)?;

            // An ordinary open BEFORE the bridge must refuse: this is the
            // state the operator starts in, and if it already opened, the
            // test below would prove nothing.
            let before = open_realm_persistence_in(temp.path(), realm_id, None, None).await;
            assert!(
                before.is_err(),
                "rkat {version} ({capture}) realm opened without the bridge; \
                 this test can no longer prove the bridge is what fixed it"
            );

            let fence = meerkat_store::migrate::RealmMaintenanceFence::acquire(
                &paths.root,
                Duration::from_secs(1),
            )?;
            let report = bridge_pre_0_8_10_realm_storage_in(temp.path(), realm_id, &fence)?;
            drop(fence);
            assert!(
                report.is_complete(),
                "rkat {version} ({capture}) realm was refused: {:?}",
                report.failures
            );
            let left_behind = report.records_left_behind().collect::<Vec<_>>();
            assert!(
                left_behind.is_empty(),
                "rkat {version} ({capture}) realm left records behind: {left_behind:?}"
            );

            let (manifest, bundle) = open_realm_persistence_in(temp.path(), realm_id, None, None)
                .await
                .unwrap_or_else(|error| {
                    panic!(
                        "rkat {version} ({capture}) realm still does not open after the \
                             documented bridge: {error}"
                    )
                });
            assert_eq!(manifest.realm.as_str(), realm_id);
            assert!(bundle.blob_store().is_persistent());

            // The realm's own sessions must be readable, not merely present.
            let sessions = bundle
                .session_store()
                .list(meerkat_core::SessionFilter::default())
                .await
                .unwrap_or_else(|error| {
                    panic!("rkat {version} ({capture}) realm sessions unreadable: {error}")
                });
            if capture == "attempted-turn" {
                assert!(
                    !sessions.is_empty(),
                    "rkat {version} ({capture}) realm carried a session before the bridge and \
                     lists none after it"
                );
                for summary in &sessions {
                    bundle
                        .session_store()
                        .load(&summary.id)
                        .await
                        .unwrap_or_else(|error| {
                            panic!(
                                "rkat {version} ({capture}) session {} does not load after the \
                                 bridge: {error}",
                                summary.id
                            )
                        });
                }
            }
        }
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

        // `to_version` is DERIVED from each domain's own migration list rather
        // than written here. The contract this test exists to pin is that the
        // bridge orchestrates the existing realm databases IN ORDER and
        // converges each one to its current head - not what that head happens
        // to be this month. Hardcoding the numbers made a true statement about
        // the bridge read as a broken bridge: `jobs` gained a fourth migration
        // (`census-live-projection`) in an unrelated fix, and this assertion
        // failed while the bridge was doing exactly the right thing. The ORDER
        // stays literal, because the order IS the contract.
        fn head(domain: &meerkat_sqlite::SchemaDomain) -> i64 {
            domain
                .migrations
                .iter()
                .map(|migration| migration.version)
                .max()
                .unwrap_or_default()
        }

        let mut expected = vec![
            (
                "session-store",
                1,
                head(&meerkat_store::sqlite_store::SESSION_STORE_DOMAIN),
            ),
            (
                "runtime-store",
                1,
                head(&meerkat_runtime::store::sqlite::RUNTIME_STORE_DOMAIN),
            ),
            (
                "schedule-store",
                1,
                head(&meerkat_store::schedule_sqlite_store::SCHEDULE_STORE_DOMAIN),
            ),
            ("workgraph", 1, head(&meerkat_workgraph::WORKGRAPH_DOMAIN)),
            ("jobs", 1, head(&meerkat_jobs::JOBS_DOMAIN)),
        ];
        #[cfg(feature = "memory-store-session")]
        expected.push(("memory", 1, head(&meerkat_memory::MEMORY_DOMAIN)));
        expected.push(("tools-tasks", 1, head(&meerkat_tools::TOOLS_TASKS_DOMAIN)));

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

    /// Defect B regression: the exact physical catalog of a realm created
    /// before the schema ledger existed - a plain-CREATE `sessions` table,
    /// its `sessions_updated_idx` index, nothing else, and no
    /// `meerkat_schema` table - must bridge to the current version chain and
    /// keep its session readable through the real store opener.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn explicit_pre_floor_bridge_recovers_plain_create_legacy_session_realm()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let realm_id = "plain-create-realm";
        let paths = realm_paths_in(temp.path(), realm_id);
        write_builtin_manifest(&paths, realm_id, RealmBackend::Sqlite)?;

        std::fs::create_dir_all(&paths.root)?;
        let conn = meerkat_sqlite::open(
            &paths.sessions_sqlite_path,
            meerkat_sqlite::ConnectionProfile::Primary { create: true },
        )?;
        conn.execute_batch(
            "CREATE TABLE sessions (
                 session_id TEXT PRIMARY KEY,
                 created_at_ms INTEGER NOT NULL,
                 updated_at_ms INTEGER NOT NULL,
                 message_count INTEGER NOT NULL,
                 total_tokens INTEGER NOT NULL,
                 metadata_json TEXT NOT NULL,
                 session_json BLOB NOT NULL
             );
             CREATE INDEX sessions_updated_idx
             ON sessions(updated_at_ms DESC, session_id ASC);",
        )?;
        let mut session = Session::new();
        session.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("plain-create pre-floor fixture"),
        ));
        let session_id = session.id().to_string();
        let mut document = serde_json::to_value(&session)?;
        document["version"] = serde_json::json!(2);
        let source = serde_json::to_vec(&document)?;
        let _released_session = meerkat_core::import_released_0810_session(&source)
            .expect("fixture must be an exact released-v2 session document");
        conn.execute(
            "INSERT INTO sessions (session_id, created_at_ms, updated_at_ms, message_count, \
             total_tokens, metadata_json, session_json) VALUES (?1, 0, 0, 1, 0, ?2, ?3)",
            (
                &session_id,
                serde_json::to_string(session.metadata())?,
                &source,
            ),
        )?;
        drop(conn);

        let fence = meerkat_store::migrate::RealmMaintenanceFence::acquire(
            &paths.root,
            Duration::from_secs(1),
        )?;
        let report = bridge_pre_0_8_10_realm_storage_in(temp.path(), realm_id, &fence)?;
        drop(fence);

        let entry = report
            .domains
            .iter()
            .find(|entry| entry.domain == "session-store")
            .expect("session-store domain must be reported");
        assert_eq!(
            (entry.from_version, entry.to_version),
            (
                1,
                meerkat_store::sqlite_store::SESSION_STORE_DOMAIN.supported_version()
            ),
            "the pre-floor catalog must bridge to the current version"
        );
        assert!(entry.ledger_established);

        // Usable through the current version chain: the real opener applies
        // the domain (preflight + current fingerprint) and the session row
        // survived the physical rebuild.
        let store = meerkat_store::SqliteSessionStore::open(&paths.sessions_sqlite_path)?;
        drop(store);
        let conn = meerkat_sqlite::open(
            &paths.sessions_sqlite_path,
            meerkat_sqlite::ConnectionProfile::ReadOnly,
        )?;
        assert_eq!(
            meerkat_sqlite::domain_version(&conn, "session-store")?,
            Some(meerkat_store::sqlite_store::SESSION_STORE_DOMAIN.supported_version()),
        );
        let stored_id: String =
            conn.query_row("SELECT session_id FROM sessions", [], |row| row.get(0))?;
        assert_eq!(stored_id, session_id, "session row must survive the bridge");

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
                supported: 3,
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

    #[tokio::test]
    async fn external_head_canonical_activation_consumes_current_store_authority()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let store = Arc::new(SqliteSessionStore::open(
            temp.path().join("sessions.sqlite3"),
        )?);
        let runtime_store = Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new_head_canonical(
                temp.path().join("runtime.sqlite3"),
            )?,
        );
        let mut session = Session::new();
        session.push(meerkat_core::Message::User(
            meerkat_core::types::UserMessage::text("sparse legacy row"),
        ));
        store.save(&session).await?;
        let erased: Arc<dyn SessionStore> = store.clone();
        let erased_runtime: Arc<dyn RuntimeStore> = runtime_store.clone();

        // Simulate process death after the physical bulk crossing commits but
        // before the runtime authority consumes its proof.
        let first = Arc::clone(&store)
            .as_incremental()
            .expect("SQLite exposes HeadCanonical capability")
            .activate_head_canonical_store()
            .await?;
        assert!(matches!(
            first,
            meerkat_core::HeadCanonicalStoreActivation::Activated(ref crossings)
                if matches!(crossings.as_slice(), [meerkat_core::HeadCanonicalAuthorityCrossing::Converted(_)])
        ));

        activate_external_head_canonical_store(
            &erased,
            &erased_runtime,
            RuntimeSessionPersistenceProfile::HeadCanonicalV1,
        )
        .await?;

        let head = Arc::clone(&erased)
            .as_incremental()
            .expect("activated store remains incremental")
            .load_head(session.id())
            .await?
            .expect("activated authority keeps the physical head");
        assert!(head.message_row_prefix.is_some());
        assert!(head.row_lineage_anchor.is_some());
        assert!(head.realtime_event_prefix.is_some());
        assert!(head.metadata_identity().is_some());
        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(session.id());
        let aligned = runtime_store
            .load_session_boundary_authority(&runtime_id)
            .await?
            .expect("retry aligns runtime authority from AlreadyCurrent proof");
        let aligned = aligned
            .head_canonical()
            .expect("aligned authority is HeadCanonical");
        assert_eq!(aligned.boundary_head(), &head);
        let revision = aligned.store_revision();

        activate_external_head_canonical_store(
            &erased,
            &erased_runtime,
            RuntimeSessionPersistenceProfile::HeadCanonicalV1,
        )
        .await?;
        assert_eq!(
            runtime_store
                .load_session_boundary_authority(&runtime_id)
                .await?
                .and_then(|authority| authority.head_canonical().cloned())
                .expect("idempotent retry retains runtime authority")
                .store_revision(),
            revision,
            "an exact retry must not allocate another runtime revision"
        );
        Ok(())
    }

    #[tokio::test]
    async fn external_head_canonical_activation_refuses_semantic_boundary_drift()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let runtime_store = Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new_head_canonical(
                temp.path().join("runtime.sqlite3"),
            )?,
        );
        let session = Session::new();
        let first_store = Arc::new(SqliteSessionStore::open(temp.path().join("first.sqlite3"))?);
        let first =
            meerkat_core::session_store::PreparedHeadCanonicalMutation::prepare_root(&session)?;
        Arc::clone(&first_store)
            .as_incremental()
            .expect("SQLite is incremental")
            .apply_prepared_head_canonical_mutation(&first)
            .await?;
        let first_erased: Arc<dyn SessionStore> = first_store;
        let runtime_erased: Arc<dyn RuntimeStore> = runtime_store;
        activate_external_head_canonical_store(
            &first_erased,
            &runtime_erased,
            RuntimeSessionPersistenceProfile::HeadCanonicalV1,
        )
        .await?;

        let mut drifted = Session::with_id(session.id().clone());
        drifted.push(meerkat_core::Message::User(
            meerkat_core::types::UserMessage::text("different semantic boundary"),
        ));
        let second_store = Arc::new(SqliteSessionStore::open(
            temp.path().join("second.sqlite3"),
        )?);
        let second =
            meerkat_core::session_store::PreparedHeadCanonicalMutation::prepare_root(&drifted)?;
        Arc::clone(&second_store)
            .as_incremental()
            .expect("SQLite is incremental")
            .apply_prepared_head_canonical_mutation(&second)
            .await?;
        let second_erased: Arc<dyn SessionStore> = second_store;
        let error = activate_external_head_canonical_store(
            &second_erased,
            &runtime_erased,
            RuntimeSessionPersistenceProfile::HeadCanonicalV1,
        )
        .await
        .expect_err("semantic content drift cannot be called representation activation");
        assert!(matches!(error, PersistenceError::Runtime(_)));
        Ok(())
    }

    #[tokio::test]
    async fn external_head_canonical_activation_refuses_non_head_canonical_store()
    -> Result<(), Box<dyn std::error::Error>> {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store: Arc<dyn RuntimeStore> =
            Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new());
        let error = activate_external_head_canonical_store(
            &store,
            &runtime_store,
            RuntimeSessionPersistenceProfile::HeadCanonicalV1,
        )
        .await
        .expect_err("HeadCanonical runtime profile requires store-wide physical activation");

        assert!(matches!(
            error,
            PersistenceError::SessionPersistenceProfileMismatch { detail, .. }
                if detail.contains("returned NotApplicable")
        ));
        Ok(())
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
