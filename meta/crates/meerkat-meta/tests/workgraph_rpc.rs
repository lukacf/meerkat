//! `meta/workgraph/*` RPC surface (docs/design/workgraph-wire-contract.md):
//! unified stdin dispatch for all 22 methods, the error taxonomy
//! (-32041 unavailable / -32042 conflict / -32602 params), server-side
//! authority-witness injection, identity-target lowering, capabilities
//! advertisement, console dispatch with read-only + ABAC gating, the
//! experience `workgraph` section, and console principal promotion into
//! `goal/confirm`.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat_client::TestClient;
use meerkat_meta::{
    AccessControlConfig, AccessController, AccessRule, AuthPolicy, BigQueryNaming, ConsolePolicy,
    DiscoverySpec, MetaConfig, MobBootstrapOptions, MobBootstrapSpec, RuntimeDecisionInputs,
    RuntimeOpsPolicy, TrustedOidcRuntimeConfig, UnifiedRuntime, build_runtime_decision_state,
    handle_unified_rpc_json, validate_access_config,
};
use meerkat_mob::ids::AgentIdentity;
use meerkat_mob::{MobDefinition, MobStorage, SpawnMemberSpec};
use serde_json::{Value, json};
use tower::ServiceExt;

/// Per-test mob id counter: 0.8.23's fail-closed in-proc registration
/// means concurrently running tests must not share a supervisor route.
static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

type HmacSha256 = Hmac<sha2::Sha256>;

/// Per-call mob id: 0.8.23's fail-closed in-proc registration means
/// concurrently running tests must not share a supervisor route.
fn workgraph_mob_toml() -> String {
    format!(
        r#"
[mob]
id = "workgraph-rpc-mob-{}"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "autonomous_host"
external_addressable = true

[profiles.worker.tools]
comms = true
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

const ALL_WORKGRAPH_METHODS: &[&str] = &[
    "meta/workgraph/snapshot",
    "meta/workgraph/list",
    "meta/workgraph/get",
    "meta/workgraph/ready",
    "meta/workgraph/events",
    "meta/workgraph/attention/list",
    "meta/workgraph/goal/status",
    "meta/workgraph/create",
    "meta/workgraph/update",
    "meta/workgraph/claim",
    "meta/workgraph/release",
    "meta/workgraph/close",
    "meta/workgraph/block",
    "meta/workgraph/link",
    "meta/workgraph/evidence/add",
    "meta/workgraph/policy/escalate",
    "meta/workgraph/goal/create",
    "meta/workgraph/goal/confirm",
    "meta/workgraph/goal/request_close",
    "meta/workgraph/attention/pause",
    "meta/workgraph/attention/resume",
    "meta/workgraph/attention/reassign",
];

fn definition() -> MobDefinition {
    MobDefinition::from_toml(&workgraph_mob_toml()).expect("parse workgraph test definition")
}

/// The runtime's OWN mob id - `definition()` mints a fresh id per call, so
/// expectations must read the id back off the runtime under test.
fn runtime_mob_id(runtime: &UnifiedRuntime) -> String {
    runtime.mob_handle().definition().id.as_str().to_string()
}

/// Standard fixture: builder-constructed ephemeral runtime — the builder path
/// wires a memory-backed WorkGraph service automatically.
async fn build_runtime() -> UnifiedRuntime {
    Box::pin(
        UnifiedRuntime::builder()
            .definition(definition())
            .default_llm_client(Arc::new(TestClient::default()))
            .build(),
    )
    .await
    .expect("workgraph runtime builds")
}

/// Counter-fixture: a manually assembled spec (the `MobBootstrapSpec::new`
/// path both gateways use) with NO workgraph service.
async fn build_runtime_without_workgraph() -> (tempfile::TempDir, UnifiedRuntime) {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let factory = AgentFactory::new(temp_dir.path()).comms(true);
    let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 8));
    let mob_spec = MobBootstrapSpec::new(definition(), MobStorage::in_memory(), session_service)
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::default())),
        });
    let module_config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: "workgraph-rpc".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    };
    let runtime = UnifiedRuntime::bootstrap(mob_spec, module_config, Duration::from_secs(2))
        .await
        .expect("bootstrap runtime without workgraph");
    (temp_dir, runtime)
}

async fn rpc(runtime: &UnifiedRuntime, method: &str, params: Value) -> Value {
    let request = json!({
        "jsonrpc": "2.0",
        "id": "wg-test",
        "method": method,
        "params": params,
    })
    .to_string();
    let response =
        handle_unified_rpc_json(runtime, &request, Duration::from_secs(5), None, None).await;
    serde_json::from_str(&response).expect("rpc response json")
}

fn result(response: &Value) -> &Value {
    assert!(
        response["error"].is_null(),
        "expected success, got {response:#?}"
    );
    &response["result"]
}

fn error_code(response: &Value) -> i64 {
    response["error"]["code"]
        .as_i64()
        .unwrap_or_else(|| panic!("expected error, got {response:#?}"))
}

async fn create_item(runtime: &UnifiedRuntime, title: &str) -> Value {
    let response = rpc(runtime, "meta/workgraph/create", json!({ "title": title })).await;
    result(&response)["item"].clone()
}

