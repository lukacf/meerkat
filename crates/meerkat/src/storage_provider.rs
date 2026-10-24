//! The realm storage-provider seam.
//!
//! One provider supplies all durable stores for a realm; the facade
//! composes `PersistenceBundle` (and, at surface level, `MeerkatMachine`)
//! from what the provider returns. The seam is **store-only** by design:
//! putting mob stores in the result would create the dependency cycle
//! `meerkat → meerkat-mob → meerkat` (mob depends on this facade), so mob
//! storage stays mob-owned and higher layers (meta's composite provider)
//! compose their own stores next to a [`RealmStorageProvider`].
//!
//! Bootstrap convergence:
//! `RuntimeBootstrap → StorageLayout → provider(manifest) → facade composition`
//! ([`crate::open_realm_persistence_with_provider`]); the built-in
//! [`DiskStorageProvider`] reproduces today's sqlite/jsonl/memory realms.
//!
//! Fail-closed durability: every slot the provider returns carries a
//! [`DurabilityDeclaration`]; a `Durable` slot that resolved non-persistent
//! without the realm manifest declaring that domain ephemeral is a startup
//! error ([`PersistenceError::DurabilityViolation`]) — never a silent
//! in-memory fallback.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::host_store::HostStore;
use meerkat_core::{ArtifactStore, BlobStore, DurabilityDeclaration, RealmLocator, StorageLayout};
use meerkat_jobs::DetachedJobStore;
use meerkat_runtime::RuntimeStore;
use meerkat_runtime::store::spi::{SpiRealm, SpiRealmConfig, SpiRuntimeStore, SpiSessionStore};
use meerkat_schedule::ScheduleStore;
use meerkat_workgraph::WorkGraphStore;

use crate::SessionStore;
use crate::persistence::PersistenceError;
use meerkat_store::{RealmManifestPin, RealmPaths};

/// Everything a provider needs to open a realm's stores.
#[derive(Clone)]
pub struct RealmOpenContext {
    /// The resolved `(state_root, realm)` locator.
    pub locator: RealmLocator,
    /// The realm's pinned manifest: builtin disk backend or an
    /// external-provider pin (provider-aware opens receive their own pins).
    pub manifest: RealmManifestPin,
    /// Canonical per-realm path fan-out under the state root.
    pub paths: RealmPaths,
    /// The bootstrap path authority, when the surface resolved one.
    pub layout: Option<StorageLayout>,
}

/// The stores (plus durability declarations) a provider supplies for one
/// realm. Store-only by design — the facade composes the bundle.
pub struct RealmStoreSet {
    pub session_store: Arc<dyn SessionStore>,
    pub runtime_store: Arc<dyn RuntimeStore>,
    pub schedule_store: Arc<dyn ScheduleStore>,
    pub workgraph_store: Arc<dyn WorkGraphStore>,
    pub job_store: Arc<dyn DetachedJobStore>,
    pub blob_store: Arc<dyn BlobStore>,
    pub artifact_store: Arc<dyn ArtifactStore>,
    /// The factory `store_path` for this realm (feature-owned relative
    /// paths — tasks.db, memory/ — hang off it).
    pub store_path: PathBuf,
    /// Where session projections and the file event log should
    /// materialize; `None` disables event projection (memory realms).
    pub projection_root: Option<PathBuf>,
    /// Per-slot durability declarations, machine-readable.
    pub durability: Vec<DurabilityDeclaration>,
}

/// One provider supplies all durable stores for a realm.
#[async_trait]
pub trait RealmStorageProvider: Send + Sync {
    /// Stable provider name (pinned in the realm manifest for external
    /// providers).
    fn name(&self) -> &str;

    /// Open (or create) the realm's stores.
    async fn open(&self, ctx: &RealmOpenContext) -> Result<RealmStoreSet, PersistenceError>;

    /// The provider's migration hook (diagnosis now; mutation verbs land as
    /// defaulted methods on [`meerkat_core::StorageMigrator`]). `None` =
    /// this provider has no migration story yet.
    fn migrator(&self) -> Option<&dyn meerkat_core::StorageMigrator> {
        None
    }
}

