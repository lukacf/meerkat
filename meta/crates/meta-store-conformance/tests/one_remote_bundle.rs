//! One remote bundle, demonstrated (M4b).
//!
//! A downstream backend implements ONE [`MetaStorageProvider`] and gets
//! sessions, continuity, events, blobs, and memory — the full
//! `meta-store-conformance` suite runs against the store set returned
//! through the single seam, and the meerkat conformance profiles run against
//! the same provider's meerkat level via `meerkat_provider()`. Nothing here
//! touches Meta internals: every handle comes out of `open_realm` /
//! `RealmStorageProvider::open`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::DurabilityResolution;
use meerkat_meta::blob_store::BinaryBlobStore;
use meerkat_meta::console_aggregator::ConsoleLogStore;
use meerkat_meta::identity_first::AgentMemoryProvider;
use meerkat_meta::storage_provider::{
    MetaLeaseAuthority, MetaRealmOpenContext, MetaRealmStoreSet, MetaStorageProvider,
    enforce_fail_closed_store_set,
};
use meerkat_meta::unified_runtime::EventLogStore;
use meta_store_conformance::{
    AgentMemoryProviderFactory, BinaryBlobStoreFactory, ConformanceFailure, ConsoleLogStoreFactory,
    EventLogStoreFactory, ReferenceMemoryBundleProvider, chapters,
};

/// The provider under test, held as the trait object a downstream would
/// register — the demonstration only ever speaks through the seam.
fn bundle() -> Arc<dyn MetaStorageProvider> {
    Arc::new(ReferenceMemoryBundleProvider::new())
}

fn realm_ctx(dir: &Path) -> MetaRealmOpenContext {
    MetaRealmOpenContext::for_state_dir(dir.join("bundle-realm"))
}

/// Open a fresh realm through the seam (each chapter assumes its storage
/// starts empty; a fresh `open_realm` on the in-memory bundle is a fresh
/// realm).
async fn open_fresh_realm(dir: &Path) -> MetaRealmStoreSet {
    bundle()
        .open_realm(&realm_ctx(dir))
        .await
        .expect("the reference bundle must open its realm store set")
}

// --- shared-handle factories over one opened slot ---------------------------
// In-memory stores share state per handle: a clone of the Arc is the correct
// restart model for deliberately non-persistent backends (see the factory
// contract).

struct SlotConsoleFactory(Arc<dyn ConsoleLogStore>);

#[async_trait]
impl ConsoleLogStoreFactory for SlotConsoleFactory {
    async fn open(&self) -> Result<Arc<dyn ConsoleLogStore>, ConformanceFailure> {
        Ok(Arc::clone(&self.0))
    }
}

struct SlotEventLogFactory(Arc<dyn EventLogStore>);

#[async_trait]
impl EventLogStoreFactory for SlotEventLogFactory {
    async fn open(&self) -> Result<Arc<dyn EventLogStore>, ConformanceFailure> {
        Ok(Arc::clone(&self.0))
    }
}

struct SlotBlobFactory(Arc<dyn BinaryBlobStore>);

#[async_trait]
impl BinaryBlobStoreFactory for SlotBlobFactory {
    async fn open(&self) -> Result<Arc<dyn BinaryBlobStore>, ConformanceFailure> {
        Ok(Arc::clone(&self.0))
    }
}

struct SlotMemoryFactory(Arc<dyn AgentMemoryProvider>);

#[async_trait]
impl AgentMemoryProviderFactory for SlotMemoryFactory {
    async fn open(&self) -> Result<Arc<dyn AgentMemoryProvider>, ConformanceFailure> {
        Ok(Arc::clone(&self.0))
    }
}

// --- the Meta level through the single seam -------------------------------