// ---------------------------------------------------------------------------
// Capabilities + availability
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn capabilities_advertise_workgraph_when_configured() {
    let runtime = build_runtime().await;
    let response = rpc(&runtime, "meta/capabilities", json!({})).await;
    let result = result(&response);
    assert_eq!(result["workgraph"], json!(true));
    let methods: Vec<&str> = result["methods"]
        .as_array()
        .expect("methods array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for method in ALL_WORKGRAPH_METHODS {
        assert!(
            methods.contains(method),
            "capabilities must advertise {method}: {methods:?}"
        );
    }
    runtime.mob_handle().stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn workgraph_unavailable_without_service() {
    let (_dir, runtime) = build_runtime_without_workgraph().await;

    let caps = rpc(&runtime, "meta/capabilities", json!({})).await;
    assert_eq!(caps["result"]["workgraph"], json!(false));
    let methods = caps["result"]["methods"].to_string();
    assert!(
        !methods.contains("meta/workgraph/"),
        "unconfigured runtimes must not advertise workgraph methods"
    );

    let response = rpc(&runtime, "meta/workgraph/snapshot", json!({})).await;
    assert_eq!(error_code(&response), -32041, "{response:#?}");
    assert_eq!(
        response["error"]["data"]["kind"],
        json!("workgraph_unavailable")
    );
    runtime.mob_handle().stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_workgraph_method_is_method_not_found() {
    let runtime = build_runtime().await;
    let response = rpc(&runtime, "meta/workgraph/bogus", json!({})).await;
    assert_eq!(error_code(&response), -32601, "{response:#?}");
    runtime.mob_handle().stop().await.expect("stop");
}

// ---------------------------------------------------------------------------
// Item lifecycle
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn item_lifecycle_end_to_end() {
    let runtime = build_runtime().await;

    // create
    let response = rpc(
        &runtime,
        "meta/workgraph/create",
        json!({
            "title": "ship the release",
            "description": "cut 0.7.30",
            "priority": "high",
            "labels": ["release"],
        }),
    )
    .await;
    let item = result(&response)["item"].clone();
    let item_id = item["id"].as_str().expect("item id").to_string();
    assert_eq!(item["title"], json!("ship the release"));
    assert_eq!(item["status"], json!("open"));
    // CANONICAL mob realm (`mob.<mob_id>`) since the item-6 fix; the mob id
    // itself is unprefixed. The `mob.` prefix spelling is the pinned part -
    // it decides where rows physically live; see
    // workgraph_wiring::scoped_workgraph_service.
    assert_eq!(
        item["realm_id"],
        json!(format!("mob.{}", runtime_mob_id(&runtime)))
    );
    let revision = item["revision"].as_u64().expect("revision");

    // get
    let response = rpc(&runtime, "meta/workgraph/get", json!({ "id": item_id })).await;
    assert_eq!(result(&response)["item"]["id"], json!(item_id.clone()));

    // list + ready
    let response = rpc(&runtime, "meta/workgraph/list", json!({})).await;
    assert_eq!(result(&response)["items"].as_array().unwrap().len(), 1);
    let response = rpc(&runtime, "meta/workgraph/ready", json!({})).await;
    assert_eq!(
        result(&response)["items"][0]["id"],
        json!(item_id.clone()),
        "an open unclaimed item is ready"
    );

    // claim (upstream nested owner form)
    let response = rpc(
        &runtime,
        "meta/workgraph/claim",
        json!({
            "id": item_id,
            "expected_revision": revision,
            "owner": { "key": { "kind": "agent", "id": "helper" } },
        }),
    )
    .await;
    let item = result(&response)["item"].clone();
    assert_eq!(item["status"], json!("in_progress"));
    assert_eq!(item["claim"]["owner"]["key"]["id"], json!("helper"));
    let revision = item["revision"].as_u64().expect("revision");

    // release
    let response = rpc(
        &runtime,
        "meta/workgraph/release",
        json!({ "id": item_id, "expected_revision": revision }),
    )
    .await;
    let item = result(&response)["item"].clone();
    assert_eq!(item["status"], json!("open"));
    let revision = item["revision"].as_u64().expect("revision");

    // update
    let response = rpc(
        &runtime,
        "meta/workgraph/update",
        json!({
            "id": item_id,
            "expected_revision": revision,
            "description": "cut 0.7.30 with workgraph",
        }),
    )
    .await;
    let item = result(&response)["item"].clone();
    assert_eq!(item["description"], json!("cut 0.7.30 with workgraph"));
    let revision = item["revision"].as_u64().expect("revision");

    // evidence/add
    let response = rpc(
        &runtime,
        "meta/workgraph/evidence/add",
        json!({
            "id": item_id,
            "expected_revision": revision,
            "evidence": { "kind": "note", "id": "ci-run-1", "summary": "green" },
        }),
    )
    .await;
    let item = result(&response)["item"].clone();
    assert_eq!(item["evidence_refs"][0]["id"], json!("ci-run-1"));
    let revision = item["revision"].as_u64().expect("revision");

    // second item: block + link
    let second = create_item(&runtime, "follow-up docs").await;
    let second_id = second["id"].as_str().expect("second id").to_string();
    let second_revision = second["revision"].as_u64().expect("revision");
    let response = rpc(
        &runtime,
        "meta/workgraph/block",
        json!({ "id": second_id, "expected_revision": second_revision }),
    )
    .await;
    assert_eq!(result(&response)["item"]["status"], json!("blocked"));

    let response = rpc(
        &runtime,
        "meta/workgraph/link",
        json!({ "kind": "related", "from_id": item_id, "to_id": second_id }),
    )
    .await;
    let edge = result(&response)["edge"].clone();
    assert_eq!(edge["kind"], json!("related"));
    assert_eq!(edge["from_id"], json!(item_id.clone()));

    // snapshot carries items + edges + high-water mark
    let response = rpc(&runtime, "meta/workgraph/snapshot", json!({})).await;
    let snapshot = result(&response).clone();
    assert_eq!(snapshot["items"].as_array().unwrap().len(), 2);
    assert_eq!(snapshot["edges"].as_array().unwrap().len(), 1);
    assert!(snapshot["event_high_water_mark"].as_i64().is_some());

    // events tail
    let response = rpc(&runtime, "meta/workgraph/events", json!({ "limit": 100 })).await;
    let events = result(&response)["events"].as_array().unwrap().clone();
    assert!(!events.is_empty(), "event log must not be empty");

    // close
    let response = rpc(
        &runtime,
        "meta/workgraph/close",
        json!({ "id": item_id, "expected_revision": revision }),
    )
    .await;
    let item = result(&response)["item"].clone();
    assert_eq!(item["status"], json!("completed"), "default close status");
    runtime.mob_handle().stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn claim_accepts_flat_owner_wire_form() {
    let runtime = build_runtime().await;
    let item = create_item(&runtime, "flat owner claim").await;
    let response = rpc(
        &runtime,
        "meta/workgraph/claim",
        json!({
            "id": item["id"],
            "expected_revision": item["revision"],
            "owner": { "kind": "session", "id": "sess-1", "display_name": "Helper" },
        }),
    )
    .await;
    let claimed = result(&response)["item"].clone();
    assert_eq!(claimed["claim"]["owner"]["key"]["kind"], json!("session"));
    assert_eq!(claimed["claim"]["owner"]["display_name"], json!("Helper"));
    runtime.mob_handle().stop().await.expect("stop");
}

// ---------------------------------------------------------------------------
// Error taxonomy
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn params_validation_errors_are_typed() {
    let runtime = build_runtime().await;

    // non-object params
    let response = rpc(&runtime, "meta/workgraph/create", json!([1, 2])).await;
    assert_eq!(error_code(&response), -32602);

    // missing required field
    let response = rpc(&runtime, "meta/workgraph/create", json!({})).await;
    assert_eq!(error_code(&response), -32602);

    // realm_id is never accepted over the wire
    let response = rpc(
        &runtime,
        "meta/workgraph/list",
        json!({ "realm_id": "other-realm" }),
    )
    .await;
    assert_eq!(error_code(&response), -32602);
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("realm_id"),
        "{response:#?}"
    );

    // close with a non-terminal status
    let item = create_item(&runtime, "bad close").await;
    let response = rpc(
        &runtime,
        "meta/workgraph/close",
        json!({ "id": item["id"], "expected_revision": item["revision"], "status": "open" }),
    )
    .await;
    assert_eq!(error_code(&response), -32602);

    // update without expected_revision
    let response = rpc(
        &runtime,
        "meta/workgraph/update",
        json!({ "id": item["id"], "title": "x" }),
    )
    .await;
    assert_eq!(error_code(&response), -32602);
    runtime.mob_handle().stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_revision_maps_to_conflict_code() {
    let runtime = build_runtime().await;
    let item = create_item(&runtime, "cas target").await;
    let stale = item["revision"].as_u64().unwrap() + 41;
    let response = rpc(
        &runtime,
        "meta/workgraph/update",
        json!({ "id": item["id"], "expected_revision": stale, "title": "stale write" }),
    )
    .await;
    assert_eq!(error_code(&response), -32042, "{response:#?}");
    assert_eq!(
        response["error"]["data"]["kind"],
        json!("workgraph_conflict")
    );
    assert!(
        response["error"]["data"]["detail"]
            .as_str()
            .unwrap()
            .contains("stale"),
        "detail carries the upstream message: {response:#?}"
    );
    runtime.mob_handle().stop().await.expect("stop");
}

// ---------------------------------------------------------------------------
// Goals + attention
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn goal_lifecycle_with_identity_target() {
    let runtime = build_runtime().await;

    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "keep the dashboards green",
            "target": { "kind": "identity", "identity": "helper" },
        }),
    )
    .await;
    let goal = result(&response).clone();
    let binding_id = goal["attention"]["binding_id"]
        .as_str()
        .unwrap()
        .to_string();
    // Identity targets lower to the mob-scoped owner key.
    assert_eq!(goal["attention"]["target"]["kind"], json!("lowered_owner"));
    assert_eq!(
        goal["attention"]["target"]["owner_key"],
        json!({
            "kind": "agent",
            "id": format!("mob/{}/agent/helper", runtime_mob_id(&runtime)),
        })
    );
    assert_eq!(goal["attention"]["status"]["state"], json!("active"));
    let binding_revision = goal["attention"]["machine_state"]["revision"]
        .as_u64()
        .expect("binding revision");

    // goal/status round-trips item + attention
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/status",
        json!({ "binding_id": binding_id }),
    )
    .await;
    let status = result(&response).clone();
    assert_eq!(status["item"]["title"], json!("keep the dashboards green"));
    assert_eq!(status["attention"]["binding_id"], json!(binding_id.clone()));

    // attention/list sees the active binding
    let response = rpc(&runtime, "meta/workgraph/attention/list", json!({})).await;
    let attention = result(&response)["attention"].as_array().unwrap().clone();
    assert_eq!(attention.len(), 1);

    // pause → resume
    let response = rpc(
        &runtime,
        "meta/workgraph/attention/pause",
        json!({ "binding_id": binding_id, "expected_revision": binding_revision }),
    )
    .await;
    let paused = result(&response)["attention"].clone();
    assert_eq!(paused["status"]["state"], json!("paused"));
    let binding_revision = paused["machine_state"]["revision"].as_u64().unwrap();

    let response = rpc(
        &runtime,
        "meta/workgraph/attention/resume",
        json!({ "binding_id": binding_id, "expected_revision": binding_revision }),
    )
    .await;
    let resumed = result(&response)["attention"].clone();
    assert_eq!(resumed["status"]["state"], json!("active"));

    // confirm (SelfAttest, defaulted evidence) then policy-gated close
    let item_revision = status["item"]["revision"].as_u64().unwrap();
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/confirm",
        json!({ "binding_id": binding_id, "expected_revision": item_revision }),
    )
    .await;
    let confirmed = result(&response).clone();
    assert_eq!(
        confirmed["item"]["evidence_refs"][0]["kind"],
        json!("self_attest"),
        "absent wire evidence defaults to the policy's admissible kind"
    );
    let item_revision = confirmed["item"]["revision"].as_u64().unwrap();

    let response = rpc(
        &runtime,
        "meta/workgraph/goal/request_close",
        json!({ "binding_id": binding_id, "expected_revision": item_revision }),
    )
    .await;
    let closed = result(&response).clone();
    assert_eq!(closed["item"]["status"], json!("completed"));
    assert_eq!(
        closed["attention"]["status"]["state"],
        json!("stopped"),
        "closing the goal stops its attention binding"
    );
    runtime.mob_handle().stop().await.expect("stop");
}

/// Round-4 Q2 (write normalization): a session target that belongs to a
/// roster member is lowered to the member's OWNER form before the write, so
/// the stored row matches identity-form occupancy checks without a roster —
/// in a co-process sharing the SQLite store, and mid-respawn in this one.
/// Non-member sessions have no aliasing and keep their session form.
#[tokio::test(flavor = "multi_thread")]
async fn goal_create_lowers_member_session_targets_to_owner_form() {
    let runtime = build_runtime().await;
    runtime
        .spawn_many(vec![SpawnMemberSpec::from_wire(
            "worker".to_string(),
            "helper".to_string(),
            None,
            None,
            None,
        )])
        .await
        .expect("spawn member");
    let session_id = runtime
        .mob_handle()
        .resolve_bridge_session_id_observation(&AgentIdentity::from("helper"))
        .await
        .expect("member session id");

    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "session-scoped goal",
            "target": { "kind": "session", "session_id": session_id.to_string() },
        }),
    )
    .await;
    let goal = result(&response).clone();
    assert_eq!(
        goal["attention"]["target"]["kind"],
        json!("lowered_owner"),
        "member session targets must be stored owner-form: {goal:#?}"
    );
    assert_eq!(
        goal["attention"]["target"]["owner_key"]["kind"],
        json!("agent")
    );
    assert_eq!(
        goal["attention"]["target"]["owner_key"]["id"],
        json!(format!("mob/{}/agent/helper", runtime_mob_id(&runtime)))
    );

    // A session that is NOT a roster member keeps its session spelling.
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "non-member session goal",
            "target": {
                "kind": "session",
                "session_id": "019e63c2-0000-7000-8000-00000000beef",
            },
        }),
    )
    .await;
    let goal = result(&response).clone();
    assert_eq!(goal["attention"]["target"]["kind"], json!("session"));

    // Unsupported target kinds are a params error.
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({ "title": "bad target", "target": { "kind": "mob" } }),
    )
    .await;
    assert_eq!(error_code(&response), -32602);
    runtime.mob_handle().stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn attention_reassign_injects_witness_server_side() {
    let runtime = build_runtime().await;
    // Coordinate mode grants can_link_derived_from, the reassign authority.
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "coordinated goal",
            "target": { "kind": "identity", "identity": "helper" },
            "mode": "coordinate",
        }),
    )
    .await;
    let goal = result(&response).clone();
    let binding_id = goal["attention"]["binding_id"]
        .as_str()
        .unwrap()
        .to_string();
    let binding_revision = goal["attention"]["machine_state"]["revision"]
        .as_u64()
        .unwrap();

    // A wire-supplied witness is rejected — it is unforgeable by design.
    let response = rpc(
        &runtime,
        "meta/workgraph/attention/reassign",
        json!({
            "binding_id": binding_id,
            "expected_revision": binding_revision,
            "target": { "kind": "identity", "identity": "backup" },
            "authority_projection": { "forged": true },
        }),
    )
    .await;
    assert_eq!(error_code(&response), -32602);
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("authority_projection"),
        "{response:#?}"
    );

    // Without one, the server fetches the live projection and reassigns.
    let response = rpc(
        &runtime,
        "meta/workgraph/attention/reassign",
        json!({
            "binding_id": binding_id,
            "expected_revision": binding_revision,
            "target": { "kind": "identity", "identity": "backup" },
        }),
    )
    .await;
    let reassigned = result(&response).clone();
    assert_eq!(
        reassigned["previous"]["status"]["state"],
        json!("superseded")
    );
    assert_eq!(reassigned["attention"]["status"]["state"], json!("active"));
    assert_eq!(
        reassigned["attention"]["target"]["owner_key"]["id"],
        json!(format!("mob/{}/agent/backup", runtime_mob_id(&runtime)))
    );
    runtime.mob_handle().stop().await.expect("stop");
}

