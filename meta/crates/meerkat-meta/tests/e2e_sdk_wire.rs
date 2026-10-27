//! End-to-end wire-format tests: SDK request shapes → RPC handler → response shapes.
//!
//! These use the in-process `handle_meta_rpc_json` path so they complete in
//! milliseconds with no subprocesses. Each test sends the exact JSON the SDK
//! would produce and asserts the response matches what the SDK parser expects.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::redundant_clone
)]

use std::time::Duration;

use meerkat_meta::{
    DiscoverySpec, MetaConfig, ModuleConfig, PreSpawnData, RestartPolicy, handle_meta_rpc_json,
    start_meta_runtime,
};
use serde_json::{Value, json};

/// Per-test mob id counter: 0.8.23's fail-closed in-proc registration
/// means concurrently running tests must not share a supervisor route.
static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

const TIMEOUT: Duration = Duration::from_secs(2);

fn noop_module(id: &str) -> ModuleConfig {
    ModuleConfig {
        id: id.to_string(),
        command: "sh".to_string(),
        args: vec![
            "-c".to_string(),
            format!(
                r#"printf '%s\n' '{{"event_id":"evt-{id}","source":"module","timestamp_ms":1,"event":{{"kind":"module","module":"{id}","event_type":"ready","payload":{{"ok":true}}}}}}'"#,
            ),
        ],
        restart_policy: RestartPolicy::Never,
    }
}

fn test_runtime() -> meerkat_meta::MetaRuntimeHandle {
    let config = MetaConfig {
        modules: vec![noop_module("alpha"), noop_module("beta")],
        discovery: DiscoverySpec {
            namespace: "e2e-wire".to_string(),
            modules: vec!["alpha".to_string()],
        },
        pre_spawn: vec![
            PreSpawnData {
                module_id: "alpha".to_string(),
                env: vec![],
            },
            PreSpawnData {
                module_id: "beta".to_string(),
                env: vec![],
            },
        ],
    };
    start_meta_runtime(config, vec![], TIMEOUT).expect("runtime starts")
}

fn rpc(runtime: &mut meerkat_meta::MetaRuntimeHandle, request: &Value) -> Value {
    let response_str = handle_meta_rpc_json(runtime, &request.to_string(), TIMEOUT);
    serde_json::from_str(&response_str).expect("valid JSON response")
}

// ---------------------------------------------------------------------------
// meta/status — SDK expects: contract_version, running, loaded_modules
// ---------------------------------------------------------------------------

#[test]
fn e2e_status_response_matches_sdk_contract() {
    let mut rt = test_runtime();
    let resp = rpc(
        &mut rt,
        &json!({
            "jsonrpc": "2.0", "id": "s1", "method": "meta/status", "params": {}
        }),
    );
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], "s1");
    let result = &resp["result"];
    assert!(result["contract_version"].is_string());
    assert!(result["running"].is_boolean());
    assert!(result["loaded_modules"].is_array());
}

// ---------------------------------------------------------------------------
// meta/capabilities — SDK expects: contract_version, methods[], loaded_modules[]
// ---------------------------------------------------------------------------

#[test]
fn e2e_capabilities_response_matches_sdk_contract() {
    let mut rt = test_runtime();
    let resp = rpc(
        &mut rt,
        &json!({
            "jsonrpc": "2.0", "id": "c1", "method": "meta/capabilities", "params": {}
        }),
    );
    let result = &resp["result"];
    assert!(result["contract_version"].is_string());
    assert!(result["methods"].is_array());
    assert!(result["loaded_modules"].is_array());
}

// ---------------------------------------------------------------------------
// meta/reconcile — strict param validation
// ---------------------------------------------------------------------------

