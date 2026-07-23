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
    let (pin, bundle) = open_realm_persistence_with_provider(
        &DiskStorageProvider,
        tmp.path(),
        "team",
        Some(RealmBackend::Sqlite),
        None,
        None,
    )
    .await
    .expect("open");
    let manifest = pin.as_builtin().expect("disk realms are builtin pins");
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
    let (pin, _bundle) = open_realm_persistence_with_provider(
        &DiskStorageProvider,
        tmp.path(),
        "ephemeral-team",
        Some(RealmBackend::Memory),
        None,
        None,
    )
    .await
    .expect("memory realms are a declared choice and must open");
    assert_eq!(
        pin.as_builtin().expect("builtin").backend,
        RealmBackend::Memory
    );
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

    async fn open(&self, _ctx: &RealmOpenContext) -> Result<RealmStoreSet, PersistenceError> {
        // Complete declarations for every slot — but blobs resolves
        // non-persistent WITHOUT a declaration: the exact silent-fallback
        // class the rule refuses.
        let mut durability: Vec<DurabilityDeclaration> =
            ["sessions", "runtime", "schedule", "workgraph", "artifacts"]
                .iter()
                .map(|domain| {
                    DurabilityDeclaration::durable(domain, DurabilityResolution::DeclaredEphemeral)
                })
                .collect();
        durability.push(DurabilityDeclaration::durable(
            "blobs",
            DurabilityResolution::NonPersistent,
        ));
        Ok(RealmStoreSet {
            session_store: Arc::new(meerkat_store::MemoryStore::new()),
            runtime_store: Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new()),
            schedule_store: Arc::new(meerkat_schedule::MemoryScheduleStore::new()),
            workgraph_store: Arc::new(meerkat_workgraph::MemoryWorkGraphStore::new()),
            blob_store: Arc::new(meerkat_store::MemoryBlobStore::new()),
            artifact_store: Arc::new(meerkat_store::MemoryArtifactStore::new()),
            store_path: std::path::PathBuf::from("."),
            projection_root: None,
            durability,
        })
    }
}

#[tokio::test]
async fn undeclared_nonpersistent_durable_slot_is_a_startup_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let result = open_realm_persistence_with_provider(
        &UndeclaredEphemeralProvider,
        tmp.path(),
        "team",
        None,
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
fn omitted_durability_declarations_are_refused() {
    // A provider cannot bypass the fail-closed rule by omitting domains
    // (or returning an empty list): completeness is enforced first.
    let manifest: meerkat_store::RealmManifest = serde_json::from_value(serde_json::json!({
        "realm_id": "team",
        "backend": "sqlite",
        "created_at": "0",
    }))
    .expect("manifest");
    let set = RealmStoreSet {
        session_store: Arc::new(meerkat_store::MemoryStore::new()),
        runtime_store: Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new()),
        schedule_store: Arc::new(meerkat_schedule::MemoryScheduleStore::new()),
        workgraph_store: Arc::new(meerkat_workgraph::MemoryWorkGraphStore::new()),
        blob_store: Arc::new(meerkat_store::MemoryBlobStore::new()),
        artifact_store: Arc::new(meerkat_store::MemoryArtifactStore::new()),
        store_path: std::path::PathBuf::from("."),
        projection_root: None,
        durability: Vec::new(),
    };
    let err = enforce_fail_closed_durability(&set, &manifest.ephemeral_domains)
        .expect_err("empty declaration list must refuse");
    assert!(
        matches!(err, PersistenceError::DurabilityViolation { .. }),
        "{err}"
    );
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
        durability: {
            let mut declarations = vec![DurabilityDeclaration::durable(
                "blobs",
                DurabilityResolution::NonPersistent,
            )];
            for domain in ["sessions", "runtime", "schedule", "workgraph", "artifacts"] {
                declarations.push(DurabilityDeclaration::durable(
                    domain,
                    DurabilityResolution::DeclaredEphemeral,
                ));
            }
            declarations
        },
    };
    enforce_fail_closed_durability(&set, &manifest.ephemeral_domains)
        .expect("declared domain admits the slot");

    let strict_manifest: meerkat_store::RealmManifest = serde_json::from_value(serde_json::json!({
        "realm_id": "team",
        "backend": "sqlite",
        "created_at": "0",
    }))
    .expect("manifest");
    let err = enforce_fail_closed_durability(&set, &strict_manifest.ephemeral_domains)
        .expect_err("undeclared refuses");
    assert!(matches!(
        err,
        PersistenceError::DurabilityViolation { domain } if domain == "blobs"
    ));
}