/// Round-5 S2: every RESULT serializes the stored binding target, whose
/// owner form is spelled `lowered_owner` — the exact string
/// `resolve_goal_target` used to reject. A read-back `attention.target`
/// must round-trip VERBATIM into `attention/reassign` params.
#[tokio::test(flavor = "multi_thread")]
async fn result_attention_target_round_trips_verbatim_into_reassign() {
    let runtime = build_runtime().await;
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "round-trip goal",
            "target": { "kind": "identity", "identity": "helper" },
            "mode": "coordinate",
        }),
    )
    .await;
    let goal = result(&response).clone();
    let read_back_target = goal["attention"]["target"].clone();
    assert_eq!(
        read_back_target["kind"],
        json!("lowered_owner"),
        "precondition: results serialize the lowered_owner spelling: {goal:#?}"
    );

    // Reassigning the binding onto its own read-back target supersedes it
    // with a fresh Active binding on the same member — the admission
    // excludes the binding being moved, and upstream has no same-target
    // rejection. Before the fix this was -32602 (unsupported target.kind).
    let response = rpc(
        &runtime,
        "meta/workgraph/attention/reassign",
        json!({
            "binding_id": goal["attention"]["binding_id"],
            "expected_revision": goal["attention"]["machine_state"]["revision"],
            "target": read_back_target,
        }),
    )
    .await;
    let reassigned = result(&response).clone();
    assert_eq!(
        reassigned["previous"]["status"]["state"],
        json!("superseded")
    );
    assert_eq!(reassigned["attention"]["status"]["state"], json!("active"));
    assert_eq!(
        reassigned["attention"]["target"], read_back_target,
        "the stored target must survive the round-trip unchanged"
    );
    runtime.mob_handle().stop().await.expect("stop");
}

/// Adversarial finding F10: a second ACTIVE binding for a target that
/// already has one bricks the member — every subsequent scoped turn is a
/// hard upstream `MultipleActiveBindings` error. `goal/create` must reject
/// it up front as the typed conflict, naming the existing binding.
#[tokio::test(flavor = "multi_thread")]
async fn duplicate_active_binding_for_same_target_is_conflict() {
    let runtime = build_runtime().await;
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "first goal",
            "target": { "kind": "identity", "identity": "helper" },
        }),
    )
    .await;
    let first = result(&response).clone();
    let first_binding = first["attention"]["binding_id"]
        .as_str()
        .unwrap()
        .to_string();

    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "second goal, same target",
            "target": { "kind": "identity", "identity": "helper" },
        }),
    )
    .await;
    assert_eq!(error_code(&response), -32042, "{response:#?}");
    assert_eq!(
        response["error"]["data"]["kind"],
        json!("workgraph_conflict")
    );
    let detail = response["error"]["data"]["detail"].as_str().unwrap();
    assert!(
        detail.contains(&first_binding),
        "conflict must name the existing binding: {detail}"
    );
    assert!(
        detail.contains("reassign") && detail.contains("close its goal"),
        "detail must hint the way out: {detail}"
    );

    // A different target is unaffected.
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "different member",
            "target": { "kind": "identity", "identity": "backup" },
        }),
    )
    .await;
    assert!(response["error"].is_null(), "{response:#?}");

    // Round-2 hole 4: pausing the existing binding does NOT free the target
    // — the pause auto-reactivates at expiry, so a goal created "into" the
    // pause becomes the second Active binding the moment it expires. The
    // conflict must name the paused binding and hint resume-or-close.
    let binding_revision = first["attention"]["machine_state"]["revision"]
        .as_u64()
        .unwrap();
    let response = rpc(
        &runtime,
        "meta/workgraph/attention/pause",
        json!({ "binding_id": first_binding, "expected_revision": binding_revision }),
    )
    .await;
    assert!(response["error"].is_null(), "{response:#?}");
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "into the pause",
            "target": { "kind": "identity", "identity": "helper" },
        }),
    )
    .await;
    assert_eq!(error_code(&response), -32042, "{response:#?}");
    let detail = response["error"]["data"]["detail"].as_str().unwrap();
    assert!(
        detail.contains("paused") && detail.contains(&first_binding) && detail.contains("resume"),
        "paused conflict must name the binding and hint resume-or-close: {detail}"
    );

    // Closing the goal (confirm + request_close stops its binding) genuinely
    // frees the target.
    let item_revision = first["item"]["revision"].as_u64().unwrap();
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/confirm",
        json!({ "binding_id": first_binding, "expected_revision": item_revision }),
    )
    .await;
    let item_revision = result(&response)["item"]["revision"].as_u64().unwrap();
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/request_close",
        json!({ "binding_id": first_binding, "expected_revision": item_revision }),
    )
    .await;
    assert!(response["error"].is_null(), "{response:#?}");
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "after close",
            "target": { "kind": "identity", "identity": "helper" },
        }),
    )
    .await;
    assert!(response["error"].is_null(), "{response:#?}");
    runtime.mob_handle().stop().await.expect("stop");
}

/// Round-2 hole 1: `attention/reassign` creates an Active binding on the
/// NEW target, so reassigning onto a member that already has one re-creates
/// the `MultipleActiveBindings` bricked state `goal/create` guards against.
/// The reassign target must pass the same occupancy guard (excluding the
/// binding being superseded, which never conflicts with its own move).
#[tokio::test(flavor = "multi_thread")]
async fn reassign_onto_occupied_target_is_conflict() {
    let runtime = build_runtime().await;
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "already watching",
            "target": { "kind": "identity", "identity": "occupied" },
            "mode": "coordinate",
        }),
    )
    .await;
    let occupied_binding = result(&response)["attention"]["binding_id"]
        .as_str()
        .unwrap()
        .to_string();

    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "about to move",
            "target": { "kind": "identity", "identity": "mover" },
            "mode": "coordinate",
        }),
    )
    .await;
    let mover = result(&response).clone();
    let mover_binding = mover["attention"]["binding_id"].as_str().unwrap();
    let mover_revision = mover["attention"]["machine_state"]["revision"]
        .as_u64()
        .unwrap();

    let response = rpc(
        &runtime,
        "meta/workgraph/attention/reassign",
        json!({
            "binding_id": mover_binding,
            "expected_revision": mover_revision,
            "target": { "kind": "identity", "identity": "occupied" },
        }),
    )
    .await;
    assert_eq!(error_code(&response), -32042, "{response:#?}");
    assert_eq!(
        response["error"]["data"]["kind"],
        json!("workgraph_conflict")
    );
    let detail = response["error"]["data"]["detail"].as_str().unwrap();
    assert!(
        detail.contains(&occupied_binding),
        "conflict must name the occupying binding: {detail}"
    );

    // A free target reassigns normally — the guard does not over-block.
    let response = rpc(
        &runtime,
        "meta/workgraph/attention/reassign",
        json!({
            "binding_id": mover_binding,
            "expected_revision": mover_revision,
            "target": { "kind": "identity", "identity": "free" },
        }),
    )
    .await;
    assert!(response["error"].is_null(), "{response:#?}");
    runtime.mob_handle().stop().await.expect("stop");
}

