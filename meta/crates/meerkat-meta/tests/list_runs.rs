#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]
//! Integration test for `meta/list_runs`.
//!
//! Boots a `UnifiedRuntime` against a mob with two flows, runs both,
//! then exercises the new RPC: list with no filter, list with
//! `flow_id` filter. Asserts the full meerkat ledger projection is
//! present on the wire (run_id, status, step_ledger, etc.).

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
    /// Per-fixture mob id: 0.8.23's fail-closed in-proc registration means
    /// concurrently running tests must not share a supervisor route.
    mob_id: String,
}

async fn build_runtime_two_flows() -> Fixture {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let session_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&session_path).expect("session path");

    let factory = AgentFactory::new(&session_path).comms(true);
    let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 16));

    static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mob_id = format!(
        "list-runs-mob-{}",
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

[flows.alpha]
description = "alpha flow"

[flows.alpha.steps.first]
role = "lead"
message = "alpha-1"

[flows.beta]
description = "beta flow"

[flows.beta.steps.first]
role = "lead"
message = "beta-1"
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
            namespace: "list-runs-test".to_string(),
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

async fn run_flow(runtime: &UnifiedRuntime, flow_id: &str) -> String {
    let response = parse_json_rpc(
        &handle_unified_rpc_json(
            runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": format!("run-{flow_id}"),
                "method": "meta/run_flow",
                "params": { "flow_id": flow_id, "params": null }
            })
            .to_string(),
            Duration::from_secs(2),
            None,
            None,
        )
        .await,
    );
    response
        .result
        .as_ref()
        .and_then(|v| v.get("run_id"))
        .and_then(Value::as_str)
        .map(String::from)
        .expect("run_id present")
}

#[tokio::test]
async fn list_runs_returns_full_ledger_projection_for_all_flows() {
    let fixture = build_runtime_two_flows().await;
    let alpha_run_id = run_flow(&fixture.runtime, "alpha").await;
    let beta_run_id = run_flow(&fixture.runtime, "beta").await;

    let response = parse_json_rpc(
        &handle_unified_rpc_json(
            &fixture.runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": "list-all",
                "method": "meta/list_runs",
                "params": {}
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
        "list_runs should not error: {:?}",
        response.error
    );
    let result = response.result.expect("list_runs result");
    let runs = result
        .get("runs")
        .and_then(Value::as_array)
        .expect("runs array");
    assert!(runs.len() >= 2, "expected >=2 runs, got {}", runs.len());

    // Build a run_id -> run_object map and assert each carries the
    // expected projection keys (the meerkat MobRun ledger projection
    // round-trips verbatim through the wire).
    let by_run_id: std::collections::BTreeMap<String, &Value> = runs
        .iter()
        .filter_map(|run| {
            run.get("run_id")
                .and_then(Value::as_str)
                .map(|id| (id.to_string(), run))
        })
        .collect();
    for run_id in [&alpha_run_id, &beta_run_id] {
        let run = by_run_id
            .get(run_id)
            .unwrap_or_else(|| panic!("run {run_id} should appear in list_runs"));
        for key in [
            "run_id",
            "mob_id",
            "flow_id",
            "status",
            "flow_state",
            "activation_params",
            "created_at",
            "step_ledger",
            "failure_ledger",
            "frames",
            "loops",
            "loop_iteration_ledger",
            "schema_version",
        ] {
            assert!(
                run.get(key).is_some(),
                "run {run_id} should carry `{key}`, got: {run}"
            );
        }
        assert_eq!(
            run.get("mob_id").and_then(Value::as_str),
            Some(fixture.mob_id.as_str()),
            "run {run_id} should carry mob_id"
        );
    }

    let shutdown = fixture.runtime.shutdown().await;
    assert!(
        shutdown.mob_stop.is_ok(),
        "mob stop failed at teardown: {:?}",
        shutdown.mob_stop
    );
}

#[tokio::test]
async fn list_runs_filters_by_flow_id() {
    let fixture = build_runtime_two_flows().await;
    let alpha_run_id = run_flow(&fixture.runtime, "alpha").await;
    let _beta_run_id = run_flow(&fixture.runtime, "beta").await;

    let response = parse_json_rpc(
        &handle_unified_rpc_json(
            &fixture.runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": "list-alpha",
                "method": "meta/list_runs",
                "params": { "flow_id": "alpha" }
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
        "list_runs filter should not error: {:?}",
        response.error
    );
    let runs = response
        .result
        .as_ref()
        .and_then(|v| v.get("runs"))
        .and_then(Value::as_array)
        .expect("runs array");
    assert!(
        !runs.is_empty(),
        "alpha-filtered result should be non-empty"
    );
    for run in runs {
        let flow_id = run.get("flow_id").and_then(Value::as_str);
        assert_eq!(
            flow_id,
            Some("alpha"),
            "flow_id filter should drop non-alpha runs, got: {run}"
        );
    }
    let alpha_run_in_results = runs
        .iter()
        .any(|run| run.get("run_id").and_then(Value::as_str) == Some(alpha_run_id.as_str()));
    assert!(alpha_run_in_results, "alpha run id should appear");

    let shutdown = fixture.runtime.shutdown().await;
    assert!(
        shutdown.mob_stop.is_ok(),
        "mob stop failed at teardown: {:?}",
        shutdown.mob_stop
    );
}
