#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::redundant_clone,
    clippy::needless_raw_string_hashes,
    clippy::ignored_unit_patterns,
    clippy::useless_vec
)]
//! TDD tests for the new UnifiedRuntimeBuilder convenience API.
//!
//! Each test is written before the corresponding implementation code exists.
//! They cover: definition loading, persistent/ephemeral paths, session hooks,
//! capability flags, defaults, and backward-compat escape hatch.

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use meerkat_client::TestClient;
use meerkat_core::service::{CreateSessionRequest, SessionError};
use meerkat_meta::{
    DiscoverySpec, MetaConfig, MobBootstrapOptions, MobBootstrapSpec, SessionHook, UnifiedRuntime,
};
use meerkat_mob::{MobDefinition, MobState, MobStorage};

/// Per-test mob id counter: 0.8.23's fail-closed in-proc registration
/// means concurrently running tests must not share a supervisor route.
static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Per-call mob id: 0.8.23's fail-closed in-proc registration means
/// concurrently running tests must not share a supervisor route.
fn minimal_mob_toml() -> String {
    format!(
        r#"
[mob]
id = "builder-test-mob-{}"

[profiles.worker]
model = "gpt-5.5"
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

// ---------------------------------------------------------------------------
// Helper: build a definition from the inline TOML
// ---------------------------------------------------------------------------
fn test_definition() -> MobDefinition {
    MobDefinition::from_toml(&minimal_mob_toml()).expect("parse test mob definition")
}

// ---------------------------------------------------------------------------
// 1. Builder ephemeral (no persistent_state → auto temp dir)
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_builder_ephemeral() {
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(test_definition())
            .default_llm_client(Arc::new(TestClient::default()))
            .build(),
    )
    .await
    .expect("ephemeral build");

    assert_eq!(
        runtime.mob_handle().status().await.unwrap(),
        MobState::Running
    );
    runtime.mob_handle().stop().await.expect("stop");
}

// ---------------------------------------------------------------------------
// 2. Builder persistent (SQLite session/runtime state + in-memory mob storage)
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_builder_persistent_default() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let state_path = tmp.path().join("state");

    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(test_definition())
            .persistent_state(&state_path)
            .default_llm_client(Arc::new(TestClient::default()))
            .build(),
    )
    .await
    .expect("persistent build");

    assert_eq!(
        runtime.mob_handle().status().await.unwrap(),
        MobState::Running
    );

    // Verify persistent artifacts were created (M2 canonical spellings on a
    // fresh state dir). Sessions live in Core's pair on runtime.sqlite: no
    // separate sessions database is created.
    assert!(
        state_path.join("runtime.sqlite").exists(),
        "Core's session and runtime store must be created"
    );
    assert!(
        !state_path.join("sessions.sqlite3").exists(),
        "no separate sessions database may be created"
    );
    assert!(
        state_path.join("meta_console.sqlite3").exists(),
        "durable Meta console log must be created"
    );

    runtime.mob_handle().stop().await.expect("stop");
}

// ---------------------------------------------------------------------------
// 3. Builder TOML definition from path
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_builder_toml_definition() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let toml_path = tmp.path().join("mob.toml");
    std::fs::write(&toml_path, minimal_mob_toml()).expect("write toml");

    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition_path(&toml_path)
            .default_llm_client(Arc::new(TestClient::default()))
            .build(),
    )
    .await
    .expect("toml definition build");

    assert_eq!(
        runtime.mob_handle().status().await.unwrap(),
        MobState::Running
    );
    runtime.mob_handle().stop().await.expect("stop");
}

// ---------------------------------------------------------------------------
// 4. Capability flags — shell(false) propagates
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_builder_capability_flags() {
    // This test verifies that the builder accepts capability flag methods
    // and that the runtime bootstraps successfully with modified flags.
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(test_definition())
            .default_llm_client(Arc::new(TestClient::default()))
            .shell(false)
            .build(),
    )
    .await
    .expect("build with shell disabled");

    assert_eq!(
        runtime.mob_handle().status().await.unwrap(),
        MobState::Running
    );
    runtime.mob_handle().stop().await.expect("stop");
}

// ---------------------------------------------------------------------------
// 5. Session hook — before_create mutates request
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_builder_session_hook_before_create() {
    struct LabelInjector;
    #[async_trait]
    impl SessionHook for LabelInjector {
        async fn before_create(&self, req: &mut CreateSessionRequest) -> Result<(), SessionError> {
            let labels = req.labels.get_or_insert_with(Default::default);
            labels.insert("injected".to_string(), "true".to_string());
            Ok(())
        }
    }

    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(test_definition())
            .default_llm_client(Arc::new(TestClient::default()))
            .session_hook(Arc::new(LabelInjector))
            .build(),
    )
    .await
    .expect("build with session hook");

    assert_eq!(
        runtime.mob_handle().status().await.unwrap(),
        MobState::Running
    );
    runtime.mob_handle().stop().await.expect("stop");
}

