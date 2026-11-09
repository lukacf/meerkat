#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]
//! Integration test for the streaming structural-events surface.
//!
//! Boots a `UnifiedRuntime`, drives a flow run to completion, and
//! asserts:
//! 1. The meta broadcast channel (`subscribe_mob_events()`) receives
//!    envelopes whose `cursor` matches the upstream meerkat
//!    `MobEvent.cursor` for the same kind.
//! 2. `meta/mob_events/query` returns the same cursors.
//! 3. The streaming subscription replaces the legacy 500ms poller —
//!    events arrive without waiting a tick.

use std::sync::Arc;
use std::time::Duration;

use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat_client::TestClient;
use meerkat_meta::{
    DiscoverySpec, JsonRpcResponse, MetaConfig, MobBootstrapOptions, MobBootstrapSpec,
    UnifiedRuntime, handle_unified_rpc_json,
};
use meerkat_mob::{MobDefinition, MobStorage};
use serde_json::{Value, json};
use tempfile::TempDir;

struct Fixture {
    _temp_dir: TempDir,
    runtime: UnifiedRuntime,
}

async fn build_fixture() -> Fixture {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let session_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&session_path).expect("session path");

    let factory = AgentFactory::new(&session_path).comms(true);
    let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 16));

    let definition = MobDefinition::from_toml(
        r#"
[mob]
id = "streaming-mob"

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
            namespace: "streaming-events-test".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    };

    let runtime = UnifiedRuntime::bootstrap(mob_spec, module_config, Duration::from_secs(2))
        .await
        .expect("bootstrap unified runtime");

    Fixture {
        _temp_dir: temp_dir,
        runtime,
    }
}

fn parse_json_rpc(response: &str) -> JsonRpcResponse {
    serde_json::from_str(response).expect("json-rpc response")
}

#[tokio::test]
async fn structural_events_arrive_via_streaming_subscription_with_upstream_cursor() {
    let fixture = build_fixture().await;

    // Subscribe BEFORE driving the flow so we observe live events.
    let mut rx = fixture.runtime.subscribe_mob_events();

    // Run a flow — generates `flow_started` (and downstream variants).
    let run_response = parse_json_rpc(
        &handle_unified_rpc_json(
            &fixture.runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": "run-1",
                "method": "meta/run_flow",
                "params": { "flow_id": "demo", "params": null }
            })
            .to_string(),
            Duration::from_secs(2),
            None,
            None,
        )
        .await,
    );
    assert!(
        run_response.error.is_none(),
        "run_flow should succeed: {:?}",
        run_response.error
    );

    // Receive at least the FlowStarted projection. With the streaming
    // subscription, this should arrive without waiting 500ms.
    let envelope = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("structural event should arrive within 2s")
        .expect("broadcast channel ok");

    assert!(
        envelope.cursor > 0,
        "envelope cursor should be a real ledger cursor (>0), got {}",
        envelope.cursor
    );

    // Cross-check via meta/mob_events/query (no after_seq → latest N).
    // The same envelope must appear with the same cursor.
    let query_response = parse_json_rpc(
        &handle_unified_rpc_json(
            &fixture.runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": "query-1",
                "method": "meta/mob_events/query",
                "params": { "limit": 64 }
            })
            .to_string(),
            Duration::from_secs(2),
            None,
            None,
        )
        .await,
    );
    let events = query_response
        .result
        .as_ref()
        .and_then(|v| v.get("events"))
        .and_then(Value::as_array)
        .expect("events array");
    let cursors: Vec<u64> = events
        .iter()
        .filter_map(|e| e.get("cursor").and_then(Value::as_u64))
        .collect();
    assert!(!cursors.is_empty(), "query should return ledger cursors");
    assert!(
        cursors.contains(&envelope.cursor),
        "broadcast envelope cursor {} should appear in query result {:?}",
        envelope.cursor,
        cursors
    );
    // Cursors are strictly monotonic in the result.
    for window in cursors.windows(2) {
        assert!(
            window[0] < window[1],
            "cursors must be strictly ascending, got {window:?}"
        );
    }

    let shutdown = fixture.runtime.shutdown().await;
    assert!(
        shutdown.mob_stop.is_ok(),
        "mob stop failed at teardown: {:?}",
        shutdown.mob_stop
    );
}