/// Round-2 hole 2: pause A, get a second binding onto the same member, then
/// resume A = two Active bindings. The second binding here is created
/// directly on the service (the member tool surface and pre-guard data can
/// both do that), so only the resume guard stands between the operator and
/// the bricked member.
///
/// Round-3 R2: a PAUSED sibling occupies too — a timed pause auto-reactivates
/// at expiry, so resuming "into" it just schedules the second Active. The
/// resume guard counts siblings exactly like create/reassign (Active OR
/// Paused); only closing the sibling's goal frees the target.
#[tokio::test(flavor = "multi_thread")]
async fn resume_with_another_active_binding_on_the_target_is_conflict() {
    let runtime = build_runtime().await;
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "first watch",
            "target": { "kind": "identity", "identity": "helper" },
        }),
    )
    .await;
    let first = result(&response).clone();
    let first_binding = first["attention"]["binding_id"].as_str().unwrap();
    let first_revision = first["attention"]["machine_state"]["revision"]
        .as_u64()
        .unwrap();
    let response = rpc(
        &runtime,
        "meta/workgraph/attention/pause",
        json!({ "binding_id": first_binding, "expected_revision": first_revision }),
    )
    .await;
    let paused_revision = result(&response)["attention"]["machine_state"]["revision"]
        .as_u64()
        .unwrap();

    // Second binding for the SAME lowered owner, created past the RPC guard.
    let service = runtime.workgraph_service().expect("workgraph service");
    let second = service
        .create_goal(meerkat::GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: "second watch".to_string(),
            description: None,
            target: meerkat::GoalAttentionTarget::Owner {
                // MUST be the running mob's own id - `definition()` mints a
                // fresh unique id per call, which would target a different
                // owner and defeat the sibling-conflict guard under test.
                owner_key: meerkat_mob::lower_agent_identity_owner_key(
                    &runtime.mob_handle().definition().id,
                    &AgentIdentity::from("helper"),
                )
                .expect("lower owner key"),
            },
            mode: Default::default(),
            completion_policy: Default::default(),
            // meerkat 0.8.22 promoted the item-shaping fields of
            // `CreateWorkItemRequest` onto `GoalCreateRequest`. Eight of the
            // ten already existed on `CreateWorkItemRequest` in 0.8.21, where
            // `create_goal` filled them from
            // `..CreateWorkItemRequest::default()`; the two join policies are
            // new in 0.8.22 and so had no prior value to preserve. The values
            // below therefore reproduce the pre-port item exactly. Both join
            // policies are inert here in any case: this fixture creates no
            // `parent` edges, so no child can fail or cancel into this goal.
            failed_child_join_policy: Default::default(),
            cancelled_child_join_policy: Default::default(),
            priority: Default::default(),
            labels: Default::default(),
            due_at: None,
            not_before: None,
            snoozed_until: None,
            external_refs: Vec::new(),
            evidence_refs: Vec::new(),
            status: None,
            delegated_authority: Default::default(),
            projection_policy: Default::default(),
        })
        .await
        .expect("service-side goal create");
    let second_binding = second.attention.binding_id.to_string();

    let response = rpc(
        &runtime,
        "meta/workgraph/attention/resume",
        json!({ "binding_id": first_binding, "expected_revision": paused_revision }),
    )
    .await;
    assert_eq!(error_code(&response), -32042, "{response:#?}");
    let detail = response["error"]["data"]["detail"].as_str().unwrap();
    assert!(
        detail.contains(&second_binding),
        "conflict must name the active binding: {detail}"
    );

    // Round-3 R2: a TIMED pause on the sibling does NOT clear the way — it
    // auto-reactivates at expiry, so the resumed binding would become the
    // second Active the moment it fires. The conflict must name the paused
    // sibling and say why.
    let second_revision = second.attention.machine_state.revision;
    let response = rpc(
        &runtime,
        "meta/workgraph/attention/pause",
        json!({
            "binding_id": second_binding,
            "expected_revision": second_revision,
            "until": "2099-01-01T00:00:00Z",
        }),
    )
    .await;
    assert!(response["error"].is_null(), "{response:#?}");
    let response = rpc(
        &runtime,
        "meta/workgraph/attention/resume",
        json!({ "binding_id": first_binding, "expected_revision": paused_revision }),
    )
    .await;
    assert_eq!(error_code(&response), -32042, "{response:#?}");
    let detail = response["error"]["data"]["detail"].as_str().unwrap();
    assert!(
        detail.contains(&second_binding)
            && detail.contains("paused")
            && detail.contains("reactivate"),
        "timed-paused sibling must block resume and name itself: {detail}"
    );

    // Closing the sibling's goal (confirm + request_close stops its binding)
    // genuinely frees the target for the resume.
    let second_item_revision = second.item.revision;
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/confirm",
        json!({ "binding_id": second_binding, "expected_revision": second_item_revision }),
    )
    .await;
    let second_item_revision = result(&response)["item"]["revision"].as_u64().unwrap();
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/request_close",
        json!({ "binding_id": second_binding, "expected_revision": second_item_revision }),
    )
    .await;
    assert!(response["error"].is_null(), "{response:#?}");
    let response = rpc(
        &runtime,
        "meta/workgraph/attention/resume",
        json!({ "binding_id": first_binding, "expected_revision": paused_revision }),
    )
    .await;
    assert!(response["error"].is_null(), "{response:#?}");
    runtime.mob_handle().stop().await.expect("stop");
}

/// Round-2 hole 3 (aliasing): upstream `attention_target_matches_session`
/// matches BOTH a `Session{session_id}` target and the lowered
/// `mob/<mob>/agent/<identity>` owner target to the same member's turns, so
/// one member with a session-form binding and an identity-form binding is
/// still bricked. The guard must resolve session↔identity through the
/// roster, in both directions.
#[tokio::test(flavor = "multi_thread")]
async fn session_and_identity_goal_targets_conflict_as_the_same_member() {
    let runtime = build_runtime().await;
    runtime
        .spawn_many(vec![SpawnMemberSpec::from_wire(
            "worker".to_string(),
            "helper".to_string(),
            None,
            None,
            None,
        )])
        .await
        .expect("spawn member");
    let session_id = runtime
        .mob_handle()
        .resolve_bridge_session_id_observation(&AgentIdentity::from("helper"))
        .await
        .expect("member session id")
        .to_string();

    // identity first, session second: the session spelling must conflict.
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "identity-form goal",
            "target": { "kind": "identity", "identity": "helper" },
        }),
    )
    .await;
    let identity_goal = result(&response).clone();
    let identity_binding = identity_goal["attention"]["binding_id"]
        .as_str()
        .unwrap()
        .to_string();
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "session-form goal for the same member",
            "target": { "kind": "session", "session_id": session_id },
        }),
    )
    .await;
    assert_eq!(error_code(&response), -32042, "{response:#?}");
    assert!(
        response["error"]["data"]["detail"]
            .as_str()
            .unwrap()
            .contains(&identity_binding),
        "{response:#?}"
    );

    // Free the member (confirm + request_close stops the binding), then the
    // reverse direction: session first, identity second.
    let item_revision = identity_goal["item"]["revision"].as_u64().unwrap();
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/confirm",
        json!({ "binding_id": identity_binding, "expected_revision": item_revision }),
    )
    .await;
    let item_revision = result(&response)["item"]["revision"].as_u64().unwrap();
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/request_close",
        json!({ "binding_id": identity_binding, "expected_revision": item_revision }),
    )
    .await;
    assert!(response["error"].is_null(), "{response:#?}");

    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "session-form goal",
            "target": { "kind": "session", "session_id": session_id },
        }),
    )
    .await;
    let session_binding = result(&response)["attention"]["binding_id"]
        .as_str()
        .unwrap()
        .to_string();
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "identity-form goal for the same member",
            "target": { "kind": "identity", "identity": "helper" },
        }),
    )
    .await;
    assert_eq!(error_code(&response), -32042, "{response:#?}");
    assert!(
        response["error"]["data"]["detail"]
            .as_str()
            .unwrap()
            .contains(&session_binding),
        "{response:#?}"
    );
    runtime.mob_handle().stop().await.expect("stop");
}

/// Round-2 hole 5 (TOCTOU): two concurrent `goal/create` calls for the same
/// target must admit exactly one — the admission gate serializes the
/// check-then-act window shared by the stdin and console surfaces.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_goal_creates_for_same_target_admit_exactly_one() {
    let runtime = Arc::new(build_runtime().await);
    let call = |title: &str| {
        let runtime = Arc::clone(&runtime);
        let title = title.to_string();
        tokio::spawn(async move {
            rpc(
                &runtime,
                "meta/workgraph/goal/create",
                json!({
                    "title": title,
                    "target": { "kind": "identity", "identity": "racer" },
                }),
            )
            .await
        })
    };
    let (left, right) = tokio::join!(call("left lane"), call("right lane"));
    let (left, right) = (left.expect("join"), right.expect("join"));

    let successes = [&left, &right]
        .iter()
        .filter(|response| response["error"].is_null())
        .count();
    let conflicts = [&left, &right]
        .iter()
        .filter(|response| response["error"]["code"] == json!(-32042))
        .count();
    assert_eq!(
        (successes, conflicts),
        (1, 1),
        "exactly one create wins the race: left={left:#?} right={right:#?}"
    );
    runtime.mob_handle().stop().await.expect("stop");
}

/// Round-2 finding B: SDKs send the attention-list `status` filter as a
/// bare string, which upstream's internally-tagged enum rejects — the
/// filter never worked over the wire. Both spellings must filter, and
/// unknown strings are a typed params error.
#[tokio::test(flavor = "multi_thread")]
async fn attention_list_status_filter_accepts_sdk_strings() {
    let runtime = build_runtime().await;
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "filter me",
            "target": { "kind": "identity", "identity": "helper" },
        }),
    )
    .await;
    let goal = result(&response).clone();
    let binding_id = goal["attention"]["binding_id"].as_str().unwrap();
    let binding_revision = goal["attention"]["machine_state"]["revision"]
        .as_u64()
        .unwrap();

    // Bare-string form (what both SDKs send).
    let response = rpc(
        &runtime,
        "meta/workgraph/attention/list",
        json!({ "status": "active" }),
    )
    .await;
    assert_eq!(
        result(&response)["attention"].as_array().unwrap().len(),
        1,
        "{response:#?}"
    );
    let response = rpc(
        &runtime,
        "meta/workgraph/attention/list",
        json!({ "status": "stopped" }),
    )
    .await;
    assert!(
        result(&response)["attention"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{response:#?}"
    );

    // Tagged-object form passes through verbatim.
    let response = rpc(
        &runtime,
        "meta/workgraph/attention/pause",
        json!({ "binding_id": binding_id, "expected_revision": binding_revision }),
    )
    .await;
    assert!(response["error"].is_null(), "{response:#?}");
    let response = rpc(
        &runtime,
        "meta/workgraph/attention/list",
        json!({ "status": "paused" }),
    )
    .await;
    assert_eq!(
        result(&response)["attention"].as_array().unwrap().len(),
        1,
        "{response:#?}"
    );
    let response = rpc(
        &runtime,
        "meta/workgraph/attention/list",
        json!({ "status": { "state": "paused" } }),
    )
    .await;
    assert_eq!(
        result(&response)["attention"].as_array().unwrap().len(),
        1,
        "{response:#?}"
    );

    // Unknown strings are a params error naming the vocabulary.
    let response = rpc(
        &runtime,
        "meta/workgraph/attention/list",
        json!({ "status": "everything" }),
    )
    .await;
    assert_eq!(error_code(&response), -32602, "{response:#?}");
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("active"),
        "{response:#?}"
    );
    runtime.mob_handle().stop().await.expect("stop");
}

