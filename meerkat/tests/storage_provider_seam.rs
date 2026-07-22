//! Phase 4 provider-seam contract: bootstrap convergence through
//! `open_realm_persistence_with_provider`, and the fail-closed durability
//! rule.

#![cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use async_trait::async_trait;
use meerkat::storage_provider::{
    DiskStorageProvider, RealmOpenContext, RealmStorageProvider, RealmStoreSet,
    enforce_fail_closed_durability,
};
use meerkat::{PersistenceError, open_realm_persistence_with_provider};
use meerkat_core::{DurabilityDeclaration, DurabilityResolution};
use meerkat_store::RealmBackend;

#[tokio::test]
async fn disk_provider_composes_a_working_sqlite_realm() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (manifest, bundle) = open_realm_persistence_with_provider(
        &DiskStorageProvider,
        tmp.path(),
        "team",
        Some(RealmBackend::Sqlite),
        None,
        None,
    )
    .await
    .expect("open");
    assert_eq!(manifest.backend, RealmBackend::Sqlite);
    assert_eq!(manifest.manifest_format, 1, "disk realms stay v1-shaped");
    // The bundle serves stores and carries the realm context.
    assert!(bundle.manifest().is_some());
    assert!(bundle.store_path().is_some());
    assert!(
        bundle.event_projection().is_some(),
        "disk realms get event projection"
    );
    // The session store round-trips through the composed bundle.
    let session = meerkat_core::Session::new();
    bundle
        .session_store()
        .save(&session)
        .await
        .expect("save through the composed bundle");
    let loaded = bundle
        .session_store()
        .load(session.id())
        .await
        .expect("load")
        .expect("present");
    assert_eq!(loaded.id(), session.id());
}

#[tokio::test]
async fn memory_realm_is_declared_ephemeral_not_silently_nonpersistent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (manifest, _bundle) = open_realm_persistence_with_provider(
        &DiskStorageProvider,
        tmp.path(),
        "ephemeral-team",
        Some(RealmBackend::Memory),
        None,
        None,
    )
    .await
    .expect("memory realms are a declared choice and must open");
    assert_eq!(manifest.backend, RealmBackend::Memory);
}

/// A provider that supplies a durable slot backed by a non-persistent store
/// WITHOUT declaring it — the exact silent-fallback class the rule exists
/// to refuse.
struct UndeclaredEphemeralProvider;

#[async_trait]
impl RealmStorageProvider for UndeclaredEphemeralProvider {
    fn name(&self) -> &'static str {
        "undeclared-ephemeral"
    }

    async fn open(&self, ctx: &RealmOpenContext) -> Result<RealmStoreSet, PersistenceError> {
        let mut set = DiskStorageProvider.open(ctx).await?;
        set.blob_store = Arc::new(meerkat_store::MemoryBlobStore::new());
        for declaration in &mut set.durability {
            if declaration.domain == "blobs" {
                declaration.resolution = DurabilityResolution::NonPersistent;
            }
        }
        Ok(set)
    }
}

#[tokio::test]
async fn undeclared_nonpersistent_durable_slot_is_a_startup_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let result = open_realm_persistence_with_provider(
        &UndeclaredEphemeralProvider,
        tmp.path(),
        "team",
        Some(RealmBackend::Sqlite),
        None,
        None,
    )
    .await;
    match result {
        Err(PersistenceError::DurabilityViolation { domain }) => assert_eq!(domain, "blobs"),
        Err(other) => panic!("wrong error: {other}"),
        Ok(_) => panic!("fail-closed durability must refuse"),
    }
}

#[test]
fn manifest_ephemeral_declaration_admits_the_slot() {
    // Unit-level: the enforcement helper honors the manifest's declared
    // ephemeral domains.
    let manifest_json = serde_json::json!({
        "realm_id": "team",
        "backend": "sqlite",
        "created_at": "0",
        "manifest_format": 2,
        "ephemeral_domains": ["blobs"],
    });
    let manifest: meerkat_store::RealmManifest =
        serde_json::from_value(manifest_json).expect("manifest");
    let set = RealmStoreSet {
        session_store: Arc::new(meerkat_store::MemoryStore::new()),
        runtime_store: Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new()),
        schedule_store: Arc::new(meerkat_schedule::MemoryScheduleStore::new()),
        workgraph_store: Arc::new(meerkat_workgraph::MemoryWorkGraphStore::new()),
        blob_store: Arc::new(meerkat_store::MemoryBlobStore::new()),
        artifact_store: Arc::new(meerkat_store::MemoryArtifactStore::new()),
        store_path: std::path::PathBuf::from("."),
        projection_root: None,
        durability: vec![DurabilityDeclaration::durable(
            "blobs",
            DurabilityResolution::NonPersistent,
        )],
    };
    enforce_fail_closed_durability(&set, &manifest).expect("declared domain admits the slot");

    let strict_manifest: meerkat_store::RealmManifest = serde_json::from_value(serde_json::json!({
        "realm_id": "team",
        "backend": "sqlite",
        "created_at": "0",
    }))
    .expect("manifest");
    let err =
        enforce_fail_closed_durability(&set, &strict_manifest).expect_err("undeclared refuses");
    assert!(matches!(
        err,
        PersistenceError::DurabilityViolation { domain } if domain == "blobs"
    ));
}
