//! The realm storage-provider seam.
//!
//! One provider supplies all durable stores for a realm; the facade
//! composes `PersistenceBundle` (and, at surface level, `MeerkatMachine`)
//! from what the provider returns. The seam is **store-only** by design:
//! putting mob stores in the result would create the dependency cycle
//! `meerkat → meerkat-mob → meerkat` (mob depends on this facade), so mob
//! storage stays mob-owned and higher layers (mobkit's composite provider)
//! compose their own stores next to a [`RealmStorageProvider`].
//!
//! Bootstrap convergence:
//! `RuntimeBootstrap → StorageLayout → provider(manifest) → facade composition`
//! ([`open_realm_persistence_with_provider`]); the built-in
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
use meerkat_core::{ArtifactStore, BlobStore, DurabilityDeclaration, RealmLocator, StorageLayout};
use meerkat_runtime::RuntimeStore;
use meerkat_schedule::ScheduleStore;
use meerkat_workgraph::WorkGraphStore;

use crate::SessionStore;
use crate::persistence::PersistenceError;
use meerkat_store::{RealmManifest, RealmPaths};

/// Everything a provider needs to open a realm's stores.
#[derive(Clone)]
pub struct RealmOpenContext {
    /// The resolved `(state_root, realm)` locator.
    pub locator: RealmLocator,
    /// The realm's pinned manifest (backend, origin, v2 fields).
    pub manifest: RealmManifest,
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
}

/// Enforce the fail-closed durability rule against the realm manifest.
pub fn enforce_fail_closed_durability(
    set: &RealmStoreSet,
    manifest: &RealmManifest,
) -> Result<(), PersistenceError> {
    for declaration in &set.durability {
        if declaration.is_undeclared_nonpersistent_durable()
            && !manifest
                .ephemeral_domains
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
        crate::persistence::open_disk_store_set(ctx)
    }
}