/// Round-2 finding K: upstream turn-overlay resolution lists attention only
/// in the default namespace, so goals/bindings filed anywhere else are
/// silently inert. Goal/attention methods must reject a non-default
/// namespace; item-level methods keep passthrough.
#[tokio::test(flavor = "multi_thread")]
async fn non_default_namespace_is_rejected_on_goal_and_attention_methods() {
    let runtime = build_runtime().await;
    let cases: &[(&str, Value)] = &[
        (
            "meta/workgraph/goal/create",
            json!({
                "title": "stranded goal",
                "target": { "kind": "identity", "identity": "helper" },
                "namespace": "sidecar",
            }),
        ),
        (
            "meta/workgraph/attention/list",
            json!({ "namespace": "sidecar" }),
        ),
        (
            "meta/workgraph/attention/pause",
            json!({ "binding_id": "b-1", "expected_revision": 0, "namespace": "sidecar" }),
        ),
        (
            "meta/workgraph/attention/resume",
            json!({ "binding_id": "b-1", "expected_revision": 0, "namespace": "sidecar" }),
        ),
        (
            "meta/workgraph/attention/reassign",
            json!({
                "binding_id": "b-1",
                "expected_revision": 0,
                "target": { "kind": "identity", "identity": "helper" },
                "namespace": "sidecar",
            }),
        ),
        (
            "meta/workgraph/policy/escalate",
            json!({
                "binding_id": "b-1",
                "id": "work_1",
                "expected_revision": 0,
                "completion_policy": { "kind": "host_confirmed" },
                "namespace": "sidecar",
            }),
        ),
    ];
    for (method, params) in cases {
        let response = rpc(&runtime, method, params.clone()).await;
        assert_eq!(error_code(&response), -32602, "{method}: {response:#?}");
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("default namespace"),
            "{method} must explain the overlay restriction: {response:#?}"
        );
    }

    // Spelling the default namespace explicitly stays accepted.
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "explicit default",
            "target": { "kind": "identity", "identity": "helper" },
            "namespace": "default",
        }),
    )
    .await;
    assert!(response["error"].is_null(), "{response:#?}");

    // Item-level methods NO LONGER pass a namespace through. Since 0.8.22 one
    // service owns one immutable grant, so a sidecar namespace is refused
    // fail-closed rather than stranded in a scope no operator surface reads.
    // A sidecar needs its own service/grant/surface.
    let response = rpc(
        &runtime,
        "meta/workgraph/create",
        json!({ "title": "namespaced item", "namespace": "sidecar" }),
    )
    .await;
    assert_eq!(
        response["error"]["code"],
        json!(-32602),
        "a non-default namespace must be refused typed, not stranded: {response:#?}"
    );
    runtime.mob_handle().stop().await.expect("stop");
}

/// Adversarial finding F11: reassign of a non-coordinate binding can never
/// succeed on meerkat 0.7.23 (only coordinate mode derives the required
/// `derived_from` link authority), and the raw upstream denial is a generic
/// invalid-input. The RPC must name the binding's mode and the restriction.
/// The coordinate-mode success path is covered by
/// `attention_reassign_injects_witness_server_side`.
#[tokio::test(flavor = "multi_thread")]
async fn reassign_of_non_coordinate_binding_names_the_mode_restriction() {
    let runtime = build_runtime().await;
    // Default mode is pursue.
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "pursue goal",
            "target": { "kind": "identity", "identity": "helper" },
        }),
    )
    .await;
    let goal = result(&response).clone();
    let binding_id = goal["attention"]["binding_id"]
        .as_str()
        .unwrap()
        .to_string();
    let binding_revision = goal["attention"]["machine_state"]["revision"]
        .as_u64()
        .unwrap();

    let response = rpc(
        &runtime,
        "meta/workgraph/attention/reassign",
        json!({
            "binding_id": binding_id,
            "expected_revision": binding_revision,
            "target": { "kind": "identity", "identity": "backup" },
        }),
    )
    .await;
    assert_eq!(error_code(&response), -32000, "{response:#?}");
    assert_eq!(response["error"]["data"]["kind"], json!("workgraph_error"));
    let detail = response["error"]["data"]["detail"].as_str().unwrap();
    assert!(
        detail.contains(&binding_id),
        "must name the binding: {detail}"
    );
    assert!(
        detail.contains("'pursue' mode"),
        "must name the binding's mode: {detail}"
    );
    assert!(
        detail.contains("coordinate"),
        "must name the mode restriction: {detail}"
    );
    runtime.mob_handle().stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn policy_escalate_injects_witness_server_side() {
    let runtime = build_runtime().await;
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "tighten me",
            "target": { "kind": "identity", "identity": "helper" },
        }),
    )
    .await;
    let goal = result(&response).clone();
    let binding_id = goal["attention"]["binding_id"]
        .as_str()
        .unwrap()
        .to_string();
    let item_id = goal["item"]["id"].as_str().unwrap().to_string();
    let item_revision = goal["item"]["revision"].as_u64().unwrap();

    // Forged witness rejected.
    let response = rpc(
        &runtime,
        "meta/workgraph/policy/escalate",
        json!({
            "binding_id": binding_id,
            "id": item_id,
            "expected_revision": item_revision,
            "completion_policy": { "kind": "host_confirmed" },
            "authority_projection": { "forged": true },
        }),
    )
    .await;
    assert_eq!(error_code(&response), -32602);

    // Server-side witness: SelfAttest → HostConfirmed is a monotonic tighten.
    let response = rpc(
        &runtime,
        "meta/workgraph/policy/escalate",
        json!({
            "binding_id": binding_id,
            "id": item_id,
            "expected_revision": item_revision,
            "completion_policy": { "kind": "host_confirmed" },
        }),
    )
    .await;
    let item = result(&response)["item"].clone();
    assert_eq!(item["completion_policy"]["kind"], json!("host_confirmed"));
    runtime.mob_handle().stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn goal_confirm_requires_principal_for_principal_confirmed_policy() {
    let runtime = build_runtime().await;
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "operator sign-off",
            "target": { "kind": "identity", "identity": "helper" },
            "completion_policy": { "kind": "principal_confirmed" },
        }),
    )
    .await;
    let goal = result(&response).clone();
    let binding_id = goal["attention"]["binding_id"].as_str().unwrap();
    let item_revision = goal["item"]["revision"].as_u64().unwrap();

    // The unified stdin surface has no wire principal to promote, so a
    // principal-confirmed policy cannot be confirmed here.
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/confirm",
        json!({ "binding_id": binding_id, "expected_revision": item_revision }),
    )
    .await;
    assert_eq!(error_code(&response), -32602, "{response:#?}");
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("principal"),
        "{response:#?}"
    );

    // Reserved confirmation classifications cannot be smuggled in evidence.
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/confirm",
        json!({
            "binding_id": binding_id,
            "expected_revision": item_revision,
            "evidence": {
                "kind": "confirmation",
                "id": "smuggle",
                "confirmation_kind": "host_confirmation",
            },
        }),
    )
    .await;
    assert_eq!(error_code(&response), -32602, "{response:#?}");
    runtime.mob_handle().stop().await.expect("stop");
}

// ---------------------------------------------------------------------------
// Console surface
// ---------------------------------------------------------------------------

fn trusted_oidc() -> TrustedOidcRuntimeConfig {
    // HS256 tokens are only honored for development (`.localhost`) issuers.
    TrustedOidcRuntimeConfig {
        discovery_json:
            r#"{"issuer":"https://trusted.meta.localhost","jwks_uri":"https://trusted.meta.localhost/.well-known/jwks.json"}"#
                .to_string(),
        jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
            .to_string(),
        audience: "meerkat-console".to_string(),
        require_verified_email: false,
    }
}

fn decision_state(require_app_auth: bool, read_only: bool) -> meerkat_meta::RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "workgraph_dataset".to_string(),
            table: "workgraph_table".to_string(),
        },
        trusted_meta_toml: r#"
[[modules]]
id = "router"
command = "router-bin"
args = []
restart_policy = "always"
"#
        .to_string(),
        auth: AuthPolicy {
            default_provider: meerkat_meta::AuthProvider::GoogleOAuth,
            email_allowlist: vec!["alice@example.test".to_string()],
        },
        trusted_oidc: trusted_oidc(),
        console: ConsolePolicy {
            require_app_auth,
            read_only,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!("../assets/release-targets.json").to_string(),
    })
    .expect("decision state builds")
}

fn sign_hs256(payload: Value, secret: &str, kid: &str) -> String {
    let header = json!({"alg":"HS256","typ":"JWT","kid":kid});
    let header_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).expect("encode header"));
    let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).expect("encode claims"));
    let signing_input = format!("{header_b64}.{payload_b64}");
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac init");
    mac.update(signing_input.as_bytes());
    let signature_b64 = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    format!("{signing_input}.{signature_b64}")
}