/// A minimal external provider: supplies memory-backed stores with complete
/// declared-ephemeral durability declarations.
struct FakeRemoteProvider;

#[async_trait]
impl RealmStorageProvider for FakeRemoteProvider {
    fn name(&self) -> &'static str {
        "fake-remote"
    }

    async fn open(&self, _ctx: &RealmOpenContext) -> Result<RealmStoreSet, PersistenceError> {
        Ok(RealmStoreSet {
            session_store: Arc::new(meerkat_store::MemoryStore::new()),
            runtime_store: Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new()),
            schedule_store: Arc::new(meerkat_schedule::MemoryScheduleStore::new()),
            workgraph_store: Arc::new(meerkat_workgraph::MemoryWorkGraphStore::new()),
            blob_store: Arc::new(meerkat_store::MemoryBlobStore::new()),
            artifact_store: Arc::new(meerkat_store::MemoryArtifactStore::new()),
            store_path: std::path::PathBuf::from("."),
            projection_root: None,
            durability: [
                "sessions",
                "runtime",
                "schedule",
                "workgraph",
                "blobs",
                "artifacts",
            ]
            .iter()
            .map(|domain| {
                DurabilityDeclaration::durable(domain, DurabilityResolution::DeclaredEphemeral)
            })
            .collect(),
        })
    }
}

#[tokio::test]
async fn external_provider_realms_pin_open_and_defend() {
    let tmp = tempfile::tempdir().expect("tempdir");

    // Fresh open through the external provider pins the manifest to it.
    let (pin, _bundle) = open_realm_persistence_with_provider(
        &FakeRemoteProvider,
        tmp.path(),
        "remote-team",
        None,
        None,
        None,
    )
    .await
    .expect("external provider must open (and pin) its own realm");
    assert_eq!(pin.provider_name(), Some("fake-remote"));

    // The persisted manifest carries the v2 pin + the old-reader defense.
    let manifest_path = tmp.path().join("remote-team").join("realm_manifest.json");
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).expect("read manifest"))
            .expect("manifest json");
    assert_eq!(raw["backend"], "external:fake-remote");
    assert_eq!(raw["manifest_format"], 2);
    assert_eq!(raw["provider"], "fake-remote");

    // Reopen through the SAME provider works (the finding this test pins:
    // provider-aware opens must be able to read external manifests).
    open_realm_persistence_with_provider(
        &FakeRemoteProvider,
        tmp.path(),
        "remote-team",
        None,
        None,
        None,
    )
    .await
    .expect("reopen through the pinned provider");

    // The disk composition refuses the realm typed.
    let disk_err =
        match meerkat::open_realm_persistence_in(tmp.path(), "remote-team", None, None).await {
            Err(err) => err,
            Ok(_) => panic!("disk must refuse an external-provider realm"),
        };
    assert!(
        matches!(
            &disk_err,
            PersistenceError::Store(meerkat_store::StoreError::ExternalProviderRealm { provider, .. })
                if provider == "fake-remote"
        ),
        "{disk_err}"
    );

    // A differently-named provider refuses typed too.
    struct OtherProvider;
    #[async_trait]
    impl RealmStorageProvider for OtherProvider {
        fn name(&self) -> &'static str {
            "other-remote"
        }
        async fn open(&self, ctx: &RealmOpenContext) -> Result<RealmStoreSet, PersistenceError> {
            FakeRemoteProvider.open(ctx).await
        }
    }
    let mismatch = match open_realm_persistence_with_provider(
        &OtherProvider,
        tmp.path(),
        "remote-team",
        None,
        None,
        None,
    )
    .await
    {
        Err(err) => err,
        Ok(_) => panic!("a different provider must not open the realm"),
    };
    assert!(
        matches!(
            &mismatch,
            PersistenceError::Store(meerkat_store::StoreError::RealmProviderMismatch { expected, found, .. })
                if expected == "other-remote" && found == "fake-remote"
        ),
        "{mismatch}"
    );
}