// ---------------------------------------------------------------------------
// 6. Backward-compat — .mob_spec() escape hatch still works
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_builder_mob_spec_escape_hatch() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let session_path = tmp.path().join("sessions");
    std::fs::create_dir_all(&session_path).expect("session path");

    let factory = meerkat::AgentFactory::new(&session_path).comms(true);
    let session_service = Arc::new(meerkat::build_ephemeral_service(
        factory,
        meerkat::Config::default(),
        16,
    ));

    let spec = MobBootstrapSpec::new(test_definition(), MobStorage::in_memory(), session_service)
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::default())),
        });

    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(spec)
            .module_config(MetaConfig {
                modules: Vec::new(),
                discovery: DiscoverySpec {
                    namespace: String::new(),
                    modules: Vec::new(),
                },
                pre_spawn: Vec::new(),
            })
            .timeout(std::time::Duration::from_secs(30))
            .build(),
    )
    .await
    .expect("escape hatch build");

    assert_eq!(
        runtime.mob_handle().status().await.unwrap(),
        MobState::Running
    );
    runtime.mob_handle().stop().await.expect("stop");
}

// ---------------------------------------------------------------------------
// 7. Builder defaults — module_config and timeout are defaulted
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore]
async fn test_builder_defaults() {
    // When using the new .definition() path, module_config and timeout
    // must be defaulted (no longer required fields).
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(test_definition())
            .default_llm_client(Arc::new(TestClient::default()))
            .build(),
    )
    .await
    .expect("build with defaults");

    assert_eq!(
        runtime.mob_handle().status().await.unwrap(),
        MobState::Running
    );
    runtime.mob_handle().stop().await.expect("stop");
}

// ---------------------------------------------------------------------------
// 8b. Per-slot WorkGraph store injection (0.8.16 item 5)
// ---------------------------------------------------------------------------
/// Item 5 is "a durable WorkGraph store injectable INDEPENDENTLY of continuity,
/// schedule, lease, console and blob". The seam
/// (`attach_workgraph_tools_with_store`) existed before this test and was
/// exercised directly by a unit test - but it was unreachable from
/// `UnifiedRuntimeBuilder`, so no caller could actually use it. A unit test
/// over the seam cannot fail on that; only a builder-level test can.
///
/// The assertion is deliberately NEGATIVE. "the field is populated" is a
/// measurement of the present, not a property of the mechanism: it stays true
/// if the value is later dropped on the floor. The absence of
/// `workgraph.sqlite3` is positive proof that the local-SQLite fallback did
/// NOT run, which is the thing injection has to accomplish.
#[tokio::test]
async fn builder_workgraph_store_injection_suppresses_the_local_sqlite_fallback() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let state_path = tmp.path().join("state");
    std::fs::create_dir_all(&state_path).expect("create state dir");

    let injected: Arc<dyn meerkat::WorkGraphStore> = Arc::new(meerkat::MemoryWorkGraphStore::new());

    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(test_definition())
            .persistent_state(&state_path)
            .workgraph_store(Arc::clone(&injected))
            .default_llm_client(Arc::new(TestClient::default()))
            .build(),
    )
    .await
    .expect("persistent build with an injected workgraph store");

    // Positive control for the assertion below: the builder demonstrably ran
    // its persistent composition in this directory, so an absent
    // workgraph.sqlite3 means "suppressed", not "nothing happened here".
    assert!(
        state_path.join("runtime.sqlite").exists(),
        "persistent composition must have run in the state dir"
    );
    assert!(
        !state_path.join("workgraph.sqlite3").exists(),
        "an injected workgraph store must suppress the local SQLite fallback; \
         the file's presence means the injected store was dropped and \
         attach_workgraph_tools_reporting ran instead"
    );

    runtime.mob_handle().stop().await.expect("stop");
}

#[tokio::test]
async fn test_builder_custom_blob_store_serves_binary_blobs() {
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(test_definition())
            .blob_store(blob_store.clone())
            .default_llm_client(Arc::new(TestClient::default()))
            .build(),
    )
    .await
    .expect("ephemeral build with custom blob store");

    let binary_store = runtime
        .binary_blob_store()
        .expect("builder-created runtime must expose binary blob serving store");
    let blob_ref = binary_store
        .put_bytes("image/png", bytes::Bytes::from_static(b"tiny-png"))
        .await
        .expect("binary put");
    let served = binary_store
        .get_bytes(&blob_ref.blob_id)
        .await
        .expect("binary get");
    assert_eq!(served.data.as_ref(), b"tiny-png");

    let stored = blob_store.get(&blob_ref.blob_id).await.expect("blob get");
    assert_eq!(stored.media_type, "image/png");
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(stored.data.as_bytes())
            .expect("stored blob base64")
            .as_slice(),
        b"tiny-png"
    );

    runtime.mob_handle().stop().await.expect("stop");
}