fn alice_bearer() -> String {
    sign_hs256(
        json!({
            "sub": "alice@example.test",
            "email": "alice@example.test",
            "provider": "google_oauth",
            "iss": "https://trusted.meta.localhost",
            "aud": "meerkat-console",
            "exp": 4_000_000_000_u64,
        }),
        "phase7-trusted-current-secret",
        "kid-current",
    )
}

async fn console_rpc_with_bearer(
    app: &axum::Router,
    method: &str,
    params: Value,
    bearer: Option<&str>,
) -> Value {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": "console-wg",
        "method": method,
        "params": params,
    });
    let mut request = Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(bearer) = bearer {
        request = request.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(Body::from(payload.to_string()))
                .expect("request"),
        )
        .await
        .expect("console rpc response");
    // 200 for dispatched calls, 401 for the auth-door rejection — both carry
    // a JSON-RPC body the callers assert on.
    assert!(
        response.status() == StatusCode::OK || response.status() == StatusCode::UNAUTHORIZED,
        "unexpected console rpc status: {}",
        response.status()
    );
    let body = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body");
    serde_json::from_slice(&body).expect("console rpc json")
}

async fn console_rpc(app: &axum::Router, method: &str, params: Value) -> Value {
    console_rpc_with_bearer(app, method, params, None).await
}

async fn console_experience(app: &axum::Router) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/console/experience")
                .body(Body::empty())
                .expect("experience request"),
        )
        .await
        .expect("experience response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("experience body");
    serde_json::from_slice(&body).expect("experience json")
}

fn workgraph_view_only_config() -> AccessControlConfig {
    AccessControlConfig {
        approvers: Default::default(),
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        groups: BTreeMap::new(),
        rules: vec![
            AccessRule {
                id: "everyone-views-agents".to_string(),
                actions: vec!["agent.view".to_string()],
                ..AccessRule::default()
            },
            AccessRule {
                id: "everyone-views-workgraph".to_string(),
                actions: vec!["workgraph.view".to_string()],
                ..AccessRule::default()
            },
        ],
    }
}

fn workgraph_manage_config() -> AccessControlConfig {
    let mut config = workgraph_view_only_config();
    config.rules.push(AccessRule {
        id: "everyone-manages-workgraph".to_string(),
        actions: vec!["workgraph.manage".to_string()],
        ..AccessRule::default()
    });
    config
}

#[tokio::test(flavor = "multi_thread")]
async fn console_dispatch_and_capabilities() {
    let runtime = build_runtime().await;
    let app = runtime.build_reference_app_router(decision_state(false, false));

    let caps = console_rpc(&app, "meta/capabilities", json!({})).await;
    assert_eq!(caps["result"]["workgraph"], json!(true));
    let methods = caps["result"]["methods"].to_string();
    assert!(methods.contains("meta/workgraph/snapshot"));
    assert!(methods.contains("meta/workgraph/goal/create"));

    let created = console_rpc(
        &app,
        "meta/workgraph/create",
        json!({ "title": "console item" }),
    )
    .await;
    assert!(created["error"].is_null(), "{created:#?}");
    assert_eq!(created["result"]["item"]["title"], json!("console item"));

    let snapshot = console_rpc(&app, "meta/workgraph/snapshot", json!({})).await;
    assert_eq!(
        snapshot["result"]["items"].as_array().unwrap().len(),
        1,
        "{snapshot:#?}"
    );

    // Experience: no access control → affordances mirror availability.
    let experience = console_experience(&app).await;
    assert_eq!(
        experience["workgraph"],
        json!({ "available": true, "can_view": true, "can_manage": true })
    );
    runtime.mob_handle().stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn console_read_only_blocks_workgraph_mutations() {
    let runtime = build_runtime().await;
    let app = runtime.build_reference_app_router(decision_state(false, true));

    let denied = console_rpc(&app, "meta/workgraph/create", json!({ "title": "nope" })).await;
    assert_eq!(denied["error"]["code"], json!(-32010), "{denied:#?}");
    assert_eq!(denied["error"]["data"]["kind"], json!("read_only"));

    // Reads still work.
    let snapshot = console_rpc(&app, "meta/workgraph/snapshot", json!({})).await;
    assert!(snapshot["error"].is_null(), "{snapshot:#?}");

    // Capabilities advertise only the read set.
    let caps = console_rpc(&app, "meta/capabilities", json!({})).await;
    let methods = caps["result"]["methods"].to_string();
    assert!(methods.contains("meta/workgraph/snapshot"));
    assert!(!methods.contains("meta/workgraph/goal/create"));
    runtime.mob_handle().stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn console_abac_gates_view_and_manage() {
    let runtime = build_runtime().await;

    // View-only grants: reads pass, mutations are access-denied, and the
    // capability list strips the mutate set.
    let controller = AccessController::new(workgraph_view_only_config()).expect("controller");
    {
        // set_access_controller needs &mut; rebuild per grant set instead.
        let mut runtime_ref = runtime;
        runtime_ref.set_access_controller(controller.clone());
        let app = runtime_ref.build_reference_app_router(decision_state(false, false));

        let snapshot = console_rpc(&app, "meta/workgraph/snapshot", json!({})).await;
        assert!(snapshot["error"].is_null(), "{snapshot:#?}");

        let denied = console_rpc(&app, "meta/workgraph/create", json!({ "title": "denied" })).await;
        assert_eq!(denied["error"]["code"], json!(-32030), "{denied:#?}");
        assert_eq!(denied["error"]["data"]["kind"], json!("access_denied"));
        assert_eq!(denied["error"]["data"]["action"], json!("workgraph.manage"));

        let caps = console_rpc(&app, "meta/capabilities", json!({})).await;
        let methods = caps["result"]["methods"].to_string();
        assert!(methods.contains("meta/workgraph/snapshot"));
        assert!(
            !methods.contains("meta/workgraph/create"),
            "grant intersection must strip unusable mutate methods: {methods}"
        );

        let experience = console_experience(&app).await;
        assert_eq!(
            experience["workgraph"],
            json!({ "available": true, "can_view": true, "can_manage": false })
        );

        // Manage grants unlock mutations end to end.
        controller
            .replace_config(workgraph_manage_config())
            .expect("upgrade grants");
        let allowed =
            console_rpc(&app, "meta/workgraph/create", json!({ "title": "allowed" })).await;
        assert!(allowed["error"].is_null(), "{allowed:#?}");

        let experience = console_experience(&app).await;
        assert_eq!(experience["workgraph"]["can_manage"], json!(true));

        runtime_ref.mob_handle().stop().await.expect("stop");
    }
}

#[test]
fn workgraph_actions_are_valid_vocabulary_and_admins_bypass() {
    let config = AccessControlConfig {
        approvers: Default::default(),
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        groups: BTreeMap::new(),
        rules: vec![AccessRule {
            id: "wg".to_string(),
            actions: vec![
                "workgraph.view".to_string(),
                "workgraph.manage".to_string(),
                "workgraph.*".to_string(),
            ],
            subjects: vec!["alice@example.test".to_string()],
            ..AccessRule::default()
        }],
    };
    validate_access_config(&config).expect("workgraph actions validate");

    let controller = AccessController::new(config).expect("controller");
    let admin = controller.view_for_subject(Some("root@example.test"));
    assert!(admin.allows("workgraph.view"));
    assert!(admin.allows("workgraph.manage"));
    let alice = controller.view_for_subject(Some("alice@example.test"));
    assert!(alice.allows("workgraph.manage"));
    let outsider = controller.view_for_subject(Some("carol@example.test"));
    assert!(!outsider.allows("workgraph.view"), "deny by default");
}

#[tokio::test(flavor = "multi_thread")]
async fn experience_reports_workgraph_unavailable_without_service() {
    let (_dir, runtime) = build_runtime_without_workgraph().await;
    let app = runtime.build_reference_app_router(decision_state(false, false));
    let experience = console_experience(&app).await;
    assert_eq!(
        experience["workgraph"],
        json!({ "available": false, "can_view": false, "can_manage": false })
    );

    let caps = console_rpc(&app, "meta/capabilities", json!({})).await;
    assert_eq!(caps["result"]["workgraph"], json!(false));
    assert!(!caps["result"]["methods"].to_string().contains("workgraph"));

    let response = console_rpc(&app, "meta/workgraph/snapshot", json!({})).await;
    assert_eq!(response["error"]["code"], json!(-32041), "{response:#?}");
    runtime.mob_handle().stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn console_goal_confirm_promotes_authenticated_principal() {
    let runtime = build_runtime().await;

    // Host-trusted create of a principal-confirmed goal via the stdin surface.
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "needs alice",
            "target": { "kind": "identity", "identity": "helper" },
            "completion_policy": { "kind": "principal_confirmed" },
        }),
    )
    .await;
    let goal = result(&response).clone();
    let binding_id = goal["attention"]["binding_id"]
        .as_str()
        .unwrap()
        .to_string();
    let item_revision = goal["item"]["revision"].as_u64().unwrap();

    let app = runtime.build_reference_app_router(decision_state(true, false));
    let bearer = alice_bearer();

    // Unauthenticated confirm is rejected at the console door.
    let unauthenticated = console_rpc(
        &app,
        "meta/workgraph/goal/confirm",
        json!({ "binding_id": binding_id, "expected_revision": item_revision }),
    )
    .await;
    assert!(
        unauthenticated["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("unauthorized"),
        "{unauthenticated:#?}"
    );

    // Alice's authenticated confirm is promoted to the trusted principal.
    let confirmed = console_rpc_with_bearer(
        &app,
        "meta/workgraph/goal/confirm",
        json!({ "binding_id": binding_id, "expected_revision": item_revision }),
        Some(&bearer),
    )
    .await;
    assert!(confirmed["error"].is_null(), "{confirmed:#?}");
    let evidence = confirmed["result"]["item"]["evidence_refs"][0].clone();
    assert_eq!(
        evidence["confirmation_kind"],
        json!("principal_confirmation")
    );
    assert_eq!(
        evidence["confirming_owner_key"],
        json!({ "kind": "principal", "id": "alice@example.test" })
    );
    runtime.mob_handle().stop().await.expect("stop");
}

// ---------------------------------------------------------------------------
// meerkat 0.7.25: attention prune (ask 24) + break-glass reassign (ask 23)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn attention_prune_removes_terminal_bindings() {
    let runtime = build_runtime().await;
    // Coordinate goal, then reassign: the previous binding goes superseded —
    // exactly the monotonically-growing row class prune exists for.
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "prunable",
            "target": { "kind": "identity", "identity": "helper" },
            "mode": "coordinate",
        }),
    )
    .await;
    let goal = result(&response).clone();
    let binding_id = goal["attention"]["binding_id"].as_str().unwrap();
    let binding_revision = goal["attention"]["machine_state"]["revision"]
        .as_u64()
        .unwrap();
    let response = rpc(
        &runtime,
        "meta/workgraph/attention/reassign",
        json!({
            "binding_id": binding_id,
            "expected_revision": binding_revision,
            "target": { "kind": "identity", "identity": "backup" },
        }),
    )
    .await;
    assert_eq!(
        result(&response)["previous"]["status"]["state"],
        json!("superseded")
    );

    let response = rpc(&runtime, "meta/workgraph/attention/prune", json!({})).await;
    assert_eq!(result(&response)["pruned"], json!(1), "{response:#?}");

    // The superseded row is gone; the live binding survives. The event
    // stream (audit history) is untouched by prune.
    let response = rpc(&runtime, "meta/workgraph/attention/list", json!({})).await;
    let bindings = result(&response)["attention"].as_array().unwrap().clone();
    assert_eq!(bindings.len(), 1, "{bindings:#?}");
    assert_eq!(bindings[0]["status"]["state"], json!("active"));

    // Idempotent: nothing terminal left.
    let response = rpc(&runtime, "meta/workgraph/attention/prune", json!({})).await;
    assert_eq!(result(&response)["pruned"], json!(0));
    runtime.mob_handle().stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn break_glass_reassign_does_not_exist_on_the_stdin_surface() {
    let runtime = build_runtime().await;
    let response = rpc(
        &runtime,
        "meta/workgraph/attention/break_glass_reassign",
        json!({
            "binding_id": "wab-000000000000000000000000000000",
            "expected_revision": 0,
            "target": { "kind": "identity", "identity": "backup" },
            "reason": "should never be dispatched",
        }),
    )
    .await;
    assert_eq!(error_code(&response), -32601, "{response:#?}");
    // The stdin capabilities catalog does not advertise it either.
    let caps = rpc(&runtime, "meta/capabilities", json!({})).await;
    let methods = result(&caps)["methods"].as_array().unwrap().clone();
    assert!(
        !methods
            .iter()
            .any(|m| m == "meta/workgraph/attention/break_glass_reassign"),
        "console-only method leaked into the stdin catalog"
    );
    runtime.mob_handle().stop().await.expect("stop");
}

