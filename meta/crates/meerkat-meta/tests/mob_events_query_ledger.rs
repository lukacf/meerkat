#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]
//! Ledger-backed coverage for `meta/mob_events/query`.
//!
//! Exercises the new batch-scan helper end-to-end against a real
//! `UnifiedRuntime`:
//! - `after_seq` past `latest_cursor` returns JSON-RPC `-32010` with
//!   `error.data.{after_cursor,latest_cursor}`.
//! - With no `after_seq` the call returns the **latest N** matching
//!   events (default `limit = 256`); cursors come back in ascending
//!   order.
//! - With `after_seq = 0` the call returns events in ascending cursor
//!   order from the start of the ledger.

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

const STALE_CURSOR_CODE: i64 = -32010;

struct Fixture {
    _temp_dir: TempDir,
    runtime: UnifiedRuntime,
    /// Per-fixture mob id: 0.8.23's fail-closed in-proc registration means
    /// concurrently running tests must not share a supervisor route.
    mob_id: String,
}

async fn build_fixture() -> Fixture {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let session_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&session_path).expect("session path");

    let factory = AgentFactory::new(&session_path).comms(true);
    let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 16));

    static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mob_id = format!(
        "query-ledger-mob-{}",
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

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
    ))
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
            namespace: "query-ledger-test".to_string(),
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
        mob_id,
    }
}

fn parse_json_rpc(response: &str) -> JsonRpcResponse {
    serde_json::from_str(response).expect("json-rpc response")
}

async fn drive_a_flow(runtime: &UnifiedRuntime) {
    let response = parse_json_rpc(
        &handle_unified_rpc_json(
            runtime,
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
        .await,
    );
    assert!(
        response.error.is_none(),
        "run_flow should succeed: {:?}",
        response.error
    );
}

async fn query(runtime: &UnifiedRuntime, params: Value) -> JsonRpcResponse {
    parse_json_rpc(
        &handle_unified_rpc_json(
            runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": "query",
                "method": "meta/mob_events/query",
                "params": params
            })
            .to_string(),
            Duration::from_secs(2),
            None,
            None,
        )
        .await,
    )
}

#[tokio::test]
async fn query_with_future_cursor_returns_typed_stale_error() {
    let fixture = build_fixture().await;
    drive_a_flow(&fixture.runtime).await;

    // Wait briefly for the streaming subscription task to project at
    // least one event, so latest_cursor advances past zero.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let response = query(
        &fixture.runtime,
        json!({ "after_seq": u64::MAX, "limit": 1 }),
    )
    .await;

    let error = response
        .error
        .as_ref()
        .expect("future cursor must produce an error");
    assert_eq!(
        error.code, STALE_CURSOR_CODE,
        "expected -32010 (event_query_stale), got {}",
        error.code
    );
    let data = error.data.as_ref().expect("error.data must carry cursors");
    assert!(
        data.get("after_cursor").is_some(),
        "error.data must carry after_cursor"
    );
    assert!(
        data.get("latest_cursor").is_some(),
        "error.data must carry latest_cursor"
    );
    let latest = data
        .get("latest_cursor")
        .and_then(Value::as_u64)
        .expect("latest_cursor u64");
    let after = data
        .get("after_cursor")
        .and_then(Value::as_u64)
        .expect("after_cursor u64");
    assert!(
        after > latest,
        "after_cursor ({}) must be > latest_cursor ({})",
        after,
        latest
    );

    let shutdown = fixture.runtime.shutdown().await;
    assert!(
        shutdown.mob_stop.is_ok(),
        "mob stop failed at teardown: {:?}",
        shutdown.mob_stop
    );
}

#[tokio::test]
async fn query_without_after_seq_returns_latest_events_in_ascending_order() {
    let fixture = build_fixture().await;
    drive_a_flow(&fixture.runtime).await;

    // Allow time for event projection.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let response = query(&fixture.runtime, json!({ "limit": 32 })).await;
    assert!(
        response.error.is_none(),
        "default query must not error: {:?}",
        response.error
    );
    let result = response.result.expect("query result");
    let events = result
        .get("events")
        .and_then(Value::as_array)
        .expect("events array");
    assert!(
        !events.is_empty(),
        "default query should return ledger events, got empty"
    );
    let cursors: Vec<u64> = events
        .iter()
        .filter_map(|e| e.get("cursor").and_then(Value::as_u64))
        .collect();
    assert_eq!(
        cursors.len(),
        events.len(),
        "every envelope must carry a u64 cursor"
    );
    for window in cursors.windows(2) {
        assert!(
            window[0] < window[1],
            "cursors must be strictly ascending, got {window:?}"
        );
    }
    let next_after_seq = result
        .get("next_after_seq")
        .and_then(Value::as_u64)
        .expect("next_after_seq must be set when events are returned");
    assert_eq!(
        next_after_seq,
        *cursors.last().expect("non-empty"),
        "next_after_seq should equal the last returned cursor"
    );

    let shutdown = fixture.runtime.shutdown().await;
    assert!(
        shutdown.mob_stop.is_ok(),
        "mob stop failed at teardown: {:?}",
        shutdown.mob_stop
    );
}