#[test]
fn e2e_reconcile_requires_string_array() {
    let mut rt = test_runtime();

    // Valid call
    let resp = rpc(
        &mut rt,
        &json!({
            "jsonrpc": "2.0", "id": "r1", "method": "meta/reconcile",
            "params": { "modules": ["alpha"] }
        }),
    );
    assert!(resp["result"]["accepted"].as_bool().unwrap());
    assert_eq!(resp["result"]["reconciled_modules"], json!(["alpha"]));

    // Missing modules field → -32602
    let resp = rpc(
        &mut rt,
        &json!({
            "jsonrpc": "2.0", "id": "r2", "method": "meta/reconcile",
            "params": {}
        }),
    );
    assert_eq!(resp["error"]["code"], -32602);

    // Non-string entry in modules → -32602
    let resp = rpc(
        &mut rt,
        &json!({
            "jsonrpc": "2.0", "id": "r3", "method": "meta/reconcile",
            "params": { "modules": ["alpha", 42] }
        }),
    );
    assert_eq!(resp["error"]["code"], -32602);
    assert!(
        resp["error"]["message"]
            .as_str()
            .unwrap()
            .contains("modules[1]")
    );

    // modules is not an array → -32602
    let resp = rpc(
        &mut rt,
        &json!({
            "jsonrpc": "2.0", "id": "r4", "method": "meta/reconcile",
            "params": { "modules": "alpha" }
        }),
    );
    assert_eq!(resp["error"]["code"], -32602);
}

// ---------------------------------------------------------------------------
// Note: meta/send_message and meta/ensure_member
// are unified-runtime-only methods (not available via handle_meta_rpc_json).
// They are tested in phase4 and the unified handler tests.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// meta/spawn_member — both module_id and profile paths
// ---------------------------------------------------------------------------

#[test]
fn e2e_spawn_member_validates_params() {
    let mut rt = test_runtime();

    // Empty module_id → -32602
    let resp = rpc(
        &mut rt,
        &json!({
            "jsonrpc": "2.0", "id": "sp1", "method": "meta/spawn_member",
            "params": { "module_id": "" }
        }),
    );
    assert_eq!(resp["error"]["code"], -32602);

    // No params at all → -32602
    let resp = rpc(
        &mut rt,
        &json!({
            "jsonrpc": "2.0", "id": "sp2", "method": "meta/spawn_member",
            "params": {}
        }),
    );
    assert_eq!(resp["error"]["code"], -32602);
}

// ---------------------------------------------------------------------------
// JSON-RPC envelope — id correlation
// ---------------------------------------------------------------------------

#[test]
fn e2e_response_id_correlates_with_request() {
    let mut rt = test_runtime();

    let resp = rpc(
        &mut rt,
        &json!({
            "jsonrpc": "2.0", "id": "unique-42", "method": "meta/status", "params": {}
        }),
    );
    assert_eq!(resp["id"], "unique-42");
    assert_eq!(resp["jsonrpc"], "2.0");
}

#[test]
fn e2e_unknown_method_returns_minus_32601() {
    let mut rt = test_runtime();
    let resp = rpc(
        &mut rt,
        &json!({
            "jsonrpc": "2.0", "id": "u1", "method": "meta/nonexistent", "params": {}
        }),
    );
    assert_eq!(resp["error"]["code"], -32601);
}

// Note: meta/ensure_member is unified-runtime-only (tested in mob_methods).

// ---------------------------------------------------------------------------
// meta/workgraph/* — unified-runtime method group (not served by
// handle_meta_rpc_json). Requests below are the exact JSON the SDKs
// produce — param shapes mirror the pins in
// sdk/python/tests/test_rpc_method_names.py and
// sdk/typescript/tests/runtime.test.ts byte-for-byte — and responses are
// asserted in the shapes the SDK parsers consume.
// ---------------------------------------------------------------------------

