//! `meta/stop_member_run` over the unified gateway RPC: meerkat's
//! run-fenced Stop of one exact member run. A stale run id is the typed
//! `not_current` receipt (never an interrupt), malformed params are -32602,
//! and an unknown member is a typed failure.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]

use std::sync::Arc;
use std::time::Duration;

use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat_client::TestClient;
use meerkat_meta::{
    DiscoverySpec, MetaConfig, MobBootstrapOptions, MobBootstrapSpec, UnifiedRuntime,
    handle_unified_rpc_json,
};
use meerkat_mob::{MobDefinition, MobStorage};
use serde_json::{Value, json};

static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn mob_toml() -> String {
    format!(
        r#"
[mob]
id = "member-run-stop-{}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

async fn runtime() -> (tempfile::TempDir, UnifiedRuntime) {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let factory = AgentFactory::new(temp_dir.path()).comms(true);
    let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 8));
    let definition = MobDefinition::from_toml(&mob_toml()).expect("parse definition");
    let mob_spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::default())),
        });
    let module_config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: "member-run-stop".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    };
    let runtime = UnifiedRuntime::bootstrap(mob_spec, module_config, Duration::from_secs(2))
        .await
        .expect("bootstrap runtime");
    (temp_dir, runtime)
}

async fn rpc(runtime: &UnifiedRuntime, method: &str, params: Value) -> Value {
    let request = json!({
        "jsonrpc": "2.0",
        "id": "member-run-stop",
        "method": method,
        "params": params,
    })
    .to_string();
    let response =
        handle_unified_rpc_json(runtime, &request, Duration::from_secs(10), None, None).await;
    serde_json::from_str(&response).expect("json-rpc response")
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_member_run_is_not_current_for_a_stale_run_and_validates_its_params() {
    let (_temp, runtime) = runtime().await;
    let ensured = rpc(
        &runtime,
        "meta/ensure_member",
        json!({ "role": "worker", "agent_identity": "w1" }),
    )
    .await;
    assert!(ensured["error"].is_null(), "{ensured:#?}");

    let stale = "01936f8b-0000-7000-8000-000000000042".to_string();
    let response = rpc(
        &runtime,
        "meta/stop_member_run",
        json!({ "member_id": "w1", "run_id": stale, "reason": "stale selection" }),
    )
    .await;
    assert!(response["error"].is_null(), "{response:#?}");
    assert_eq!(response["result"]["member_id"], json!("w1"));
    assert_eq!(
        response["result"]["receipt"]["outcome"],
        json!("not_current")
    );
    assert_eq!(response["result"]["receipt"]["run_id"], json!(stale));

    for params in [
        json!({ "member_id": "w1", "reason": "x" }),
        json!({ "member_id": "w1", "run_id": "not-a-uuid", "reason": "x" }),
        json!({ "member_id": "w1", "run_id": stale }),
        json!({ "run_id": stale, "reason": "x" }),
    ] {
        let response = rpc(&runtime, "meta/stop_member_run", params.clone()).await;
        assert_eq!(
            response["error"]["code"],
            json!(-32602),
            "{params}: {response:#?}"
        );
    }

    let response = rpc(
        &runtime,
        "meta/stop_member_run",
        json!({ "member_id": "missing", "run_id": stale, "reason": "x" }),
    )
    .await;
    assert!(response["error"].is_object(), "{response:#?}");
}

/// `meta/force_cancel_member` answers a mob refusal with meerkat's typed code
/// and message, the rendering `meta/stop_member_run` uses, instead of a
/// `-32000` with the refusal flattened into text: an unknown member is
/// meerkat's `MemberNotFound`. The reload-required rendering (-32029 with its
/// data) is pinned by the unit test
/// `a_member_that_needs_a_reload_keeps_its_code_and_named_repair`.
#[tokio::test(flavor = "multi_thread")]
async fn force_cancel_member_answers_a_mob_refusal_typed() {
    let (_temp, runtime) = runtime().await;
    let response = rpc(
        &runtime,
        "meta/force_cancel_member",
        json!({ "member_id": "missing" }),
    )
    .await;
    assert_eq!(response["error"]["code"], json!(-32602), "{response:#?}");
    assert_eq!(
        response["error"]["message"],
        json!("mob member not found: missing"),
        "{response:#?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_member_run_is_advertised_by_the_gateway() {
    let (_temp, runtime) = runtime().await;
    let response = rpc(&runtime, "meta/capabilities", json!({})).await;
    let text = response.to_string();
    assert!(
        text.contains("meta/stop_member_run"),
        "stop_member_run must be advertised: {response:#?}"
    );
}