#[tokio::test]
async fn subscribe_url_carries_continuation_cursor_and_filters() {
    // meta/mob_events/subscribe must return a subscribe_url that
    // includes after_seq + the original filters so the SSE handler
    // resumes from the snapshot frontier — closing the gap between
    // the JSON-RPC response and the SSE handshake.
    let fixture = build_fixture().await;
    drive_a_flow(&fixture.runtime).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let response = parse_json_rpc(
        &handle_unified_rpc_json(
            &fixture.runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": "subscribe-1",
                "method": "meta/mob_events/subscribe",
                "params": {
                    "mob_id": fixture.mob_id,
                    "event_types": ["flow_started", "flow_completed"],
                    "limit": 8
                }
            })
            .to_string(),
            Duration::from_secs(2),
            None,
            None,
        )
        .await,
    );
    assert!(
        response.error.is_none(),
        "subscribe must succeed: {:?}",
        response.error
    );
    let result = response.result.expect("subscribe result");
    let url = result
        .get("subscribe_url")
        .and_then(Value::as_str)
        .expect("subscribe_url string")
        .to_string();
    assert!(
        url.starts_with("/meta/mob_events/stream?"),
        "subscribe_url should anchor to the SSE route, got {url}"
    );
    assert!(
        url.contains("after_seq="),
        "subscribe_url must carry after_seq, got {url}"
    );
    assert!(
        url.contains(&format!("mob_id={}", fixture.mob_id)),
        "subscribe_url must carry mob_id filter, got {url}"
    );
    assert!(
        url.contains("event_types=flow_started%2Cflow_completed"),
        "subscribe_url must carry event_types filter (URL-encoded), got {url}"
    );

    let shutdown = fixture.runtime.shutdown().await;
    assert!(
        shutdown.mob_stop.is_ok(),
        "mob stop failed at teardown: {:?}",
        shutdown.mob_stop
    );
}

#[tokio::test]
async fn query_with_empty_filter_match_still_advances_next_after_seq() {
    // Regression: pre-fix, `next_after_seq` was `null` when the filter
    // matched no events. A polling SDK with no anchor would either
    // skip new matches (restart from latest) or replay from zero. The
    // fix returns either the caller's `after_seq`, or the latest_cursor
    // captured at handshake — never null.
    let fixture = build_fixture().await;
    drive_a_flow(&fixture.runtime).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Filter that matches nothing.
    let response = query(
        &fixture.runtime,
        json!({ "mob_id": "no-such-mob-anywhere", "limit": 32 }),
    )
    .await;
    assert!(
        response.error.is_none(),
        "filter-with-no-match must succeed: {:?}",
        response.error
    );
    let result = response.result.expect("result");
    let events = result
        .get("events")
        .and_then(Value::as_array)
        .expect("events");
    assert!(events.is_empty(), "filter must match no events");
    let next = result.get("next_after_seq").and_then(Value::as_u64);
    assert!(
        next.is_some(),
        "next_after_seq must be numeric when no events match (was null pre-fix)"
    );
    assert!(
        next.unwrap() > 0,
        "next_after_seq must advance past 0 once events exist on the ledger"
    );

    let shutdown = fixture.runtime.shutdown().await;
    assert!(
        shutdown.mob_stop.is_ok(),
        "mob stop failed at teardown: {:?}",
        shutdown.mob_stop
    );
}

#[tokio::test]
async fn query_after_seq_zero_paginates_forward_from_start() {
    let fixture = build_fixture().await;
    drive_a_flow(&fixture.runtime).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // First page: from the start, take a small batch.
    let first = query(&fixture.runtime, json!({ "after_seq": 0, "limit": 4 })).await;
    assert!(first.error.is_none(), "first page must succeed");
    let first_events = first
        .result
        .as_ref()
        .and_then(|v| v.get("events"))
        .and_then(Value::as_array)
        .expect("events array");
    if first_events.is_empty() {
        // No events landed yet for this very fast machine; bail early.
        let shutdown = fixture.runtime.shutdown().await;
        assert!(
            shutdown.mob_stop.is_ok(),
            "mob stop failed at teardown: {:?}",
            shutdown.mob_stop
        );
        return;
    }
    let first_cursors: Vec<u64> = first_events
        .iter()
        .filter_map(|e| e.get("cursor").and_then(Value::as_u64))
        .collect();
    for window in first_cursors.windows(2) {
        assert!(
            window[0] < window[1],
            "first page cursors must ascend, got {window:?}"
        );
    }

    let next_cursor = first
        .result
        .as_ref()
        .and_then(|v| v.get("next_after_seq"))
        .and_then(Value::as_u64)
        .expect("next_after_seq present on non-empty page");

    // Second page: strictly newer than the last cursor.
    let second = query(
        &fixture.runtime,
        json!({ "after_seq": next_cursor, "limit": 64 }),
    )
    .await;
    assert!(second.error.is_none(), "second page must succeed");
    let second_events = second
        .result
        .as_ref()
        .and_then(|v| v.get("events"))
        .and_then(Value::as_array)
        .expect("events array");
    for event in second_events {
        let cursor = event
            .get("cursor")
            .and_then(Value::as_u64)
            .expect("cursor u64");
        assert!(
            cursor > next_cursor,
            "second page cursor {} must be > {}",
            cursor,
            next_cursor
        );
    }

    let shutdown = fixture.runtime.shutdown().await;
    assert!(
        shutdown.mob_stop.is_ok(),
        "mob stop failed at teardown: {:?}",
        shutdown.mob_stop
    );
}
