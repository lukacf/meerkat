#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]
//! Cursor-persistence contract for the streaming structural-events
//! subscription.
//!
//! When a `UnifiedRuntime` is constructed with a `SqliteMetadataStore`,
//! every projected envelope must checkpoint its cursor to the
//! `meta_metadata` table so a fresh runtime instance built against
//! the same SQLite path can resume via
//! `MobEventsView::subscribe_after`.
//!
//! This test wires the SQLite-backed adapter through the
//! `UnifiedRuntimeBuilder`, drives a flow run, waits for the
//! subscription task to project the events, and asserts that the
//! cursor stored in SQLite matches the latest cursor visible on the
//! in-process broadcast channel.

use std::sync::Arc;
use std::time::Duration;

use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat_client::TestClient;
use meerkat_meta::{
    DiscoverySpec, MetaConfig, MobBootstrapOptions, MobBootstrapSpec, PersistentMetadataStore,
    SqliteMetadataStore, UnifiedRuntime, handle_unified_rpc_json,
};
use meerkat_mob::{MobDefinition, MobStorage};
use serde_json::json;

#[tokio::test]
async fn projected_cursor_is_persisted_to_sqlite_metadata_store() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let session_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&session_path).expect("session path");

    let factory = AgentFactory::new(&session_path).comms(true);
    let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 16));

    let definition = MobDefinition::from_toml(
        r#"
[mob]
id = "cursor-persist-mob"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true

[profiles.lead.tools]
comms = true

[flows.demo]
description = "demo"

[flows.demo.steps.first]
role = "lead"
message = "first"
"#,
    )
    .expect("parse mob definition");

    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::default())),
        });
    let module_config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: "cursor-persist-test".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    };

    // SQLite-backed metadata adapter sitting next to the (in-memory)
    // mob storage. The cursor write path is the same regardless of the
    // mob storage backend.
    let metadata_path = temp_dir.path().join("meta_metadata.sqlite");
    let metadata_store =
        Arc::new(SqliteMetadataStore::open(&metadata_path).expect("open SqliteMetadataStore"));

    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(mob_spec)
            .module_config(module_config)
            .timeout(Duration::from_secs(2))
            .persistent_metadata(metadata_store.clone())
            .build(),
    )
    .await
    .expect("build runtime with persistent metadata");

    let mut rx = runtime.subscribe_mob_events();

    // Drive a flow so the streaming subscription has events to project.
    let _ = handle_unified_rpc_json(
        &runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "drive",
            "method": "meta/run_flow",
            "params": { "flow_id": "demo", "params": null }
        })
        .to_string(),
        Duration::from_secs(2),
        None,
        None,
    )
    .await;

    // Pull the first projected envelope so we know the subscription
    // task has at least begun writing cursors to the persistent store.
    let envelope = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("event should arrive within 2s")
        .expect("broadcast ok");

    // Yield a few times so the subscription's set_subscription_cursor
    // call gets a chance to land. Polling for parity instead of sleeping
    // ties the assertion to the actual write completing.
    let mut persisted = None;
    for _ in 0..20 {
        if let Ok(Some(c)) = metadata_store
            .get_subscription_cursor("cursor-persist-mob")
            .await
            && c >= envelope.cursor
        {
            persisted = Some(c);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let persisted = persisted.unwrap_or_else(|| {
        panic!(
            "cursor for mob 'cursor-persist-mob' should be persisted >= {} within 200ms",
            envelope.cursor
        )
    });
    assert!(
        persisted >= envelope.cursor,
        "persisted cursor ({}) should be >= last broadcast cursor ({})",
        persisted,
        envelope.cursor
    );

    let shutdown = runtime.shutdown().await;
    assert!(
        shutdown.mob_stop.is_ok(),
        "mob stop failed at teardown: {:?}",
        shutdown.mob_stop
    );
}