/// The store slots every provider must declare durability for — one
/// declaration per slot, no omissions (an omitted declaration would bypass
/// the fail-closed rule entirely).
pub const REQUIRED_DURABILITY_DOMAINS: [&str; 7] = [
    "sessions",
    "runtime",
    "schedule",
    "workgraph",
    "jobs",
    "blobs",
    "artifacts",
];

/// Enforce the fail-closed durability rule against the realm manifest.
///
/// Completeness first: every slot in [`REQUIRED_DURABILITY_DOMAINS`] must
/// carry exactly one declaration — a provider cannot dodge the rule by
/// omitting a domain or returning an empty list. Then each declaration is
/// checked: a `Durable` slot resolving non-persistent without the realm
/// manifest declaring that domain ephemeral refuses startup typed.
pub fn enforce_fail_closed_durability(
    set: &RealmStoreSet,
    ephemeral_domains: &[String],
) -> Result<(), PersistenceError> {
    for required in REQUIRED_DURABILITY_DOMAINS {
        let count = set
            .durability
            .iter()
            .filter(|declaration| declaration.domain == required)
            .count();
        if count != 1 {
            return Err(PersistenceError::DurabilityViolation {
                domain: format!(
                    "{required} (provider supplied {count} durability declarations for this \
                     slot; exactly one is required)"
                ),
            });
        }
    }
    for declaration in &set.durability {
        if declaration.is_undeclared_nonpersistent_durable()
            && !ephemeral_domains
                .iter()
                .any(|domain| domain == &declaration.domain)
        {
            return Err(PersistenceError::DurabilityViolation {
                domain: declaration.domain.clone(),
            });
        }
    }
    Ok(())
}

/// Open realm persistence through an externally resolved [`StorageLayout`]
/// (built-in disk composition).
///
/// The layout is the single path authority: its state root is the realm
/// root, and its realm-root candidates arm the cross-candidate first-start
/// reservation (`meerkat_store::realm::ensure_realm_manifest_pin_with_candidates`),
/// so a surface that resolved dual roots at bootstrap gets race-safe first
/// materialization without resolving twice. Surfaces without a resolved
/// layout keep using [`crate::open_realm_persistence_in`], which builds a
/// single-candidate layout from its explicit root.
pub async fn open_realm_persistence_with_layout(
    layout: StorageLayout,
    realm_id: &str,
    backend_hint: Option<meerkat_store::RealmBackend>,
    origin_hint: Option<meerkat_store::RealmOrigin>,
) -> Result<(meerkat_store::RealmManifest, crate::PersistenceBundle), PersistenceError> {
    crate::persistence::open_realm_persistence_builtin_with_layout(
        layout,
        realm_id,
        backend_hint,
        origin_hint,
    )
    .await
}

/// The built-in disk provider: sqlite / jsonl / memory realms exactly as
/// before the seam existed.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiskStorageProvider;

#[async_trait]
impl RealmStorageProvider for DiskStorageProvider {
    fn name(&self) -> &'static str {
        "disk"
    }

    async fn open(&self, ctx: &RealmOpenContext) -> Result<RealmStoreSet, PersistenceError> {
        crate::persistence::open_disk_store_set(ctx).await
    }

    fn migrator(&self) -> Option<&dyn meerkat_core::StorageMigrator> {
        static DISK_MIGRATOR: meerkat_store::doctor::DiskStorageMigrator =
            meerkat_store::doctor::DiskStorageMigrator;
        Some(&DISK_MIGRATOR)
    }
}

/// The stores a host supplies next to a host store realm: everything the
/// host store SPI does not carry yet (schedules, the work graph, detached
/// jobs, blobs and artifacts), with one durability declaration per slot.
pub struct HostSuppliedStores {
    pub schedule_store: Arc<dyn ScheduleStore>,
    pub workgraph_store: Arc<dyn WorkGraphStore>,
    pub job_store: Arc<dyn DetachedJobStore>,
    pub blob_store: Arc<dyn BlobStore>,
    pub artifact_store: Arc<dyn ArtifactStore>,
    /// The factory `store_path` for the realm.
    pub store_path: PathBuf,
    /// Where session projections materialize; `None` disables them.
    pub projection_root: Option<PathBuf>,
    /// Declarations for `schedule`, `workgraph`, `jobs`, `blobs` and
    /// `artifacts`. The provider declares `sessions` and `runtime` itself.
    pub durability: Vec<DurabilityDeclaration>,
}