/// Ask 23 end-to-end: a PURSUE-mode binding (which the agent-plane reassign
/// refuses — only coordinate mode derives the authority) is recovered by an
/// authenticated console operator through the break-glass seam, with the
/// principal recorded server-side and the reason mandatory.
#[tokio::test(flavor = "multi_thread")]
async fn console_break_glass_reassign_recovers_pursue_binding_with_audit() {
    let runtime = build_runtime().await;
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "stuck on a wedged pursuer",
            "target": { "kind": "identity", "identity": "helper" },
        }),
    )
    .await;
    let goal = result(&response).clone();
    let binding_id = goal["attention"]["binding_id"]
        .as_str()
        .unwrap()
        .to_string();
    let binding_revision = goal["attention"]["machine_state"]["revision"]
        .as_u64()
        .unwrap();

    let app = runtime.build_reference_app_router(decision_state(true, false));
    let bearer = alice_bearer();

    // Reason is mandatory.
    let response = console_rpc_with_bearer(
        &app,
        "meta/workgraph/attention/break_glass_reassign",
        json!({
            "binding_id": binding_id,
            "expected_revision": binding_revision,
            "target": { "kind": "identity", "identity": "backup" },
        }),
        Some(&bearer),
    )
    .await;
    assert_eq!(response["error"]["code"], json!(-32602), "{response:#?}");
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("reason"),
        "{response:#?}"
    );

    // The principal is never a wire parameter.
    let response = console_rpc_with_bearer(
        &app,
        "meta/workgraph/attention/break_glass_reassign",
        json!({
            "binding_id": binding_id,
            "expected_revision": binding_revision,
            "target": { "kind": "identity", "identity": "backup" },
            "reason": "operator recovery",
            "principal": "mallory@example.test",
        }),
        Some(&bearer),
    )
    .await;
    assert_eq!(response["error"]["code"], json!(-32602), "{response:#?}");

    // The authenticated operator's break-glass move succeeds on a binding
    // the agent-plane reassign would refuse for mode.
    let response = console_rpc_with_bearer(
        &app,
        "meta/workgraph/attention/break_glass_reassign",
        json!({
            "binding_id": binding_id,
            "expected_revision": binding_revision,
            "target": { "kind": "identity", "identity": "backup" },
            "reason": "pursuer wedged; no coordinator holds authority",
        }),
        Some(&bearer),
    )
    .await;
    assert!(response["error"].is_null(), "{response:#?}");
    assert_eq!(
        response["result"]["previous"]["status"]["state"],
        json!("superseded")
    );
    assert_eq!(
        response["result"]["attention"]["target"]["owner_key"]["id"],
        json!(format!("mob/{}/agent/backup", runtime_mob_id(&runtime)))
    );

    // The audit trail carries the authenticated principal and the reason.
    let events = rpc(&runtime, "meta/workgraph/events", json!({ "limit": 200 })).await;
    let events_json = serde_json::to_string(result(&events)).expect("events json");
    assert!(events_json.contains("alice@example.test"), "{events_json}");
    assert!(
        events_json.contains("pursuer wedged"),
        "reason missing from the audit stream: {events_json}"
    );
    runtime.mob_handle().stop().await.expect("stop");
}

/// Without an authenticated console principal there is no one to attribute
/// the break-glass move to: the method refuses (access_denied) instead of
/// minting an anonymous audit row.
#[tokio::test(flavor = "multi_thread")]
async fn console_break_glass_reassign_requires_authenticated_principal() {
    let runtime = build_runtime().await;
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "anonymous operators need not apply",
            "target": { "kind": "identity", "identity": "helper" },
        }),
    )
    .await;
    let goal = result(&response).clone();

    // Console with app auth DISABLED: requests dispatch, but there is no
    // authenticated principal.
    let app = runtime.build_reference_app_router(decision_state(false, false));
    let response = console_rpc(
        &app,
        "meta/workgraph/attention/break_glass_reassign",
        json!({
            "binding_id": goal["attention"]["binding_id"],
            "expected_revision": goal["attention"]["machine_state"]["revision"],
            "target": { "kind": "identity", "identity": "backup" },
            "reason": "no principal available",
        }),
    )
    .await;
    assert_eq!(response["error"]["code"], json!(-32030), "{response:#?}");
    assert_eq!(response["error"]["data"]["kind"], json!("access_denied"));
    runtime.mob_handle().stop().await.expect("stop");
}

// ---------------------------------------------------------------------------
// Mob-realm scoping of member-bound attention (meerkat 0.8.41)
// ---------------------------------------------------------------------------

fn mob_realm(mob_id: &str) -> String {
    meerkat_core::mob_realm_id(mob_id)
        .expect("mob realm")
        .as_str()
        .to_string()
}

/// A goal bound to a member of THIS mob through the console/RPC surface lands
/// in the realm the member resolves attention from: the runtime's service is
/// scoped to `mob.<id>`, which is exactly what `mob_scoped_workgraph_service`
/// (the rescoping meerkat applies before every member turn) resolves to.
#[tokio::test(flavor = "multi_thread")]
async fn member_bound_goal_lands_in_the_realm_members_read() {
    let runtime = build_runtime().await;
    let mob_id = runtime_mob_id(&runtime);

    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "review the release notes",
            "target": { "kind": "identity", "identity": "reviewer" },
        }),
    )
    .await;
    let goal = result(&response).clone();
    let binding_id = goal["attention"]["binding_id"]
        .as_str()
        .unwrap()
        .to_string();

    let host_service = runtime.workgraph_service().expect("workgraph configured");
    assert_eq!(host_service.default_realm_id(), mob_realm(&mob_id));
    let member_view = meerkat_mob::mob_scoped_workgraph_service(
        &host_service,
        &meerkat_mob::MobId::from(mob_id.as_str()),
    )
    .expect("mob realm")
    .expect("mob-scoped service");
    let visible = member_view
        .list_attention(meerkat::AttentionListRequest {
            realm_id: None,
            namespace: None,
            target: None,
            status: None,
        })
        .await
        .expect("member-realm attention list")
        .attention;
    assert_eq!(visible.len(), 1, "the member's realm must hold the binding");
    assert_eq!(visible[0].binding_id.to_string(), binding_id);
    assert_eq!(
        visible[0].target.owner_key().expect("owner key"),
        meerkat::WorkOwnerKey::mob_agent(&mob_id, "reviewer").expect("member key")
    );
    let report = runtime
        .workgraph_realm_migration()
        .expect("migration ran at bootstrap");
    assert!(
        report.migrated.is_empty(),
        "nothing to migrate on a fresh store"
    );
    runtime.mob_handle().stop().await.expect("stop");
}