/// The bundle's store set is complete, fail-closed valid, and every slot is
/// an explicit in-memory declaration.
#[tokio::test]
async fn bundle_realm_opens_complete_and_fail_closed_valid() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = bundle();
    assert_eq!(provider.name(), "reference-memory");
    let ctx = realm_ctx(dir.path());
    let set = provider.open_realm(&ctx).await.expect("realm opens");

    enforce_fail_closed_store_set(&set, &ctx).expect("declared-ephemeral set composes");
    assert!(matches!(
        set.lease_authority,
        MetaLeaseAuthority::FencingFloor(0)
    ));
    assert!(set.event_log_store.is_some());
    assert!(set.agent_memory_provider.is_some());
    assert!(!set.blob_store.is_persistent());
    for domain in [
        "continuity",
        "event_log",
        "console",
        "metadata",
        "blobs",
        "agent_memory",
        "schedule",
    ] {
        let declaration = set
            .durability
            .iter()
            .find(|declaration| declaration.domain == domain)
            .unwrap_or_else(|| panic!("bundle must declare the {domain} slot"));
        assert_eq!(
            declaration.resolution,
            DurabilityResolution::DeclaredEphemeral,
            "the in-memory bundle declares every slot ephemeral explicitly"
        );
    }
    assert!(
        provider.migrator().is_none(),
        "the reference bundle honestly reports no migration story"
    );
}

#[tokio::test]
async fn bundle_event_log_slot_passes_the_event_log_profile() {
    let dir = tempfile::tempdir().expect("tempdir");
    let set = open_fresh_realm(dir.path()).await;
    let store: Arc<dyn EventLogStore> =
        Arc::from(set.event_log_store.expect("bundle supplies an event log"));
    chapters::event_log(&SlotEventLogFactory(store))
        .await
        .expect("the bundle's event log slot must satisfy the event log profile");
}

#[tokio::test(flavor = "multi_thread")]
async fn bundle_console_slot_passes_the_console_profile() {
    let dir = tempfile::tempdir().expect("tempdir");
    let set = open_fresh_realm(dir.path()).await;
    chapters::console_log(&SlotConsoleFactory(set.console_log_store))
        .await
        .expect("the bundle's console slot must satisfy the console profile");
}

#[tokio::test]
async fn bundle_blob_slot_passes_the_binary_blob_profile() {
    let dir = tempfile::tempdir().expect("tempdir");
    let set = open_fresh_realm(dir.path()).await;
    chapters::binary_blobs(&SlotBlobFactory(set.blob_store), Some(false))
        .await
        .expect("the bundle's blob slot must satisfy the binary blob profile");
}

#[tokio::test]
async fn bundle_memory_slot_passes_the_capability_matrix() {
    let dir = tempfile::tempdir().expect("tempdir");
    let set = open_fresh_realm(dir.path()).await;
    chapters::agent_memory(&SlotMemoryFactory(
        set.agent_memory_provider
            .expect("bundle supplies agent memory"),
    ))
    .await
    .expect("the bundle's memory slot must satisfy the capability matrix");
}

// --- the meerkat level via meerkat_provider() --------------------------------

fn meerkat_realm_ctx(root: &Path) -> meerkat::storage_provider::RealmOpenContext {
    let realm = meerkat_core::RealmId::parse("bundle-conformance").expect("realm id");
    meerkat::storage_provider::RealmOpenContext {
        locator: meerkat_core::RealmLocator {
            state_root: root.to_path_buf(),
            realm: realm.clone(),
        },
        manifest: meerkat_store::RealmManifestPin::Builtin(meerkat_store::RealmManifest {
            realm,
            backend: meerkat_store::RealmBackend::Memory,
            origin: meerkat_store::RealmOrigin::Explicit,
            created_at: "2026-07-23T00:00:00Z".to_string(),
            manifest_format: 2,
            provider: Some("reference-memory".to_string()),
            ephemeral_domains: [
                "sessions",
                "runtime",
                "schedule",
                "workgraph",
                "blobs",
                "artifacts",
            ]
            .iter()
            .map(ToString::to_string)
            .collect(),
            // An ungoverned realm: the paired 0.9 store records a governance
            // posture only in governed-format (3) manifests, and refuses one
            // in a format 2 manifest like this.
            governance: None,
        }),
        paths: meerkat_store::realm_paths_in(root, "bundle-conformance"),
        layout: None,
    }
}