/// Per-call mob id: 0.8.23's fail-closed in-proc registration means
/// concurrently running tests must not share a supervisor route.
fn workgraph_wire_mob_toml() -> String {
    format!(
        r#"
[mob]
id = "e2e-wire-workgraph-{}"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "autonomous_host"
external_addressable = true
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// Builder-constructed runtime: the builder path wires a memory-backed
/// WorkGraph service automatically.
async fn workgraph_runtime() -> meerkat_meta::UnifiedRuntime {
    Box::pin(
        meerkat_meta::UnifiedRuntime::builder()
            .definition(
                meerkat_mob::MobDefinition::from_toml(&workgraph_wire_mob_toml())
                    .expect("parse workgraph wire definition"),
            )
            .default_llm_client(std::sync::Arc::new(meerkat_client::TestClient::default()))
            .build(),
    )
    .await
    .expect("workgraph wire runtime builds")
}

/// Counter-fixture on the manual `MobBootstrapSpec::new` path, which wires
/// no WorkGraph service — the -32041 shape source.
async fn runtime_without_workgraph() -> (tempfile::TempDir, meerkat_meta::UnifiedRuntime) {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let factory = meerkat::AgentFactory::new(temp_dir.path()).comms(true);
    let session_service = std::sync::Arc::new(meerkat::build_ephemeral_service(
        factory,
        meerkat::Config::default(),
        8,
    ));
    let mob_spec = meerkat_meta::MobBootstrapSpec::new(
        meerkat_mob::MobDefinition::from_toml(&workgraph_wire_mob_toml())
            .expect("parse workgraph wire definition"),
        meerkat_mob::MobStorage::in_memory(),
        session_service,
    )
    .with_options(meerkat_meta::MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(std::sync::Arc::new(meerkat_client::TestClient::default())),
    });
    let module_config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: "e2e-wire-workgraph".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    };
    let runtime =
        meerkat_meta::UnifiedRuntime::bootstrap(mob_spec, module_config, Duration::from_secs(2))
            .await
            .expect("bootstrap runtime without workgraph");
    (temp_dir, runtime)
}