/// A realm whose runtime and session state live in the host's own database
/// through the host store SPI ([`meerkat_core::host_store`]).
///
/// [`HostStoreProvider::open`] acquires the realm's fence once
/// ([`SpiRealm`]); every store set it opens shares that realm, and the
/// host builds the realm's mob storage over [`HostStoreProvider::realm`]
/// (meerkat-mob's host store adapter), so the runtime and the mob adapters
/// commit under one fence. The session slot is a read adapter over the
/// runtime store ([`SpiSessionStore`]): a host store realm is WholeBlob and
/// the session service writes sessions only through the runtime store.
///
/// OAuth login flows in progress stay process-local on such a realm: a
/// restart cancels a login in progress and the user starts it again.
/// Completed credentials are not affected. Temporary councils are
/// process-local too: the default council store declares itself
/// process-bound, so a council that requires durable custody is refused and
/// a process-bound one does not survive a restart.
pub struct HostStoreProvider {
    name: String,
    realm: Arc<SpiRealm>,
    runtime_store: Arc<SpiRuntimeStore>,
    supplied: HostSuppliedStores,
}

impl HostStoreProvider {
    /// Open the realm over `host`: acquire its fence (exclusive unless
    /// `config` says take over), list its heads, and open the runtime store.
    ///
    /// # Errors
    /// A held fence, a host failure or host data that does not decode, as
    /// [`PersistenceError::Runtime`].
    pub async fn open(
        name: impl Into<String>,
        host: Arc<dyn HostStore>,
        config: SpiRealmConfig,
        supplied: HostSuppliedStores,
    ) -> Result<Self, PersistenceError> {
        let realm = SpiRealm::open(host, config)
            .await
            .map_err(|error| PersistenceError::Runtime(error.into_read()))?;
        let runtime_store = Arc::new(
            SpiRuntimeStore::open(Arc::clone(&realm))
                .await
                .map_err(|error| PersistenceError::Runtime(error.into_read()))?,
        );
        Ok(Self {
            name: name.into(),
            realm,
            runtime_store,
            supplied,
        })
    }

    /// The realm every adapter of this process commits through: build the
    /// mob storage over it.
    #[must_use]
    pub fn realm(&self) -> &Arc<SpiRealm> {
        &self.realm
    }

    /// The realm's runtime store.
    #[must_use]
    pub fn runtime_store(&self) -> &Arc<SpiRuntimeStore> {
        &self.runtime_store
    }
}

#[async_trait]
impl RealmStorageProvider for HostStoreProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn open(&self, _ctx: &RealmOpenContext) -> Result<RealmStoreSet, PersistenceError> {
        use meerkat_core::DurabilityResolution;
        let mut durability = vec![
            DurabilityDeclaration::durable("sessions", DurabilityResolution::Persistent),
            DurabilityDeclaration::durable("runtime", DurabilityResolution::Persistent),
        ];
        durability.extend(self.supplied.durability.iter().cloned());
        Ok(RealmStoreSet {
            session_store: Arc::new(SpiSessionStore::new(Arc::clone(&self.runtime_store))),
            runtime_store: Arc::clone(&self.runtime_store) as Arc<dyn RuntimeStore>,
            schedule_store: Arc::clone(&self.supplied.schedule_store),
            workgraph_store: Arc::clone(&self.supplied.workgraph_store),
            job_store: Arc::clone(&self.supplied.job_store),
            blob_store: Arc::clone(&self.supplied.blob_store),
            artifact_store: Arc::clone(&self.supplied.artifact_store),
            store_path: self.supplied.store_path.clone(),
            projection_root: self.supplied.projection_root.clone(),
            durability,
        })
    }
}