async fn open_fresh_meerkat_realm(
    provider: &Arc<dyn MetaStorageProvider>,
    root: &Path,
) -> meerkat::storage_provider::RealmStoreSet {
    let ctx = meerkat_realm_ctx(root);
    let set = provider
        .meerkat_provider()
        .open(&ctx)
        .await
        .expect("the bundle's meerkat provider must open its realm");
    let ephemeral_domains = match &ctx.manifest {
        meerkat_store::RealmManifestPin::Builtin(manifest) => manifest.ephemeral_domains.clone(),
        meerkat_store::RealmManifestPin::External(manifest) => manifest.ephemeral_domains.clone(),
    };
    meerkat::storage_provider::enforce_fail_closed_durability(&set, &ephemeral_domains)
        .expect("declared-ephemeral meerkat set composes");
    set
}

/// The same provider covers the meerkat level: its session store passes the
/// upstream baseline, append-only, and incremental profiles; blobs and
/// artifacts pass theirs.
#[tokio::test]
async fn bundle_meerkat_level_passes_the_upstream_profiles() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = bundle();

    let set = open_fresh_meerkat_realm(&provider, dir.path()).await;
    let sessions = set.session_store;
    meerkat_store_conformance::chapters::baseline(
        &meerkat_store_conformance::FnSessionStoreFactory::new(|| {
            let sessions = Arc::clone(&sessions);
            async move { Ok(sessions) }
        }),
    )
    .await
    .expect("the bundle's meerkat session store must satisfy the baseline profile");

    let set = open_fresh_meerkat_realm(&provider, dir.path()).await;
    let sessions = set.session_store;
    meerkat_store_conformance::chapters::append_only(
        &meerkat_store_conformance::FnSessionStoreFactory::new(|| {
            let sessions = Arc::clone(&sessions);
            async move { Ok(sessions) }
        }),
    )
    .await
    .expect("the bundle's meerkat session store must satisfy the append-only profile");

    let set = open_fresh_meerkat_realm(&provider, dir.path()).await;
    let sessions = set.session_store;
    meerkat_store_conformance::chapters::incremental(
        &meerkat_store_conformance::FnSessionStoreFactory::new(|| {
            let sessions = Arc::clone(&sessions);
            async move { Ok(sessions) }
        }),
    )
    .await
    .expect("the bundle's meerkat session store must satisfy the incremental profile");

    let set = open_fresh_meerkat_realm(&provider, dir.path()).await;
    let blobs = set.blob_store;
    meerkat_store_conformance::chapters::blobs(
        &meerkat_store_conformance::FnBlobStoreFactory::new(|| {
            let blobs = Arc::clone(&blobs);
            async move { Ok(blobs) }
        }),
    )
    .await
    .expect("the bundle's meerkat blob store must satisfy the blob profile");

    let set = open_fresh_meerkat_realm(&provider, dir.path()).await;
    let artifacts = set.artifact_store;
    meerkat_store_conformance::chapters::artifacts(
        &meerkat_store_conformance::FnArtifactStoreFactory::new(|| {
            let artifacts = Arc::clone(&artifacts);
            async move { Ok(artifacts) }
        }),
    )
    .await
    .expect("the bundle's meerkat artifact store must satisfy the artifact profile");
}

#[tokio::test]
async fn bundle_meerkat_level_supplies_the_inherited_jobs_authority() {
    let dir = tempfile::tempdir().expect("tempdir");
    let provider = bundle();
    let set = open_fresh_meerkat_realm(&provider, dir.path()).await;
    let service = meerkat::DetachedJobService::new(set.job_store);
    let session =
        meerkat_core::SessionId::parse("019f74fb-1907-7b21-932d-ab22c4d1f532").expect("session");
    let receipt = service
        .submit(meerkat::JobSpec::new(
            "bundle-conformance",
            session,
            meerkat::ExecutionIntentId::from_string("intent:bundle").expect("intent"),
            meerkat::InteractionLineageId::from_string("lineage:bundle").expect("lineage"),
            meerkat::ToolIdentity::new("scan", "1").expect("tool"),
            meerkat::RunnerIdentity::new("scan.runner", "1").expect("runner"),
            meerkat::RestartClass::NonResumable,
            meerkat::CanonicalArgumentsHash::new(format!("sha256:{}", "b".repeat(64)))
                .expect("arguments"),
            meerkat::JobSubmissionKey::new("bundle:jobs:1").expect("submission"),
        ))
        .await
        .expect("submit");
    assert!(service.get(&receipt.job_id).await.expect("get").is_some());
}