async fn unified_rpc(
    runtime: &meerkat_meta::UnifiedRuntime,
    id: &str,
    method: &str,
    params: Value,
) -> Value {
    let request = json!({
        "jsonrpc": "2.0", "id": id, "method": method, "params": params,
    })
    .to_string();
    let response = meerkat_meta::handle_unified_rpc_json(
        runtime,
        &request,
        Duration::from_secs(5),
        None,
        None,
    )
    .await;
    serde_json::from_str(&response).expect("valid JSON response")
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_workgraph_create_snapshot_list_get_roundtrip() {
    let rt = workgraph_runtime().await;

    // create — Python SDK pin: {"title": "Ship it", "priority": "high"}
    let resp = unified_rpc(
        &rt,
        "wg-create",
        "meta/workgraph/create",
        json!({ "title": "Ship it", "priority": "high" }),
    )
    .await;
    assert!(resp["error"].is_null(), "{resp:#?}");
    let item = &resp["result"]["item"];
    let item_id = item["id"].as_str().expect("item id is a string");
    assert_eq!(item["title"], json!("Ship it"));
    assert_eq!(item["priority"], json!("high"));
    assert_eq!(item["status"], json!("open"));
    assert!(item["revision"].as_u64().is_some(), "{item:#?}");

    // snapshot — Python SDK pin: {"namespace": "default"}
    let resp = unified_rpc(
        &rt,
        "wg-snap",
        "meta/workgraph/snapshot",
        json!({ "namespace": "default" }),
    )
    .await;
    let snapshot = &resp["result"];
    assert_eq!(snapshot["items"].as_array().expect("items array").len(), 1);
    assert!(snapshot["edges"].is_array(), "{snapshot:#?}");
    assert!(snapshot["attention"].is_array(), "{snapshot:#?}");
    assert!(snapshot["realm_id"].is_string(), "{snapshot:#?}");

    // list — Python SDK pin: {"statuses": ["open"]}
    let resp = unified_rpc(
        &rt,
        "wg-list",
        "meta/workgraph/list",
        json!({ "statuses": ["open"] }),
    )
    .await;
    assert_eq!(
        resp["result"]["items"].as_array().expect("items").len(),
        1,
        "{resp:#?}"
    );

    // get — Python SDK pin: {"id": <item id>}
    let resp = unified_rpc(
        &rt,
        "wg-get",
        "meta/workgraph/get",
        json!({ "id": item_id }),
    )
    .await;
    assert_eq!(resp["result"]["item"]["id"], json!(item_id));
    rt.mob_handle().stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_workgraph_claim_accepts_flat_owner_wire_form() {
    let rt = workgraph_runtime().await;
    let created = unified_rpc(
        &rt,
        "wg-c1",
        "meta/workgraph/create",
        json!({ "title": "Claim me" }),
    )
    .await;
    let item = &created["result"]["item"];

    // TypeScript SDK pin: flat owner + lease_seconds.
    let resp = unified_rpc(
        &rt,
        "wg-claim",
        "meta/workgraph/claim",
        json!({
            "id": item["id"],
            "expected_revision": item["revision"],
            "owner": { "kind": "agent", "id": "agent-1", "display_name": "Agent One" },
            "lease_seconds": 60,
        }),
    )
    .await;
    assert!(resp["error"].is_null(), "{resp:#?}");
    let claimed = &resp["result"]["item"];
    assert_eq!(claimed["status"], json!("in_progress"));
    assert_eq!(claimed["claim"]["owner"]["key"]["kind"], json!("agent"));
    assert_eq!(claimed["claim"]["owner"]["key"]["id"], json!("agent-1"));
    assert_eq!(
        claimed["claim"]["owner"]["display_name"],
        json!("Agent One")
    );
    rt.mob_handle().stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_workgraph_goal_attention_confirm_roundtrip() {
    let rt = workgraph_runtime().await;

    // goal/create — TypeScript SDK pin, meta identity target form.
    let resp = unified_rpc(
        &rt,
        "wg-goal",
        "meta/workgraph/goal/create",
        json!({ "title": "Track it", "target": { "kind": "identity", "identity": "helper" } }),
    )
    .await;
    assert!(resp["error"].is_null(), "{resp:#?}");
    let goal = &resp["result"];
    let binding_id = goal["attention"]["binding_id"]
        .as_str()
        .expect("binding id is a string");
    assert_eq!(goal["attention"]["status"]["state"], json!("active"));
    assert_eq!(goal["attention"]["target"]["kind"], json!("lowered_owner"));
    let item_revision = goal["item"]["revision"].as_u64().expect("item revision");

    // attention/list — both SDKs send the status filter as a bare string.
    let resp = unified_rpc(
        &rt,
        "wg-att",
        "meta/workgraph/attention/list",
        json!({ "status": "active" }),
    )
    .await;
    assert!(resp["error"].is_null(), "{resp:#?}");
    let attention = resp["result"]["attention"].as_array().expect("attention");
    assert_eq!(attention.len(), 1, "{resp:#?}");
    assert_eq!(attention[0]["binding_id"], json!(binding_id));

    // goal/confirm — TypeScript SDK pin: binding_id/expected_revision/evidence.
    let resp = unified_rpc(
        &rt,
        "wg-confirm",
        "meta/workgraph/goal/confirm",
        json!({
            "binding_id": binding_id,
            "expected_revision": item_revision,
            "evidence": { "kind": "self_attest", "id": "ev-1" },
        }),
    )
    .await;
    assert!(resp["error"].is_null(), "{resp:#?}");
    let confirmed = &resp["result"];
    assert_eq!(
        confirmed["item"]["evidence_refs"][0]["kind"],
        json!("self_attest")
    );
    assert!(
        confirmed["attention"]["binding_id"].is_string(),
        "SDK parses item + attention from goal/confirm: {confirmed:#?}"
    );
    rt.mob_handle().stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_workgraph_stale_revision_returns_typed_conflict_shape() {
    let rt = workgraph_runtime().await;
    let created = unified_rpc(
        &rt,
        "wg-c2",
        "meta/workgraph/create",
        json!({ "title": "Ship it" }),
    )
    .await;
    let item = &created["result"]["item"];
    let stale = item["revision"].as_u64().expect("revision") + 41;

    // update — Python SDK pin: {"id", "expected_revision", "title"}.
    let resp = unified_rpc(
        &rt,
        "wg-stale",
        "meta/workgraph/update",
        json!({ "id": item["id"], "expected_revision": stale, "title": "Ship it faster" }),
    )
    .await;
    assert_eq!(resp["error"]["code"], json!(-32042), "{resp:#?}");
    assert_eq!(
        resp["error"]["data"]["kind"],
        json!("workgraph_conflict"),
        "SDKs reify the conflict on data.kind: {resp:#?}"
    );
    assert!(
        resp["error"]["data"]["detail"].is_string(),
        "detail carries the upstream message: {resp:#?}"
    );
    rt.mob_handle().stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_workgraph_unavailable_returns_minus_32041_shape() {
    let (_dir, rt) = runtime_without_workgraph().await;
    let resp = unified_rpc(&rt, "wg-un", "meta/workgraph/snapshot", json!({})).await;
    assert_eq!(resp["error"]["code"], json!(-32041), "{resp:#?}");
    assert_eq!(
        resp["error"]["data"]["kind"],
        json!("workgraph_unavailable"),
        "SDKs reify unavailability on data.kind: {resp:#?}"
    );
    rt.mob_handle().stop().await.expect("stop");
}

// ---------------------------------------------------------------------------
// meta/storage/doctor — read-only state-directory diagnosis (M1)
// ---------------------------------------------------------------------------

#[test]
fn e2e_storage_doctor_reports_twins_and_is_advertised() {
    let mut rt = test_runtime();

    // Advertised on the module-only capabilities list.
    let caps = rpc(
        &mut rt,
        &json!({
            "jsonrpc": "2.0", "id": "caps-doc", "method": "meta/capabilities", "params": {}
        }),
    );
    let methods = caps["result"]["methods"].as_array().expect("methods");
    assert!(
        methods
            .iter()
            .any(|m| m.as_str() == Some("meta/storage/doctor")),
        "{methods:?}"
    );

    // Twin fixture: both sessions spellings side by side in one state dir.
    let state = tempfile::tempdir().expect("state dir");
    std::fs::write(state.path().join("sessions.db"), b"").expect("twin a");
    std::fs::write(state.path().join("sessions.sqlite"), b"").expect("twin b");
    let resp = rpc(
        &mut rt,
        &json!({
            "jsonrpc": "2.0", "id": "doc1", "method": "meta/storage/doctor",
            "params": { "state_dir": state.path() }
        }),
    );
    let result = &resp["result"];
    assert!(result["state_dir"].is_string(), "{resp:#?}");
    assert!(
        result["storage"].is_null(),
        "module-only surface has no live durability summary: {resp:#?}"
    );
    let findings = result["diagnosis"]["findings"]
        .as_array()
        .expect("findings");
    assert!(
        findings
            .iter()
            .any(|f| f["code"] == "file-name-twins" && f["severity"] == "error"),
        "{findings:#?}"
    );
    assert!(
        findings
            .iter()
            .any(|f| f["code"] == "durability-census-unavailable"),
        "{findings:#?}"
    );
    assert!(result["diagnosis"]["inventory"].is_array());
}

#[test]
fn e2e_storage_doctor_requires_state_dir_on_module_only_surface() {
    let mut rt = test_runtime();
    let resp = rpc(
        &mut rt,
        &json!({
            "jsonrpc": "2.0", "id": "doc2", "method": "meta/storage/doctor", "params": {}
        }),
    );
    assert_eq!(resp["error"]["code"], json!(-32602), "{resp:#?}");
    assert!(
        resp["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("state_dir"),
        "{resp:#?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_unified_storage_doctor_missing_state_dir_is_typed_capability_error() {
    let (_dir, rt) = runtime_without_workgraph().await;
    let resp = unified_rpc(&rt, "doc3", "meta/storage/doctor", json!({})).await;
    assert_eq!(resp["error"]["code"], json!(-32004), "{resp:#?}");

    // Advertised on the unified capabilities list.
    let caps = unified_rpc(&rt, "caps-doc3", "meta/capabilities", json!({})).await;
    let methods = caps["result"]["methods"].as_array().expect("methods");
    assert!(
        methods
            .iter()
            .any(|m| m.as_str() == Some("meta/storage/doctor")),
        "{methods:?}"
    );
    rt.mob_handle().stop().await.expect("stop");
}