/// A goal, reassignment or break-glass reassignment whose target names a
/// member of ANOTHER mob is refused before the write with the typed realm
/// mismatch; nothing is stored in this runtime's realm.
#[tokio::test(flavor = "multi_thread")]
async fn member_of_another_mob_is_refused_typed_before_the_write() {
    let runtime = build_runtime().await;
    let mob_id = runtime_mob_id(&runtime);
    let foreign_key =
        meerkat::WorkOwnerKey::mob_agent("some-other-mob", "reviewer").expect("foreign member key");

    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "belongs to another mob",
            "target": { "kind": "owner", "owner_key": foreign_key },
        }),
    )
    .await;
    assert_eq!(error_code(&response), -32602);
    let data = &response["error"]["data"];
    assert_eq!(data["kind"], json!("attention_target_realm_mismatch"));
    assert_eq!(data["mob_id"], json!("some-other-mob"));
    assert_eq!(
        data["required_realm_id"],
        json!(mob_realm("some-other-mob"))
    );
    assert_eq!(data["realm_id"], json!(mob_realm(&mob_id)));
    assert_eq!(data["owner_key"], json!(foreign_key.canonical()));

    let listed = rpc(&runtime, "meta/workgraph/attention/list", json!({})).await;
    assert!(
        result(&listed)["attention"].as_array().unwrap().is_empty(),
        "a refused goal must leave no binding behind"
    );

    // Reassigning an existing binding onto the foreign member is refused the
    // same way and leaves the binding on its current target.
    let created = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "stays with the local reviewer",
            "target": { "kind": "identity", "identity": "reviewer" },
            "mode": "coordinate",
        }),
    )
    .await;
    let goal = result(&created).clone();
    let binding_id = goal["attention"]["binding_id"]
        .as_str()
        .unwrap()
        .to_string();
    let revision = goal["attention"]["machine_state"]["revision"]
        .as_u64()
        .unwrap();
    let reassigned = rpc(
        &runtime,
        "meta/workgraph/attention/reassign",
        json!({
            "binding_id": binding_id,
            "expected_revision": revision,
            "target": { "kind": "owner", "owner_key": foreign_key },
        }),
    )
    .await;
    assert_eq!(error_code(&reassigned), -32602);
    assert_eq!(
        reassigned["error"]["data"]["kind"],
        json!("attention_target_realm_mismatch")
    );
    let status = rpc(
        &runtime,
        "meta/workgraph/goal/status",
        json!({ "binding_id": binding_id }),
    )
    .await;
    assert_eq!(
        result(&status)["attention"]["target"]["owner_key"]["id"],
        json!(format!("mob/{mob_id}/agent/reviewer"))
    );
    runtime.mob_handle().stop().await.expect("stop");
}

/// Seed a binding the way a pre-0.8.41 runtime left it: in this mob's realm,
/// bound to a member of a child mob. The service layer refuses this now, so
/// the rows go in through the store, exactly as legacy rows exist on disk.
async fn seed_legacy_child_member_binding(
    service: &meerkat::WorkGraphService,
    child_mob: &str,
    identity: &str,
    title: &str,
) -> meerkat::WorkAttentionBinding {
    let realm_id = service.default_realm_id().to_string();
    let namespace = service.default_namespace().clone();
    let now = service
        .store()
        .get_store_time_utc()
        .await
        .expect("store time");
    let (item, item_event) = meerkat::WorkGraphMachine::create_item(
        meerkat::CreateWorkItemRequest {
            realm_id: Some(realm_id.clone()),
            namespace: Some(namespace.clone()),
            title: title.to_string(),
            description: None,
            completion_policy: meerkat::WorkCompletionPolicy::SelfAttest,
            failed_child_join_policy: Default::default(),
            cancelled_child_join_policy: Default::default(),
            priority: Default::default(),
            labels: Default::default(),
            due_at: None,
            not_before: None,
            snoozed_until: None,
            external_refs: Vec::new(),
            evidence_refs: Vec::new(),
            status: None,
        },
        realm_id.clone(),
        namespace.clone(),
        now,
    )
    .expect("legacy item");
    let attention = meerkat::WorkAttentionBinding {
        binding_id: meerkat::WorkAttentionBindingId::generated(),
        work_ref: meerkat::WorkItemRef {
            realm_id: realm_id.clone(),
            namespace: namespace.clone(),
            item_id: item.id.clone(),
        },
        target: meerkat::WorkAttentionTarget::LoweredOwner {
            owner_key: meerkat::WorkOwnerKey::mob_agent(child_mob, identity).expect("member key"),
        },
        mode: meerkat::WorkAttentionMode::Pursue,
        status: meerkat::WorkAttentionStatus::Active,
        machine_state: Default::default(),
        delegated_authority: meerkat::AttentionDelegatedAuthority::default(),
        projection_policy: meerkat::AttentionProjectionPolicy::default(),
        created_at: now,
        updated_at: now,
    };
    let event = meerkat::WorkGraphEvent::graph(
        realm_id,
        namespace,
        meerkat::WorkGraphEventKind::AttentionCreated,
        now,
        json!({ "attention": attention }),
    );
    let (_, attention) = service
        .store()
        .insert_goal(item, item_event, attention, event)
        .await
        .expect("seed legacy goal");
    attention
}

/// Bootstrap over a store that still holds a pre-rescoping child-member
/// binding: the runtime moves it into the child's realm, reports it on
/// `meta/status`, and a second bootstrap over the same store has nothing
/// left to move.
#[tokio::test(flavor = "multi_thread")]
async fn bootstrap_migrates_legacy_child_member_bindings_and_reports_them() {
    let definition = definition();
    let mob_id = definition.id.as_str().to_string();
    let child_mob = format!("{mob_id}-child");
    let store: Arc<dyn meerkat::WorkGraphStore> = Arc::new(meerkat::MemoryWorkGraphStore::new());
    let service = meerkat::WorkGraphService::with_scope(
        Arc::clone(&store),
        mob_realm(&mob_id),
        meerkat::WorkNamespace::default(),
    );
    let legacy =
        seed_legacy_child_member_binding(&service, &child_mob, "scout", "map the site").await;

    let bootstrap = |definition: MobDefinition| {
        let service = service.clone();
        async move {
            let temp_dir = tempfile::tempdir().expect("temp dir");
            let factory = AgentFactory::new(temp_dir.path()).comms(true);
            let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 8));
            let mob_spec =
                MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
                    .with_workgraph_service(Some(service))
                    .with_options(MobBootstrapOptions {
                        allow_ephemeral_sessions: true,
                        notify_orchestrator_on_resume: true,
                        default_llm_client: Some(Arc::new(TestClient::default())),
                    });
            let module_config = MetaConfig {
                modules: vec![],
                discovery: DiscoverySpec {
                    namespace: "workgraph-rpc".to_string(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            };
            let runtime =
                UnifiedRuntime::bootstrap(mob_spec, module_config, Duration::from_secs(2))
                    .await
                    .expect("bootstrap runtime with legacy binding");
            (temp_dir, runtime)
        }
    };

    let (_dir, runtime) = bootstrap(definition).await;
    let report = runtime
        .workgraph_realm_migration()
        .expect("migration report");
    assert_eq!(
        report.outcome,
        meerkat_meta::WorkGraphRealmMigrationOutcome::Completed
    );
    assert_eq!(report.migrated.len(), 1, "{report:#?}");
    let moved = &report.migrated[0];
    assert_eq!(moved.binding_id, legacy.binding_id.to_string());
    assert_eq!(moved.mob_id, child_mob);
    assert_eq!(moved.from_realm_id, mob_realm(&mob_id));
    assert_eq!(moved.to_realm_id, mob_realm(&child_mob));
    let new_binding_id = moved.new_binding_id.clone().expect("new binding id");

    // The child's realm now holds the binding on the same member.
    let child_view = meerkat::WorkGraphService::with_scope(
        Arc::clone(&store),
        mob_realm(&child_mob),
        meerkat::WorkNamespace::default(),
    );
    let child_bindings = child_view
        .list_attention(meerkat::AttentionListRequest {
            realm_id: None,
            namespace: None,
            target: None,
            status: None,
        })
        .await
        .expect("child attention")
        .attention;
    assert_eq!(child_bindings.len(), 1);
    assert_eq!(child_bindings[0].binding_id.to_string(), new_binding_id);
    assert_eq!(
        child_bindings[0].target.owner_key().expect("owner key"),
        meerkat::WorkOwnerKey::mob_agent(&child_mob, "scout").expect("member key")
    );

    // The original is stopped in this runtime's realm and the status surface
    // carries the report.
    let listed = rpc(&runtime, "meta/workgraph/attention/list", json!({})).await;
    let original = result(&listed)["attention"]
        .as_array()
        .unwrap()
        .iter()
        .find(|binding| binding["binding_id"] == json!(legacy.binding_id.to_string()))
        .expect("original binding row kept")
        .clone();
    assert_eq!(original["status"]["state"], json!("stopped"));
    let status = rpc(&runtime, "meta/capabilities", json!({})).await;
    let reported = &result(&status)["workgraph_realm_migration"];
    assert_eq!(reported["mode"], json!("apply"));
    assert_eq!(reported["migrated"].as_array().unwrap().len(), 1);
    assert_eq!(reported["outcome"]["kind"], json!("completed"));
    runtime.mob_handle().stop().await.expect("stop");

    // Idempotent: the same scan over the same store (what a second bootstrap
    // runs) finds only the terminal original and migrates nothing.
    let again = meerkat_meta::workgraph_realm::migrate_member_bindings_to_mob_realms(
        &service,
        meerkat_meta::WorkGraphRealmMigrationMode::Apply,
    )
    .await;
    assert_eq!(
        again.outcome,
        meerkat_meta::WorkGraphRealmMigrationOutcome::Completed
    );
    assert!(again.migrated.is_empty(), "{again:#?}");
    assert_eq!(again.skipped_terminal, 1);
    assert_eq!(
        child_view
            .list_attention(meerkat::AttentionListRequest {
                realm_id: None,
                namespace: None,
                target: None,
                status: None,
            })
            .await
            .expect("child attention")
            .attention
            .len(),
        1
    );
}
